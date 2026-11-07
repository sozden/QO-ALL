//! The station at work: one thread reads the receiver and hands the blocks around
//! (spectrum, listening, beacon lock, FT8, FT4), another turns the microphone into the
//! uplink signal when PTT or VOX asks for it.

use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender, SyncSender, TryRecvError, TrySendError};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use num_complex::Complex;

use crate::audio::{self, AudioBuffer, AudioInput, AudioOutput, Volume};
use crate::bandplan::Refusal;
use crate::beacon::{self, QO100_BEACON_HZ, SharedBeacon};
use crate::demod::Mode;
use crate::dsp::DcBlocker;
use crate::ft8::worker::Ft8;
use crate::iio::{IioRadio, TX_ATTENUATION_MAX};
use crate::listen::{ListenSettings, Listener};
use crate::radio::{RxDevice, TxDevice};
use crate::sim::SimRx;
use crate::source::{self, Block, Frame, View};
use crate::tx::SsbModulator;
use crate::vox::{DelayLine, TxControl, TxState, Vox, VoxSettings};

/// Blocks a consumer may have queued before new ones are dropped for it.
const QUEUE_BLOCKS: usize = 16;
/// Frequency and level of the tune tone.
const TUNE_HZ: f64 = 1_000.0;
/// A full-scale tone: the modulator's limiter brings it to its peak level.
const TUNE_LEVEL: f32 = 1.0;
/// Tune tone made per round (s); the radio's write paces the loop.
const TUNE_CHUNK_S: f64 = 0.02;
/// Room kept at the band edges for the transmit signal (Hz).
const TX_EDGE_HZ: f64 = 50e3;

/// Which radio to use.
#[derive(Clone, Debug, PartialEq)]
pub enum DeviceChoice {
    Simulator,
    /// A libiio URI such as "ip:192.168.2.1" or "usb:1.5.5".
    Iio(String),
}

/// Settings that need the station restarted when they change.
#[derive(Clone, Debug, PartialEq)]
pub struct StationConfig {
    pub device: DeviceChoice,
    /// LNB local oscillator, nominal (Hz).
    pub lnb_lo_hz: f64,
    /// Radio frequency at the middle of the received band (Hz). A little below the
    /// transponder, so the receiver's DC spur stays out of it.
    pub rx_center_hz: f64,
    pub sample_rate: f64,
    pub rx_gain: Option<f64>,
    /// Transmit LO (Hz), outside the uplink band, so its leakage and the image of the
    /// signal stay out of the transponder; the signal is moved digitally from there.
    pub tx_lo_hz: f64,
    /// Sound devices (None: the system's default).
    pub microphone: Option<String>,
    pub speaker: Option<String>,
}

impl Default for StationConfig {
    fn default() -> Self {
        Self {
            device: DeviceChoice::Simulator,
            lnb_lo_hz: 9_750e6,
            rx_center_hz: 10_489.445e6,
            sample_rate: 2.4e6,
            rx_gain: Some(40.0),
            tx_lo_hz: 2_399.9e6,
            microphone: None,
            speaker: None,
        }
    }
}

impl StationConfig {
    /// Receiver frequency the radio is tuned to (IF).
    pub fn center_if(&self) -> f64 {
        self.rx_center_hz - self.lnb_lo_hz
    }
}

pub enum Command {
    Gain(Option<f64>),
}

pub enum Event {
    /// The radio is open; `tx` tells whether it can transmit.
    Opened { model: String, tx: Result<(), String>, warning: Option<String> },
    Frame(Frame),
    /// Samples per second arriving from the radio.
    Rate(f64),
    Error(String),
}

/// What the window sets for the transmitter.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TxSettings {
    pub mode: Mode,
    /// Uplink frequency of the suppressed carrier, corrections included (Hz).
    pub uplink_hz: f64,
    /// Transmit attenuation (dB, 0 = the radio's full power).
    pub attenuation_db: f64,
    pub mic_gain_db: f32,
    pub compress: bool,
    pub vox: VoxSettings,
    /// Transmitting enabled by the operator.
    pub armed: bool,
    pub ptt: bool,
    /// Sends a steady 1 kHz tone instead of the microphone (setting the power, finding
    /// one's own signal on the waterfall).
    pub tune: bool,
    /// Band plan check of the signal's place.
    pub band: Result<(), Refusal>,
    pub time_out: Duration,
}

