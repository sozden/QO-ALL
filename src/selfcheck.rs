//! Automatic uplink correction: finding one's own signal on the downlink.
//!
//! In full duplex the transmitted voice comes back from the satellite about a quarter
//! second later, and what was sent is known. Both are cut into short spectra (100 ms,
//! 10 Hz bins); the voice's pitch harmonics and formants make a pattern that matches
//! only at the right frequency shift and delay. The shift found is the uplink's error:
//! the transmitter's own oscillator error, which the beacon lock of the receiver cannot
//! see. Works on speech and on the tune tone alike.

use std::collections::VecDeque;
use std::sync::mpsc::{Receiver, RecvTimeoutError};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use num_complex::Complex;
use rustfft::{Fft, FftPlanner};

use crate::demod::{FirDecimator, Mixer, halfband, lowpass, taps_for};
use crate::source::Block;

/// Rate both signals are compared at (S/s).
pub const RATE: f64 = 12_000.0;
/// Spectrum length (100 ms: 10 Hz bins) and step between spectra (50 ms).
const FRAME: usize = 1_200;
const HOP: usize = 600;
const BIN_HZ: f64 = RATE / FRAME as f64;
/// Audio band compared (Hz above the carrier).
const BAND_HZ: (f64, f64) = (250.0, 2_800.0);
/// Largest error looked for (Hz).
pub const MAX_OFFSET_HZ: f64 = 2_000.0;
/// Path delays looked at (s): the satellite's quarter second plus both radios' buffers.
const DELAY_S: (f64, f64) = (0.1, 1.0);
/// Spectra kept, and the stretch of transmission one measurement uses (s).
const KEEP_S: f64 = 16.0;
const ANALYSE_S: f64 = 8.0;
/// Transmitting needed for a measurement (s of spectra with sound in them).
const MIN_SOUND_S: f64 = 1.5;
/// A measurement during a long transmission every so often (s).
const EVERY_S: f64 = 6.0;
/// The match must stand out of all other shifts and delays by this many standard
/// deviations.
const MIN_SCORE: f32 = 8.0;
/// Spectra this far below the loudest one in the stretch count as silence (dB).
const SILENCE_DB: f32 = 25.0;

/// A stream brought to `RATE` around one frequency.
pub struct Downconverter {
    in_rate: f64,
    mixer: Mixer,
    halfbands: Vec<FirDecimator<Complex<f32>>>,
    filter: FirDecimator<Complex<f32>>,
    rate: f64,
    pos: f64,
    prev: Complex<f32>,
}

impl Downconverter {
    pub fn new(in_rate: f64) -> Self {
        let mut rate = in_rate;
        let mut halfbands = Vec::new();
        while rate / 2.0 >= 2.0 * RATE {
            halfbands.push(halfband());
            rate /= 2.0;
        }
        let filter = FirDecimator::new(&lowpass(5_000.0 / rate, taps_for(rate, 800.0, 401)), 1);
        Self { in_rate, mixer: Mixer::new(0.0, in_rate), halfbands, filter, rate, pos: 0.0, prev: Complex::default() }
    }

    pub fn in_rate(&self) -> f64 {
        self.in_rate
    }

    /// Moves `shift_hz` of the input to zero and appends the result at `RATE`.
    pub fn process(&mut self, input: &[Complex<f32>], shift_hz: f64, out: &mut Vec<Complex<f32>>) {
        self.mixer.set(-shift_hz);
        let mut a = input.to_vec();
        self.mixer.mix(&mut a);
        for hb in &mut self.halfbands {
            let mut b = Vec::with_capacity(a.len() / 2 + 1);
            hb.process(&a, &mut b);
            a = b;
        }
        let mut b = Vec::with_capacity(a.len());
        self.filter.process(&a, &mut b);
        let step = self.rate / RATE;
        for &x in &b {
            while self.pos < 1.0 {
                out.push(self.prev + (x - self.prev) * self.pos as f32);
                self.pos += step;
            }
            self.pos -= 1.0;
            self.prev = x;
        }
    }
}

/// One short spectrum: its time (s) and power (dB) per 10 Hz bin, from -6 kHz up.
#[derive(Clone, Debug)]
pub struct Frame {
    pub t: f64,
    pub db: Vec<f32>,
}

/// Cuts a stream at `RATE` into overlapping spectra.
pub struct Framer {
    fft: Arc<dyn Fft<f32>>,
    window: Vec<f32>,
    buf: VecDeque<Complex<f32>>,
}

