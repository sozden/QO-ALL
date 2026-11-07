//! A simulated QO-100 station for trying the program without a radio: the narrow-band
//! transponder with its beacons, some SSB, CW and FT8 users, a drifting LNB, and the
//! operator's own transmission coming back from the satellite a quarter second later.

use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use num_complex::Complex;

use crate::bandplan::TRANSLATION_HZ;
use crate::demod::Mixer;
use crate::radio::{RxDevice, TxDevice};

/// Path delay up to the satellite and back (s).
pub const ECHO_DELAY_S: f64 = 0.25;
/// The simulated transmitter's own frequency error (Hz on the uplink), like the -930 Hz
/// a real Pluto showed: what the automatic uplink correction has to find.
pub const TX_ERROR_HZ: f64 = -900.0;
/// Amplitude of one's own signal coming back at full power (the transmitter's peak
/// is 0.5).
const ECHO_GAIN: f32 = 0.12;
/// Noise amplitude per component.
const NOISE: f32 = 0.004;
/// Transmit samples queued at most before `write` waits (s).
const TX_QUEUE_S: f64 = 0.08;

/// What the transmitting half hands to the receiving one.
struct Shared {
    queue: Mutex<VecDeque<Complex<f32>>>,
    tx_lo_hz: Mutex<f64>,
    /// Linear gain from the transmit attenuation.
    tx_gain: Mutex<f32>,
    powered: AtomicBool,
    /// Transmit sample rate.
    rate: Mutex<f64>,
}

#[derive(Clone, Copy)]
enum Kind {
    /// CW beacon keyed between two tones 393 Hz apart, a quarter second each.
    Beacon,
    /// BPSK beacon, 400 symbols/s.
    Psk,
    /// Keyed carrier sending the `CALL` text in Morse at 22 WPM.
    Cw,
    /// Voice-like SSB: three formants with syllables, talking now and then.
    Voice { phase: f64 },
    /// An FT8 transmission at `audio` Hz above 10489.540 MHz, in even or odd slots.
    Ft8 { audio: f64, odd: bool, text: &'static str },
}

struct Signal {
    rf_hz: f64,
    amp: f32,
    kind: Kind,
    mixers: Vec<Mixer>,
    /// FT8: the tones of its message.
    tones: Vec<usize>,
    /// Amplitude of each mixer's part for the current block.
    part_amps: Vec<f32>,
    /// PSK: current sign, its smoothed value (soft phase changes keep the spectrum
    /// narrow, as the real beacon's shaping does) and samples left in the symbol.
    sign: f32,
    smooth: f32,
    left: usize,
}

const CALL: &str = "CQ CQ DE TA4SO TA4SO K   ";

/// Morse of the letters used in `CALL`, as dot units (true = key down).
fn morse_units(text: &str) -> Vec<bool> {
    let code = |c: char| match c {
        'A' => ".-",
        'C' => "-.-.",
        'D' => "-..",
        'E' => ".",
        'K' => "-.-",
        'O' => "---",
        'Q' => "--.-",
        'S' => "...",
        'T' => "-",
        '4' => "....-",
        _ => "",
    };
    let mut units = Vec::new();
    for c in text.chars() {
        if c == ' ' {
            units.extend([false; 4]);
            continue;
        }
        for e in code(c).chars() {
            units.extend(std::iter::repeat_n(true, if e == '-' { 3 } else { 1 }));
            units.push(false);
        }
        units.extend([false; 2]);
    }
    units
}

/// The receiving half of the simulated station.
pub struct SimRx {
    shared: Arc<Shared>,
    lnb_lo_hz: f64,
    center_hz: f64,
    rate: f64,
    gain: f32,
    started: Instant,
    /// UTC at `started` (FT8 keeps to the clock's slots).
    utc0: f64,
    produced: u64,
    seed: u32,
    signals: Vec<Signal>,
    echo: VecDeque<Complex<f32>>,
    echo_mixer: Mixer,
    /// Resampling of the transmitted samples to the receive rate: position between
    /// the previous and the current sample.
    echo_phase: f64,
    echo_prev: Complex<f32>,
    echo_cur: Complex<f32>,
    morse: Vec<bool>,
    /// One-pole smoothing of the PSK beacon's phase changes.
    psk_smoothing: f32,
}

/// LNB error at `t` seconds (Hz): 3 kHz off, warming by 300 Hz over some minutes.
fn utc_now() -> f64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0.0, |d| d.as_secs_f64())
}

fn lnb_error(t: f64) -> f64 {
    3_000.0 + 300.0 * (1.0 - (-t / 300.0).exp())
}