impl Default for TxSettings {
    fn default() -> Self {
        Self {
            mode: Mode::Usb,
            uplink_hz: 2_400.225e6,
            attenuation_db: 20.0,
            mic_gain_db: 0.0,
            compress: false,
            vox: VoxSettings::default(),
            armed: false,
            ptt: false,
            tune: false,
            band: Ok(()),
            time_out: crate::vox::DEFAULT_TIME_OUT,
        }
    }
}

/// What the transmitter reports back.
#[derive(Clone, Debug)]
pub struct TxStatus {
    pub state: TxState,
    /// Microphone level (dBFS) and VOX keying.
    pub mic_db: f32,
    pub vox_keyed: bool,
    /// Limiter gain reduction while transmitting (dB).
    pub alc_db: f32,
    /// Time left before the time-out timer stops the transmission.
    pub remaining: Option<Duration>,
    /// Samples per second handed to the radio while transmitting (should be its rate).
    pub sent_rate: f64,
    pub error: Option<String>,
}

impl Default for TxStatus {
    fn default() -> Self {
        Self { state: TxState::Receive, mic_db: -150.0, vox_keyed: false, alc_db: 0.0, remaining: None, sent_rate: 0.0, error: None }
    }
}

/// The running station. Dropping it stops the threads and closes the radio.
pub struct Engine {
    pub config: StationConfig,
    commands: Sender<Command>,
    pub events: Receiver<Event>,
    /// Part of the received band the spectrum shows (relative to the tuned centre).
    pub view: Arc<Mutex<View>>,
    pub listener: Listener,
    pub volume: Volume,
    pub beacon: SharedBeacon,
    pub ft8: Ft8,
    pub ft4: Ft8,
    pub tx: Arc<Mutex<TxSettings>>,
    pub tx_status: Arc<Mutex<TxStatus>>,
    /// Name of the open microphone and speaker, or why they could not be opened.
    pub microphone: Result<String, String>,
    pub speaker: Result<String, String>,
    stop: Arc<AtomicBool>,
    _audio_out: Option<AudioOutput>,
    _mic: Option<AudioInput>,
}