impl Default for Framer {
    fn default() -> Self {
        let window = (0..FRAME).map(|i| (std::f32::consts::PI * (i as f32 + 0.5) / FRAME as f32).sin().powi(2)).collect();
        Self { fft: FftPlanner::new().plan_fft_forward(FRAME), window, buf: VecDeque::new() }
    }
}

impl Framer {
    /// Adds samples whose last one was taken at `t_last` (s); returns new spectra.
    pub fn push(&mut self, samples: &[Complex<f32>], t_last: f64) -> Vec<Frame> {
        self.buf.extend(samples);
        let mut out = Vec::new();
        while self.buf.len() >= FRAME {
            // Time of the frame's middle: counted back from the newest sample.
            let t = t_last - (self.buf.len() as f64 - FRAME as f64 / 2.0) / RATE;
            let mut x: Vec<Complex<f32>> = self.buf.iter().take(FRAME).zip(&self.window).map(|(s, w)| s * *w).collect();
            self.fft.process(&mut x);
            let half = FRAME / 2;
            let db = (0..FRAME).map(|k| 10.0 * (x[(k + half) % FRAME].norm_sqr() + 1e-12).log10()).collect();
            out.push(Frame { t, db });
            self.buf.drain(..HOP);
        }
        out
    }
}

/// A measured error of the own signal's place.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Estimate {
    /// The own signal came down this much above where it was expected (Hz).
    pub offset_hz: f64,
    /// Path delay found (s).
    pub delay_s: f64,
    /// How far the match stands out (standard deviations).
    pub score: f32,
}

fn bin_of(hz: f64) -> usize {
    ((hz / BIN_HZ).round() as isize + (FRAME / 2) as isize).clamp(0, FRAME as isize - 1) as usize
}

/// Values normalised to zero mean and unit spread.
fn standardise(values: &[f32]) -> Vec<f32> {
    let n = values.len().max(1) as f32;
    let mean = values.iter().sum::<f32>() / n;
    let var = values.iter().map(|v| (v - mean).powi(2)).sum::<f32>() / n;
    let sd = var.sqrt().max(1e-6);
    values.iter().map(|v| (v - mean) / sd).collect()
}

/// Finds the shift and delay at which the received spectra match the transmitted ones.
pub fn estimate(tx: &[Frame], rx: &[Frame]) -> Option<Estimate> {
    let (lo, hi) = (bin_of(BAND_HZ.0), bin_of(BAND_HZ.1));
    let max_lag = (MAX_OFFSET_HZ / BIN_HZ).round() as isize;
    let lags = 2 * max_lag as usize + 1;
    // Transmitted spectra with sound in them, every other one (100 ms apart).
    let power = |f: &Frame| {
        let p: f32 = f.db[lo..=hi].iter().map(|d| 10f32.powf(d / 10.0)).sum::<f32>() / (hi - lo + 1) as f32;
        10.0 * (p + 1e-12).log10()
    };
    let loudest = tx.iter().map(power).fold(f32::MIN, f32::max);
    let sent: Vec<(f64, Vec<f32>)> = tx
        .iter()
        .step_by(2)
        .filter(|f| power(f) >= loudest - SILENCE_DB)
        .map(|f| (f.t, standardise(&f.db[lo..=hi])))
        .collect();
    if (sent.len() as f64) * 0.1 < MIN_SOUND_S || rx.len() < 2 {
        return None;
    }
    // Received spectra over the band plus the search range.
    let (rlo, rhi) = ((lo as isize - max_lag).max(0) as usize, (hi as isize + max_lag).min(FRAME as isize - 1) as usize);
    let received: Vec<(f64, Vec<f32>)> = rx.iter().map(|f| (f.t, standardise(&f.db[rlo..=rhi]))).collect();
    let nearest = |t: f64| {
        let i = received.partition_point(|r| r.0 < t);
        [i.checked_sub(1), Some(i)].into_iter().flatten().filter(|&j| j < received.len()).min_by(|&a, &b| (received[a].0 - t).abs().total_cmp(&(received[b].0 - t).abs()))
    };
    let hop_s = HOP as f64 / RATE;
    let delays: Vec<f64> = (0..).map(|i| DELAY_S.0 + i as f64 * hop_s).take_while(|d| *d <= DELAY_S.1 + 1e-9).collect();
    let width = hi - lo + 1;
    let mut scores = vec![0f32; delays.len() * lags];
    for (di, d) in delays.iter().enumerate() {
        let mut used = 0;
        let row = &mut scores[di * lags..(di + 1) * lags];
        for (t, z) in &sent {
            let Some(j) = nearest(t + d) else { continue };
            if (received[j].0 - (t + d)).abs() > hop_s * 0.6 {
                continue;
            }
            used += 1;
            let r = &received[j].1;
            for (li, cell) in row.iter_mut().enumerate() {
                let start = lo as isize + li as isize - max_lag - rlo as isize;
                if start < 0 || start as usize + width > r.len() {
                    continue;
                }
                let s = start as usize;
                *cell += z.iter().zip(&r[s..s + width]).map(|(a, b)| a * b).sum::<f32>();
            }
        }
        if used > 0 {
            row.iter_mut().for_each(|c| *c /= used as f32);
        }
    }
    let n = scores.len() as f32;
    let mean = scores.iter().sum::<f32>() / n;
    let sd = (scores.iter().map(|s| (s - mean).powi(2)).sum::<f32>() / n).sqrt().max(1e-6);
    let (best, &peak) = scores.iter().enumerate().max_by(|a, b| a.1.total_cmp(b.1))?;
    let score = (peak - mean) / sd;
    if score < MIN_SCORE {
        return None;
    }
    let (di, li) = (best / lags, best % lags);
    // Between bins: a parabola through the peak and its neighbours.
    let at = |l: usize| scores[di * lags + l];
    let frac = if li > 0 && li + 1 < lags {
        let (a, b, c) = (at(li - 1), at(li), at(li + 1));
        let den = a - 2.0 * b + c;
        if den.abs() > 1e-9 { (0.5 * (a - c) / den).clamp(-0.5, 0.5) as f64 } else { 0.0 }
    } else {
        0.0
    };
    Some(Estimate { offset_hz: (li as f64 - max_lag as f64 + frac) * BIN_HZ, delay_s: delays[di], score })
}

