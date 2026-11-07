//! SSB modulation: microphone audio to IQ samples at the radio's rate.
//!
//! Chain: speech band filter → optional compressor → one sideband kept by shifting the
//! wanted half of the band to zero and low-pass filtering it (the filter method) →
//! limiter → ×5 polyphase interpolation → linear resampling to the radio's rate (the
//! ratio can be nudged to follow the sound card's clock) → shift to the place of the
//! signal relative to the transmit LO.

use std::f64::consts::PI;

use num_complex::Complex;

use crate::demod::{FirDecimator, Mixer, Mode, lowpass, taps_for};

/// Speech band sent (Hz), as the receiver's default SSB passband.
pub const AUDIO_LOW: f64 = 300.0;
pub const AUDIO_HIGH: f64 = 2700.0;
/// Peak amplitude of the IQ output (full scale 1.0): headroom for the DAC and for the
/// power amplifier's linearity.
pub const PEAK: f32 = 0.5;
/// First interpolation factor (audio rate → intermediate rate).
const INTERPOLATION: usize = 5;

/// Interpolating FIR filter (polyphase): `factor` outputs per input.
struct FirInterpolator {
    /// Taps of each phase, gain already multiplied by the factor.
    phases: Vec<Vec<f32>>,
    history: Vec<Complex<f32>>,
}

impl FirInterpolator {
    fn new(taps: &[f32], factor: usize) -> Self {
        let per_phase = taps.len().div_ceil(factor);
        let phases = (0..factor)
            .map(|p| (0..per_phase).map(|k| taps.get(p + k * factor).copied().unwrap_or(0.0) * factor as f32).collect())
            .collect();
        Self { phases, history: vec![Complex::default(); per_phase] }
    }

    fn process(&mut self, input: &[Complex<f32>], out: &mut Vec<Complex<f32>>) {
        for &x in input {
            self.history.rotate_right(1);
            self.history[0] = x;
            for phase in &self.phases {
                out.push(phase.iter().zip(&self.history).map(|(&t, &h)| h * t).sum());
            }
        }
    }
}

/// Linear-interpolating resampler for complex samples.
struct Resampler {
    pos: f64,
    prev: Complex<f32>,
}

impl Resampler {
    fn process(&mut self, input: &[Complex<f32>], step: f64, out: &mut Vec<Complex<f32>>) {
        for &x in input {
            while self.pos < 1.0 {
                out.push(self.prev + (x - self.prev) * self.pos as f32);
                self.pos += step;
            }
            self.pos -= 1.0;
            self.prev = x;
        }
    }
}

/// Envelope follower used by the compressor and the limiter.
struct Envelope {
    value: f32,
    attack: f32,
    release: f32,
}

impl Envelope {
    fn new(rate: f64, attack_s: f64, release_s: f64) -> Self {
        let coeff = |t: f64| (1.0 - (-1.0 / (t * rate)).exp()) as f32;
        Self { value: 0.0, attack: coeff(attack_s), release: coeff(release_s) }
    }

    fn follow(&mut self, level: f32) -> f32 {
        let k = if level > self.value { self.attack } else { self.release };
        self.value += (level - self.value) * k;
        self.value
    }
}

/// Speech compressor: above the threshold the level rises only 1/ratio as fast.
struct Compressor {
    envelope: Envelope,
    threshold: f32,
    ratio: f32,
    makeup: f32,
}

impl Compressor {
    fn new(rate: f64) -> Self {
        let threshold = 10f32.powf(-20.0 / 20.0);
        let ratio = 3.0;
        // Brings a full-scale input back near full scale.
        let makeup = 1.0 / (threshold * (1.0 / threshold).powf(1.0 / ratio));
        Self { envelope: Envelope::new(rate, 0.005, 0.2), threshold, ratio, makeup }
    }

    fn apply(&mut self, x: f32) -> f32 {
        let env = self.envelope.follow(x.abs());
        let gain = if env > self.threshold { (self.threshold / env).powf(1.0 - 1.0 / self.ratio) } else { 1.0 };
        x * gain * self.makeup
    }
}

/// SSB modulator.
pub struct SsbModulator {
    mode: Mode,
    audio_rate: f64,
    out_rate: f64,
    /// Linear gain applied to the microphone.
    pub mic_gain: f32,
    pub compress: bool,
    speech: FirDecimator<f32>,
    compressor: Compressor,
    /// Moves the middle of the wanted sideband to zero, and back.
    down: Mixer,
    up: Mixer,
    sideband: FirDecimator<Complex<f32>>,
    limiter: Envelope,
    interpolator: FirInterpolator,
    resampler: Resampler,
    offset: Mixer,
    /// Largest gain reduction of the limiter in the last call (dB, ≥ 0): the ALC reading.
    pub alc_db: f32,
    /// Peak audio level after the microphone gain in the last call (dBFS).
    pub mic_peak_db: f32,
    /// The SSB signal of the last call at the audio rate, carrier at zero (for finding
    /// it again on the downlink).
    pub baseband: Vec<Complex<f32>>,
    scratch: Vec<Complex<f32>>,
    scratch2: Vec<Complex<f32>>,
}