impl Engine {
    /// Opens the radio (on its own thread) and the sound devices, and starts everything.
    /// `notify` is called when there is something new to show.
    pub fn start(config: StationConfig, listen: ListenSettings, tx: TxSettings, volume: f32, notify: impl Fn() + Send + Sync + 'static) -> Self {
        let notify: Arc<dyn Fn() + Send + Sync> = Arc::new(notify);
        let (cmd_tx, cmd_rx) = mpsc::channel();
        let (ev_tx, ev_rx) = mpsc::channel();
        let stop = Arc::new(AtomicBool::new(false));

        // Speaker.
        let audio = AudioBuffer::default();
        let volume = Volume::new(volume);
        let (audio_out, speaker) = match audio::open(config.speaker.as_deref(), audio.clone(), volume.clone()) {
            Ok(out) => {
                let name = out.name.clone();
                (Some(out), Ok(name))
            }
            Err(e) => (None, Err(e)),
        };
        let audio_rate = audio_out.as_ref().map_or(48_000.0, |o| o.sample_rate as f64);
        let listener = Listener::new(ListenSettings { audio_rate, ..listen });

        // Consumers of the received blocks.
        let mut consumers = Vec::new();
        let mut channel = || {
            let (tx, rx) = mpsc::sync_channel::<Block>(QUEUE_BLOCKS);
            consumers.push(tx);
            rx
        };
        listener.spawn(channel(), audio);
        let beacon = beacon::spawn(channel(), QO100_BEACON_HZ, config.lnb_lo_hz, 0.0);
        let ft8 = Ft8::new(&crate::ft8::FT8);
        ft8.spawn(channel());
        let ft4 = Ft8::new(&crate::ft8::FT4);
        ft4.spawn(channel());
        let spectrum_rx = channel();
        let view = Arc::new(Mutex::new(View::full(config.sample_rate)));
        {
            let (view, events, notify) = (view.clone(), ev_tx.clone(), notify.clone());
            thread::Builder::new()
                .name("spectrum".into())
                .spawn(move || {
                    source::spectrum_worker(spectrum_rx, view, |f| {
                        let ok = events.send(Event::Frame(f)).is_ok();
                        notify();
                        ok
                    })
                })
                .expect("spectrum thread");
        }

        // Microphone and transmitter.
        let (mic_tx, mic_rx) = mpsc::sync_channel::<Vec<f32>>(64);
        let (mic, microphone) = match audio::open_input(config.microphone.as_deref(), mic_tx) {
            Ok(input) => {
                let name = input.name.clone();
                (Some(input), Ok(name))
            }
            Err(e) => (None, Err(e)),
        };
        let mic_rate = mic.as_ref().map_or(48_000.0, |m| m.sample_rate as f64);
        let tx_settings = Arc::new(Mutex::new(tx));
        let tx_status = Arc::new(Mutex::new(TxStatus::default()));
        let (handoff_tx, handoff_rx) = mpsc::sync_channel::<Box<dyn TxDevice>>(1);
        {
            let link = TxLink {
                settings: tx_settings.clone(),
                status: tx_status.clone(),
                devices: handoff_rx,
                mic: mic.is_some().then_some(mic_rx),
                mic_rate,
                listener: listener.clone(),
                stop: stop.clone(),
                tx_lo_hz: config.tx_lo_hz,
                rate: config.sample_rate,
                notify: notify.clone(),
            };
            thread::Builder::new().name("transmit".into()).spawn(move || transmit(link)).expect("transmit thread");
        }

        // Receiver.
        {
            let config = config.clone();
            let notify = notify.clone();
            thread::Builder::new()
                .name("receive".into())
                .spawn(move || {
                    if let Err(e) = receive(&config, &cmd_rx, &ev_tx, consumers, handoff_tx, notify.clone()) {
                        let _ = ev_tx.send(Event::Error(e));
                        notify();
                    }
                })
                .expect("receive thread");
        }

        Self {
            config,
            commands: cmd_tx,
            events: ev_rx,
            view,
            listener,
            volume,
            beacon,
            ft8,
            ft4,
            tx: tx_settings,
            tx_status,
            microphone,
            speaker,
            stop,
            _audio_out: audio_out,
            _mic: mic,
        }
    }

    pub fn set_gain(&self, gain: Option<f64>) {
        let _ = self.commands.send(Command::Gain(gain));
    }

    /// Converter correction measured on the beacon now (Hz), if locked.
    pub fn correction(&self) -> Option<f64> {
        self.beacon.lock().unwrap().correction_at(Instant::now())
    }
}

impl Drop for Engine {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
    }
}

/// Samples read per block: about 20 ms, so changes take effect quickly.
fn block_len(rate: f64) -> usize {
    ((rate / 50.0) as usize / 1024).max(16) * 1024
}

fn open(config: &StationConfig) -> Result<Box<dyn RxDevice>, String> {
    let mut radio: Box<dyn RxDevice> = match &config.device {
        DeviceChoice::Simulator => Box::new(SimRx::new(config.lnb_lo_hz)),
        DeviceChoice::Iio(uri) => Box::new(IioRadio::open(uri)?),
    };
    radio.set_sample_rate(config.sample_rate)?;
    radio.set_frequency(config.center_if())?;
    radio.set_gain(config.rx_gain)?;
    Ok(radio)
}