/// A measurement handed to the window.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Measurement {
    /// Counts up with every new measurement.
    pub id: u64,
    pub estimate: Estimate,
}

struct Inner {
    /// Receiver frequency (IF) where the own suppressed carrier should come down.
    expected_hz: Option<f64>,
    tx_down: Option<Downconverter>,
    tx_framer: Framer,
    tx_frames: VecDeque<Frame>,
    rx_frames: VecDeque<Frame>,
    /// Time of the last transmitted spectrum, and of the first since the last
    /// measurement.
    last_tx: f64,
    first_tx: Option<f64>,
    latest: Option<Measurement>,
}

/// Shared between the transmit thread (what goes out), the receive side (what comes
/// back) and the window (where it should be, and the result).
#[derive(Clone)]
pub struct SelfCheck {
    inner: Arc<Mutex<Inner>>,
    epoch: Instant,
}

impl Default for SelfCheck {
    fn default() -> Self {
        Self {
            inner: Arc::new(Mutex::new(Inner {
                expected_hz: None,
                tx_down: None,
                tx_framer: Framer::default(),
                tx_frames: VecDeque::new(),
                rx_frames: VecDeque::new(),
                last_tx: f64::MIN,
                first_tx: None,
                latest: None,
            })),
            epoch: Instant::now(),
        }
    }
}

impl SelfCheck {
    fn now(&self) -> f64 {
        self.epoch.elapsed().as_secs_f64()
    }

    /// Where the own carrier should come down (IF); None stops measuring. A move of
    /// more than a few hundred Hz starts over.
    pub fn set_expected(&self, hz: Option<f64>) {
        let mut s = self.inner.lock().unwrap();
        let moved = match (s.expected_hz, hz) {
            (Some(a), Some(b)) => (a - b).abs() > 300.0,
            (a, b) => a.is_some() != b.is_some(),
        };
        if moved {
            s.tx_frames.clear();
            s.rx_frames.clear();
            s.first_tx = None;
        }
        s.expected_hz = hz;
    }

    /// What the modulator sends: complex baseband with the carrier at zero, at `rate`.
    pub fn push_tx(&self, baseband: &[Complex<f32>], rate: f64) {
        let now = self.now();
        let mut s = self.inner.lock().unwrap();
        if s.tx_down.as_ref().is_none_or(|d| d.in_rate() != rate) {
            s.tx_down = Some(Downconverter::new(rate));
        }
        let mut down = Vec::new();
        s.tx_down.as_mut().expect("tx downconverter").process(baseband, 0.0, &mut down);
        let frames = s.tx_framer.push(&down, now);
        if !frames.is_empty() {
            s.last_tx = now;
            s.first_tx.get_or_insert(frames[0].t);
        }
        s.tx_frames.extend(frames);
        while s.tx_frames.front().is_some_and(|f| f.t < now - KEEP_S) {
            s.tx_frames.pop_front();
        }
    }