impl SimRx {
    pub fn new(lnb_lo_hz: f64) -> Self {
        let signal = |rf_hz: f64, amp: f32, kind: Kind| {
            let tones = match kind {
                Kind::Voice { .. } => 3,
                _ => 1,
            };
            let ft8 = match kind {
                Kind::Ft8 { text, .. } => crate::ft8::encode::tones(&crate::ft8::FT8, text).expect("simulated FT8 message"),
                _ => Vec::new(),
            };
            Signal { rf_hz, amp, kind, mixers: (0..tones).map(|_| Mixer::new(0.0, 2.4e6)).collect(), tones: ft8, part_amps: Vec::new(), sign: 1.0, smooth: 1.0, left: 0 }
        };
        let signals = vec![
            signal(10_489.500e6, 0.05, Kind::Beacon),
            signal(10_489.750e6, 0.012, Kind::Psk),
            signal(10_490.000e6, 0.04, Kind::Beacon),
            signal(10_489.512e6, 0.02, Kind::Cw),
            signal(10_489.540e6, 0.006, Kind::Ft8 { audio: 1_243.0, odd: false, text: "CQ DL1ABC JO62" }),
            signal(10_489.540e6, 0.004, Kind::Ft8 { audio: 870.0, odd: true, text: "CQ EA7HJ IM76" }),
            signal(10_489.540e6, 0.003, Kind::Ft8 { audio: 1_810.0, odd: false, text: "G4ABC SP9UNU -08" }),
            signal(10_489.600e6, 0.015, Kind::Voice { phase: 0.0 }),
            signal(10_489.668e6, 0.025, Kind::Voice { phase: 7.0 }),
            signal(10_489.705e6, 0.012, Kind::Voice { phase: 3.0 }),
            signal(10_489.790e6, 0.02, Kind::Voice { phase: 11.0 }),
            signal(10_489.862e6, 0.018, Kind::Voice { phase: 5.0 }),
            signal(10_489.910e6, 0.01, Kind::Voice { phase: 15.0 }),
        ];
        let shared = Arc::new(Shared {
            queue: Mutex::new(VecDeque::new()),
            tx_lo_hz: Mutex::new(2_399.9e6),
            tx_gain: Mutex::new(0.0),
            powered: AtomicBool::new(false),
            rate: Mutex::new(2.4e6),
        });
        Self {
            shared,
            lnb_lo_hz,
            center_hz: 739.445e6,
            rate: 2.4e6,
            gain: 1.0,
            started: Instant::now(),
            utc0: utc_now(),
            produced: 0,
            seed: 12_345,
            signals,
            echo: VecDeque::new(),
            echo_mixer: Mixer::new(0.0, 2.4e6),
            echo_phase: 0.0,
            echo_prev: Complex::default(),
            echo_cur: Complex::default(),
            morse: morse_units(CALL),
            psk_smoothing: psk_smoothing(2.4e6),
        }
    }

    fn noise(&mut self) -> f32 {
        // Sum of three uniforms: close enough to Gaussian for a noise floor.
        let mut sum = 0.0;
        for _ in 0..3 {
            self.seed ^= self.seed << 13;
            self.seed ^= self.seed >> 17;
            self.seed ^= self.seed << 5;
            sum += self.seed as f32 / u32::MAX as f32 - 0.5;
        }
        sum
    }

    /// Frequencies (relative to the tuned centre) and amplitudes of a signal's parts at
    /// time `t`.
    fn parts(&self, s: &Signal, t: f64, base: f64) -> Vec<(f64, f32)> {
        match s.kind {
            Kind::Beacon => vec![(base + if (t * 4.0) as u64 % 2 == 0 { 0.0 } else { 393.0 }, s.amp)],
            Kind::Psk => vec![(base, s.amp)],
            Kind::Cw => {
                let dot = 1.2 / 22.0;
                let on = self.morse[(t / dot) as usize % self.morse.len()];
                vec![(base, if on { s.amp } else { 0.0 })]
            }
            Kind::Voice { phase } => {
                let talking = ((t + phase * 3.0) % 26.0) < 15.0;
                let syllable = (0.5 + 0.5 * (2.0 * std::f64::consts::PI * 3.1 * t + phase).sin()).powi(2) as f32;
                let wobble = 80.0 * (t * 1.7 + phase).sin();
                let a = if talking { s.amp * (0.2 + 0.8 * syllable) } else { 0.0 };
                vec![(base + 450.0 + wobble, a), (base + 1_050.0 + wobble * 1.5, a * 0.6), (base + 1_800.0 + wobble * 2.0, a * 0.35)]
            }
            Kind::Ft8 { audio, odd, .. } => {
                let utc = self.utc0 + t;
                let slot = (utc / 15.0) as u64;
                let into = utc % 15.0;
                let active = (slot % 2 == 1) == odd && (0.5..13.14).contains(&into);
                let symbol = (((into - 0.5) / 0.16).max(0.0) as usize).min(s.tones.len() - 1);
                vec![(base + audio + s.tones[symbol] as f64 * 6.25, if active { s.amp } else { 0.0 })]
            }
        }
    }