impl SsbModulator {
    /// `mode` USB or LSB (CW is sent as USB of a tone).
    pub fn new(mode: Mode, audio_rate: f64, out_rate: f64) -> Self {
        let speech_taps = band_pass(AUDIO_LOW / audio_rate, AUDIO_HIGH / audio_rate, taps_for(audio_rate, 150.0, 801));
        let middle = (AUDIO_LOW + AUDIO_HIGH) / 2.0;
        let half = (AUDIO_HIGH - AUDIO_LOW) / 2.0;
        let sign = if mode == Mode::Lsb { -1.0 } else { 1.0 };
        // The unwanted sideband starts at middle + AUDIO_LOW from zero after the shift.
        let transition = AUDIO_LOW * 2.0;
        let sideband = FirDecimator::new(&lowpass((half + transition / 4.0) / audio_rate, taps_for(audio_rate, transition / 2.0, 1201)), 1);
        let mid_rate = audio_rate * INTERPOLATION as f64;
        let interpolator = FirInterpolator::new(&lowpass(4e3 / mid_rate, taps_for(mid_rate, audio_rate - 8e3, 255)), INTERPOLATION);
        Self {
            mode,
            audio_rate,
            out_rate,
            mic_gain: 1.0,
            compress: false,
            speech: FirDecimator::new(&speech_taps, 1),
            compressor: Compressor::new(audio_rate),
            down: Mixer::new(-sign * middle, audio_rate),
            up: Mixer::new(sign * middle, audio_rate),
            sideband,
            limiter: Envelope::new(audio_rate, 0.0005, 0.1),
            interpolator,
            resampler: Resampler { pos: 0.0, prev: Complex::default() },
            offset: Mixer::new(0.0, out_rate),
            alc_db: 0.0,
            mic_peak_db: -150.0,
            baseband: Vec::new(),
            scratch: Vec::new(),
            scratch2: Vec::new(),
        }
    }

    pub fn mode(&self) -> Mode {
        self.mode
    }

    /// Place of the suppressed carrier (the dial frequency) relative to the transmit LO.
    pub fn set_offset(&mut self, hz: f64) {
        self.offset.set(hz);
    }

    /// Modulates `audio` (at the audio rate) and appends IQ at the output rate. `speed`
    /// slightly above 1 produces fewer samples.
    pub fn process(&mut self, audio: &[f32], speed: f64, out: &mut Vec<Complex<f32>>) {
        let mut speech = Vec::with_capacity(audio.len());
        let mut peak = 0f32;
        let scaled: Vec<f32> = audio
            .iter()
            .map(|&x| {
                let v = x * self.mic_gain;
                peak = peak.max(v.abs());
                v
            })
            .collect();
        self.mic_peak_db = 20.0 * (peak + 1e-9).log10();
        self.speech.process(&scaled, &mut speech);
        let mut a = std::mem::take(&mut self.scratch);
        let mut b = std::mem::take(&mut self.scratch2);
        a.clear();
        for &x in &speech {
            let x = if self.compress { self.compressor.apply(x) } else { x };
            a.push(Complex::new(x, 0.0) * self.down.next());
        }
        b.clear();
        self.sideband.process(&a, &mut b);
        // Back to the sideband's place, then the limiter keeps the envelope at PEAK.
        let mut reduction = 1f32;
        for z in b.iter_mut() {
            *z *= self.up.next();
            // The filter halves the power of a real signal's one sideband.
            *z *= 2.0 * PEAK;
            let env = self.limiter.follow(z.norm());
            if env > PEAK {
                let g = PEAK / env;
                reduction = reduction.min(g);
                *z *= g;
            }
            let m = z.norm();
            if m > PEAK {
                *z *= PEAK / m;
            }
        }
        self.alc_db = -20.0 * reduction.log10();
        self.baseband.clear();
        self.baseband.extend_from_slice(&b);
        a.clear();
        self.interpolator.process(&b, &mut a);
        let step = self.audio_rate * INTERPOLATION as f64 / self.out_rate * speed;
        let start = out.len();
        self.resampler.process(&a, step, out);
        self.offset.mix(&mut out[start..]);
        self.scratch = a;
        self.scratch2 = b;
    }
}