    pub fn latest(&self) -> Option<Measurement> {
        self.inner.lock().unwrap().latest
    }

    /// Runs on the received blocks until they stop.
    pub fn spawn(&self, blocks: Receiver<Block>) {
        let me = self.clone();
        thread::Builder::new().name("selfcheck".into()).spawn(move || me.run(blocks)).expect("selfcheck thread");
    }

    fn run(self, blocks: Receiver<Block>) {
        let mut down: Option<Downconverter> = None;
        let mut framer = Framer::default();
        let mut buf = Vec::new();
        loop {
            let block = match blocks.recv_timeout(Duration::from_millis(500)) {
                Ok(b) => b,
                Err(RecvTimeoutError::Timeout) => continue,
                Err(RecvTimeoutError::Disconnected) => return,
            };
            let now = self.now();
            let Some(expected) = self.inner.lock().unwrap().expected_hz else {
                down = None;
                continue;
            };
            if (expected - block.center_hz).abs() > block.sample_rate / 2.0 - 8e3 {
                continue;
            }
            if down.as_ref().is_none_or(|d| d.in_rate() != block.sample_rate) {
                down = Some(Downconverter::new(block.sample_rate));
                framer = Framer::default();
            }
            buf.clear();
            down.as_mut().expect("rx downconverter").process(&block.samples, expected - block.center_hz, &mut buf);
            let frames = framer.push(&buf, now);
            let job = {
                let mut s = self.inner.lock().unwrap();
                s.rx_frames.extend(frames);
                while s.rx_frames.front().is_some_and(|f| f.t < now - KEEP_S) {
                    s.rx_frames.pop_front();
                }
                // Measure once the transmission has ended and its echo is in, or every
                // so often during a long one.
                let ended = now - s.last_tx > DELAY_S.1 + 0.2;
                match s.first_tx {
                    Some(first) if (ended && s.last_tx - first >= MIN_SOUND_S) || now - first >= EVERY_S + DELAY_S.1 => {
                        s.first_tx = if ended { None } else { Some(now) };
                        let from = s.last_tx - ANALYSE_S;
                        let tx: Vec<Frame> = s.tx_frames.iter().filter(|f| f.t >= from).cloned().collect();
                        let rx: Vec<Frame> = s.rx_frames.iter().filter(|f| f.t >= from).cloned().collect();
                        Some((tx, rx))
                    }
                    Some(first) if ended && s.last_tx - first < MIN_SOUND_S => {
                        s.first_tx = None;
                        None
                    }
                    _ => None,
                }
            };
            if let Some((tx, rx)) = job
                && let Some(e) = estimate(&tx, &rx)
            {
                let mut s = self.inner.lock().unwrap();
                let id = s.latest.map_or(1, |m| m.id + 1);
                s.latest = Some(Measurement { id, estimate: e });
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::f64::consts::PI;

    /// Voice-like SSB baseband at `RATE`: harmonics of a gliding pitch under a
    /// formant envelope, in syllables.
    fn voice(seconds: f64) -> Vec<Complex<f32>> {
        let n = (seconds * RATE) as usize;
        let mut phases = vec![0f64; 40];
        (0..n)
            .map(|i| {
                let t = i as f64 / RATE;
                let pitch = 140.0 + 35.0 * (2.0 * PI * 0.6 * t).sin();
                let syllable = (0.5 + 0.5 * (2.0 * PI * 3.0 * t).sin()).powi(2) as f32;
                let mut z = Complex::default();
                for (k, ph) in phases.iter_mut().enumerate() {
                    let f = pitch * (k + 1) as f64;
                    *ph += 2.0 * PI * f / RATE;
                    if !(300.0..2_700.0).contains(&f) {
                        continue;
                    }
                    let formant = (-((f - 700.0) / 400.0).powi(2)).exp() + 0.6 * (-((f - 1_800.0) / 500.0).powi(2)).exp();
                    z += Complex::from_polar(formant as f32, *ph as f32);
                }
                z * syllable * 0.1
            })
            .collect()
    }

    fn noise(n: usize, level: f32, seed: &mut u32) -> Vec<Complex<f32>> {
        let mut r = || {
            *seed ^= *seed << 13;
            *seed ^= *seed >> 17;
            *seed ^= *seed << 5;
            *seed as f32 / u32::MAX as f32 - 0.5
        };
        (0..n).map(|_| Complex::new(r(), r()) * level).collect()
    }

    /// Spectra of a stream starting at time `t0`, fed in 10 ms pieces.
    fn frames(x: &[Complex<f32>], t0: f64) -> Vec<Frame> {
        let mut f = Framer::default();
        let mut out = Vec::new();
        for (i, piece) in x.chunks(120).enumerate() {
            let t_last = t0 + ((i * 120 + piece.len()) as f64 - 1.0) / RATE;
            out.extend(f.push(piece, t_last));
        }
        out
    }

    /// The echo: shifted by `offset` Hz, delayed by `delay` s, in noise.
    fn echo(x: &[Complex<f32>], offset: f64, delay: f64, noise_level: f32) -> Vec<Frame> {
        let mut seed = 99;
        let shifted: Vec<Complex<f32>> =
            x.iter().enumerate().map(|(i, z)| z * Complex::from_polar(0.3, (2.0 * PI * offset * i as f64 / RATE) as f32)).collect();
        let n = noise(shifted.len(), noise_level, &mut seed);
        let rx: Vec<Complex<f32>> = shifted.iter().zip(&n).map(|(a, b)| a + b).collect();
        frames(&rx, delay)
    }

    #[test]
    fn finds_the_shift_of_a_voice() {
        let x = voice(5.0);
        let tx = frames(&x, 0.0);
        for offset in [137.0, -930.0, 1_480.0] {
            let rx = echo(&x, offset, 0.31, 0.02);
            let e = estimate(&tx, &rx).expect("a match");
            assert!((e.offset_hz - offset).abs() < 4.0, "{offset}: {e:?}");
            assert!((e.delay_s - 0.31).abs() < 0.06, "{e:?}");
        }
    }

    #[test]
    fn finds_the_shift_of_the_tune_tone() {
        let tone: Vec<Complex<f32>> = (0..(3.0 * RATE) as usize).map(|i| Complex::from_polar(0.3, (2.0 * PI * 1_000.0 * i as f64 / RATE) as f32)).collect();
        let tx = frames(&tone, 0.0);
        let rx = echo(&tone, -423.0, 0.27, 0.02);
        let e = estimate(&tx, &rx).expect("a match");
        assert!((e.offset_hz + 423.0).abs() < 3.0, "{e:?}");
    }

    #[test]
    fn nothing_without_the_echo() {
        let x = voice(5.0);
        let tx = frames(&x, 0.0);
        let mut seed = 5;
        let rx = frames(&noise(x.len(), 0.05, &mut seed), 0.3);
        assert_eq!(estimate(&tx, &rx), None);
        // Too short a transmission.
        let short = frames(&x[..(0.8 * RATE) as usize], 0.0);
        assert_eq!(estimate(&short, &echo(&x, 0.0, 0.3, 0.02)), None);
    }

    #[test]
    fn downconverter_brings_the_carrier_to_zero() {
        let rate = 600e3;
        let carrier = 37_000.0;
        let x: Vec<Complex<f32>> = (0..(rate * 0.5) as usize)
            .map(|i| Complex::from_polar(0.5, (2.0 * PI * (carrier + 1_000.0) * i as f64 / rate) as f32))
            .collect();
        let mut d = Downconverter::new(rate);
        let mut out = Vec::new();
        for piece in x.chunks(12_000) {
            d.process(piece, carrier, &mut out);
        }
        assert!((out.len() as f64 - RATE * 0.5).abs() < 20.0);
        let f = frames(&out, 0.0);
        let last = f.last().unwrap();
        let peak = last.db.iter().enumerate().max_by(|a, b| a.1.total_cmp(b.1)).unwrap().0;
        assert_eq!(peak, bin_of(1_000.0));
    }

    /// Speed of one measurement: `cargo test --release --lib estimate_speed -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn estimate_speed() {
        let x = voice(8.0);
        let tx = frames(&x, 0.0);
        let rx = echo(&x, 300.0, 0.3, 0.02);
        let t = Instant::now();
        let e = estimate(&tx, &rx);
        println!("{e:?} in {:.0} ms", t.elapsed().as_secs_f64() * 1e3);
    }
}