fn receive(
    config: &StationConfig,
    commands: &Receiver<Command>,
    events: &Sender<Event>,
    mut consumers: Vec<SyncSender<Block>>,
    handoff: SyncSender<Box<dyn TxDevice>>,
    notify: Arc<dyn Fn() + Send + Sync>,
) -> Result<(), String> {
    let mut radio = open(config)?;
    let tx = radio.transmitter();
    let _ = events.send(Event::Opened { model: radio.model(), tx: tx.as_ref().map(|_| ()).map_err(|e| e.clone()), warning: radio.warning() });
    notify();
    if let Ok(t) = tx {
        let _ = handoff.send(t);
    }
    drop(handoff);

    let center = config.center_if();
    let rate = config.sample_rate;
    let mut dc = DcBlocker::new(1e-5);
    let mut window = (Instant::now(), 0u64);
    loop {
        loop {
            match commands.try_recv() {
                Ok(Command::Gain(g)) => radio.set_gain(g)?,
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => return Ok(()),
            }
        }
        let mut samples = vec![Complex::default(); block_len(rate)];
        let mut filled = 0;
        while filled < samples.len() {
            filled += radio.read(&mut samples[filled..])?;
        }
        dc.process(&mut samples);
        window.1 += samples.len() as u64;
        let block = Block { samples: Arc::new(samples), center_hz: center, sample_rate: rate };
        consumers.retain(|c| !matches!(c.try_send(block.clone()), Err(TrySendError::Disconnected(_))));
        let elapsed = window.0.elapsed();
        if elapsed >= Duration::from_secs(1) {
            if events.send(Event::Rate(window.1 as f64 / elapsed.as_secs_f64())).is_err() {
                return Ok(());
            }
            notify();
            window = (Instant::now(), 0);
        }
    }
}

/// What the transmit thread works with.
struct TxLink {
    settings: Arc<Mutex<TxSettings>>,
    status: Arc<Mutex<TxStatus>>,
    devices: Receiver<Box<dyn TxDevice>>,
    mic: Option<Receiver<Vec<f32>>>,
    mic_rate: f64,
    listener: Listener,
    stop: Arc<AtomicBool>,
    tx_lo_hz: f64,
    rate: f64,
    notify: Arc<dyn Fn() + Send + Sync>,
}

fn db_to_gain(db: f32) -> f32 {
    10f32.powf(db / 20.0)
}

