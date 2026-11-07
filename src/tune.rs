//! Automatic lock: where to put the dial for a signal clicked on the spectrum.
//!
//! An SSB voice starts about 300 Hz above its suppressed carrier (USB), so the dial goes
//! 300 Hz below the signal's lower edge; LSB mirrors it; CW goes on the carrier itself.

use crate::demod::Mode;
use crate::tx::AUDIO_LOW;

/// How far from the click a signal is looked for (Hz).
const SEARCH_HZ: f64 = 1_500.0;
/// Above the noise floor by this much counts as signal (dB).
const ABOVE_DB: f32 = 6.0;
/// Gaps in a signal (between syllables, formants) up to this wide are bridged (Hz).
const BRIDGE_HZ: f64 = 200.0;
/// Widest signal taken as one (Hz).
const MAX_WIDTH_HZ: f64 = 4_000.0;

fn median(values: &[f32]) -> f32 {
    let mut v = values.to_vec();
    v.sort_by(f32::total_cmp);
    v.get(v.len() / 2).copied().unwrap_or(-150.0)
}

/// The dial for the signal nearest to `click_hz` on a spectrum line `db` spanning
/// `lo_hz..hi_hz`; None when nothing stands out of the noise there.
pub fn snap(db: &[f32], lo_hz: f64, hi_hz: f64, click_hz: f64, mode: Mode) -> Option<f64> {
    let n = db.len();
    if n < 8 || hi_hz <= lo_hz {
        return None;
    }
    let bin = (hi_hz - lo_hz) / n as f64;
    let index = |hz: f64| ((hz - lo_hz) / bin).floor().clamp(0.0, n as f64 - 1.0) as usize;
    let threshold = median(db) + ABOVE_DB;
    let (from, to) = (index(click_hz - SEARCH_HZ), index(click_hz + SEARCH_HZ));
    let click = index(click_hz);
    let seed = (from..=to).filter(|&i| db[i] >= threshold).min_by_key(|&i| i.abs_diff(click))?;
    let bridge = (BRIDGE_HZ / bin).ceil() as usize;
    let max_bins = (MAX_WIDTH_HZ / bin).ceil() as usize;
    // Grows the signal from the seed, over short gaps.
    let grow = |step: isize| {
        let (mut edge, mut gap, mut i) = (seed, 0, seed as isize);
        loop {
            i += step;
            if i < 0 || i >= n as isize || i.unsigned_abs().abs_diff(seed) > max_bins {
                return edge;
            }
            if db[i as usize] >= threshold {
                edge = i as usize;
                gap = 0;
            } else {
                gap += 1;
                if gap > bridge {
                    return edge;
                }
            }
        }
    };
    let (left, right) = (grow(-1), grow(1));
    let low = lo_hz + left as f64 * bin;
    let high = lo_hz + (right + 1) as f64 * bin;
    let dial = match mode {
        Mode::Usb => low - AUDIO_LOW,
        Mode::Lsb => high + AUDIO_LOW,
        Mode::Cw => {
            // Power-weighted middle of the carrier (a flat top would otherwise give one
            // of its ends).
            let (mut sum, mut weighted) = (0.0, 0.0);
            for i in left..=right {
                let p = 10f64.powf(db[i] as f64 / 10.0);
                sum += p;
                weighted += p * (lo_hz + (i as f64 + 0.5) * bin);
            }
            weighted / sum
        }
    };
    Some((dial / 10.0).round() * 10.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 10 Hz bins from 0 to 20 kHz, noise at -100 dB, signals as (from, to, level).
    fn line(signals: &[(f64, f64, f32)]) -> Vec<f32> {
        (0..2000)
            .map(|i| {
                let hz = i as f64 * 10.0 + 5.0;
                let noise = -100.0 + ((i * 7919) % 5) as f32 * 0.5;
                signals.iter().filter(|s| hz >= s.0 && hz < s.1).map(|s| s.2).fold(noise, f32::max)
            })
            .collect()
    }

    #[test]
    fn usb_voice_dial_is_below_its_lower_edge() {
        // Carrier at 5000: voice from 5300 to 7700 with a quiet gap between formants.
        let db = line(&[(5_300.0, 6_000.0, -70.0), (6_100.0, 7_700.0, -75.0)]);
        assert_eq!(snap(&db, 0.0, 20_000.0, 6_500.0, Mode::Usb), Some(5_000.0));
        // A click a little beside the signal still finds it.
        assert_eq!(snap(&db, 0.0, 20_000.0, 8_500.0, Mode::Usb), Some(5_000.0));
    }

    #[test]
    fn lsb_and_cw() {
        let db = line(&[(12_300.0, 14_700.0, -72.0), (17_000.0, 17_040.0, -60.0)]);
        assert_eq!(snap(&db, 0.0, 20_000.0, 13_000.0, Mode::Lsb), Some(15_000.0));
        assert_eq!(snap(&db, 0.0, 20_000.0, 17_100.0, Mode::Cw), Some(17_020.0));
    }

    #[test]
    fn empty_place_gives_nothing() {
        let db = line(&[(5_300.0, 7_700.0, -70.0)]);
        assert_eq!(snap(&db, 0.0, 20_000.0, 15_000.0, Mode::Usb), None);
    }
}