/// Windowed-sinc band-pass (Blackman), edges in cycles per sample.
fn band_pass(low: f64, high: f64, taps: usize) -> Vec<f32> {
    let n = taps | 1;
    let mid = (n / 2) as f64;
    (0..n)
        .map(|i| {
            let k = i as f64 - mid;
            let ideal = if k == 0.0 { 2.0 * (high - low) } else { ((2.0 * PI * high * k).sin() - (2.0 * PI * low * k).sin()) / (PI * k) };
            let x = 2.0 * PI * (i + 1) as f64 / (n + 1) as f64;
            (ideal * (0.42 - 0.5 * x.cos() + 0.08 * (2.0 * x).cos())) as f32
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    const AUDIO: f64 = 48e3;
    const RATE: f64 = 2.4e6;
    const OFFSET: f64 = 300e3;

    fn tone(hz: f64, amp: f32, n: usize) -> Vec<f32> {
        (0..n).map(|i| amp * (2.0 * PI * hz * i as f64 / AUDIO).sin() as f32).collect()
    }

    /// Amplitude of one frequency in complex samples (single-bin DFT).
    fn bin(x: &[Complex<f32>], hz: f64) -> f64 {
        let mut acc = Complex::<f64>::default();
        for (i, z) in x.iter().enumerate() {
            let w = Complex::from_polar(1.0, -2.0 * PI * hz * i as f64 / RATE);
            acc += Complex::new(z.re as f64, z.im as f64) * w;
        }
        acc.norm() / x.len() as f64
    }

    fn modulate(mode: Mode, audio: &[f32]) -> Vec<Complex<f32>> {
        let mut m = SsbModulator::new(mode, AUDIO, RATE);
        m.set_offset(OFFSET);
        let mut out = Vec::new();
        for piece in audio.chunks(480) {
            m.process(piece, 1.0, &mut out);
        }
        out
    }

    fn db(a: f64, b: f64) -> f64 {
        20.0 * (a / b).log10()
    }

    #[test]
    fn usb_tone_lands_above_the_dial_alone() {
        let out = modulate(Mode::Usb, &tone(1e3, 0.3, 48_000));
        let tail = &out[out.len() / 2..];
        let wanted = bin(tail, OFFSET + 1e3);
        assert!((wanted - 0.15).abs() < 0.01, "wanted {wanted}");
        assert!(db(wanted, bin(tail, OFFSET - 1e3)) > 50.0, "other sideband");
        assert!(db(wanted, bin(tail, OFFSET)) > 50.0, "carrier");
        assert!(db(wanted, bin(tail, -OFFSET - 1e3)) > 60.0, "image");
        // Interpolation images at the intermediate rate's multiples.
        assert!(db(wanted, bin(tail, OFFSET + 1e3 + AUDIO)) > 50.0, "48 kHz image");
        assert!(db(wanted, bin(tail, OFFSET + 1e3 + 5.0 * AUDIO)) > 50.0, "240 kHz image");
    }

    #[test]
    fn lsb_tone_lands_below_the_dial() {
        let out = modulate(Mode::Lsb, &tone(1e3, 0.3, 48_000));
        let tail = &out[out.len() / 2..];
        let wanted = bin(tail, OFFSET - 1e3);
        assert!((wanted - 0.15).abs() < 0.01, "wanted {wanted}");
        assert!(db(wanted, bin(tail, OFFSET + 1e3)) > 50.0);
    }

    #[test]
    fn speech_band_edges_are_filtered() {
        let out = modulate(Mode::Usb, &tone(100.0, 0.3, 48_000));
        let low = bin(&out[out.len() / 2..], OFFSET + 100.0);
        let out = modulate(Mode::Usb, &tone(1e3, 0.3, 48_000));
        let mid = bin(&out[out.len() / 2..], OFFSET + 1e3);
        assert!(db(mid, low) > 30.0, "100 Hz hum {low}");
    }

    #[test]
    fn output_rate_and_peak() {
        let out = modulate(Mode::Usb, &tone(1e3, 3.0, 48_000));
        assert!((out.len() as f64 - RATE).abs() < 100.0, "{} samples", out.len());
        let peak = out.iter().map(|z| z.norm()).fold(0f32, f32::max);
        assert!(peak <= PEAK + 1e-4, "peak {peak}");
        let mut m = SsbModulator::new(Mode::Usb, AUDIO, RATE);
        let mut silent = Vec::new();
        m.process(&vec![0.0; 4800], 1.0, &mut silent);
        assert!(silent.iter().all(|z| z.norm() < 1e-6));
    }

    #[test]
    fn loud_voice_drives_the_alc() {
        let mut m = SsbModulator::new(Mode::Usb, AUDIO, RATE);
        let mut out = Vec::new();
        m.process(&tone(1e3, 0.2, 4800), 1.0, &mut out);
        assert!(m.alc_db < 0.5, "quiet: {}", m.alc_db);
        m.process(&tone(1e3, 4.0, 4800), 1.0, &mut out);
        assert!(m.alc_db > 10.0, "loud: {}", m.alc_db);
    }

    #[test]
    fn compressor_lifts_quiet_speech() {
        let level = |compress| {
            let mut m = SsbModulator::new(Mode::Usb, AUDIO, RATE);
            m.compress = compress;
            m.set_offset(OFFSET);
            let mut out = Vec::new();
            m.process(&tone(1e3, 0.05, 48_000), 1.0, &mut out);
            bin(&out[out.len() / 2..], OFFSET + 1e3)
        };
        assert!(db(level(true), level(false)) > 6.0);
    }

    /// Speed: `cargo test --release --lib modulator_speed -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn modulator_speed() {
        let audio = tone(1e3, 0.3, 48_000);
        let t = std::time::Instant::now();
        let out = modulate(Mode::Usb, &audio);
        println!("{} samples, {:.1}× real time", out.len(), 1.0 / t.elapsed().as_secs_f64());
    }
}
