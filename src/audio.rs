//! Sound output: the computer's playback devices through cpal (WASAPI on Windows,
//! ALSA/PulseAudio/PipeWire on Linux, Core Audio on macOS).

use std::collections::VecDeque;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex};

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{FromSample, SampleFormat, SizedSample};

/// The rate audio runs at inside the station (Hz). A sound device working at another
/// rate is resampled to it, so devices can be changed while the station runs.
pub const RATE: f64 = 48_000.0;

/// Linear resampling of a stream (enough for voice and the speaker; the digital modes
/// are made and decoded at `RATE`).
pub struct Resampler {
    /// Input samples per output sample.
    step: f64,
    pos: f64,
    prev: f32,
    cur: f32,
}

impl Resampler {
    pub fn new(from: f64, to: f64) -> Self {
        Self { step: from / to, pos: 0.0, prev: 0.0, cur: 0.0 }
    }

    pub fn is_identity(&self) -> bool {
        (self.step - 1.0).abs() < 1e-9
    }

    pub fn push(&mut self, input: &[f32], out: &mut Vec<f32>) {
        if self.is_identity() {
            out.extend_from_slice(input);
            return;
        }
        for &x in input {
            self.prev = self.cur;
            self.cur = x;
            while self.pos < 1.0 {
                out.push(self.prev + (self.cur - self.prev) * self.pos as f32);
                self.pos += self.step;
            }
            self.pos -= 1.0;
        }
    }
}

/// Mono audio waiting for the sound card. The demodulator fills it, the sound card's
/// callback empties it.
#[derive(Clone, Default)]
pub struct AudioBuffer(Arc<Mutex<Queue>>);

#[derive(Default)]
struct Queue {
    samples: VecDeque<f32>,
    /// Playback waits until this many samples are queued (after start or an underrun),
    /// so that small timing jitter does not cause gaps.
    prime: usize,
    primed: bool,
}

impl AudioBuffer {
    pub fn push(&self, samples: &[f32]) {
        self.0.lock().unwrap().samples.extend(samples);
    }

    pub fn len(&self) -> usize {
        self.0.lock().unwrap().samples.len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Drops the oldest samples down to `keep` (when too much has piled up).
    pub fn trim_to(&self, keep: usize) {
        let mut q = self.0.lock().unwrap();
        let excess = q.samples.len().saturating_sub(keep);
        q.samples.drain(..excess);
    }

    pub fn clear(&self) {
        let mut q = self.0.lock().unwrap();
        q.samples.clear();
        q.primed = false;
    }

    pub fn set_prime(&self, samples: usize) {
        self.0.lock().unwrap().prime = samples;
    }

    /// Fills `out`, with silence where there is nothing to play.
    pub(crate) fn pop_into(&self, out: &mut [f32]) {
        let mut q = self.0.lock().unwrap();
        if !q.primed {
            if q.samples.len() < q.prime.max(out.len()) {
                out.fill(0.0);
                return;
            }
            q.primed = true;
        }
        for v in out.iter_mut() {
            match q.samples.pop_front() {
                Some(s) => *v = s,
                None => {
                    *v = 0.0;
                    q.primed = false;
                }
            }
        }
    }
}

/// A playback device.
#[derive(Clone, Debug, PartialEq)]
pub struct OutputDevice {
    /// Stable identifier, kept in the settings.
    pub id: String,
    pub name: String,
}

/// Playback devices of the default sound system.
pub fn output_devices() -> Vec<OutputDevice> {
    let host = cpal::default_host();
    let Ok(devices) = host.output_devices() else { return Vec::new() };
    devices
        .filter_map(|d| {
            let id = d.id().ok()?.to_string();
            let name = d.description().map(|desc| desc.name().to_string()).unwrap_or_else(|_| d.to_string());
            Some(OutputDevice { id, name })
        })
        .collect()
}

/// Volume as a linear factor shared with the sound card's callback.
#[derive(Clone)]
pub struct Volume(Arc<AtomicU32>);

impl Volume {
    pub fn new(v: f32) -> Self {
        Self(Arc::new(AtomicU32::new(v.to_bits())))
    }

