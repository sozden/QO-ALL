//! The QO-100 narrow-band transponder: frequencies, the uplink/downlink relation and
//! where transmitting is allowed.

/// Downlink = uplink + this (Hz); the satellite's translation.
pub const TRANSLATION_HZ: f64 = 8_089.5e6;
/// Narrow-band transponder downlink edges (Hz).
pub const DOWNLINK_LOW_HZ: f64 = 10_489.500e6;
pub const DOWNLINK_HIGH_HZ: f64 = 10_490.000e6;
/// Narrow-band uplink edges (Hz).
pub const UPLINK_LOW_HZ: f64 = DOWNLINK_LOW_HZ - TRANSLATION_HZ;
pub const UPLINK_HIGH_HZ: f64 = DOWNLINK_HIGH_HZ - TRANSLATION_HZ;

/// Beacons (downlink, Hz): lower CW, middle PSK, upper CW.
pub const BEACONS_HZ: [f64; 3] = [10_489.500e6, 10_489.750e6, 10_490.000e6];
/// Downlink stretches nobody may transmit into, around the beacons (Hz).
pub const RESERVED_HZ: [(f64, f64); 3] =
    [(10_489.495e6, 10_489.505e6), (10_489.745e6, 10_489.755e6), (10_489.995e6, 10_490.005e6)];

/// Uplink frequency that comes down at `downlink_hz`.
pub fn uplink(downlink_hz: f64) -> f64 {
    downlink_hz - TRANSLATION_HZ
}

/// Downlink frequency of an uplink frequency.
pub fn downlink(uplink_hz: f64) -> f64 {
    uplink_hz + TRANSLATION_HZ
}

/// Why a transmission is refused.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Refusal {
    /// Part of the signal would fall outside the narrow-band transponder.
    OutsideTransponder,
    /// Part of the signal would fall on a beacon.
    OnBeacon,
}

/// Whether a signal occupying `low_hz..high_hz` on the downlink may be sent: wholly
/// inside the transponder and clear of the beacons.
pub fn check(low_hz: f64, high_hz: f64) -> Result<(), Refusal> {
    let inner_low = RESERVED_HZ[0].1;
    let inner_high = RESERVED_HZ[2].0;
    if low_hz < inner_low || high_hz > inner_high {
        return Err(Refusal::OutsideTransponder);
    }
    if RESERVED_HZ.iter().any(|&(a, b)| low_hz < b && high_hz > a) {
        return Err(Refusal::OnBeacon);
    }
    Ok(())
}

/// Usual activity in a part of the transponder (IARU Region 1 / AMSAT-DL band plan),
/// for labels.
pub fn segment(downlink_hz: f64) -> &'static str {
    let khz = (downlink_hz - DOWNLINK_LOW_HZ) / 1e3;
    match khz {
        k if k < 5.0 => "beacon",
        k if k < 40.0 => "CW",
        k if k < 80.0 => "digital",
        k if k < 245.0 => "SSB",
        k if k < 255.0 => "beacon",
        k if k < 495.0 => "SSB",
        _ => "beacon",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn uplink_and_downlink_are_inverse() {
        assert!((uplink(10_489.725e6) - 2_400.225e6).abs() < 1e-3);
        assert!((downlink(2_400.0e6) - 10_489.5e6).abs() < 1e-3);
        assert!((UPLINK_LOW_HZ - 2_400.0e6).abs() < 1e-3);
        assert!((UPLINK_HIGH_HZ - 2_400.5e6).abs() < 1e-3);
    }

    #[test]
    fn allowed_and_refused_places() {
        // A USB voice at 10489.725 (passband +300..+2700).
        assert_eq!(check(10_489.7253e6, 10_489.7277e6), Ok(()));
        // Touching the middle beacon.
        assert_eq!(check(10_489.7440e6, 10_489.7464e6), Err(Refusal::OnBeacon));
        // Below the lower beacon, beyond the upper one.
        assert_eq!(check(10_489.4990e6, 10_489.5010e6), Err(Refusal::OutsideTransponder));
        assert_eq!(check(10_489.9940e6, 10_489.9964e6), Err(Refusal::OutsideTransponder));
        // FT8 at 10489.540 + 1.5 kHz.
        assert_eq!(check(10_489.5415e6, 10_489.5416e6), Ok(()));
    }

    #[test]
    fn segments() {
        assert_eq!(segment(10_489.520e6), "CW");
        assert_eq!(segment(10_489.541e6), "digital");
        assert_eq!(segment(10_489.725e6), "SSB");
        assert_eq!(segment(10_489.750e6), "beacon");
    }
}
