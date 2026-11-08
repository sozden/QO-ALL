//! The listening thread: takes blocks from the receiver, demodulates the dial frequency
//! and keeps the sound card's queue at a steady level.

use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::mpsc::{Receiver, RecvTimeoutError};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use crate::audio::AudioBuffer;
use crate::demod::{Chain, ChainConfig, Mode};
use crate::source::Block;

/// Audio kept queued for the sound card (seconds): enough to ride out scheduling jitter,
/// short enough that tuning feels immediate.
const TARGET_DELAY: f64 = 0.12;
/// Above this the oldest audio is dropped (seconds).
const MAX_DELAY: f64 = 0.5;
/// Largest speed correction used to hold the queue level (network radios can deliver
/// slightly fewer samples than nominal).
const MAX_SPEED_CORRECTION: f64 = 0.03;
/// Speed correction per unit of relative queue error.
const SPEED_GAIN: f64 = 0.03;

/// What to listen to; the window writes it, the thread reads it for every block.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ListenSettings {
    /// Dial frequency in the receiver's own frequencies (IF, Hz).
    pub dial_hz: f64,
    pub mode: Mode,
    pub bandwidth_hz: f64,
    pub audio_rate: f64,
    /// Linear volume, used for the speaker level reported to anti-VOX.
    pub volume: f32,
}

/// Text decoded from a CW signal.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct CwInfo {
    pub text: String,
    pub wpm: f64,
    pub key_down: bool,
}

#[derive(Clone)]
pub struct Listener {
    pub settings: Arc<Mutex<ListenSettings>>,
    /// Channel power of the listened signal (dBFS).
    level: Arc<AtomicU32>,
    /// Level of the audio sent to the speaker (dBFS, after the volume).
    speaker: Arc<AtomicU32>,
    cw: Arc<Mutex<Option<CwInfo>>>,
    /// CW sidetone on (the key is down), and samples of alert beep left.
    sidetone: Arc<AtomicBool>,
    beep: Arc<AtomicU32>,
}

/// Sidetone pitch and level, alert beep tones and length.
const SIDETONE_HZ: f64 = crate::keyer::TONE_HZ;
const SIDETONE_LEVEL: f32 = 0.25;
const BEEP_HZ: (f64, f64) = (880.0, 1_320.0);
const BEEP_S: f64 = 0.36;

fn atomic(v: f32) -> Arc<AtomicU32> {
    Arc::new(AtomicU32::new(v.to_bits()))
}

impl Listener {
    pub fn new(settings: ListenSettings) -> Self {
        Self {
            settings: Arc::new(Mutex::new(settings)),
            level: atomic(-150.0),
            speaker: atomic(-150.0),
            cw: Arc::default(),
            sidetone: Arc::default(),
            beep: Arc::default(),
        }
    }

    pub fn update(&self, f: impl FnOnce(&mut ListenSettings)) {
        f(&mut self.settings.lock().unwrap());
    }

    pub fn level_db(&self) -> f32 {
        f32::from_bits(self.level.load(Ordering::Relaxed))
    }

    pub fn speaker_db(&self) -> f32 {
        f32::from_bits(self.speaker.load(Ordering::Relaxed))
    }

    /// Text decoded from the CW signal listened to, None in other modes.
    pub fn cw(&self) -> Option<CwInfo> {
        self.cw.lock().unwrap().clone()
    }

    /// The CW sidetone follows the key.
    pub fn set_sidetone(&self, on: bool) {
        self.sidetone.store(on, Ordering::Relaxed);
    }

    /// A short two-tone beep on the speaker (alerts).
    pub fn beep(&self) {
        let rate = self.settings.lock().unwrap().audio_rate;
        self.beep.store((BEEP_S * rate) as u32, Ordering::Relaxed);
    }

    /// Runs until the receiver stops (its block channel closes).
    pub fn spawn(&self, blocks: Receiver<Block>, audio: AudioBuffer) {
        let me = self.clone();
        thread::Builder::new().name("listen".into()).spawn(move || run(blocks, me, audio)).expect("listen thread");
    }
}

