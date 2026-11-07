//! Received samples as they are handed between threads, and the spectrum worker.

use std::sync::mpsc::{Receiver, RecvTimeoutError};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use num_complex::Complex;

use crate::dsp::{Spectrum, fft_size_for, max_pool};

/// Spectrum frames per second.
const FRAMES_PER_SECOND: f64 = 25.0;
/// Most values sent per spectrum frame (about the width of a large screen).
pub const FRAME_POINTS: usize = 2048;

/// One block of received IQ samples.
#[derive(Clone)]
pub struct Block {
    pub samples: Arc<Vec<Complex<f32>>>,
    /// Frequency at the block's centre, in the receiver's own frequencies (IF) (Hz).
    pub center_hz: f64,
    pub sample_rate: f64,
}

/// The part of the received band shown on the spectrum.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct View {
    /// View centre relative to the tuned centre (Hz).
    pub offset_hz: f64,
    pub span_hz: f64,
}

impl View {
    pub fn full(sample_rate: f64) -> Self {
        Self { offset_hz: 0.0, span_hz: sample_rate }
    }

    /// Keeps the view inside the received band.
    pub fn clamped(self, sample_rate: f64) -> Self {
        let span = self.span_hz.clamp(1_000.0, sample_rate);
        let room = (sample_rate - span) / 2.0;
        Self { offset_hz: self.offset_hz.clamp(-room, room), span_hz: span }
    }
}

/// One spectrum line.
pub struct Frame {
    pub center_hz: f64,
    pub sample_rate: f64,
    /// Receiver frequencies (IF) of the first and last value's outer edges (Hz).
    pub lo_hz: f64,
    pub hi_hz: f64,
    /// Power in dBFS, evenly spaced from `lo_hz` to `hi_hz`.
    pub db: Vec<f32>,
    /// Width of one FFT bin (Hz).
    pub bin_hz: f64,
}

/// Computes spectrum frames of `view` from the blocks and hands them to `send`; returns
/// when the blocks stop or `send` refuses.
pub fn spectrum_worker(blocks: Receiver<Block>, view: Arc<Mutex<View>>, send: impl Fn(Frame) -> bool) {
    let mut spectrum = Spectrum::new(2048);
    // Settings the accumulated blocks were received with; a change starts over.
    let mut current: Option<(f64, f64, usize)> = None;
    let mut since_frame = 0usize;
    loop {
        let block = match blocks.recv_timeout(Duration::from_millis(500)) {
            Ok(b) => b,
            Err(RecvTimeoutError::Timeout) => continue,
            Err(RecvTimeoutError::Disconnected) => return,
        };
        let v = view.lock().unwrap().clamped(block.sample_rate);
        let size = fft_size_for(block.sample_rate, v.span_hz, FRAME_POINTS);
        let settings = (block.center_hz, block.sample_rate, size);
        if current != Some(settings) {
            if current.is_none_or(|c| c.2 != size) {
                spectrum = Spectrum::new(size);
            } else {
                let _ = spectrum.take();
            }
            current = Some(settings);
            since_frame = 0;
        }
        spectrum.push(&block.samples);
        since_frame += block.samples.len();
        if (since_frame as f64) < block.sample_rate / FRAMES_PER_SECOND {
            continue;
        }
        let Some(db) = spectrum.take() else { continue };
        since_frame = 0;
        let n = db.len() as f64;
        let bin_hz = block.sample_rate / n;
        let index = |offset: f64| offset / bin_hz + n / 2.0;
        let lo = index(v.offset_hz - v.span_hz / 2.0).floor().clamp(0.0, n) as usize;
        let hi = index(v.offset_hz + v.span_hz / 2.0).ceil().clamp(0.0, n) as usize;
        let edge = |i: usize| block.center_hz + (i as f64 - n / 2.0 - 0.5) * bin_hz;
        let frame = Frame {
            center_hz: block.center_hz,
            sample_rate: block.sample_rate,
            lo_hz: edge(lo),
            hi_hz: edge(hi),
            db: max_pool(&db, lo, hi, FRAME_POINTS),
            bin_hz,
        };
        if !send(frame) {
            return;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn view_stays_inside_the_band() {
        let v = View { offset_hz: 2e6, span_hz: 1e6 }.clamped(2.4e6);
        assert_eq!(v.offset_hz, 0.7e6);
        let v = View { offset_hz: 0.5e6, span_hz: 5e6 }.clamped(2.4e6);
        assert_eq!(v, View::full(2.4e6));
    }

    #[test]
    fn worker_frames_a_tone_at_its_place() {
        let rate = 2.4e6;
        let (tx, rx) = std::sync::mpsc::sync_channel(64);
        let tone: Vec<Complex<f32>> =
            (0..rate as usize / 10).map(|i| Complex::from_polar(0.5, (2.0 * std::f64::consts::PI * 300e3 * i as f64 / rate) as f32)).collect();
        for c in tone.chunks(48_000) {
            tx.send(Block { samples: Arc::new(c.to_vec()), center_hz: 740e6, sample_rate: rate }).unwrap();
        }
        drop(tx);
        let view = Arc::new(Mutex::new(View { offset_hz: 300e3, span_hz: 520e3 }));
        let frames = Mutex::new(Vec::new());
        spectrum_worker(rx, view, |f| {
            frames.lock().unwrap().push(f);
            true
        });
        let frames = frames.into_inner().unwrap();
        let f = frames.last().expect("a frame");
        assert!((f.lo_hz - (740e6 + 40e3)).abs() < 2e3 && (f.hi_hz - (740e6 + 560e3)).abs() < 2e3, "{} {}", f.lo_hz, f.hi_hz);
        let peak = f.db.iter().enumerate().max_by(|a, b| a.1.total_cmp(b.1)).unwrap().0;
        let at = f.lo_hz + (peak as f64 + 0.5) / f.db.len() as f64 * (f.hi_hz - f.lo_hz);
        assert!((at - 740.3e6).abs() < 1e3, "peak at {at}");
    }
}