fn transmit(link: TxLink) {
    let mut device: Option<Box<dyn TxDevice>> = None;
    let mut vox = Vox::new(VoxSettings::default(), link.mic_rate);
    let mut delay = DelayLine::new(0);
    let mut control = TxControl::default();
    let mut modulator: Option<SsbModulator> = None;
    let mut pending: VecDeque<f32> = VecDeque::new();
    let mut delayed = Vec::new();
    let mut iq = Vec::new();
    // Transmitting now, and the attenuation set on the radio.
    let mut on = false;
    let mut attenuation = f64::NAN;
    let mut last_state = TxState::Receive;
    let mut error: Option<String> = None;
    let mut tune_phase = 0.0f64;
    // Samples sent in the current second, and the rate of the last one.
    let mut sent = (Instant::now(), 0usize);
    let mut sent_rate = 0.0;
    let fail = |device: &mut Option<Box<dyn TxDevice>>, error: &mut Option<String>, e: String| {
        *error = Some(e);
        *device = None;
    };
    while !link.stop.load(Ordering::Relaxed) {
        if let Ok(mut d) = link.devices.try_recv() {
            let setup = d
                .set_frequency(link.tx_lo_hz)
                .and_then(|_| d.set_bandwidth(link.rate))
                .and_then(|_| d.set_attenuation(TX_ATTENUATION_MAX))
                .and_then(|_| d.set_powered(false));
            match setup {
                Ok(()) => device = Some(d),
                Err(e) => error = Some(e),
            }
        }
        // While the tune tone goes out, the radio's write paces the loop: no waiting for
        // the microphone, or the radio would be fed slower than real time.
        let tuning = on && link.settings.lock().unwrap().tune;
        let wait = if tuning { Duration::ZERO } else { Duration::from_millis(20) };
        let chunk = match &link.mic {
            Some(rx) => match rx.recv_timeout(wait) {
                Ok(c) => Some(c),
                Err(RecvTimeoutError::Timeout) => None,
                Err(RecvTimeoutError::Disconnected) => return,
            },
            None => {
                thread::sleep(wait);
                None
            }
        };
        let s = *link.settings.lock().unwrap();
        vox.settings = s.vox;
        control.time_out = s.time_out;
        delay.set_length((s.vox.preroll_ms as f64 / 1e3 * link.mic_rate) as usize);
        if let Some(chunk) = &chunk {
            vox.process(chunk, link.listener.speaker_db());
            delayed.clear();
            delay.process(chunk, &mut delayed);
            pending.extend(&delayed);
        }
        let offset = s.uplink_hz - link.tx_lo_hz;
        let band = if offset.abs() > link.rate / 2.0 - TX_EDGE_HZ { Err(Refusal::OutsideTransponder) } else { s.band };
        let state = control.update(Instant::now(), s.ptt || s.tune || vox.keyed(), s.armed, device.is_some(), band);
        if s.tune {
            // The tone replaces the microphone.
            pending.clear();
            let n = (link.mic_rate * TUNE_CHUNK_S) as usize;
            let step = 2.0 * std::f64::consts::PI * TUNE_HZ / link.mic_rate;
            pending.extend((0..n).map(|_| {
                tune_phase = (tune_phase + step) % (2.0 * std::f64::consts::PI);
                TUNE_LEVEL * tune_phase.sin() as f32
            }));
        }

        if state == TxState::Transmit
            && let Some(d) = device.as_mut()
        {
            if modulator.as_ref().is_none_or(|m| m.mode() != s.mode) {
                modulator = Some(SsbModulator::new(s.mode, link.mic_rate, link.rate));
            }
            let m = modulator.as_mut().expect("modulator");
            m.mic_gain = db_to_gain(s.mic_gain_db);
            m.compress = s.compress;
            m.set_offset(offset);
            let mut result = Ok(());
            if attenuation != s.attenuation_db {
                attenuation = s.attenuation_db;
                result = d.set_attenuation(attenuation);
            }
            if !on && result.is_ok() {
                result = d.set_powered(true);
                on = true;
                error = None;
            }
            if result.is_ok() && !pending.is_empty() {
                let audio: Vec<f32> = pending.drain(..).collect();
                iq.clear();
                m.process(&audio, 1.0, &mut iq);
                result = d.write(&iq);
                sent.1 += iq.len();
            }
            let elapsed = sent.0.elapsed().as_secs_f64();
            if elapsed >= 1.0 {
                sent_rate = sent.1 as f64 / elapsed;
                sent = (Instant::now(), 0);
            }
            if let Err(e) = result {
                fail(&mut device, &mut error, e);
                on = false;
            }
        } else {
            if on {
                if let Some(d) = device.as_mut() {
                    let result = d.set_powered(false).and_then(|_| d.set_attenuation(TX_ATTENUATION_MAX));
                    d.stop();
                    if let Err(e) = result {
                        fail(&mut device, &mut error, e);
                    }
                }
                on = false;
                attenuation = f64::NAN;
                sent = (Instant::now(), 0);
                sent_rate = 0.0;
                delay.clear();
                modulator = None;
            }
            pending.clear();
        }

        let mut st = link.status.lock().unwrap();
        *st = TxStatus {
            state,
            mic_db: vox.level_db,
            vox_keyed: vox.keyed(),
            alc_db: if on { modulator.as_ref().map_or(0.0, |m| m.alc_db) } else { 0.0 },
            remaining: control.remaining(Instant::now()),
            sent_rate,
            error: error.clone(),
        };
        drop(st);
        if state != last_state || chunk.is_some() {
            last_state = state;
            (link.notify)();
        }
    }
    // Leaving: make sure nothing goes on transmitting.
    if let Some(d) = device.as_mut() {
        let _ = d.set_powered(false);
        let _ = d.set_attenuation(TX_ATTENUATION_MAX);
        d.stop();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn blocks_are_about_20_ms() {
        assert_eq!(block_len(2.4e6), 46 * 1024);
        assert_eq!(block_len(250e3), 16 * 1024);
    }

    #[test]
    fn default_band_fits_the_transponder() {
        let c = StationConfig::default();
        let lo = c.rx_center_hz - c.sample_rate / 2.0;
        let hi = c.rx_center_hz + c.sample_rate / 2.0;
        assert!(lo < 10_489.49e6 && hi > 10_490.01e6);
        // DC spur outside the transponder.
        assert!(c.rx_center_hz < crate::bandplan::DOWNLINK_LOW_HZ - 20e3);
        // The whole uplink is reachable from the transmit LO, which is outside it.
        let reach = c.sample_rate / 2.0 - TX_EDGE_HZ;
        assert!(c.tx_lo_hz < crate::bandplan::UPLINK_LOW_HZ);
        assert!(crate::bandplan::UPLINK_HIGH_HZ - c.tx_lo_hz < reach);
    }

    /// The whole station on the simulator, without sound devices: transmits with PTT
    /// and checks that the uplink signal comes back on the spectrum.
    /// `cargo test --release --lib station_on_simulator -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn station_on_simulator() {
        let config = StationConfig { microphone: Some("none".into()), speaker: Some("none".into()), ..Default::default() };
        let listen = ListenSettings { dial_hz: 0.0, mode: Mode::Usb, bandwidth_hz: 2.4e3, audio_rate: 48e3, volume: 1.0 };
        let engine = Engine::start(config, listen, TxSettings::default(), 0.5, || {});
        let started = Instant::now();
        let mut frames = 0;
        while started.elapsed() < Duration::from_secs(8) {
            match engine.events.recv_timeout(Duration::from_secs(2)) {
                Ok(Event::Opened { model, tx, .. }) => println!("opened {model}, tx {tx:?}"),
                Ok(Event::Frame(_)) => frames += 1,
                Ok(Event::Rate(r)) => println!("rate {r:.0}"),
                Ok(Event::Error(e)) => panic!("{e}"),
                Err(e) => panic!("{e}"),
            }
        }
        println!("{frames} frames, beacon {:?}", *engine.beacon.lock().unwrap());
        assert!(frames > 100);
        let correction = engine.correction().expect("beacon lock");
        assert!((correction - 3_000.0).abs() < 100.0, "beacon correction {correction}");

        // Tune tone on 10489.700 (USB): it must come back at 10489.701.
        let dial = 10_489.700e6;
        let lo = engine.config.lnb_lo_hz;
        {
            let mut tx = engine.tx.lock().unwrap();
            tx.uplink_hz = crate::bandplan::uplink(dial);
            tx.armed = true;
            tx.tune = true;
        }
        let tone_if = dial + TUNE_HZ - lo - correction;
        *engine.view.lock().unwrap() = View { offset_hz: tone_if - engine.config.center_if(), span_hz: 12e3 };
        let started = Instant::now();
        let mut last = None;
        while started.elapsed() < Duration::from_secs(3) {
            if let Ok(Event::Frame(f)) = engine.events.recv_timeout(Duration::from_secs(2)) {
                last = Some(f);
            }
        }
        println!("tx status {:?}", engine.tx_status.lock().unwrap());
        assert!(engine.tx_status.lock().unwrap().sent_rate > 2.3e6, "the radio is fed in real time");
        let f = last.expect("frame");
        let peak = f.db.iter().enumerate().max_by(|a, b| a.1.total_cmp(b.1)).unwrap();
        let at = f.lo_hz + (peak.0 as f64 + 0.5) / f.db.len() as f64 * (f.hi_hz - f.lo_hz);
        let mut sorted = f.db.clone();
        sorted.sort_by(f32::total_cmp);
        let median = sorted[sorted.len() / 2];
        println!("peak {:.1} dB over the median at {:+.0} Hz from the expected place", peak.1 - median, at - tone_if);
        assert!(peak.1 - median > 20.0 && (at - tone_if).abs() < 60.0);
        assert_eq!(engine.tx_status.lock().unwrap().state, TxState::Transmit);
    }
}
