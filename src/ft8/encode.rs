//! FT8 and FT4 transmission: message text to tones to a GFSK audio waveform, as WSJT-X
//! shapes it (Gaussian-filtered frequency steps, BT 2 for FT8 and 1 for FT4, the
//! amplitude ramped up and down over the ends).

use std::f64::consts::PI;

use super::message::{pack_standard, pack_text};
use super::{FT4, Protocol, ldpc};

/// Seconds per symbol.
fn symbol_s(p: &Protocol) -> f64 {
    p.sps as f64 / p.rate
}

/// The tones of a message, or None if it cannot be encoded (standard messages and free
/// text of up to 13 characters).
pub fn tones(p: &Protocol, text: &str) -> Option<Vec<usize>> {
    let mut bits = pack_standard(text).or_else(|| pack_text(text))?;
    if let Some(r) = p.scramble {
        bits.iter_mut().zip(r.iter()).for_each(|(b, r)| *b ^= r);
    }
    Some(p.tones_of(&ldpc::encode(&bits)))
}

/// Area under the Gaussian frequency pulse of one symbol, at `t` symbols from its
/// middle (WSJT-X's gfsk_pulse).
fn pulse(bt: f64, t: f64) -> f64 {
    let c = PI * (2.0 / 2f64.ln()).sqrt();
    0.5 * (erf(c * bt * (t + 0.5)) - erf(c * bt * (t - 0.5)))
}

/// The error function (Abramowitz and Stegun 7.1.26 refined: |error| < 1.2e-7).
fn erf(x: f64) -> f64 {
    let t = 1.0 / (1.0 + 0.5 * x.abs());
    let y = 1.0
        - t * (-x * x - 1.265_512_23
            + t * (1.000_023_68
                + t * (0.374_091_96
                    + t * (0.096_784_18 + t * (-0.186_288_06 + t * (0.278_868_07 + t * (-1.135_203_98 + t * (1.488_515_87 + t * (-0.822_152_23 + t * 0.170_872_77)))))))))
            .exp();
    if x >= 0.0 { y } else { -y }
}

/// Audio of a transmission at `rate`, tone 0 at `audio_hz`, amplitude `level`. It is
/// meant to start at the slot start plus 0.5 s (FT4's ramp symbol included).
pub fn waveform(p: &Protocol, tones: &[usize], audio_hz: f64, rate: f64, level: f32) -> Vec<f32> {
    let bt = if std::ptr::eq(p, &FT4) { 1.0 } else { 2.0 };
    let sym = symbol_s(p);
    // FT4 has a dummy symbol at either end (holding the first and last tone) for the
    // amplitude ramps; FT8 ramps within its first and last symbol.
    let ft4 = std::ptr::eq(p, &FT4);
    let seq: Vec<usize> = if ft4 { std::iter::once(tones[0]).chain(tones.iter().copied()).chain(std::iter::once(*tones.last().unwrap())).collect() } else { tones.to_vec() };
    let n = (seq.len() as f64 * sym * rate).round() as usize;
    let spacing = p.tone_spacing();
    let ramp = if ft4 { sym } else { sym / 8.0 };
    let total = n as f64 / rate;
    let mut phase = 0.0f64;
    (0..n)
        .map(|i| {
            let t = i as f64 / rate;
            let s = t / sym;
            // Frequency: each symbol's tone smoothed by the Gaussian pulse over ±1.5
            // symbols; beyond the ends the end tones carry on.
            let k = s.floor() as isize;
            let mut f = 0.0;
            for j in k - 2..=k + 2 {
                let tone = seq[j.clamp(0, seq.len() as isize - 1) as usize] as f64;
                f += tone * pulse(bt, s - (j as f64 + 0.5));
            }
            let freq = audio_hz + f * spacing;
            phase = (phase + 2.0 * PI * freq / rate) % (2.0 * PI);
            let edge = t.min(total - t);
            let amp = if edge < ramp { 0.5 * (1.0 - (PI * edge / ramp).cos()) } else { 1.0 };
            level * (amp * phase.sin()) as f32
        })
        .collect()
}