    /// Noise, the signals and the echo for the samples of `out`.
    fn fill(&mut self, out: &mut [Complex<f32>]) {
        for z in out.iter_mut() {
            let mut v = Complex::new(self.noise(), self.noise()) * NOISE;
            for s in self.signals.iter_mut() {
                let mut sum = Complex::default();
                for (m, a) in s.mixers.iter_mut().zip(&s.part_amps) {
                    sum += m.next() * *a;
                }
                if matches!(s.kind, Kind::Psk) {
                    if s.left == 0 {
                        self.seed ^= self.seed << 13;
                        self.seed ^= self.seed >> 17;
                        self.seed ^= self.seed << 5;
                        s.sign = if self.seed & 1 == 0 { 1.0 } else { -1.0 };
                        s.left = (self.rate / 400.0) as usize;
                    }
                    s.left -= 1;
                    s.smooth += (s.sign - s.smooth) * self.psk_smoothing;
                    sum *= s.smooth;
                }
                v += sum;
            }
            let back = self.echo.pop_front().unwrap_or_default();
            v += self.echo_mixer.next() * back;
            *z = v * self.gain;
        }
    }
}

impl RxDevice for SimRx {
    fn read(&mut self, out: &mut [Complex<f32>]) -> Result<usize, String> {
        // Real time: wait until these samples would have arrived.
        let due = self.started + Duration::from_secs_f64((self.produced + out.len() as u64) as f64 / self.rate);
        if let Some(wait) = due.checked_duration_since(Instant::now()) {
            std::thread::sleep(wait);
        }
        let t = self.produced as f64 / self.rate;
        let err = lnb_error(t);
        let if_of = |rf: f64| rf - self.lnb_lo_hz - err;
        // One's own signal back from the satellite.
        let n = out.len();
        let ratio = *self.shared.rate.lock().unwrap() / self.rate;
        let sent: Vec<Complex<f32>> = {
            let mut q = self.shared.queue.lock().unwrap();
            (0..n)
                .map(|_| {
                    self.echo_phase += ratio;
                    while self.echo_phase >= 1.0 {
                        self.echo_phase -= 1.0;
                        self.echo_prev = self.echo_cur;
                        self.echo_cur = q.pop_front().unwrap_or_default();
                    }
                    self.echo_prev + (self.echo_cur - self.echo_prev) * self.echo_phase as f32
                })
                .collect()
        };
        if self.echo.len() < (ECHO_DELAY_S * self.rate) as usize {
            self.echo.extend(std::iter::repeat_n(Complex::default(), (ECHO_DELAY_S * self.rate) as usize - self.echo.len()));
        }
        self.echo.extend(sent);
        let tx_lo = *self.shared.tx_lo_hz.lock().unwrap();
        self.echo_mixer.set(if_of(tx_lo + TX_ERROR_HZ + TRANSLATION_HZ) - self.center_hz);

        // In 1 ms pieces, each with the signals' parts of its time (FT8 symbols change
        // on time).
        let piece = ((self.rate / 1_000.0) as usize).max(1);
        for (k, chunk) in out.chunks_mut(piece).enumerate() {
            let tk = (self.produced + (k * piece) as u64) as f64 / self.rate;
            for i in 0..self.signals.len() {
                let base = self.signals[i].rf_hz - self.lnb_lo_hz - err - self.center_hz;
                let parts = self.parts(&self.signals[i], tk, base);
                for (m, (hz, _)) in self.signals[i].mixers.iter_mut().zip(&parts) {
                    m.set(*hz);
                }
                // Amplitudes ride along in the mixers' order.
                self.signals[i].amp_parts(parts.iter().map(|p| p.1));
            }
            self.fill(chunk);
        }
        self.produced += n as u64;
        Ok(n)
    }

    fn set_frequency(&mut self, hz: f64) -> Result<(), String> {
        self.center_hz = hz;
        Ok(())
    }

    fn set_sample_rate(&mut self, rate: f64) -> Result<(), String> {
        self.rate = rate;
        *self.shared.rate.lock().unwrap() = crate::iio::tx_rate_for(rate);
        for s in &mut self.signals {
            s.mixers = (0..s.mixers.len()).map(|_| Mixer::new(0.0, rate)).collect();
        }
        self.echo_mixer = Mixer::new(0.0, rate);
        self.psk_smoothing = psk_smoothing(rate);
        self.echo.clear();
        self.started = Instant::now();
        self.utc0 = utc_now();
        self.produced = 0;
        Ok(())
    }

    fn set_gain(&mut self, gain: Option<f64>) -> Result<(), String> {
        self.gain = 10f32.powf((gain.unwrap_or(40.0) as f32 - 40.0) / 20.0);
        Ok(())
    }

