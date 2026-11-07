//! Received samples as they are handed between threads.

use std::sync::Arc;

use num_complex::Complex;

/// One block of received IQ samples.
#[derive(Clone)]
pub struct Block {
    pub samples: Arc<Vec<Complex<f32>>>,
    /// Frequency at the block's centre, before the converter (IF) (Hz).
    pub center_hz: f64,
    pub sample_rate: f64,
}
