//! Deciding when to transmit: VOX (voice-operated switching) for headsets, the PTT, a
//! time-out timer and the band plan.

use std::collections::VecDeque;
use std::time::{Duration, Instant};

use crate::bandplan::Refusal;

/// VOX settings.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct VoxSettings {
    pub enabled: bool,
    /// Microphone level that keys the transmitter (dBFS).
    pub threshold_db: f32,
    /// How long the transmitter stays on after the voice stops (ms).
    pub hang_ms: u32,
    /// How far back the microphone audio is delayed, so the first syllable is not lost
    /// while VOX switches on (ms).
    pub preroll_ms: u32,
    /// Anti-VOX: the voice must be this much (dB) above what the speaker plays to key the
    /// transmitter. `None` turns it off (headsets rarely need it).
    pub anti_vox_db: Option<f32>,
}

impl Default for VoxSettings {
    fn default() -> Self {
        Self { enabled: false, threshold_db: -32.0, hang_ms: 600, preroll_ms: 150, anti_vox_db: None }
    }
}

/// Length of one level measurement (s).
const FRAME_S: f64 = 0.010;

/// Voice detector.
pub struct Vox {
    pub settings: VoxSettings,
    rate: f64,
    frame: usize,
    /// Sum of squares and count of the frame being measured.
    acc: (f64, usize),
    /// Level of the last full frame (dBFS).
    pub level_db: f32,
    /// Samples since the voice was last above the threshold.
    since_voice: u64,
}

impl Vox {
    pub fn new(settings: VoxSettings, rate: f64) -> Self {
        let frame = ((rate * FRAME_S) as usize).max(1);
        Self { settings, rate, frame, acc: (0.0, 0), level_db: -150.0, since_voice: u64::MAX / 2 }
    }

    /// Takes microphone audio; `speaker_db` is the level the speaker is playing (for
    /// anti-VOX). Returns whether VOX keys the transmitter at the end of `mic`.
    pub fn process(&mut self, mic: &[f32], speaker_db: f32) -> bool {
        for &x in mic {
            self.acc.0 += (x as f64) * (x as f64);
            self.acc.1 += 1;
            self.since_voice = self.since_voice.saturating_add(1);
            if self.acc.1 == self.frame {
                self.level_db = (10.0 * (self.acc.0 / self.frame as f64 + 1e-15).log10()) as f32;
                self.acc = (0.0, 0);
                let above = self.level_db >= self.settings.threshold_db;
                let over_speaker = self.settings.anti_vox_db.is_none_or(|margin| self.level_db >= speaker_db + margin);
                if above && over_speaker {
                    self.since_voice = 0;
                }
            }
        }
        self.keyed()
    }

    pub fn keyed(&self) -> bool {
        let hang = (self.settings.hang_ms as f64 / 1e3 * self.rate) as u64;
        self.settings.enabled && self.since_voice <= hang + self.frame as u64
    }
}

/// Fixed delay for the microphone audio (the VOX pre-roll).
pub struct DelayLine {
    buffer: VecDeque<f32>,
    length: usize,
}

impl DelayLine {
    pub fn new(length: usize) -> Self {
        Self { buffer: std::iter::repeat_n(0.0, length).collect(), length }
    }

    pub fn set_length(&mut self, length: usize) {
        while self.buffer.len() < length {
            self.buffer.push_front(0.0);
        }
        while self.buffer.len() > length {
            self.buffer.pop_front();
        }
        self.length = length;
    }

    /// Pushes `input` and returns as many delayed samples.
    pub fn process(&mut self, input: &[f32], out: &mut Vec<f32>) {
        for &x in input {
            self.buffer.push_back(x);
            out.push(if self.buffer.len() > self.length { self.buffer.pop_front().unwrap_or(0.0) } else { 0.0 });
        }
    }

    /// Silences what is waiting (after a transmission, so old audio is not sent later).
    pub fn clear(&mut self) {
        self.buffer.iter_mut().for_each(|x| *x = 0.0);
    }
}

/// Why the transmitter stays off although asked to transmit.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Blocked {
    /// Transmitting is not enabled (the TX switch in the window).
    NotArmed,
    /// The signal would leave the transponder or fall on a beacon.
    Band(Refusal),
    /// The time-out timer ran out; released by letting go of PTT and VOX.
    TimedOut,
    /// No radio able to transmit.
    NoRadio,
}

/// Transmitter state.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TxState {
    Receive,
    Transmit,
    Blocked(Blocked),
}

/// Combines PTT, VOX, the time-out timer and the band plan.
pub struct TxControl {
    pub time_out: Duration,
    keyed_since: Option<Instant>,
    timed_out: bool,
}

/// Default time-out timer.
pub const DEFAULT_TIME_OUT: Duration = Duration::from_secs(180);

impl Default for TxControl {
    fn default() -> Self {
        Self { time_out: DEFAULT_TIME_OUT, keyed_since: None, timed_out: false }
    }
}