    fn model(&self) -> String {
        "Simulator".into()
    }

    fn transmitter(&self) -> Result<Box<dyn TxDevice>, String> {
        Ok(Box::new(SimTx { shared: Arc::clone(&self.shared) }))
    }
}

fn psk_smoothing(rate: f64) -> f32 {
    (1.0 - (-2.0 * std::f64::consts::PI * 500.0 / rate).exp()) as f32
}

impl Signal {
    fn amp_parts(&mut self, amps: impl Iterator<Item = f32>) {
        self.part_amps.clear();
        self.part_amps.extend(amps);
    }
}

/// The transmitting half of the simulated station.
pub struct SimTx {
    shared: Arc<Shared>,
}

impl TxDevice for SimTx {
    fn set_frequency(&mut self, hz: f64) -> Result<(), String> {
        *self.shared.tx_lo_hz.lock().unwrap() = hz;
        Ok(())
    }

    fn set_attenuation(&mut self, db: f64) -> Result<(), String> {
        *self.shared.tx_gain.lock().unwrap() = ECHO_GAIN * 10f32.powf(-db as f32 / 20.0);
        Ok(())
    }

    fn set_bandwidth(&mut self, _hz: f64) -> Result<(), String> {
        Ok(())
    }

    fn set_sample_rate(&mut self, rate: f64) {
        *self.shared.rate.lock().unwrap() = rate;
    }

    fn set_powered(&mut self, on: bool) -> Result<(), String> {
        self.shared.powered.store(on, Ordering::Relaxed);
        Ok(())
    }

    fn write(&mut self, samples: &[Complex<f32>]) -> Result<(), String> {
        let gain = if self.shared.powered.load(Ordering::Relaxed) { *self.shared.tx_gain.lock().unwrap() } else { 0.0 };
        let limit = (*self.shared.rate.lock().unwrap() * TX_QUEUE_S) as usize;
        self.shared.queue.lock().unwrap().extend(samples.iter().map(|z| z * gain));
        // Paced by the receiving half taking the samples in real time.
        let started = Instant::now();
        while self.shared.queue.lock().unwrap().len() > limit {
            if started.elapsed() > Duration::from_secs(1) {
                return Err("the simulated receiver stopped".into());
            }
            std::thread::sleep(Duration::from_millis(2));
        }
        Ok(())
    }

    fn stop(&mut self) {
        self.shared.queue.lock().unwrap().clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn morse_of_the_call() {
        let u = morse_units("TE");
        // T = dah (3 on, 1 off) + letter gap 2; E = dit + 1 off + 2.
        assert_eq!(u, [true, true, true, false, false, false, true, false, false, false]);
    }

    #[test]
    fn own_signal_comes_back_a_quarter_second_later() {
        let lo = 9_750e6;
        let mut rx = SimRx::new(lo);
        let rate = 240e3;
        rx.set_sample_rate(rate).unwrap();
        let center = 10_489.75e6 - lo;
        rx.set_frequency(center).unwrap();
        let mut tx = rx.transmitter().unwrap();
        tx.set_sample_rate(rate);
        // Transmit LO 2400.25 MHz, carrier 20 kHz below it → 10489.730 MHz down.
        tx.set_frequency(2_400.25e6).unwrap();
        tx.set_attenuation(0.0).unwrap();
        tx.set_powered(true).unwrap();
        let offset = -20e3;
        let n = (rate * 0.075) as usize;
        let carrier: Vec<Complex<f32>> = (0..n).map(|i| Complex::from_polar(0.5, (2.0 * std::f64::consts::PI * offset * i as f64 / rate) as f32)).collect();
        tx.write(&carrier).unwrap();
        let mut got = vec![Complex::default(); (rate * 0.4) as usize];
        let mut filled = 0;
        while filled < got.len() {
            filled += rx.read(&mut got[filled..]).unwrap();
        }
        // Expected place, relative to the centre, after the LNB error.
        let expected = 2_400.25e6 + offset + TX_ERROR_HZ + TRANSLATION_HZ - lo - lnb_error(0.0) - center;
        let power = |from: f64, to: f64| {
            let (a, b) = ((from * rate) as usize, (to * rate) as usize);
            let mut acc = Complex::<f64>::default();
            for (i, z) in got[a..b].iter().enumerate() {
                let w = Complex::from_polar(1.0, -2.0 * std::f64::consts::PI * expected * (a + i) as f64 / rate);
                acc += Complex::new(z.re as f64, z.im as f64) * w;
            }
            acc.norm() / (b - a) as f64
        };
        let before = power(0.05, 0.24);
        let during = power(0.26, 0.32);
        assert!(during > 0.04 && before < 0.005, "before {before:.4}, during {during:.4}");
    }
}