fn run(blocks: Receiver<Block>, me: Listener, audio: AudioBuffer) {
    let mut chain: Option<Chain> = None;
    let mut cw_dial = f64::NAN;
    let mut out = Vec::new();
    // Phases of the sidetone and of the beep.
    let (mut side_phase, mut beep_phase) = (0.0f64, 0.0f64);
    loop {
        let block = match blocks.recv_timeout(Duration::from_millis(500)) {
            Ok(b) => b,
            Err(RecvTimeoutError::Timeout) => continue,
            Err(RecvTimeoutError::Disconnected) => return,
        };
        let s = *me.settings.lock().unwrap();
        let config = ChainConfig { mode: s.mode, bandwidth: s.bandwidth_hz, input_rate: block.sample_rate, audio_rate: s.audio_rate };
        if chain.as_ref().is_none_or(|c| c.config != config) {
            chain = Some(Chain::new(config));
            audio.clear();
            audio.set_prime((s.audio_rate * TARGET_DELAY) as usize);
        }
        let chain = chain.as_mut().expect("chain");
        // Moved to another signal: the CW text starts over.
        if (s.dial_hz - cw_dial).abs() > 200.0 {
            cw_dial = s.dial_hz;
            if chain.cw.is_some() {
                chain.cw = Some(crate::cw::CwDecoder::new(chain.channel_rate()));
            }
        }
        let offset = s.dial_hz - block.center_hz;
        if offset.abs() > block.sample_rate / 2.0 {
            me.level.store((-150.0f32).to_bits(), Ordering::Relaxed);
            continue;
        }
        let queued = audio.len() as f64 / s.audio_rate;
        if queued > MAX_DELAY {
            audio.trim_to((s.audio_rate * TARGET_DELAY) as usize);
        }
        let error = (queued - TARGET_DELAY) / TARGET_DELAY;
        let speed = 1.0 + (error * SPEED_GAIN).clamp(-MAX_SPEED_CORRECTION, MAX_SPEED_CORRECTION);

        out.clear();
        chain.process(&block.samples, offset, speed, None, &mut out);
        me.level.store(chain.level_db.to_bits(), Ordering::Relaxed);
        let power = out.iter().map(|v| v * v).sum::<f32>() / out.len().max(1) as f32;
        let speaker = 10.0 * (power * s.volume * s.volume + 1e-15).log10();
        me.speaker.store(speaker.to_bits(), Ordering::Relaxed);
        // Sidetone and alert beep, on top of what is heard.
        let sidetone = me.sidetone.load(Ordering::Relaxed);
        let mut beep = me.beep.load(Ordering::Relaxed);
        if sidetone || beep > 0 {
            let total = (BEEP_S * s.audio_rate) as u32;
            for v in out.iter_mut() {
                if sidetone {
                    side_phase = (side_phase + 2.0 * std::f64::consts::PI * SIDETONE_HZ / s.audio_rate) % (2.0 * std::f64::consts::PI);
                    *v += SIDETONE_LEVEL * side_phase.sin() as f32;
                }
                if beep > 0 {
                    // Two tones, with a short pause between them.
                    let into = total.saturating_sub(beep) as f64 / total as f64;
                    let hz = if into < 0.45 { BEEP_HZ.0 } else { BEEP_HZ.1 };
                    beep_phase = (beep_phase + 2.0 * std::f64::consts::PI * hz / s.audio_rate) % (2.0 * std::f64::consts::PI);
                    if !(0.45..0.55).contains(&into) {
                        *v += 0.3 * beep_phase.sin() as f32;
                    }
                    beep -= 1;
                }
            }
            me.beep.store(beep, Ordering::Relaxed);
        }
        audio.push(&out);
        *me.cw.lock().unwrap() = chain.cw.as_ref().map(|d| CwInfo { text: d.text().to_string(), wpm: d.wpm(), key_down: d.key_down() });
    }
}
