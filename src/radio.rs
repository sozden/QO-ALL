//! What the engine needs from a radio, so that the simulator can stand in for one.

use num_complex::Complex;

use crate::iio::{IioRadio, IioTransmitter};

/// The receiving half.
pub trait RxDevice: Send {
    /// Fills part of `out` with samples; returns how many.
    fn read(&mut self, out: &mut [Complex<f32>]) -> Result<usize, String>;
    fn set_frequency(&mut self, hz: f64) -> Result<(), String>;
    fn set_sample_rate(&mut self, rate: f64) -> Result<(), String>;
    fn set_gain(&mut self, gain: Option<f64>) -> Result<(), String>;
    /// Description for the status bar.
    fn model(&self) -> String;
    /// The transmitting half, if the radio has one.
    fn transmitter(&self) -> Result<Box<dyn TxDevice>, String>;
}

/// The transmitting half.
pub trait TxDevice: Send {
    fn set_frequency(&mut self, hz: f64) -> Result<(), String>;
    /// Output attenuation (dB, 0 = full power).
    fn set_attenuation(&mut self, db: f64) -> Result<(), String>;
    fn set_bandwidth(&mut self, hz: f64) -> Result<(), String>;
    fn set_powered(&mut self, on: bool) -> Result<(), String>;
    /// Sends samples; blocks to pace the caller at the sample rate.
    fn write(&mut self, samples: &[Complex<f32>]) -> Result<(), String>;
    /// Drops what is still queued (end of a transmission).
    fn stop(&mut self);
}

impl RxDevice for IioRadio {
    fn read(&mut self, out: &mut [Complex<f32>]) -> Result<usize, String> {
        IioRadio::read(self, out)
    }
    fn set_frequency(&mut self, hz: f64) -> Result<(), String> {
        IioRadio::set_frequency(self, hz)
    }
    fn set_sample_rate(&mut self, rate: f64) -> Result<(), String> {
        IioRadio::set_sample_rate(self, rate)
    }
    fn set_gain(&mut self, gain: Option<f64>) -> Result<(), String> {
        IioRadio::set_gain(self, gain)
    }
    fn model(&self) -> String {
        let model = self.identity().model;
        if model.is_empty() { self.uri().to_string() } else { format!("{model} · {}", self.uri()) }
    }
    fn transmitter(&self) -> Result<Box<dyn TxDevice>, String> {
        Ok(Box::new(IioRadio::transmitter(self)?))
    }
}

impl TxDevice for IioTransmitter {
    fn set_frequency(&mut self, hz: f64) -> Result<(), String> {
        IioTransmitter::set_frequency(self, hz)
    }
    fn set_attenuation(&mut self, db: f64) -> Result<(), String> {
        IioTransmitter::set_attenuation(self, db)
    }
    fn set_bandwidth(&mut self, hz: f64) -> Result<(), String> {
        IioTransmitter::set_bandwidth(self, hz)
    }
    fn set_powered(&mut self, on: bool) -> Result<(), String> {
        IioTransmitter::set_powered(self, on)
    }
    fn write(&mut self, samples: &[Complex<f32>]) -> Result<(), String> {
        IioTransmitter::write(self, samples)
    }
    fn stop(&mut self) {
        IioTransmitter::stop(self)
    }
}