/// Length of a transmission (s), FT4's ramp symbols included.
pub fn duration_s(p: &Protocol) -> f64 {
    let extra = if std::ptr::eq(p, &FT4) { 2 } else { 0 };
    (p.symbols + extra) as f64 * symbol_s(p)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::demod::{FirDecimator, Mixer, lowpass};
    use crate::ft8::FT8;
    use crate::ft8::decode::decode_slot;
    use crate::ft8::decode::tests::{noise, slot_samples};
    use crate::ft8::message::CallBook;
    use num_complex::Complex;

    #[test]
    fn erf_values() {
        assert!((erf(0.5) - 0.520_499_877_8).abs() < 1e-6);
        assert!((erf(-1.2) + 0.910_313_978_2).abs() < 1e-6);
        // A pulse integrates to one symbol of frequency step.
        let area: f64 = (-300..300).map(|i| pulse(2.0, i as f64 / 100.0) / 100.0).sum();
        assert!((area - 1.0).abs() < 1e-3);
    }

    /// The audio, brought to the decoder's baseband (`audio_center` Hz at zero) and placed
    /// in a slot as the receiver would hand it over, decodes to the text sent.
    fn round_trip(p: &'static Protocol, text: &str) {
        // A whole multiple of the decoder's rate, so the test can simply decimate.
        let rate = p.rate * 8.0;
        let audio_hz = 1_234.0;
        let audio = waveform(p, &tones(p, text).expect(text), audio_hz, rate, 0.5);
        assert!((audio.len() as f64 / rate - duration_s(p)).abs() < 1e-3);
        // Analytic signal around 1500 Hz, at the decoder's rate.
        let center = 1_500.0;
        let mut mixer = Mixer::new(-center, rate);
        let mut shifted: Vec<Complex<f32>> = audio.iter().map(|&x| Complex::new(x, 0.0) * mixer.next()).collect();
        let factor = (rate / p.rate).round() as usize;
        let mut lp = FirDecimator::new(&lowpass(0.4 * p.rate / rate, 801), factor);
        let mut down = Vec::new();
        lp.process(&std::mem::take(&mut shifted), &mut down);
        // Slot buffer: starts early_s before the slot; the audio starts at +0.5 s.
        let mut slot = noise(slot_samples(p), 0.02, 3);
        let at = ((p.early_s + 0.5) * p.rate) as usize;
        for (i, z) in down.iter().enumerate() {
            if let Some(s) = slot.get_mut(at + i) {
                *s += z * 2.0;
            }
        }
        let found = decode_slot(p, &slot, -1_500.0, 1_500.0, &mut CallBook::default());
        let d = found.iter().find(|d| d.text == text).unwrap_or_else(|| panic!("{} not decoded: {found:?}", text));
        assert!((d.freq_hz - (audio_hz - center)).abs() < 2.0, "{d:?}");
        assert!(d.dt_s.abs() < 0.05, "{d:?}");
    }

    #[test]
    fn ft8_round_trip() {
        round_trip(&FT8, "CQ TA4SO KM46");
        round_trip(&FT8, "DL1ABC TA4SO R-07");
    }

    #[test]
    fn ft4_round_trip() {
        round_trip(&FT4, "TA4SO DL1ABC RR73");
    }

    #[test]
    fn constant_envelope_inside() {
        let a = waveform(&FT8, &tones(&FT8, "CQ TA4SO KM46").unwrap(), 1_500.0, 12_000.0, 0.8);
        let mid = &a[a.len() / 2..a.len() / 2 + 1_200];
        let peak = mid.iter().fold(0f32, |m, v| m.max(v.abs()));
        assert!((peak - 0.8).abs() < 0.01);
        assert!(a[0].abs() < 1e-3 && a[a.len() - 1].abs() < 0.01);
    }
}