impl TxControl {
    /// `request` = PTT pressed or VOX keyed; `radio_ok` = a transmitter is open; `armed` =
    /// transmitting enabled by the operator; `band` = band plan check of the signal.
    pub fn update(&mut self, now: Instant, request: bool, armed: bool, radio_ok: bool, band: Result<(), Refusal>) -> TxState {
        if !request {
            self.keyed_since = None;
            self.timed_out = false;
            return TxState::Receive;
        }
        let blocked = if !radio_ok {
            Some(Blocked::NoRadio)
        } else if !armed {
            Some(Blocked::NotArmed)
        } else if self.timed_out {
            Some(Blocked::TimedOut)
        } else {
            band.err().map(Blocked::Band)
        };
        if let Some(b) = blocked {
            self.keyed_since = None;
            return TxState::Blocked(b);
        }
        let since = *self.keyed_since.get_or_insert(now);
        if now.duration_since(since) >= self.time_out {
            self.timed_out = true;
            self.keyed_since = None;
            return TxState::Blocked(Blocked::TimedOut);
        }
        TxState::Transmit
    }

    /// Time left before the time-out timer stops the transmission.
    pub fn remaining(&self, now: Instant) -> Option<Duration> {
        self.keyed_since.map(|s| self.time_out.saturating_sub(now.duration_since(s)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const RATE: f64 = 48e3;

    fn noise_free_tone(amp: f32, seconds: f64) -> Vec<f32> {
        (0..(seconds * RATE) as usize).map(|i| amp * (i as f32 * 0.13).sin()).collect()
    }

    fn vox() -> Vox {
        Vox::new(VoxSettings { enabled: true, ..Default::default() }, RATE)
    }

    #[test]
    fn voice_keys_and_hang_holds() {
        let mut v = vox();
        assert!(!v.process(&noise_free_tone(0.001, 0.2), -150.0), "-63 dBFS must not key");
        assert!(v.process(&noise_free_tone(0.1, 0.05), -150.0), "-23 dBFS keys");
        assert!(v.process(&vec![0.0; (0.5 * RATE) as usize], -150.0), "held for 600 ms");
        assert!(!v.process(&vec![0.0; (0.2 * RATE) as usize], -150.0), "released after the hang");
    }

    #[test]
    fn disabled_vox_never_keys() {
        let mut v = Vox::new(VoxSettings::default(), RATE);
        assert!(!v.process(&noise_free_tone(0.5, 0.1), -150.0));
        assert!(v.level_db > -10.0, "the level is still measured");
    }

    #[test]
    fn anti_vox_ignores_the_speaker() {
        let mut v = Vox::new(VoxSettings { enabled: true, anti_vox_db: Some(6.0), ..Default::default() }, RATE);
        // Speaker at -20 dBFS leaks into the microphone at -23 dBFS.
        assert!(!v.process(&noise_free_tone(0.1, 0.1), -20.0));
        // The operator speaks louder than the leak.
        assert!(v.process(&noise_free_tone(0.5, 0.1), -20.0));
    }

    #[test]
    fn delay_line_delays_and_clears() {
        let mut d = DelayLine::new(3);
        let mut out = Vec::new();
        d.process(&[1.0, 2.0, 3.0, 4.0, 5.0], &mut out);
        assert_eq!(out, [0.0, 0.0, 0.0, 1.0, 2.0]);
        d.clear();
        out.clear();
        d.process(&[6.0], &mut out);
        assert_eq!(out, [0.0]);
        d.set_length(1);
        out.clear();
        d.process(&[7.0, 8.0], &mut out);
        assert_eq!(out, [6.0, 7.0]);
    }

    #[test]
    fn control_blocks_and_times_out() {
        let t0 = Instant::now();
        let mut c = TxControl { time_out: Duration::from_secs(10), ..Default::default() };
        assert_eq!(c.update(t0, false, true, true, Ok(())), TxState::Receive);
        assert_eq!(c.update(t0, true, false, true, Ok(())), TxState::Blocked(Blocked::NotArmed));
        assert_eq!(c.update(t0, true, true, false, Ok(())), TxState::Blocked(Blocked::NoRadio));
        assert_eq!(c.update(t0, true, true, true, Err(Refusal::OnBeacon)), TxState::Blocked(Blocked::Band(Refusal::OnBeacon)));
        assert_eq!(c.update(t0, true, true, true, Ok(())), TxState::Transmit);
        assert_eq!(c.remaining(t0 + Duration::from_secs(4)), Some(Duration::from_secs(6)));
        assert_eq!(c.update(t0 + Duration::from_secs(9), true, true, true, Ok(())), TxState::Transmit);
        let late = t0 + Duration::from_secs(10);
        assert_eq!(c.update(late, true, true, true, Ok(())), TxState::Blocked(Blocked::TimedOut));
        assert_eq!(c.update(late, true, true, true, Ok(())), TxState::Blocked(Blocked::TimedOut), "stays off while held");
        assert_eq!(c.update(late, false, true, true, Ok(())), TxState::Receive);
        assert_eq!(c.update(late, true, true, true, Ok(())), TxState::Transmit, "released and pressed again");
    }
}