    pub fn set(&self, v: f32) {
        self.0.store(v.to_bits(), Ordering::Relaxed);
    }

    fn get(&self) -> f32 {
        f32::from_bits(self.0.load(Ordering::Relaxed))
    }
}

/// An open playback stream; playing stops when it is dropped.
pub struct AudioOutput {
    _stream: cpal::Stream,
    pub sample_rate: u32,
    pub name: String,
}

/// Opens `device_id` (or the system's default device) and plays from `buffer`.
pub fn open(device_id: Option<&str>, buffer: AudioBuffer, volume: Volume) -> Result<AudioOutput, String> {
    let host = cpal::default_host();
    let device = match device_id {
        Some(id) => {
            let id: cpal::DeviceId = id.parse().map_err(|e| format!("{e:?}"))?;
            host.device_by_id(&id).ok_or("the selected sound device is not available")?
        }
        None => host.default_output_device().ok_or("no sound output device")?,
    };
    let name = device.description().map(|d| d.name().to_string()).unwrap_or_else(|_| device.to_string());
    let supported = device.default_output_config().map_err(|e| e.to_string())?;
    let config = supported.config();
    let stream = match supported.sample_format() {
        SampleFormat::F32 => build::<f32>(&device, config.clone(), buffer, volume),
        SampleFormat::I16 => build::<i16>(&device, config.clone(), buffer, volume),
        SampleFormat::U16 => build::<u16>(&device, config.clone(), buffer, volume),
        SampleFormat::I32 => build::<i32>(&device, config.clone(), buffer, volume),
        other => return Err(format!("unsupported sample format {other:?}")),
    }?;
    stream.play().map_err(|e| e.to_string())?;
    Ok(AudioOutput { _stream: stream, sample_rate: config.sample_rate, name })
}

fn build<T: SizedSample + FromSample<f32>>(
    device: &cpal::Device,
    config: cpal::StreamConfig,
    buffer: AudioBuffer,
    volume: Volume,
) -> Result<cpal::Stream, String> {
    let channels = config.channels.max(1) as usize;
    let mut resampler = Resampler::new(RATE, config.sample_rate as f64);
    let (mut mono, mut chunk, mut made) = (Vec::new(), Vec::new(), Vec::new());
    device
        .build_output_stream(
            config,
            move |data: &mut [T], _| {
                let n = data.len() / channels;
                if resampler.is_identity() {
                    mono.resize(n, 0.0);
                    buffer.pop_into(&mut mono);
                } else {
                    // Station audio in until enough comes out; the rest waits for the
                    // next call.
                    while made.len() < n {
                        let want = (((n - made.len()) as f64 * resampler.step).ceil() as usize).max(1);
                        chunk.resize(want, 0.0);
                        buffer.pop_into(&mut chunk);
                        resampler.push(&chunk, &mut made);
                    }
                    mono.clear();
                    mono.extend(made.drain(..n));
                }
                let gain = volume.get();
                for (frame, &v) in data.chunks_exact_mut(channels).zip(&mono) {
                    let s = T::from_sample((v * gain).clamp(-1.0, 1.0));
                    frame.fill(s);
                }
            },
            |e| eprintln!("audio: {e}"),
            None,
        )
        .map_err(|e| e.to_string())
}

/// An open recording stream; recording stops when it is dropped.
pub struct AudioInput {
    _stream: cpal::Stream,
    pub sample_rate: u32,
    pub name: String,
}

/// Recording devices (microphones) of the default sound system.
pub fn input_devices() -> Vec<OutputDevice> {
    let host = cpal::default_host();
    let Ok(devices) = host.input_devices() else { return Vec::new() };
    devices
        .filter_map(|d| {
            let id = d.id().ok()?.to_string();
            let name = d.description().map(|desc| desc.name().to_string()).unwrap_or_else(|_| d.to_string());
            Some(OutputDevice { id, name })
        })
        .collect()
}

/// Records from `device_id` (or the system's default microphone). Mono blocks (the
/// first channel) go to `sink`; blocks that do not fit in it are dropped.
pub fn open_input(device_id: Option<&str>, sink: std::sync::mpsc::SyncSender<Vec<f32>>) -> Result<AudioInput, String> {
    let host = cpal::default_host();
    let device = match device_id {
        Some(id) => {
            let id: cpal::DeviceId = id.parse().map_err(|e| format!("{e:?}"))?;
            host.device_by_id(&id).ok_or("the selected microphone is not available")?
        }
        None => host.default_input_device().ok_or("no microphone")?,
    };
    let name = device.description().map(|d| d.name().to_string()).unwrap_or_else(|_| device.to_string());
    let supported = device.default_input_config().map_err(|e| e.to_string())?;
    let config = supported.config();
    let stream = match supported.sample_format() {
        SampleFormat::F32 => build_input::<f32>(&device, config.clone(), sink),
        SampleFormat::I16 => build_input::<i16>(&device, config.clone(), sink),
        SampleFormat::U16 => build_input::<u16>(&device, config.clone(), sink),
        SampleFormat::I32 => build_input::<i32>(&device, config.clone(), sink),
        other => return Err(format!("unsupported sample format {other:?}")),
    }?;
    stream.play().map_err(|e| e.to_string())?;
    Ok(AudioInput { _stream: stream, sample_rate: config.sample_rate, name })
}

fn build_input<T: SizedSample>(
    device: &cpal::Device,
    config: cpal::StreamConfig,
    sink: std::sync::mpsc::SyncSender<Vec<f32>>,
) -> Result<cpal::Stream, String>
where
    f32: FromSample<T>,
{
    let channels = config.channels.max(1) as usize;
    let mut resampler = Resampler::new(config.sample_rate as f64, RATE);
    device
        .build_input_stream(
            config,
            move |data: &[T], _| {
                let mono: Vec<f32> = data.chunks_exact(channels).map(|frame| f32::from_sample_(frame[0])).collect();
                let mut out = Vec::with_capacity(mono.len() + 16);
                resampler.push(&mono, &mut out);
                let _ = sink.try_send(out);
            },
            |e| eprintln!("audio input: {e}"),
            None,
        )
        .map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn buffer_waits_until_primed_and_reports_underrun() {
        let b = AudioBuffer::default();
        b.set_prime(4);
        b.push(&[0.1, 0.2]);
        let mut out = [9.0; 2];
        b.pop_into(&mut out);
        assert_eq!(out, [0.0, 0.0]);
        b.push(&[0.3, 0.4]);
        b.pop_into(&mut out);
        assert_eq!(out, [0.1, 0.2]);
        let mut out = [9.0; 3];
        b.pop_into(&mut out);
        assert_eq!(out, [0.3, 0.4, 0.0]);
        b.push(&[0.5]);
        let mut out = [9.0; 1];
        b.pop_into(&mut out);
        assert_eq!(out, [0.0], "after an underrun playback waits to prime again");
    }

    #[test]
    fn resampling_keeps_the_rate_and_the_tone() {
        // 44.1 kHz → 48 kHz: a second in gives a second out, the 1 kHz tone intact.
        let mut r = Resampler::new(44_100.0, RATE);
        let input: Vec<f32> = (0..44_100).map(|i| (2.0 * std::f32::consts::PI * 1000.0 * i as f32 / 44_100.0).sin()).collect();
        let mut out = Vec::new();
        for c in input.chunks(441) {
            r.push(c, &mut out);
        }
        assert!((out.len() as i64 - 48_000).abs() <= 2, "{}", out.len());
        let worst = out.iter().enumerate().skip(10).map(|(i, &v)| (v - (2.0 * std::f32::consts::PI * 1000.0 * i as f32 / 48_000.0 - 2.0 * std::f32::consts::PI * 1000.0 / 44_100.0).sin()).abs()).fold(0.0, f32::max);
        assert!(worst < 0.03, "{worst}");
    }

    /// Lists playback devices: `cargo test --lib audio_devices -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn audio_devices() {
        for d in output_devices() {
            println!("{} | {}", d.name, d.id);
        }
    }
}
