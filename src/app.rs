//! The QO-ALL window: status bar, the two frequency displays, the transponder's spectrum
//! and waterfall, and the decoder, sound/VOX and transmit cards.

use std::time::{Duration, SystemTime, UNIX_EPOCH};

use eframe::egui::{self, Align2, Color32, ColorImage, CornerRadius, FontId, Rect, RichText, Sense, Stroke, TextureHandle, TextureOptions, pos2, vec2};

use qo_all::bandplan::{self, Refusal};
use qo_all::beacon::BeaconStatus;
use qo_all::demod::Mode;
use qo_all::engine::{DeviceChoice, Engine, Event, StationConfig, TxSettings};
use qo_all::ft8::worker::Ft8Message;
use qo_all::iio;
use qo_all::listen::ListenSettings;
use qo_all::source::{FRAME_POINTS, Frame, View};
use qo_all::ft8::{FT4, FT8, Protocol};
use qo_all::tune;
use qo_all::vox::{Blocked, TxState, VoxSettings};

use qo_all::serialio::{KeyInput, PttLine, SerialConfig, SerialIo};

use crate::ftpanel::{self, FtPanel};
use crate::logpanel::{LogPanel, Station};
use crate::settings::Settings;
use crate::ui::{self, palette::*};

/// The transponder as shown when not zoomed in (Hz).
const FULL_VIEW: (f64, f64) = (10_489.490e6, 10_490.010e6);
/// Zoom spans (Hz); None shows the whole transponder.
const ZOOMS: [Option<f64>; 3] = [None, Some(60e3), Some(12e3)];
const WATERFALL_ROWS: usize = 300;
/// FT8 and FT4 dial on QO-100 (downlink, Hz).
const FT_DIAL_HZ: f64 = 10_489.540e6;
/// Sample rate for radios on Ethernet: a Pluto there takes only about 5 MB/s, and the
/// transmitter needs it at the receiver's rate. 0.6 MS/s still covers the transponder
/// (±250 kHz around the middle beacon).
const LOW_RATE: f64 = 0.6e6;
/// Modes offered: SSB on QO-100 is USB only.
const MODES: [Mode; 2] = [Mode::Usb, Mode::Cw];
/// Largest transmit attenuation offered on the slider (dB).
const MAX_ATTENUATION: f64 = 60.0;

#[derive(Clone, Copy, PartialEq)]
enum Lang {
    En,
    Tr,
}

#[derive(Clone, Copy, PartialEq)]
enum DecoderTab {
    Ft8,
    Ft4,
    Cw,
}

/// The kinds of radio offered in the settings.
#[derive(Clone, Copy, PartialEq)]
enum RadioKind {
    Simulator,
    Iio,
    RtlUsb,
    RtlTcp,
    HackRf,
}

impl RadioKind {
    fn of(d: &DeviceChoice) -> Self {
        match d {
            DeviceChoice::Simulator => Self::Simulator,
            DeviceChoice::Iio(_) => Self::Iio,
            DeviceChoice::RtlSdr(_) => Self::RtlUsb,
            DeviceChoice::RtlTcp(_) => Self::RtlTcp,
            DeviceChoice::HackRf(_) => Self::HackRf,
        }
    }
}

/// The allowed sample rate nearest to `rate`.
fn fit_rate(device: &DeviceChoice, rate: f64) -> f64 {
    device.rates().iter().copied().min_by(|a, b| (a - rate).abs().total_cmp(&(b - rate).abs())).unwrap_or(rate)
}

pub struct App {
    settings: Settings,
    lang: Lang,
    config: StationConfig,
    engine: Option<Engine>,
    /// What the radio said when it opened, or why it did not.
    model: Option<String>,
    tx_capable: Option<Result<(), String>>,
    warning: Option<String>,
    /// Shown after the sample rate was changed for the radio on its own.
    rate_notice: Option<String>,
    /// Added to the estimated dBm (dB), from a calibration against a known level.
    dbm_cal: f32,
    /// The receive level, smoothed (dBm).
    meter_dbm: f32,
    /// The S-meter's needle: signal over the noise floor, smoothed (dB).
    snr: f32,
    /// The level fed in for a calibration (dBm).
    cal_known_dbm: f32,
    error: Option<String>,
    rate: f64,
    /// Last known converter correction (Hz), kept for the next start.
    correction: f64,

    /// Dial frequencies on the downlink (Hz); the transmitter uses `tx_dial` only when
    /// not linked to the receiver.
    rx_dial: f64,
    tx_dial: f64,
    linked: bool,
    mode: Mode,
    auto_lock: bool,
    /// Added to the uplink frequency to make up for the radio's own error (Hz).
    tx_correction: f64,
    /// Receive and transmit offsets from the dials (Hz), for small differences between
    /// the two: each applies only while switched on.
    rit_on: bool,
    rit: f64,
    xit_on: bool,
    xit: f64,
    /// Whether the self check's measurements correct the uplink by themselves, the last
    /// measurement, and the one already applied.
    auto_uplink: bool,
    last_measurement: Option<qo_all::selfcheck::Measurement>,
    applied_measurement: u64,

    armed: bool,
    ptt_latched: bool,
    tune: bool,
    /// Digital and CW audio level (0..1).
    drive: f32,
    /// CW: words per minute and the text box.
    cw_wpm: f64,
    cw_text: String,
    /// The country file path being edited in the settings.
    edit_cty: String,
    attenuation: f64,
    mic_gain_db: f32,
    compress: bool,
    vox: VoxSettings,
    time_out_s: u32,
    volume: f32,
    /// Speaker off without losing the volume set.
    muted: bool,

    zoom: usize,
    /// Centre of the zoomed view (downlink, Hz).
    zoom_center: f64,
    /// Smoothed spectrum line, its band (downlink, Hz) and noise floor.
    line: Vec<f32>,
    line_band: (f64, f64),
    floor: f32,
    waterfall: ColorImage,
    waterfall_tex: Option<TextureHandle>,
    logo: TextureHandle,

    tab: DecoderTab,
    ft4_on: bool,
    show_settings: bool,
    /// Settings being edited before they are applied.
    edit_kind: RadioKind,
    edit_uri: String,
    edit_rtl_index: u32,
    edit_rtl_tcp: String,
    edit_hackrf: String,
    edit_tx_enabled: bool,
    /// The antenna/SWR warning is open, waiting for an answer.
    tx_warning: bool,
    edit_ppm: f64,
    rtl_found: Vec<(u32, String)>,
    hackrf_found: Vec<String>,
    /// PTT out and the CW key in on serial ports, and their settings being edited.
    serial: SerialIo,
    edit_serial: SerialConfig,
    ports: Vec<(String, String)>,
    edit_lnb_mhz: f64,
    edit_gain: f64,
    edit_auto_gain: bool,
    edit_rate: f64,
    usb_found: Vec<(String, String)>,
    mics: Vec<qo_all::audio::OutputDevice>,
    /// FT8 or FT4 (the receiver then works in USB on the FT dial), and the SSB dial to
    /// go back to.
    digital: Option<&'static Protocol>,
    ssb_dial: f64,
    ft: FtPanel,
    /// The log window, always at the bottom.
    log: LogPanel,
    edit_call: String,
    edit_grid: String,
    edit_log: String,
    speakers: Vec<qo_all::audio::OutputDevice>,
}

impl App {
    pub fn new(cc: &eframe::CreationContext) -> Self {
        let s = Settings::load();
        // Night (dark) or paper (light) theme.
        ui::palette::set_light(s.text("theme").as_deref() == Some("paper"));
        ui::apply_theme(&cc.egui_ctx);
        let lang = match s.text("lang").as_deref() {
            Some("en") => Lang::En,
            Some("tr") => Lang::Tr,
            _ => {
                if sys_locale::get_locale().is_some_and(|l| l.starts_with("tr")) {
                    Lang::Tr
                } else {
                    Lang::En
                }
            }
        };
        let device = DeviceChoice::parse(&s.text("device").unwrap_or_default());
        let defaults = StationConfig::default();
        // Ethernet links carry too little for 2.4 MS/s both ways: the low rate there.
        let rate = s.get("sample_rate").unwrap_or(if matches!(device, DeviceChoice::Iio(ref u) if u.starts_with("ip:")) { LOW_RATE } else { 2.4e6 });
        // An earlier rate the radio is no longer used at (1.2 MS/s was too much for an
        // Ethernet link) becomes the nearest one offered now.
        let rate = fit_rate(&device, rate);
        let config = StationConfig {
            device,
            lnb_lo_hz: s.get("lnb_lo_hz").unwrap_or(defaults.lnb_lo_hz),
            rx_gain: if s.get("rx_auto_gain").unwrap_or(false) { None } else { Some(s.get("rx_gain").unwrap_or(40.0)) },
            microphone: s.text("microphone"),
            speaker: s.text("speaker"),
            tx_enabled: s.get("tx_enabled").unwrap_or(true),
            ppm: s.get("ppm").unwrap_or(0.0),
            ..defaults
        }
        .with_rate(rate);
        let serial_config = SerialConfig {
            ptt_port: s.text("ptt_port").unwrap_or_default(),
            ptt_line: PttLine::from_name(&s.text("ptt_line").unwrap_or_default()),
            key_port: s.text("key_port").unwrap_or_default(),
            key_input: KeyInput::from_name(&s.text("key_input").unwrap_or_default()),
        };
        let logo = qo_all::logo::rgba(96, false);
        let logo = cc.egui_ctx.load_texture("logo", ColorImage::from_rgba_unmultiplied([96, 96], &logo), TextureOptions::LINEAR);
        // LSB is not used on QO-100: a saved LSB becomes USB.
        let mode = s.text("mode").and_then(|m| Mode::from_name(&m)).filter(|m| MODES.contains(m)).unwrap_or(Mode::Usb);
        let tx_defaults = TxSettings::default();
        let vox = VoxSettings {
            enabled: s.get("vox").unwrap_or(false),
            threshold_db: s.get("vox_threshold_db").unwrap_or(-32.0),
            hang_ms: s.get("vox_hang_ms").unwrap_or(600),
            preroll_ms: s.get("vox_preroll_ms").unwrap_or(150),
            anti_vox_db: s.get("anti_vox_db"),
        };
        let rx_dial = s.get("rx_dial_hz").unwrap_or(10_489.725e6);
        let mut app = Self {
            lang,
            correction: s.get("correction_hz").unwrap_or(0.0),
            rx_dial,
            tx_dial: s.get("tx_dial_hz").unwrap_or(rx_dial),
            linked: s.get("linked").unwrap_or(true),
            mode,
            auto_lock: s.get("auto_lock").unwrap_or(true),
            tx_correction: s.get("tx_correction_hz").unwrap_or(0.0),
            rit_on: s.get("rit_on").unwrap_or(false),
            rit: s.get("rit_hz").unwrap_or(0.0),
            xit_on: s.get("xit_on").unwrap_or(false),
            xit: s.get("xit_hz").unwrap_or(0.0),
            auto_uplink: s.get("auto_uplink").unwrap_or(true),
            last_measurement: None,
            applied_measurement: 0,
            armed: false,
            ptt_latched: false,
            tune: false,
            drive: s.get("drive").unwrap_or(1.0),
            cw_wpm: s.get("cw_wpm").unwrap_or(20.0),
            cw_text: String::new(),
            edit_cty: s.text("cty_path").unwrap_or_default(),
            attenuation: s.get("tx_attenuation_db").unwrap_or(tx_defaults.attenuation_db),
            mic_gain_db: s.get("mic_gain_db").unwrap_or(0.0),
            compress: s.get("compress").unwrap_or(false),
            vox,
            time_out_s: s.get("time_out_s").unwrap_or(180),
            volume: s.get("volume").unwrap_or(0.4),
            muted: s.get("muted").unwrap_or(false),
            zoom: s.get("zoom").unwrap_or(0).min(ZOOMS.len() - 1),
            zoom_center: rx_dial,
            line: Vec::new(),
            line_band: FULL_VIEW,
            floor: -100.0,
            waterfall: ColorImage::new([FRAME_POINTS, WATERFALL_ROWS], vec![DEEP(); FRAME_POINTS * WATERFALL_ROWS]),
            waterfall_tex: None,
            logo,
            tab: match s.text("decoder_tab").as_deref() {
                Some("FT4") => DecoderTab::Ft4,
                Some("CW") => DecoderTab::Cw,
                _ => DecoderTab::Ft8,
            },
            ft4_on: s.get("ft4").unwrap_or(false),
            show_settings: false,
            edit_kind: RadioKind::of(&config.device),
            edit_uri: match &config.device {
                DeviceChoice::Iio(u) => u.clone(),
                _ => s.text("last_uri").unwrap_or_else(|| "ip:192.168.2.1".into()),
            },
            edit_rtl_index: match config.device {
                DeviceChoice::RtlSdr(i) => i,
                _ => 0,
            },
            edit_rtl_tcp: match &config.device {
                DeviceChoice::RtlTcp(a) => a.clone(),
                _ => s.text("last_rtl_tcp").unwrap_or_default(),
            },
            edit_hackrf: match &config.device {
                DeviceChoice::HackRf(Some(sn)) => sn.clone(),
                _ => String::new(),
            },
            edit_tx_enabled: config.tx_enabled,
            tx_warning: false,
            edit_ppm: config.ppm,
            rtl_found: Vec::new(),
            hackrf_found: Vec::new(),
            serial: SerialIo::start(serial_config.clone()),
            edit_serial: serial_config,
            ports: Vec::new(),
            edit_lnb_mhz: config.lnb_lo_hz / 1e6,
            edit_gain: config.rx_gain.unwrap_or(40.0),
            edit_auto_gain: config.rx_gain.is_none(),
            edit_rate: config.sample_rate,
            usb_found: Vec::new(),
            mics: qo_all::audio::input_devices(),
            // The last mode, FT8/FT4 included.
            digital: match s.text("digital").as_deref() {
                Some("FT8") => Some(&FT8),
                Some("FT4") => Some(&FT4),
                _ => None,
            },
            ssb_dial: s.get("ssb_dial_hz").unwrap_or(rx_dial),
            log: LogPanel::new(s.text("adif_path").map(std::path::PathBuf::from).unwrap_or_else(ftpanel::default_log)),
            ft: FtPanel::new(&s),
            edit_call: s.text("my_call").unwrap_or_default(),
            edit_grid: s.text("my_grid").unwrap_or_default(),
            edit_log: s.text("adif_path").unwrap_or_else(|| ftpanel::default_log().display().to_string()),
            speakers: qo_all::audio::output_devices(),
            config,
            engine: None,
            model: None,
            tx_capable: None,
            warning: None,
            rate_notice: None,
            dbm_cal: s.get("dbm_cal_db").unwrap_or(0.0),
            meter_dbm: -160.0,
            snr: 0.0,
            cal_known_dbm: -80.0,
            error: None,
            rate: 0.0,
            settings: s,
        };
        // Starting in FT8/FT4: the receiver on the FT dial.
        if app.digital.is_some() && (app.rx_dial - FT_DIAL_HZ).abs() > 3e3 {
            app.rx_dial = FT_DIAL_HZ;
            app.zoom_center = FT_DIAL_HZ;
            app.linked = true;
        }
        app.start(&cc.egui_ctx);
        app
    }

    fn t(&self, en: &'static str, tr: &'static str) -> &'static str {
        if self.lang == Lang::Tr { tr } else { en }
    }

    fn start(&mut self, ctx: &egui::Context) {
        self.serial.attach(None);
        self.engine = None;
        self.model = None;
        self.tx_capable = None;
        self.warning = None;
        self.error = None;
        self.rate = 0.0;
        self.armed = false;
        self.ptt_latched = false;
        self.tune = false;
        // The new station's measurements count from one again.
        self.applied_measurement = 0;
        // Nothing of the previous radio stays on screen (a failed connection must not
        // look like a working one).
        self.line.clear();
        self.waterfall.pixels.fill(DEEP());
        if let Some(tex) = &mut self.waterfall_tex {
            tex.set(self.waterfall.clone(), TextureOptions::LINEAR);
        }
        let listen = ListenSettings { dial_hz: 0.0, mode: self.mode, bandwidth_hz: self.mode.default_bandwidth(), audio_rate: 48e3, volume: self.volume };
        let ctx = ctx.clone();
        let engine = Engine::start(self.config.clone(), listen, TxSettings::default(), self.volume, move || ctx.request_repaint());
        self.serial.attach(Some(engine.serial_station()));
        self.engine = Some(engine);
        self.sync();
    }

    /// Switches between the night and the paper theme; the waterfall starts over in the
    /// new colours.
    fn set_theme(&mut self, ctx: &egui::Context, paper: bool) {
        ui::palette::set_light(paper);
        ui::apply_theme(ctx);
        self.waterfall.pixels.fill(DEEP());
        if let Some(tex) = &mut self.waterfall_tex {
            tex.set(self.waterfall.clone(), TextureOptions::LINEAR);
        }
        self.settings.set("theme", if paper { "paper" } else { "night" });
        self.save();
    }

    /// The volume the speaker plays at.
    fn speaker_volume(&self) -> f32 {
        if self.muted { 0.0 } else { self.volume }
    }

    /// A level in dBFS as dBm at the radio's antenna input (after the LNB): the radio's
    /// full scale at 0 dB gain, less the receive gain, plus the calibration. Estimated
    /// (automatic gain counts as 40 dB) until calibrated.
    fn dbm(&self, dbfs: f32) -> f32 {
        let full_scale = match self.config.device {
            DeviceChoice::Iio(_) => -5.0,
            DeviceChoice::RtlSdr(_) | DeviceChoice::RtlTcp(_) => -10.0,
            DeviceChoice::HackRf(_) => -5.0,
            DeviceChoice::Simulator => 0.0,
        };
        dbfs + full_scale - self.config.rx_gain.unwrap_or(40.0) as f32 + self.dbm_cal
    }

    /// Signal to noise in the receiver's passband, as on the spectrum: its strongest
    /// point over the noise floor (dB). Needs no calibration.
    fn snr_db(&self) -> Option<f32> {
        let n = self.line.len();
        if n < 2 {
            return None;
        }
        let (lo, hi) = self.line_band;
        let (pa, pb) = self.mode.passband(self.mode.default_bandwidth());
        let listen = self.rx_listen();
        let index = |hz: f64| (((hz - lo) / (hi - lo)) * n as f64).floor();
        let (a, b) = (index(listen + pa), index(listen + pb));
        if b < 0.0 || a >= n as f64 {
            return None;
        }
        let (a, b) = (a.max(0.0) as usize, (b.max(a) as usize).min(n - 1));
        let peak = self.line[a..=b].iter().copied().fold(f32::MIN, f32::max);
        Some((peak - self.floor).max(0.0))
    }

    fn correction_now(&self) -> f64 {
        self.engine.as_ref().and_then(|e| e.correction()).unwrap_or(self.correction)
    }

    fn tx_downlink(&self) -> f64 {
        if self.linked { self.rx_dial } else { self.tx_dial }
    }

    /// Where the receiver listens: the dial plus the RIT.
    fn rx_listen(&self) -> f64 {
        self.rx_dial + if self.rit_on { self.rit } else { 0.0 }
    }

    /// Where the transmitted signal comes down: the transmit dial plus the XIT.
    fn tx_signal(&self) -> f64 {
        self.tx_downlink() + if self.xit_on { self.xit } else { 0.0 }
    }

    fn uplink(&self) -> f64 {
        bandplan::uplink(self.tx_signal()) + self.tx_correction
    }

    /// Band plan check of the transmitted signal (in FT8/FT4 only its tones).
    fn tx_band(&self) -> Result<(), Refusal> {
        let d = self.tx_signal();
        match self.digital {
            Some(p) => bandplan::check(d + self.ft.audio_hz, d + self.ft.audio_hz + FtPanel::signal_width(p)),
            // CW: the carrier on the dial and its keying sidebands.
            None if self.mode == Mode::Cw => bandplan::check(d - 100.0, d + 100.0),
            None => bandplan::check(d + qo_all::tx::AUDIO_LOW, d + qo_all::tx::AUDIO_HIGH),
        }
    }

    /// Switches between the voice modes and FT8/FT4.
    fn set_digital(&mut self, d: Option<&'static Protocol>) {
        if d.is_some() && self.digital.is_none() {
            self.ssb_dial = self.rx_dial;
            self.tune_rx(FT_DIAL_HZ);
            self.linked = true;
            self.mode = Mode::Usb;
        }
        if d.is_none() && self.digital.is_some() {
            self.ft.halt(self.engine.as_ref());
            self.tune_rx(self.ssb_dial);
        }
        if d.is_some() && !std::ptr::eq(d.unwrap(), self.digital.unwrap_or(d.unwrap())) {
            // FT8 ↔ FT4: what was scheduled belongs to the other one.
            self.ft.halt(self.engine.as_ref());
        }
        self.digital = d;
        self.ptt_latched = false;
        self.tune = false;
    }

    fn view_band(&self) -> (f64, f64) {
        match ZOOMS[self.zoom] {
            None => FULL_VIEW,
            Some(span) => (self.zoom_center - span / 2.0, self.zoom_center + span / 2.0),
        }
    }

    /// Hands the window's settings to the engine.
    fn sync(&mut self) {
        let Some(e) = &self.engine else { return };
        let c = self.correction_now();
        let to_if = |rf: f64| rf - self.config.lnb_lo_hz - c;
        e.listener.update(|l| {
            l.dial_hz = to_if(self.rx_listen());
            l.mode = self.mode;
            l.bandwidth_hz = self.mode.default_bandwidth();
            l.volume = self.speaker_volume();
        });
        e.volume.set(self.speaker_volume());
        {
            let mut tx = e.tx.lock().unwrap();
            tx.mode = Mode::Usb;
            tx.uplink_hz = self.uplink();
            tx.attenuation_db = self.attenuation;
            tx.mic_gain_db = self.mic_gain_db;
            tx.compress = self.compress;
            tx.vox = self.vox;
            // CW transmission comes in a later version.
            tx.armed = self.armed;
            tx.cw = self.mode == Mode::Cw && self.digital.is_none();
            tx.drive = self.drive;
            tx.tune = self.tune && tx.armed;
            // The tune tone goes where the mode's signal goes: 1 kHz above the dial in SSB,
            // the middle of the FT tones, the CW carrier (the modulator sits below it).
            tx.tune_hz = match self.digital {
                Some(p) => self.ft.audio_hz + FtPanel::signal_width(p) / 2.0,
                None if self.mode == Mode::Cw => qo_all::keyer::TONE_HZ,
                None => 1_000.0,
            };
            tx.band = self.tx_band();
            tx.digital = self.digital.is_some();
            tx.time_out = Duration::from_secs(self.time_out_s as u64);
        }
        // In FT8/FT4 the decoders follow the dial; otherwise FT8 runs on its usual one.
        let ft_dial = if self.digital.is_some() { self.rx_dial } else { FT_DIAL_HZ };
        e.ft8.set_dial(Some(to_if(ft_dial)));
        e.ft8.set_my_call(&self.ft.seq.my_call);
        e.ft4.set_my_call(&self.ft.seq.my_call);
        // The own signal's carrier should come down on the transmit dial (plus XIT).
        // In CW the modulator's carrier sits the tone's pitch below the dial.
        let carrier = self.tx_signal() - if self.mode == Mode::Cw && self.digital.is_none() { qo_all::keyer::TONE_HZ } else { 0.0 };
        e.selfcheck.set_expected(Some(to_if(carrier)));
        e.keyer.lock().unwrap().wpm = self.cw_wpm;
        let ft4 = self.ft4_on || self.digital.is_some_and(|p| std::ptr::eq(p, &FT4));
        e.ft4.set_dial(ft4.then(|| to_if(ft_dial)));
        let (lo, hi) = self.view_band();
        let view = View { offset_hz: to_if((lo + hi) / 2.0) - self.config.center_if(), span_hz: hi - lo };
        *e.view.lock().unwrap() = view;
    }

    fn set_ptt(&mut self, on: bool) {
        if let Some(e) = &self.engine {
            e.tx.lock().unwrap().ptt = on;
        }
    }

    fn poll(&mut self, ctx: &egui::Context) {
        let Some(e) = &self.engine else { return };
        let mut faster = false;
        let events: Vec<Event> = e.events.try_iter().collect();
        // A new measurement of the own signal: it came down this much off, so the uplink
        // moves the other way. Small ones (within the measuring accuracy) are left.
        if let Some(m) = e.selfcheck.latest()
            && m.id != self.applied_measurement
        {
            self.applied_measurement = m.id;
            self.last_measurement = Some(m);
            if self.auto_uplink && m.estimate.offset_hz.abs() >= 3.0 {
                self.tx_correction = (self.tx_correction - m.estimate.offset_hz).round();
            }
        }
        if let Some(c) = e.correction() {
            self.correction = c;
        }
        for ev in events {
            match ev {
                Event::Opened { model, tx, warning } => {
                    faster = needs_full_rate(&self.config, &model);
                    self.model = Some(model);
                    self.tx_capable = Some(tx);
                    self.warning = warning;
                }
                Event::Frame(f) => self.push_frame(f),
                Event::Rate(r) => self.rate = r,
                Event::Error(e) => self.error = Some(e),
            }
        }
        if faster {
            // A gigabit AD936x board (LibreSDR) drops the connection a few seconds into
            // the low rate's FIR set-up; 2.4 MS/s works there.
            self.config = self.config.clone().with_rate(2.4e6);
            self.edit_rate = 2.4e6;
            self.save();
            self.start(ctx);
            self.rate_notice = Some(self.t("Not a Pluto: switched to 2.4 MS/s", "Pluto değil: 2,4 MS/s'ye geçildi").into());
        }
    }

    fn push_frame(&mut self, f: Frame) {
        let shift = self.config.lnb_lo_hz + self.correction;
        let band = (f.lo_hz + shift, f.hi_hz + shift);
        let n = f.db.len();
        if n == 0 {
            return;
        }
        if self.line.len() == n && (band.0 - self.line_band.0).abs() < (band.1 - band.0) * 0.01 {
            for (s, &v) in self.line.iter_mut().zip(&f.db) {
                *s += (v - *s) * 0.45;
            }
        } else {
            self.line = f.db.clone();
        }
        self.line_band = band;
        let mut sorted = f.db.clone();
        sorted.sort_by(f32::total_cmp);
        let floor = sorted[n / 5];
        self.floor += (floor - self.floor) * 0.1;

        // Waterfall: scroll down one row and write the new row at the top.
        let w = FRAME_POINTS;
        self.waterfall.pixels.copy_within(0..w * (WATERFALL_ROWS - 1), w);
        let lo = self.floor + 3.0;
        for (x, px) in self.waterfall.pixels[..w].iter_mut().enumerate() {
            let v = f.db[(x * n / w).min(n - 1)];
            *px = ui::colormap((v - lo) / 45.0);
        }
        if let Some(tex) = &mut self.waterfall_tex {
            tex.set(self.waterfall.clone(), TextureOptions::LINEAR);
        }
    }

    fn save(&mut self) {
        let s = &mut self.settings;
        s.set("lang", if self.lang == Lang::Tr { "tr" } else { "en" });
        s.set("device", self.config.device.setting());
        s.set("last_uri", &self.edit_uri);
        s.set("last_rtl_tcp", &self.edit_rtl_tcp);
        s.set("tx_enabled", self.config.tx_enabled);
        s.set("ppm", self.config.ppm);
        s.set("dbm_cal_db", self.dbm_cal);
        s.set("ptt_port", &self.serial.config.ptt_port);
        s.set("ptt_line", self.serial.config.ptt_line.name());
        s.set("key_port", &self.serial.config.key_port);
        s.set("key_input", self.serial.config.key_input.name());
        s.set("lnb_lo_hz", self.config.lnb_lo_hz);
        s.set("sample_rate", self.config.sample_rate);
        s.set("rx_auto_gain", self.config.rx_gain.is_none());
        s.set("rx_gain", self.config.rx_gain.unwrap_or(self.edit_gain));
        s.set_opt("microphone", self.config.microphone.as_ref());
        s.set_opt("speaker", self.config.speaker.as_ref());
        s.set("correction_hz", self.correction.round());
        s.set("rx_dial_hz", self.rx_dial);
        s.set("tx_dial_hz", self.tx_dial);
        s.set("linked", self.linked);
        s.set("mode", self.mode.name());
        s.set("auto_lock", self.auto_lock);
        s.set("tx_correction_hz", self.tx_correction);
        s.set("rit_on", self.rit_on);
        s.set("rit_hz", self.rit);
        s.set("xit_on", self.xit_on);
        s.set("xit_hz", self.xit);
        s.set("auto_uplink", self.auto_uplink);
        s.set("tx_attenuation_db", self.attenuation);
        s.set("mic_gain_db", self.mic_gain_db);
        s.set("compress", self.compress);
        s.set("vox", self.vox.enabled);
        s.set("vox_threshold_db", self.vox.threshold_db);
        s.set("vox_hang_ms", self.vox.hang_ms);
        s.set("vox_preroll_ms", self.vox.preroll_ms);
        s.set_opt("anti_vox_db", self.vox.anti_vox_db);
        s.set("time_out_s", self.time_out_s);
        s.set("volume", self.volume);
        s.set("drive", self.drive);
        s.set("cw_wpm", self.cw_wpm);
        s.set("muted", self.muted);
        s.set("zoom", self.zoom);
        s.set("ft4", self.ft4_on);
        s.set("decoder_tab", match self.tab {
            DecoderTab::Ft8 => "FT8",
            DecoderTab::Ft4 => "FT4",
            DecoderTab::Cw => "CW",
        });
        self.ft.save(s);
        s.set("digital", self.digital.map_or("", |p| p.name));
        s.set("ssb_dial_hz", self.ssb_dial);
        s.save();
    }

    /// Moves the receiver (and a linked transmitter) to `hz`.
    fn tune_rx(&mut self, hz: f64) {
        self.rx_dial = (hz / 10.0).round() * 10.0;
        if let Some(span) = ZOOMS[self.zoom] {
            if (self.rx_dial - self.zoom_center).abs() > span * 0.4 {
                self.zoom_center = self.rx_dial;
                self.waterfall.pixels.fill(DEEP());
            }
        }
    }

    // ---- Top bar ------------------------------------------------------------------

    fn header(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            ui.image((self.logo.id(), vec2(30.0, 30.0)));
            ui.label(RichText::new("QO").size(19.0).strong().color(STRONG()));
            ui.add_space(-4.0);
            ui.label(RichText::new("ALL").size(19.0).strong().color(RX()));
            ui.add_space(10.0);
            // The mode, as on a transceiver's front: one row of joined keys.
            let digital = |p: &'static Protocol| self.digital.is_some_and(|d| std::ptr::eq(d, p));
            let mut keys: Vec<(&str, bool)> = MODES.iter().map(|m| (m.name(), self.mode == *m && self.digital.is_none())).collect();
            keys.push((FT8.name, digital(&FT8)));
            keys.push((FT4.name, digital(&FT4)));
            if let Some(i) = ui::segmented(ui, &keys, RX()) {
                if i < MODES.len() {
                    self.set_digital(None);
                    self.mode = MODES[i];
                } else {
                    let p: &'static Protocol = if i == MODES.len() { &FT8 } else { &FT4 };
                    if !keys[i].1 {
                        self.set_digital(Some(p));
                    }
                }
            }
            ui.add_space(10.0);
            match (&self.error, &self.model) {
                (Some(e), _) => {
                    ui::pill(ui, e.clone(), ALERT(), ALERT_BG());
                }
                (None, Some(m)) => {
                    let duplex = match &self.tx_capable {
                        Some(Ok(())) => self.t("full duplex", "full duplex"),
                        _ => self.t("receive only", "yalnızca alıcı"),
                    };
                    ui::pill(ui, format!("{m} · {duplex}"), BLUE(), RAISED());
                }
                (None, None) => {
                    ui::pill(ui, self.t("Connecting…", "Bağlanıyor…"), MUTED(), RAISED());
                }
            }
            let beacon = self.engine.as_ref().map(|e| *e.beacon.lock().unwrap());
            let (text, fg, bg) = match beacon.map(|b| b.status) {
                Some(BeaconStatus::Locked { .. }) => (
                    format!("{} · LNB {:+.0} Hz", self.t("Beacon locked", "Beacon kilitli"), self.correction),
                    GOOD(),
                    GOOD_BG(),
                ),
                Some(BeaconStatus::NotFound) => (self.t("Beacon not found", "Beacon bulunamadı").to_string(), ALERT(), RAISED()),
                Some(BeaconStatus::OutsideBand) => (self.t("Beacon outside the band", "Beacon bant dışında").to_string(), ALERT(), RAISED()),
                _ => (self.t("Looking for the beacon…", "Beacon aranıyor…").to_string(), MUTED(), RAISED()),
            };
            ui::pill(ui, text, fg, bg);
            if let Some(w) = &self.warning {
                ui::pill(ui, w.clone(), TX(), RAISED());
            }
            if let Some(n) = &self.rate_notice {
                ui::pill(ui, n.clone(), BLUE(), RAISED());
            }
            if self.rate > 0.0 && self.rate < self.config.sample_rate * 0.95 {
                ui::pill(ui, format!("{:.2} MS/s", self.rate / 1e6), ALERT(), RAISED())
                    .on_hover_text(self.t("The radio delivers fewer samples than set", "Cihaz ayarlanandan az örnek gönderiyor"));
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui.add(egui::Button::new(RichText::new(self.t("Settings", "Ayarlar")).color(TEXT()))).clicked() {
                    self.show_settings = !self.show_settings;
                    if self.show_settings {
                        self.ports = qo_all::serial::ports();
                        // The settings as they are now, not as last left unapplied.
                        self.edit_serial = self.serial.config.clone();
                    }
                }
                // Language: both shown, the one in use marked.
                for (lang, code) in [(Lang::En, "EN"), (Lang::Tr, "TR")] {
                    if ui::choice(ui, code, self.lang == lang, BLUE()) && self.lang != lang {
                        self.lang = lang;
                        self.save();
                    }
                }
                ui.add_space(10.0);
                // Speaker volume and mute, in every mode (right to left here).
                if ui::choice(ui, self.t("Mute", "Sessiz"), self.muted, ALERT()) {
                    self.muted = !self.muted;
                }
                let mut percent = (self.volume * 100.0).round();
                ui.spacing_mut().slider_width = 110.0;
                let r = ui.add(egui::Slider::new(&mut percent, 0.0..=100.0).step_by(1.0).show_value(false));
                if r.changed() {
                    self.volume = percent / 100.0;
                    self.muted = false;
                }
                let level = if self.muted { self.t("off", "kapalı").to_string() } else { format!("{percent:.0}%") };
                ui.label(RichText::new(format!("{} {level}", self.t("Volume", "Ses"))).size(12.0).color(MUTED()));
                ui.add_space(10.0);
                let now = SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs());
                ui.label(RichText::new(format!("{} UTC", clock(now))).monospace().size(14.0).color(MUTED()));
            });
        });
    }

    // ---- Frequency displays -------------------------------------------------------

    fn vfos(&mut self, ui: &mut egui::Ui) {
        let status = self.engine.as_ref().map(|e| e.tx_status.lock().unwrap().clone()).unwrap_or_default();
        let transmitting = status.state == TxState::Transmit;
        let level = self.engine.as_ref().map_or(-150.0, |e| e.listener.level_db());
        let width = (ui.available_width() - 60.0) / 2.0;
        // The level in dBm (for the caption and the calibration), and the S-meter's
        // needle: the signal over the noise, rising fast and falling slower.
        let dbm = self.dbm(level);
        self.meter_dbm += (dbm - self.meter_dbm) * if dbm > self.meter_dbm { 0.6 } else { 0.25 };
        let snr = self.snr_db().unwrap_or(0.0);
        self.snr += (snr - self.snr) * if snr > self.snr { 0.5 } else { 0.15 };
        let needle = self.snr;
        let caption_dbm = self.meter_dbm;
        ui.ctx().request_repaint_after(Duration::from_millis(50));
        // Top-aligned, so both displays line up whatever each holds.
        ui.horizontal_top(|ui| {
            // Receiver.
            let column = egui::Layout::top_down(egui::Align::Min);
            let rx_change = ui
                .allocate_ui_with_layout(vec2(width, 80.0), column, |ui| {
                    ui::accent_card(ui, DEEP(), RX(), |ui| {
                        ui.set_width(ui.available_width());
                        // The frequency on the left, the signal meter on the right.
                        ui.horizontal_top(|ui| {
                            let meter_w = 250.0;
                            let left = (ui.available_width() - meter_w - 12.0).max(200.0);
                            let change = ui
                                .allocate_ui_with_layout(vec2(left, 150.0), column, |ui| {
                                    let mode = self.digital.map_or(self.mode.name(), |p| p.name);
                                    ui::caption(ui, format!("RX · 10 GHz · {mode}"), RX());
                                    let change = ui::vfo(ui, self.rx_dial, 34.0, RX());
                                    // Under it: the band plan's part, the signal in figures,
                                    // and the RIT when on.
                                    let rit = if self.rit_on { format!(" · RIT {:+.0} Hz", self.rit) } else { String::new() };
                                    let seg = bandplan::segment(self.rx_listen()).to_uppercase();
                                    let line = format!("{seg} · {} · {caption_dbm:.0} dBm · S/N {needle:.0} dB{rit}", ui::s_units(needle));
                                    ui.label(RichText::new(line).monospace().size(13.0).color(RX().gamma_multiply(0.75)));
                                    change
                                })
                                .inner;
                            ui.with_layout(egui::Layout::right_to_left(egui::Align::Min), |ui| {
                                ui::s_meter(ui, meter_w, needle);
                            });
                            change
                        })
                        .inner
                    })
                })
                .inner;
            if rx_change != 0.0 {
                self.tune_rx(self.rx_dial + rx_change);
            }
            let tip = self.t(
                "Linked: the transmitter follows the receiver. Click to set a separate (shift) frequency.",
                "Bağlı: verici alıcıyı izler. Ayrı (shift) frekans için tıklayın.",
            );
            if ui::link_button(ui, self.linked, tip) {
                if self.linked {
                    self.tx_dial = self.rx_dial;
                }
                self.linked = !self.linked;
            }
            // Transmitter.
            let shift = self.tx_downlink() - self.rx_dial;
            // On the air the card turns red, whatever the theme.
            let (fill, accent) = if transmitting { (ALERT_BG(), ALERT()) } else { (DEEP(), TX()) };
            // The big number is where the transmission comes down (10 GHz: in split the
            // place among the others), the uplink actually sent (2.4 GHz, corrections
            // included) under it.
            let uplink = self.uplink();
            let down = self.tx_signal();
            let tx_change = ui
                .allocate_ui_with_layout(vec2(width, 80.0), column, |ui| {
                    ui::accent_card(ui, fill, accent, |ui| {
                        ui.set_width(ui.available_width());
                        // As tall as the receive card with its meter.
                        ui.set_min_height(150.0);
                        let mode = self.digital.map_or(if self.mode == Mode::Cw { "CW" } else { "USB" }, |p| p.name);
                        ui::caption(ui, format!("TX · 2.4 GHz · {mode}"), TX());
                        let change = ui::vfo(ui, down, 34.0, TX());
                        let how = if self.linked { self.t("LINKED", "BAĞLI").to_string() } else { format!("SHIFT {:+.2} kHz", shift / 1e3) };
                        let state = if transmitting { self.t(" · ON AIR", " · YAYINDA") } else { "" };
                        ui.label(RichText::new(format!("UPLINK {:.5} MHz · {how}{state}", uplink / 1e6)).monospace().size(13.0).color(TX().gamma_multiply(0.75)));
                        change
                    })
                })
                .inner;
            if tx_change != 0.0 {
                if self.linked {
                    self.tune_rx(self.rx_dial + tx_change);
                } else {
                    self.tx_dial = ((self.tx_dial + tx_change) / 10.0).round() * 10.0;
                }
            }
        });
        ui.horizontal(|ui| {
            let lock = if self.auto_lock { self.t("Lock: automatic", "Kilit: otomatik") } else { self.t("Lock: manual", "Kilit: elle") };
            if ui::choice(ui, lock, self.auto_lock, BLUE()) {
                self.auto_lock = !self.auto_lock;
            }
            ui.add_space(12.0);
            offset_control(ui, "RIT", &mut self.rit_on, &mut self.rit, RX());
            offset_control(ui, "XIT", &mut self.xit_on, &mut self.xit, TX());
            ui.add_space(12.0);
            for (i, label) in ["520 kHz", "60 kHz", "12 kHz"].iter().enumerate() {
                if ui::choice(ui, label, self.zoom == i, BLUE()) {
                    self.zoom = i;
                    self.zoom_center = self.rx_dial;
                    self.waterfall.pixels.fill(DEEP());
                    self.line.clear();
                }
            }
            // Sound devices, changed at once (right to left here).
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let default = self.t("System default", "Sistem varsayılanı");
                let current = self.config.speaker.clone();
                let (pick, open) = device_combo(ui, "speaker", &self.speakers, &current, default);
                ui.label(RichText::new(self.t("Speaker", "Hoparlör")).size(12.0).color(MUTED()));
                if open {
                    self.speakers = qo_all::audio::output_devices();
                }
                if let Some(id) = pick {
                    self.config.speaker = id.clone();
                    if let Some(e) = self.engine.as_mut() {
                        e.set_speaker(id);
                    }
                    self.save();
                }
                ui.add_space(8.0);
                let current = self.config.microphone.clone();
                let (pick, open) = device_combo(ui, "mic", &self.mics, &current, default);
                ui.label(RichText::new(self.t("Microphone", "Mikrofon")).size(12.0).color(MUTED()));
                if open {
                    self.mics = qo_all::audio::input_devices();
                }
                if let Some(id) = pick {
                    self.config.microphone = id.clone();
                    if let Some(e) = self.engine.as_mut() {
                        e.set_microphone(id);
                    }
                    self.save();
                }
            });
        });
    }

    // ---- Spectrum and waterfall ---------------------------------------------------

    fn spectrum(&mut self, ui: &mut egui::Ui) {
        let rect = ui.available_rect_before_wrap();
        let response = ui.interact(rect, ui.id().with("spectrum"), Sense::click());
        let tex = self
            .waterfall_tex
            .get_or_insert_with(|| ui.ctx().load_texture("waterfall", self.waterfall.clone(), TextureOptions::LINEAR))
            .clone();
        let painter = ui.painter_at(rect);
        painter.rect_filled(rect, CornerRadius::same(8), DEEP());
        let spec = Rect::from_min_size(rect.min, vec2(rect.width(), (rect.height() * 0.36).max(80.0)));
        // Under the spectrum: the band plan's segments, then the frequency axis.
        let strip_h = 16.0;
        let axis_h = strip_h + 18.0;
        let wf = Rect::from_min_max(pos2(rect.left(), spec.bottom() + axis_h), rect.max);
        painter.image(tex.id(), wf, Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)), Color32::WHITE);

        let (lo_hz, hi_hz) = if self.line.is_empty() { self.view_band() } else { self.line_band };
        let width_hz = (hi_hz - lo_hz).max(1.0);
        let x_of = |hz: f64| rect.left() + ((hz - lo_hz) / width_hz) as f32 * rect.width();
        let hz_of = |x: f32| lo_hz + ((x - rect.left()) / rect.width()) as f64 * width_hz;

        // Beacons: nobody transmits there.
        for (a, b) in bandplan::RESERVED_HZ {
            let r = Rect::from_min_max(pos2(x_of(a).max(rect.left()), spec.top()), pos2(x_of(b).min(rect.right()), spec.bottom()));
            if r.width() > 0.0 {
                painter.rect_filled(r, CornerRadius::ZERO, ALERT().gamma_multiply(0.08));
            }
        }
        // Frequency grid and axis.
        let step = ui::nice_step(width_hz / (rect.width() as f64 / 110.0));
        let axis = Rect::from_min_max(pos2(rect.left(), spec.bottom()), pos2(rect.right(), wf.top()));
        painter.rect_filled(axis, CornerRadius::ZERO, BG());
        let strip = Rect::from_min_size(axis.min, vec2(axis.width(), strip_h));
        let scale_y = (strip.bottom() + axis.bottom()) / 2.0;
        let mut f = (lo_hz / step).ceil() * step;
        while f <= hi_hz {
            let x = x_of(f);
            painter.line_segment([pos2(x, spec.top()), pos2(x, spec.bottom())], Stroke::new(1.0, GRID()));
            let label = if step >= 1e3 { format!("{:.3}", f / 1e6) } else { format!("{:.4}", f / 1e6) };
            painter.text(pos2(x, scale_y), Align2::CENTER_CENTER, label, FontId::monospace(11.0), DIM());
            f += step;
        }
        // Band plan: CW, digital (FT8), mixed, SSB and the beacons.
        for (a, b, use_) in bandplan::SEGMENTS {
            let (x0, x1) = (x_of(a).max(rect.left()), x_of(b).min(rect.right()));
            if x1 <= x0 {
                continue;
            }
            let color = match use_ {
                bandplan::Use::Beacon => ALERT(),
                bandplan::Use::Cw => GOOD(),
                bandplan::Use::Digital => BLUE(),
                bandplan::Use::Mixed => MUTED(),
                bandplan::Use::Ssb => TX(),
            };
            let r = Rect::from_min_max(pos2(x0, strip.top() + 2.0), pos2(x1, strip.bottom() - 1.0)).shrink2(vec2(1.0, 0.0));
            painter.rect_filled(r, CornerRadius::same(3), color.gamma_multiply(0.22));
            // A faint tint of the same colour along the spectrum's foot.
            painter.rect_filled(Rect::from_min_max(pos2(x0, spec.bottom() - 2.0), pos2(x1, spec.bottom())), CornerRadius::ZERO, color.gamma_multiply(0.6));
            let label = match use_ {
                bandplan::Use::Digital => "FT8 · DIGI".to_string(),
                u => u.label().to_uppercase(),
            };
            let galley = painter.layout_no_wrap(label, FontId::proportional(10.5), color);
            if galley.size().x + 6.0 < r.width() {
                painter.galley(r.center() - galley.size() / 2.0, galley, color);
            }
        }
        // Spectrum line, filled below.
        let (db_lo, db_hi) = (self.floor - 8.0, self.floor + 55.0);
        let y_of = |db: f32| spec.bottom() - ((db - db_lo) / (db_hi - db_lo)).clamp(0.0, 1.0) * (spec.height() - 14.0);
        // Level scale in dBm (per FFT bin), every 10 dB, at the left.
        let off = self.dbm(0.0);
        let mut d = ((db_lo + off) / 10.0).ceil() * 10.0;
        while d <= db_hi + off {
            let y = y_of(d - off);
            if y > spec.top() + 16.0 && y < spec.bottom() - 4.0 {
                painter.line_segment([pos2(rect.left(), y), pos2(rect.right(), y)], Stroke::new(1.0, GRID().gamma_multiply(0.7)));
                painter.text(pos2(rect.left() + 4.0, y - 1.0), Align2::LEFT_BOTTOM, format!("{d:.0} dBm"), FontId::monospace(10.0), DIM());
            }
            d += 10.0;
        }
        if self.line.len() > 1 {
            let n = self.line.len();
            let points: Vec<egui::Pos2> =
                self.line.iter().enumerate().map(|(i, &v)| pos2(rect.left() + (i as f32 + 0.5) / n as f32 * rect.width(), y_of(v))).collect();
            let mut mesh = egui::Mesh::default();
            // Brightest under the peaks, fading to the foot: the depth of a real display.
            let (top, foot) = (BLUE().gamma_multiply(0.5), BLUE().gamma_multiply(0.04));
            let span = spec.height().max(1.0);
            for p in &points {
                let t = ((spec.bottom() - p.y) / span).clamp(0.0, 1.0);
                mesh.colored_vertex(*p, foot.lerp_to_gamma(top, 0.25 + 0.75 * t));
                mesh.colored_vertex(pos2(p.x, spec.bottom()), foot);
            }
            for i in 0..(n as u32 - 1) {
                let k = i * 2;
                mesh.add_triangle(k, k + 1, k + 2);
                mesh.add_triangle(k + 1, k + 3, k + 2);
            }
            painter.add(egui::Shape::mesh(mesh));
            painter.add(egui::Shape::line(points, Stroke::new(1.5, BLUE())));
        }
        // FT8 segment marker.
        let ft = x_of(FT_DIAL_HZ);
        if (rect.left()..rect.right()).contains(&ft) {
            painter.text(pos2(ft + 3.0, spec.top() + 4.0), Align2::LEFT_TOP, "FT8", FontId::proportional(11.0), DIM());
        }
        // Receiver passband and dial.
        let (pa, pb) = self.mode.passband(self.mode.default_bandwidth());
        let listen = self.rx_listen();
        let rx_band = Rect::from_min_max(pos2(x_of(listen + pa), rect.top()), pos2(x_of(listen + pb), rect.bottom()));
        let rx_band = if rx_band.width() < 3.0 { rx_band.expand2(vec2((3.0 - rx_band.width()) / 2.0, 0.0)) } else { rx_band };
        painter.rect_filled(rx_band, CornerRadius::ZERO, RX().gamma_multiply(0.2));
        let rx_x = x_of(listen);
        painter.line_segment([pos2(rx_x, rect.top()), pos2(rx_x, rect.bottom())], Stroke::new(1.5, RX()));
        painter.text(pos2(rx_x + 5.0, rect.top() + 3.0), Align2::LEFT_TOP, "RX", FontId::proportional(12.0), RX());
        // Transmitter dial.
        if let Some(p) = self.digital {
            // FT8/FT4: the tones' place.
            let a = self.tx_signal() + self.ft.audio_hz;
            let r = Rect::from_min_max(pos2(x_of(a), rect.top()), pos2(x_of(a + FtPanel::signal_width(p)).max(x_of(a) + 2.0), rect.bottom()));
            painter.rect_stroke(r, CornerRadius::ZERO, Stroke::new(1.5, TX()), egui::StrokeKind::Middle);
            painter.text(pos2(r.left() - 4.0, rect.top() + 3.0), Align2::RIGHT_TOP, "TX", FontId::proportional(12.0), TX());
        } else {
            let tx_x = x_of(self.tx_signal());
            ui::dashed_vline(&painter, tx_x, rect.top(), rect.bottom(), Stroke::new(1.5, TX()));
            painter.text(pos2(tx_x - 5.0, rect.top() + 3.0), Align2::RIGHT_TOP, "TX", FontId::proportional(12.0), TX());
        }

        // How to use it, faintly in the corner.
        let hint = self.t(
            "Click: listen · Right click: transmit here · Wheel: 10 Hz (Shift 100 Hz)",
            "Tık: dinle · Sağ tık: burada yayın · Tekerlek: 10 Hz (Shift 100 Hz)",
        );
        painter.text(pos2(rect.right() - 6.0, spec.top() + 4.0), Align2::RIGHT_TOP, hint, FontId::proportional(11.0), DIM());
        // Hover readout.
        if let Some(p) = response.hover_pos() {
            let hz = hz_of(p.x);
            painter.line_segment([pos2(p.x, spec.top()), pos2(p.x, spec.bottom())], Stroke::new(1.0, MUTED().gamma_multiply(0.5)));
            let text = format!("{:.3} MHz · {}", hz / 1e6, bandplan::segment(hz));
            painter.text(pos2(p.x + 8.0, spec.bottom() - 6.0), Align2::LEFT_BOTTOM, text, FontId::monospace(11.0), TEXT());
            let (steps, shift) = ui::wheel_steps(ui);
            if steps != 0 {
                self.tune_rx(self.rx_dial + steps as f64 * if shift { 100.0 } else { 10.0 });
            }
        }
        let place = |app: &Self, x: f32| {
            let hz = hz_of(x);
            if app.auto_lock {
                tune::snap(&app.line, app.line_band.0, app.line_band.1, hz, app.mode).unwrap_or(hz)
            } else {
                hz
            }
        };
        if response.clicked()
            && let Some(pos) = response.interact_pointer_pos()
        {
            if let Some(p) = self.digital {
                // FT8/FT4: a click sets where the own tones go (the dial stays).
                let max = 2_700.0 - FtPanel::signal_width(p);
                self.ft.audio_hz = (hz_of(pos.x) - self.tx_signal()).clamp(300.0, max).round();
            } else {
                let hz = place(self, pos.x);
                self.tune_rx(hz);
            }
        }
        if response.secondary_clicked()
            && let Some(p) = response.interact_pointer_pos()
        {
            self.tx_dial = (place(self, p.x) / 10.0).round() * 10.0;
            self.linked = false;
        }
    }

    // ---- Bottom cards -------------------------------------------------------------

    fn decoders(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            for (tab, name) in [(DecoderTab::Ft8, "FT8"), (DecoderTab::Ft4, "FT4"), (DecoderTab::Cw, "CW")] {
                if ui::choice(ui, name, self.tab == tab, BLUE()) {
                    self.tab = tab;
                }
            }
            if self.tab == DecoderTab::Ft4 {
                let label = self.t("decode", "çöz");
                ui.checkbox(&mut self.ft4_on, label);
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let text = match self.tab {
                    DecoderTab::Cw => format!("{} · {:.3}", self.t("dial", "dinlenen"), self.rx_dial / 1e6),
                    _ => format!("{:.3}", FT_DIAL_HZ / 1e6),
                };
                ui.label(RichText::new(text).size(11.0).color(DIM()));
            });
        });
        let Some(e) = &self.engine else { return };
        let lines: Vec<String> = match self.tab {
            DecoderTab::Ft8 | DecoderTab::Ft4 => {
                let worker = if self.tab == DecoderTab::Ft8 { &e.ft8 } else { &e.ft4 };
                let messages = worker.messages();
                let mut lines: Vec<String> = messages.iter().rev().take(60).map(ft_line).collect();
                lines.reverse();
                if lines.is_empty() {
                    let waiting = if self.tab == DecoderTab::Ft4 && !self.ft4_on {
                        self.t("FT4 decoding is off.", "FT4 çözme kapalı.")
                    } else {
                        self.t("Waiting for the end of the slot…", "Dilimin sonu bekleniyor…")
                    };
                    vec![waiting.to_string()]
                } else {
                    lines
                }
            }
            DecoderTab::Cw => match (self.mode, e.listener.cw()) {
                (Mode::Cw, Some(cw)) => {
                    // The last 600 characters (by character: Turkish letters are wider).
                    let count = cw.text.chars().count();
                    let text: String = cw.text.chars().skip(count.saturating_sub(600)).collect();
                    vec![format!("{:.0} WPM {}", cw.wpm, if cw.key_down { "■" } else { "" }), text]
                }
                _ => vec![self.t("Switch to CW and click a CW signal.", "CW'ye geçip bir CW sinyaline tıklayın.").to_string()],
            },
        };
        egui::ScrollArea::vertical().auto_shrink([false, false]).stick_to_bottom(true).show(ui, |ui| {
            for l in lines {
                ui.label(RichText::new(l).monospace().size(12.0).color(TEXT()));
            }
        });
    }

    /// The log window, with what a new contact is logged with.
    fn log_ui(&mut self, ui: &mut egui::Ui) {
        let mode = match self.digital {
            Some(p) => p.name,
            None if self.mode == Mode::Cw => "CW",
            None => "SSB",
        };
        let (my_call, my_grid) = (self.ft.seq.my_call.clone(), self.ft.seq.my_grid.clone());
        let st = Station { my_call: &my_call, my_grid: &my_grid, mode, tx_hz: self.uplink(), rx_hz: self.rx_listen() };
        let tr = self.lang == Lang::Tr;
        self.log.ui(ui, &st, tr);
        if std::mem::take(&mut self.log.changed) {
            self.ft.reload_worked();
        }
    }

    fn sound(&mut self, ui: &mut egui::Ui) {
        let status = self.engine.as_ref().map(|e| e.tx_status.lock().unwrap().clone()).unwrap_or_default();
        ui.horizontal(|ui| {
            ui::caption(ui, self.t("SOUND AND VOX", "SES VE VOX"), TX());
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let (text, on) = if self.vox.enabled { (self.t("VOX on", "VOX açık"), true) } else { (self.t("VOX off", "VOX kapalı"), false) };
                if ui::choice(ui, text, on, GOOD()) {
                    self.vox.enabled = !self.vox.enabled;
                }
            });
        });
        let mic = self.engine.as_ref().map(|e| e.microphone.clone());
        let mic_text = match &mic {
            Some(Ok(name)) => format!("{} · {name}", self.t("Microphone", "Mikrofon")),
            Some(Err(e)) => format!("{}: {e}", self.t("No microphone", "Mikrofon yok")),
            None => String::new(),
        };
        ui.label(RichText::new(mic_text).size(11.0).color(if matches!(mic, Some(Err(_))) { ALERT() } else { MUTED() }));
        let bar_color = if status.vox_keyed { ALERT() } else { GOOD() };
        ui::level_bar(ui, ui.available_width(), status.mic_db, (-70.0, 0.0), bar_color, Some(self.vox.threshold_db));
        egui::Grid::new("vox").num_columns(2).spacing(vec2(10.0, 3.0)).show(ui, |ui| {
            ui.label(RichText::new(self.t("Threshold", "Eşik")).size(12.0));
            ui.add(egui::Slider::new(&mut self.vox.threshold_db, -70.0..=0.0).suffix(" dBFS").step_by(1.0));
            ui.end_row();
            ui.label(RichText::new(self.t("Hang", "Bekleme")).size(12.0));
            ui.add(egui::Slider::new(&mut self.vox.hang_ms, 100..=2000).suffix(" ms").step_by(50.0));
            ui.end_row();
            ui.label(RichText::new(self.t("Pre-roll", "Ön tampon")).size(12.0))
                .on_hover_text(self.t("Delays the microphone so the first syllable is sent too", "İlk hece de gitsin diye mikrofon sesini geciktirir"));
            ui.add(egui::Slider::new(&mut self.vox.preroll_ms, 0..=300).suffix(" ms").step_by(10.0));
            ui.end_row();
            let mut anti = self.vox.anti_vox_db.is_some();
            ui.checkbox(&mut anti, RichText::new("Anti-VOX").size(12.0))
                .on_hover_text(self.t("Ignores the speaker's sound reaching the microphone", "Hoparlörden mikrofona gelen sesi yok sayar"));
            let mut margin = self.vox.anti_vox_db.unwrap_or(6.0);
            ui.add_enabled(anti, egui::Slider::new(&mut margin, 0.0..=20.0).suffix(" dB").step_by(1.0));
            self.vox.anti_vox_db = anti.then_some(margin);
            ui.end_row();
            ui.label(RichText::new(self.t("Mic gain", "Mikrofon kazancı")).size(12.0));
            ui.horizontal(|ui| {
                ui.add(egui::Slider::new(&mut self.mic_gain_db, -20.0..=20.0).suffix(" dB").step_by(1.0));
                let label = self.t("Compressor", "Kompresör");
                ui.checkbox(&mut self.compress, label);
            });
            ui.end_row();
            ui.label(RichText::new(self.t("Volume", "Ses")).size(12.0));
            let mut percent = self.volume * 100.0;
            ui.add(egui::Slider::new(&mut percent, 0.0..=100.0).suffix(" %").step_by(1.0));
            self.volume = percent / 100.0;
            ui.end_row();
        });
    }

    fn transmit(&mut self, ui: &mut egui::Ui) {
        let status = self.engine.as_ref().map(|e| e.tx_status.lock().unwrap().clone()).unwrap_or_default();
        let can = matches!(self.tx_capable, Some(Ok(())));
        ui.vertical_centered(|ui| {
            let (arm_text, arm_color) =
                if self.armed { (self.t("TX enabled", "TX etkin"), ALERT()) } else { (self.t("TX disabled", "TX kapalı"), MUTED()) };
            if ui::choice(ui, arm_text, self.armed, arm_color) && can {
                self.armed = !self.armed;
                if !self.armed {
                    self.ptt_latched = false;
                    self.tune = false;
                }
            }
            if let Some(Err(why)) = &self.tx_capable {
                let text = if !self.config.tx_enabled {
                    self.t("Receive only: TX is off in the settings", "Yalnızca alıcı: TX ayarlarda kapalı").to_string()
                } else {
                    format!("{}: {why}", self.t("Receive only", "Yalnızca alıcı"))
                };
                ui.label(RichText::new(text).size(11.0).color(TX()));
            }
            ui.add_space(2.0);
            ui.label(RichText::new(self.t("Output power", "Çıkış gücü")).size(11.0).color(MUTED()));
            let mut power = -self.attenuation;
            ui.spacing_mut().slider_width = 70.0;
            ui.add(egui::Slider::new(&mut power, -MAX_ATTENUATION..=0.0).suffix(" dB").step_by(0.5));
            self.attenuation = -power;
            if self.digital.is_some() || self.mode == Mode::Cw {
                // The level of the digital/CW audio: the power through the sound, as
                // WSJT-X's Pwr slider.
                ui.label(RichText::new("Drive").size(11.0).color(MUTED()));
                let mut d = (self.drive * 100.0).round();
                ui.add(egui::Slider::new(&mut d, 5.0..=100.0).suffix(" %").step_by(1.0).fixed_decimals(0));
                self.drive = d / 100.0;
            }
            ui.add_space(2.0);
            let on = status.state == TxState::Transmit;
            // Tune in every mode: a steady tone where the signal will be, to find it on
            // the waterfall and set the power.
            let tip = match self.digital {
                Some(_) => self.t("A steady tone on the FT transmit frequency", "FT gönderme frekansında sabit bir ton"),
                None if self.mode == Mode::Cw => self.t("A steady carrier on the dial", "Dinleme frekansında sabit bir taşıyıcı"),
                None => self.t("A steady tone 1 kHz above the dial", "Dinleme frekansının 1 kHz üstünde sabit bir ton"),
            };
            if ui::choice_response(ui, "Tune", self.tune, ALERT()).on_hover_text(tip).clicked() && can && self.armed {
                self.tune = !self.tune;
            }
            // In FT8/FT4 the sequence keys the transmitter, in CW the keyer: no PTT.
            if self.digital.is_none() && self.mode != Mode::Cw {
                if ui::ptt_button(ui, self.ptt_latched || on, can && self.armed) && can && self.armed {
                    self.ptt_latched = !self.ptt_latched;
                }
            } else {
                let text = if on { "TX" } else { "RX" };
                let color = if on { ALERT() } else { DIM() };
                ui.label(RichText::new(text).size(28.0).strong().color(color));
            }
            let state = match status.state {
                TxState::Transmit => match status.remaining {
                    Some(r) => format!(
                        "TX · {}:{:02} · ALC {:.0} dB\n{:.2} MS/s",
                        r.as_secs() / 60,
                        r.as_secs() % 60,
                        status.alc_db,
                        status.sent_rate / 1e6
                    ),
                    None => "TX".into(),
                },
                TxState::Receive => "RX".into(),
                TxState::Blocked(b) => self.blocked_text(b).to_string(),
            };
            let color = match status.state {
                TxState::Transmit => ALERT(),
                TxState::Receive => GOOD(),
                TxState::Blocked(_) => TX(),
            };
            ui.label(RichText::new(state).size(12.0).color(color));
            let rate = self.config.tx_rate();
            if status.state == TxState::Transmit && status.sent_rate > 0.0 && status.sent_rate < rate * 0.9 {
                let text = format!(
                    "{} {:.2}/{:.2} MS/s",
                    self.t("The link is too slow:", "Bağlantı yetmiyor:"),
                    status.sent_rate / 1e6,
                    rate / 1e6
                );
                ui.label(RichText::new(text).size(11.0).color(ALERT()));
            }
            let mode = if self.auto_uplink { self.t("auto", "oto") } else { self.t("manual", "elle") };
            let mut line = format!("{} {:+.0} Hz · {mode}", self.t("Uplink corr.", "Uplink düz."), self.tx_correction);
            if let Some(m) = self.last_measurement {
                line += &format!("\n{} {:+.0} Hz", self.t("last measured", "son ölçüm"), m.estimate.offset_hz);
            }
            ui.label(RichText::new(line).size(11.0).color(MUTED())).on_hover_text(self.t(
                "Your own signal is found on the downlink while you transmit (voice or tune tone) and the uplink is corrected by what it is off",
                "Yayın yaparken kendi sinyaliniz downlink'te bulunur (ses ya da ton) ve uplink kayma kadar düzeltilir",
            ));
            if let Some(e) = &status.error {
                ui.label(RichText::new(e).size(11.0).color(ALERT()));
            }
        });
    }

    /// CW sending: text, ready messages, speed, a straight key and paddles.
    fn cw_card(&mut self, ui: &mut egui::Ui) {
        let Some(e) = self.engine.as_ref() else { return };
        let keyer = e.keyer.clone();
        let call = self.ft.seq.my_call.clone();
        ui.horizontal(|ui| {
            ui::caption(ui, self.t("CW SENDING", "CW GÖNDERME"), TX());
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.add(egui::Slider::new(&mut self.cw_wpm, 8.0..=40.0).suffix(" WPM").step_by(1.0).fixed_decimals(0));
            });
        });
        if !self.armed {
            ui.label(RichText::new(self.t("Switch TX on (right) to send", "Göndermek için sağdaki TX'i açın")).size(11.0).color(TX()));
        }
        // Text: Enter sends it.
        let hint = self.t("Text to send, Enter", "Gönderilecek metin, Enter");
        let r = ui.add(egui::TextEdit::singleline(&mut self.cw_text).desired_width(ui.available_width()).hint_text(hint));
        let enter = r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
        let mut send: Option<String> = None;
        ui.horizontal_wrapped(|ui| {
            if ui::choice(ui, self.t("Send", "Gönder"), false, TX()) || enter {
                send = Some(self.cw_text.clone());
                self.cw_text.clear();
            }
            if ui::choice(ui, self.t("Stop", "Durdur"), false, TX()) {
                keyer.lock().unwrap().stop();
            }
            let me = if call.is_empty() { "DE".to_string() } else { call.clone() };
            for (label, text) in [
                ("CQ", format!("CQ CQ CQ DE {me} {me} K")),
                ("DE", format!("DE {me} K")),
                ("599", "UR 599 5NN BK".to_string()),
                ("73", format!("TU 73 DE {me} <SK>")),
            ] {
                if ui::choice(ui, label, false, BLUE()) {
                    send = Some(text);
                }
            }
        });
        if let Some(text) = send.filter(|t| !t.trim().is_empty())
            && self.armed
        {
            keyer.lock().unwrap().send(&text.to_uppercase());
        }
        let left = keyer.lock().unwrap().text_left();
        if left > 0 {
            let secs = left as f64 * 1.2 / self.cw_wpm;
            ui.label(RichText::new(format!("{} · {secs:.0} s", self.t("Sending", "Gönderiliyor"))).size(11.0).color(ALERT()));
        }
        ui.add_space(4.0);
        // The keys: held with the mouse (or [ ] and Space on the keyboard).
        ui.horizontal(|ui| {
            let key = |ui: &mut egui::Ui, label: &str| {
                let b = egui::Button::new(RichText::new(label).size(15.0).strong()).min_size(vec2(64.0, 40.0));
                ui.add(b).is_pointer_button_down_on()
            };
            let straight = key(ui, self.t("KEY", "ANAHTAR"));
            ui.add_space(8.0);
            let dit = key(ui, "· dit");
            let dah = key(ui, "— dah");
            let typing = ui.ctx().egui_wants_keyboard_input();
            let (k_straight, k_dit, k_dah) = ui.input(|i| (i.key_down(egui::Key::Space), i.key_down(egui::Key::OpenBracket), i.key_down(egui::Key::CloseBracket)));
            let armed = self.armed;
            let mut k = keyer.lock().unwrap();
            k.straight = armed && (straight || (k_straight && !typing));
            k.dit = armed && (dit || (k_dit && !typing));
            k.dah = armed && (dah || (k_dah && !typing));
        });
        ui.label(RichText::new(self.t("Keyboard: Space = key, [ = dit, ] = dah", "Klavye: Boşluk = anahtar, [ = dit, ] = dah")).size(11.0).color(DIM()));
    }

    /// FT8/FT4: band activity, the contact, the controls and the transmitter.
    fn ft_cards(&mut self, ui: &mut egui::Ui, p: &'static Protocol, h: f32) {
        let tr = self.lang == Lang::Tr;
        let armed = self.armed && matches!(self.tx_capable, Some(Ok(())));
        ui.horizontal_top(|ui| {
            let w = ui.available_width();
            // Band activity, the contact and the controls equally wide.
            let tx = 200.0f32;
            let third = ((w - tx - 3.0 * ui.spacing().item_spacing.x - 40.0) / 3.0).max(260.0);
            let (activity, qso, controls) = (third, third, third);
            let column = egui::Layout::top_down(egui::Align::Min);
            ui.allocate_ui_with_layout(vec2(activity, h), column, |ui| {
                ui::card(ui, DEEP(), |ui| {
                    ui.set_min_size(vec2(ui.available_width(), h - 18.0));
                    if let Some(e) = self.engine.as_ref() {
                        self.ft.activity(ui, e, p, tr);
                    }
                });
            });
            ui.allocate_ui_with_layout(vec2(qso, h), column, |ui| {
                ui::card(ui, DEEP(), |ui| {
                    ui.set_min_size(vec2(ui.available_width(), h - 18.0));
                    self.ft.qso_pane(ui, tr);
                });
            });
            ui.allocate_ui_with_layout(vec2(controls, h), column, |ui| {
                ui::card(ui, CARD(), |ui| {
                    ui.set_min_size(vec2(ui.available_width(), h - 18.0));
                    self.ft.controls(ui, self.engine.as_ref(), p, armed, tr);
                });
            });
            ui.allocate_ui_with_layout(vec2(tx, h), column, |ui| {
                ui::card(ui, CARD(), |ui| {
                    ui.set_min_size(vec2(ui.available_width(), h - 18.0));
                    self.transmit(ui);
                });
            });
        });
    }

    fn blocked_text(&self, b: Blocked) -> &'static str {
        match b {
            Blocked::NotArmed => self.t("TX is disabled", "TX kapalı"),
            Blocked::Band(Refusal::OnBeacon) => self.t("On a beacon: not allowed", "Beacon üzerinde: yasak"),
            Blocked::Band(Refusal::OutsideTransponder) => self.t("Outside the transponder", "Transponder dışında"),
            Blocked::TimedOut => self.t("Time-out: release PTT/VOX", "Süre doldu: PTT/VOX bırakın"),
            Blocked::NoRadio => self.t("No transmitter", "Verici yok"),
        }
    }

    // ---- Settings -----------------------------------------------------------------

    /// The radio being set up, from the settings' fields.
    fn edit_device(&self) -> DeviceChoice {
        match self.edit_kind {
            RadioKind::Simulator => DeviceChoice::Simulator,
            RadioKind::Iio => DeviceChoice::Iio(iio::normalize_uri(&self.edit_uri)),
            RadioKind::RtlUsb => DeviceChoice::RtlSdr(self.edit_rtl_index),
            RadioKind::RtlTcp => DeviceChoice::RtlTcp(self.edit_rtl_tcp.trim().to_string()),
            RadioKind::HackRf => {
                let sn = self.edit_hackrf.trim();
                DeviceChoice::HackRf((!sn.is_empty()).then(|| sn.to_string()))
            }
        }
    }

    /// The radio: its kind and address, transmitting or not, the sample rate.
    fn radio_settings(&mut self, ui: &mut egui::Ui) {
        ui::caption(ui, self.t("RADIO", "CİHAZ"), RX());
        let kinds = [
            (RadioKind::Simulator, self.t("Simulator (no radio)", "Simülatör (cihazsız)")),
            (RadioKind::Iio, self.t("ADALM-Pluto, LibreSDR (AD936x) · RX + TX", "ADALM-Pluto, LibreSDR (AD936x) · RX + TX")),
            (RadioKind::RtlUsb, self.t("RTL-SDR on USB · RX only", "RTL-SDR, USB · yalnızca RX")),
            (RadioKind::RtlTcp, self.t("RTL-SDR over the network (rtl_tcp) · RX only", "RTL-SDR, ağ üzerinden (rtl_tcp) · yalnızca RX")),
            (RadioKind::HackRf, self.t("HackRF One on USB · RX only", "HackRF One, USB · yalnızca RX")),
        ];
        for (kind, label) in kinds {
            ui.radio_value(&mut self.edit_kind, kind, label);
        }
        ui.add_space(2.0);
        let small = |ui: &mut egui::Ui, text: String, color: Color32| {
            ui.label(RichText::new(text).size(11.0).color(color));
        };
        match self.edit_kind {
            RadioKind::Simulator => {}
            RadioKind::Iio => {
                ui.horizontal(|ui| {
                    ui.label("URI");
                    ui.text_edit_singleline(&mut self.edit_uri);
                });
                if ui.button(self.t("Search USB", "USB'de ara")).clicked() {
                    self.usb_found = iio::scan_usb();
                    if self.usb_found.is_empty() {
                        self.usb_found.push((String::new(), self.t("Nothing found on USB", "USB'de cihaz bulunamadı").into()));
                    }
                }
                for (uri, description) in self.usb_found.clone() {
                    if !uri.is_empty() && ui.button(format!("{uri} · {description}")).clicked() {
                        self.edit_uri = uri;
                    } else if uri.is_empty() {
                        small(ui, description, MUTED());
                    }
                }
                small(ui, self.t("Network: ip:192.168.2.1 · USB: usb:1.2.5", "Ağ: ip:192.168.2.1 · USB: usb:1.2.5").into(), DIM());
                if let Err(e) = iio::library_status() {
                    small(ui, format!("libiio: {e}"), ALERT());
                }
            }
            RadioKind::RtlUsb => {
                ui.horizontal(|ui| {
                    ui.label(self.t("Device no.", "Cihaz no."));
                    ui.add(egui::DragValue::new(&mut self.edit_rtl_index).range(0..=15));
                    if ui.button(self.t("Search USB", "USB'de ara")).clicked() {
                        self.rtl_found = qo_all::rtlsdr::devices();
                    }
                });
                for (i, d) in self.rtl_found.clone() {
                    if ui.selectable_label(self.edit_rtl_index == i, format!("#{i} · {d}")).clicked() {
                        self.edit_rtl_index = i;
                    }
                }
                match qo_all::rtlsdr::library_status() {
                    Err(e) => small(ui, format!("librtlsdr: {e}"), ALERT()),
                    Ok(()) if cfg!(windows) => small(ui, self.t("Windows: the dongle needs the WinUSB driver (Zadig)", "Windows: cihaz için WinUSB sürücüsü gerekir (Zadig)").into(), DIM()),
                    Ok(()) => {}
                }
            }
            RadioKind::RtlTcp => {
                ui.horizontal(|ui| {
                    ui.label(self.t("Server", "Sunucu"));
                    ui.add(egui::TextEdit::singleline(&mut self.edit_rtl_tcp).hint_text("192.168.1.20:1234"));
                });
                small(ui, self.t("On the other computer: rtl_tcp -a <its address>", "Diğer bilgisayarda: rtl_tcp -a <adresi>").into(), DIM());
            }
            RadioKind::HackRf => {
                ui.horizontal(|ui| {
                    ui.label(self.t("Serial no.", "Seri no."));
                    let hint = self.t("the first one", "ilk bulunan");
                    ui.add(egui::TextEdit::singleline(&mut self.edit_hackrf).desired_width(150.0).hint_text(hint));
                    if ui.button(self.t("Search", "Ara")).clicked() {
                        self.hackrf_found = qo_all::hackrf::devices();
                    }
                });
                for sn in self.hackrf_found.clone() {
                    if ui.selectable_label(self.edit_hackrf == sn, &sn).clicked() {
                        self.edit_hackrf = sn;
                    }
                }
                if let Err(e) = qo_all::hackrf::library_status() {
                    small(ui, format!("libhackrf: {e}"), ALERT());
                }
            }
        }
        let device = self.edit_device();
        // Transmitting: only on radios that can, and only after the warning.
        if device.can_transmit() {
            let mut on = self.edit_tx_enabled;
            let label = self.t("TX enabled (transmit with this radio)", "TX etkin (bu cihazla yayın yap)");
            let color = if on { ALERT() } else { TEXT() };
            if ui.checkbox(&mut on, RichText::new(label).color(color)).changed() {
                if on {
                    self.tx_warning = true;
                } else {
                    self.edit_tx_enabled = false;
                }
            }
            if !self.edit_tx_enabled {
                small(ui, self.t("Receive only: the transmitter stays off", "Yalnızca alıcı: verici kapalı kalır").into(), DIM());
            }
        }
        ui.add_space(4.0);
        ui.label(if device.can_transmit() { self.t("Sample rate (receive and transmit)", "Örnekleme hızı (alma ve gönderme)") } else { self.t("Sample rate", "Örnekleme hızı") });
        if !device.rates().contains(&self.edit_rate) {
            self.edit_rate = fit_rate(&device, if matches!(device, DeviceChoice::Iio(ref u) if u.starts_with("ip:")) { LOW_RATE } else { 2.4e6 });
        }
        for &rate in device.rates() {
            let label = match (&device, rate) {
                (DeviceChoice::Simulator | DeviceChoice::Iio(_), r) if r < 1e6 => self.t("0.6 MS/s · Pluto on Ethernet (FIR filter in the radio)", "0.6 MS/s · Ethernet'li Pluto (cihazda FIR filtresi)").to_string(),
                (DeviceChoice::Simulator | DeviceChoice::Iio(_), _) => self.t("2.4 MS/s · USB, LibreSDR", "2.4 MS/s · USB, LibreSDR").to_string(),
                (DeviceChoice::RtlSdr(_) | DeviceChoice::RtlTcp(_), r) if r < 2e6 => self.t("1.8 MS/s · for a slower network", "1.8 MS/s · yavaş ağ için").to_string(),
                (_, r) => format!("{:.1} MS/s", r / 1e6),
            };
            ui.radio_value(&mut self.edit_rate, rate, label);
        }
    }

    /// Asked before transmitting is allowed: an open output can damage the radio.
    fn tx_warning_ui(&mut self, ctx: &egui::Context) {
        let mut answer = None;
        egui::Modal::new(egui::Id::new("tx_warning")).show(ctx, |ui| {
            ui.set_max_width(420.0);
            ui.label(RichText::new(self.t("Transmit with this radio?", "Bu cihazla yayın yapılsın mı?")).size(17.0).strong().color(ALERT()));
            ui.add_space(6.0);
            ui.label(self.t(
                "Make sure an antenna (or a dummy load) is connected to the TX output and its SWR is good, and that the amplifier after it is set up. Transmitting into an open or badly matched output can damage the radio and the amplifier.",
                "TX çıkışına bir anten (ya da yapay yük) bağlı olduğundan, SWR'sinin düzgün olduğundan ve arkasındaki amplifikatörün hazır olduğundan emin olun. Açık ya da uyumsuz bir çıkışa yayın yapmak cihaza ve amplifikatöre zarar verebilir.",
            ));
            ui.add_space(10.0);
            ui.horizontal(|ui| {
                if ui.add(egui::Button::new(RichText::new(self.t("Yes, the antenna is connected", "Evet, anten bağlı")).color(BG())).fill(ALERT())).clicked() {
                    answer = Some(true);
                }
                if ui.button(self.t("Cancel", "Vazgeç")).clicked() {
                    answer = Some(false);
                }
            });
        });
        if let Some(yes) = answer {
            self.edit_tx_enabled = yes;
            self.tx_warning = false;
        }
    }

    /// PTT out and the CW key in on serial ports.
    fn serial_settings(&mut self, ui: &mut egui::Ui) {
        ui::caption(ui, self.t("SERIAL PORTS (PTT, CW KEY)", "SERİ PORTLAR (PTT, CW ANAHTARI)"), TX());
        let none = self.t("none", "yok");
        let ports = self.ports.clone();
        let port_combo = |ui: &mut egui::Ui, salt: &str, value: &mut String| {
            let text = if value.is_empty() { none.to_string() } else { value.clone() };
            egui::ComboBox::from_id_salt(salt).width(110.0).selected_text(text).show_ui(ui, |ui| {
                ui.selectable_value(value, String::new(), none);
                for (name, driver) in &ports {
                    let label = if driver.is_empty() { name.clone() } else { format!("{name} · {driver}") };
                    ui.selectable_value(value, name.clone(), label);
                }
            });
        };
        egui::Grid::new("serial").num_columns(3).spacing(vec2(8.0, 4.0)).show(ui, |ui| {
            ui.label(self.t("PTT out", "PTT çıkışı"));
            port_combo(ui, "ptt_port", &mut self.edit_serial.ptt_port);
            let off = self.t("off", "kapalı");
            let line_name = |l: PttLine| if l == PttLine::Off { off } else { l.name() };
            egui::ComboBox::from_id_salt("ptt_line").width(70.0).selected_text(line_name(self.edit_serial.ptt_line)).show_ui(ui, |ui| {
                for l in PttLine::ALL {
                    ui.selectable_value(&mut self.edit_serial.ptt_line, l, line_name(l));
                }
            });
            ui.end_row();
            ui.label(self.t("CW key in", "CW anahtar girişi"));
            port_combo(ui, "key_port", &mut self.edit_serial.key_port);
            ui.label("");
            ui.end_row();
        });
        let wiring = |k: KeyInput, tr: bool| match (k, tr) {
            (KeyInput::Off, false) => "off",
            (KeyInput::Off, true) => "kapalı",
            (KeyInput::StraightCts, false) => "Straight key on CTS",
            (KeyInput::StraightCts, true) => "Düz anahtar CTS'de",
            (KeyInput::StraightDsr, false) => "Straight key on DSR",
            (KeyInput::StraightDsr, true) => "Düz anahtar DSR'de",
            (KeyInput::PaddleDsrCts, false) => "Paddle: dit DSR, dah CTS",
            (KeyInput::PaddleDsrCts, true) => "Paddle: dit DSR, dah CTS",
            (KeyInput::PaddleCtsDsr, false) => "Paddle: dit CTS, dah DSR",
            (KeyInput::PaddleCtsDsr, true) => "Paddle: dit CTS, dah DSR",
        };
        let tr = self.lang == Lang::Tr;
        egui::ComboBox::from_id_salt("key_input").width(220.0).selected_text(wiring(self.edit_serial.key_input, tr)).show_ui(ui, |ui| {
            for k in KeyInput::ALL {
                ui.selectable_value(&mut self.edit_serial.key_input, k, wiring(k, tr));
            }
        });
        ui.label(
            RichText::new(self.t(
                "PTT goes high while transmitting, in every mode. The key gets its supply from DTR and RTS (held high): wire it between DTR and CTS/DSR. It works in CW with TX enabled.",
                "PTT her modda yayın boyunca yüksek olur. Anahtar beslemesini DTR ve RTS'den alır (yüksek tutulur): DTR ile CTS/DSR arasına bağlayın. CW'de, TX etkinken çalışır.",
            ))
            .size(11.0)
            .color(DIM()),
        );
        if ui.small_button(self.t("Read the ports again", "Portları yeniden oku")).clicked() {
            self.ports = qo_all::serial::ports();
        }
        // What the lines do now.
        if self.serial.active() {
            let st = self.serial.state();
            let mut text = format!("PTT {}", if st.ptt { "●" } else { "○" });
            if self.serial.config.key_input != KeyInput::Off {
                let (s, dit, dah) = st.key;
                text += &format!(" · {} {}{}{}", self.t("key", "anahtar"), if s { "■" } else { "□" }, if dit { " dit" } else { "" }, if dah { " dah" } else { "" });
            }
            ui.label(RichText::new(text).size(11.0).color(GOOD()));
        }
        for e in &self.serial.errors {
            ui.label(RichText::new(e).size(11.0).color(ALERT()));
        }
    }

    fn settings_panel(&mut self, ui: &mut egui::Ui) {
        egui::ScrollArea::vertical().show(ui, |ui| {
            ui.heading(self.t("Settings", "Ayarlar"));
            ui.add_space(6.0);
            // Theme: applied at once.
            ui.horizontal(|ui| {
                ui.label(self.t("Theme", "Tema"));
                let light = ui::palette::light();
                let (night, paper) = (self.t("Night", "Gece"), self.t("Paper", "Kâğıt"));
                if ui.radio(!light, night).clicked() && light {
                    self.set_theme(ui.ctx(), false);
                }
                if ui.radio(light, paper).clicked() && !light {
                    self.set_theme(ui.ctx(), true);
                }
            });
            ui.add_space(6.0);
            self.radio_settings(ui);
            ui.add_space(8.0);
            ui::caption(ui, self.t("RECEIVER", "ALICI"), RX());
            ui.horizontal(|ui| {
                ui.label(self.t("LNB LO", "LNB LO"));
                ui.add(egui::DragValue::new(&mut self.edit_lnb_mhz).speed(0.001).suffix(" MHz").max_decimals(3));
            });
            ui.horizontal(|ui| {
                ui.label(self.t("Radio frequency error", "Cihaz frekans hatası"));
                ui.add(egui::DragValue::new(&mut self.edit_ppm).speed(0.1).range(-200.0..=200.0).suffix(" ppm").max_decimals(1));
            })
            .response
            .on_hover_text(self.t(
                "For radios without a TCXO (many RTL-SDRs, HackRF): the beacon lock looks only ±15 kHz around where the beacon should be. Positive: the radio tunes high.",
                "TCXO'suz cihazlar için (çoğu RTL-SDR, HackRF): beacon kilidi beacon'ın olması gereken yerin yalnızca ±15 kHz çevresine bakar. Pozitif: cihaz yüksek ayarlıyor.",
            ));
            ui.horizontal(|ui| {
                ui.label(self.t("dBm calibration", "dBm kalibrasyonu"));
                if ui.add(egui::DragValue::new(&mut self.dbm_cal).speed(0.5).range(-60.0..=60.0).suffix(" dB").max_decimals(1)).changed() {
                    self.save();
                }
            })
            .response
            .on_hover_text(self.t(
                "dBm readings are estimated from the radio's gain. Feed a known level (signal generator) and set this so the meter shows it.",
                "dBm değerleri cihaz kazancından tahmin edilir. Bilinen bir seviye (sinyal üreteci) verip gösterge onu gösterecek şekilde ayarlayın.",
            ));
            // Calibrating in one step: the level fed in, the meter's reading now.
            let reading = self.meter_dbm;
            ui.horizontal(|ui| {
                ui.label(self.t("Level fed in", "Verilen seviye"));
                ui.add(egui::DragValue::new(&mut self.cal_known_dbm).speed(0.5).range(-150.0..=0.0).suffix(" dBm").max_decimals(1));
                let ok = reading > -150.0;
                if ui.add_enabled(ok, egui::Button::new(self.t("Calibrate", "Kalibre et"))).clicked() {
                    self.dbm_cal = ((self.dbm_cal + self.cal_known_dbm - reading) * 10.0).round() / 10.0;
                    self.save();
                }
            })
            .response
            .on_hover_text(self.t(
                "With the generator's tone in the receiver's passband (manual gain): the calibration is set so the meter shows this level.",
                "Üretecin tonu alıcının geçiş bandındayken (elle kazanç): kalibrasyon, gösterge bu seviyeyi gösterecek şekilde ayarlanır.",
            ));
            ui.label(RichText::new(format!("{}: {reading:.1} dBm", self.t("Meter now", "Gösterge şu an"))).size(11.0).color(DIM()));
            ui.horizontal(|ui| {
                let label = self.t("Automatic gain", "Otomatik kazanç");
                ui.checkbox(&mut self.edit_auto_gain, label);
                ui.add_enabled(!self.edit_auto_gain, egui::Slider::new(&mut self.edit_gain, 0.0..=70.0).suffix(" dB").step_by(1.0));
            });
            ui.add_space(8.0);
            ui::caption(ui, self.t("TRANSMITTER", "VERİCİ"), TX());
            let auto = self.t("Correct the uplink automatically", "Uplink'i otomatik düzelt");
            ui.checkbox(&mut self.auto_uplink, auto).on_hover_text(self.t(
                "Finds your own signal on the downlink while you transmit and moves the uplink so that it comes down on your dial (plus XIT)",
                "Yayın yaparken kendi sinyalinizi downlink'te bulur ve uplink'i, sinyal dinleme frekansınıza (artı XIT) insin diye kaydırır",
            ));
            ui.horizontal(|ui| {
                ui.label(self.t("Uplink correction", "Uplink düzeltmesi"));
                ui.add(egui::DragValue::new(&mut self.tx_correction).speed(10.0).suffix(" Hz"));
            })
            .response
            .on_hover_text(self.t(
                "Added to the uplink frequency so that your signal comes down where you listen",
                "Sinyaliniz dinlediğiniz yere insin diye uplink frekansına eklenir",
            ));
            ui.horizontal(|ui| {
                ui.label(self.t("Time-out", "Zaman aşımı"));
                ui.add(egui::Slider::new(&mut self.time_out_s, 30..=600).suffix(" s").step_by(10.0));
            });
            ui.add_space(8.0);
            ui::caption(ui, self.t("STATION AND LOG (FT8/FT4)", "İSTASYON VE LOG (FT8/FT4)"), BLUE());
            egui::Grid::new("station").num_columns(2).spacing(vec2(8.0, 4.0)).show(ui, |ui| {
                ui.label(self.t("Call sign", "Çağrı işareti"));
                ui.add(egui::TextEdit::singleline(&mut self.edit_call).desired_width(120.0).char_limit(11));
                ui.end_row();
                ui.label(self.t("Locator", "Lokatör"));
                ui.add(egui::TextEdit::singleline(&mut self.edit_grid).desired_width(120.0).char_limit(6));
                ui.end_row();
            });
            ui.label(self.t("ADIF log file", "ADIF log dosyası"));
            ui.add(egui::TextEdit::singleline(&mut self.edit_log).desired_width(ui.available_width() - 10.0));
            ui.horizontal(|ui| {
                if ui.button(self.t("QO-ALL's own", "QO-ALL'ın kendi")).clicked() {
                    self.edit_log = ftpanel::default_log().display().to_string();
                }
                if let Some(w) = ftpanel::wsjtx_log()
                    && ui.button(self.t("WSJT-X's log", "WSJT-X logu")).clicked()
                {
                    self.edit_log = w.display().to_string();
                }
            });
            ui.label(RichText::new(format!("{} {}", self.ft.worked.len(), self.t("stations in the log", "istasyon logda"))).size(11.0).color(DIM()));
            ui.add_space(8.0);
            ui::caption(ui, self.t("ALERTS (FT8/FT4)", "UYARILAR (FT8/FT4)"), ftpanel::alert_color());
            egui::Grid::new("alerts").num_columns(3).spacing(vec2(10.0, 3.0)).show(ui, |ui| {
                ui.label("");
                ui.label(RichText::new(self.t("show", "göster")).size(11.0).color(MUTED()));
                ui.label(RichText::new(self.t("sound", "ses")).size(11.0).color(MUTED()));
                ui.end_row();
                ui.label(self.t("New DXCC", "Yeni DXCC"));
                ui.checkbox(&mut self.ft.alerts.dxcc_show, "");
                ui.checkbox(&mut self.ft.alerts.dxcc_sound, "");
                ui.end_row();
                ui.label(self.t("New locator (square)", "Yeni lokatör (kare)"));
                ui.checkbox(&mut self.ft.alerts.grid_show, "");
                ui.checkbox(&mut self.ft.alerts.grid_sound, "");
                ui.end_row();
            });
            ui.label(self.t("Country file (cty.dat)", "Ülke dosyası (cty.dat)"));
            let hint = self.t("found automatically", "kendiliğinden bulunur");
            ui.add(egui::TextEdit::singleline(&mut self.edit_cty).desired_width(ui.available_width() - 10.0).hint_text(hint));
            let (worked, known) = self.ft.entity_counts();
            let cty_state = match &self.ft.cty_path {
                Some(p) if known > 0 => format!("{known} {} · {worked} {} · {}", self.t("entities", "ülke"), self.t("worked", "çalışılmış"), p.display()),
                _ => self.t("Not found: install WSJT-X or give the file", "Bulunamadı: WSJT-X kurun ya da dosyayı girin").to_string(),
            };
            ui.label(RichText::new(cty_state).size(11.0).color(DIM()));
            ui.add_space(8.0);
            self.serial_settings(ui);
            ui.add_space(12.0);
            // Only a change of the radio's set-up restarts it; the rest applies at once.
            let mut config = StationConfig {
                device: self.edit_device(),
                lnb_lo_hz: self.edit_lnb_mhz * 1e6,
                tx_enabled: self.edit_tx_enabled,
                ppm: self.edit_ppm,
                ..self.config.clone()
            }
            .with_rate(self.edit_rate);
            config.rx_gain = (!self.edit_auto_gain).then_some(self.edit_gain);
            let restart = StationConfig { rx_gain: self.config.rx_gain, ..config.clone() } != self.config;
            let label = if restart { self.t("Apply (restarts the radio)", "Uygula (cihaz yeniden başlar)") } else { self.t("Apply", "Uygula") };
            if ui.add(egui::Button::new(RichText::new(label).color(BG())).fill(RX())).clicked() {
                if let DeviceChoice::Iio(u) = &config.device {
                    self.edit_uri = u.clone();
                }
                if !restart
                    && config.rx_gain != self.config.rx_gain
                    && let Some(e) = &self.engine
                {
                    e.set_gain(config.rx_gain);
                }
                self.config = config;
                self.ft.set_station(&self.edit_call, &self.edit_grid, std::path::PathBuf::from(self.edit_log.trim()));
                self.log.set_path(self.ft.log_path.clone());
                let cty = self.edit_cty.trim();
                self.ft.set_cty((!cty.is_empty()).then(|| std::path::PathBuf::from(cty)));
                if self.edit_serial != self.serial.config {
                    // The old ports are let go before the new ones open.
                    self.serial = SerialIo::start(SerialConfig::default());
                    self.serial = SerialIo::start(self.edit_serial.clone());
                    self.serial.attach(self.engine.as_ref().map(|e| e.serial_station()));
                }
                self.save();
                if restart {
                    self.start(ui.ctx());
                }
                self.show_settings = false;
            }
            ui.add_space(8.0);
            ui.label(RichText::new(format!("QO-ALL {} · GPL-3.0-or-later", env!("CARGO_PKG_VERSION"))).size(11.0).color(DIM()));
        });
    }
}

/// RIT or XIT: a switch and its offset. Dragging or the mouse wheel changes the offset
/// by 10 Hz steps (Shift: 100 Hz); a double click sets it back to zero.
fn offset_control(ui: &mut egui::Ui, name: &str, on: &mut bool, hz: &mut f64, color: Color32) {
    if ui::choice(ui, name, *on, color) {
        *on = !*on;
    }
    let response = ui.add_enabled(
        *on,
        egui::DragValue::new(hz).speed(10.0).range(-5_000.0..=5_000.0).suffix(" Hz").custom_formatter(|v, _| format!("{v:+.0}")),
    );
    if *on && response.hovered() {
        let (steps, shift) = ui::wheel_steps(ui);
        *hz = (*hz + steps as f64 * if shift { 100.0 } else { 10.0 }).clamp(-5_000.0, 5_000.0);
    }
    if response.double_clicked() {
        *hz = 0.0;
    }
    *hz = (*hz / 10.0).round() * 10.0;
}

/// A sound device chooser: the device picked (Some(None) = the system's default), and
/// whether the list was just opened (to read the devices again).
fn device_combo(ui: &mut egui::Ui, salt: &str, list: &[qo_all::audio::OutputDevice], current: &Option<String>, default: &str) -> (Option<Option<String>>, bool) {
    let name = current.as_ref().and_then(|id| list.iter().find(|d| &d.id == id)).map_or(default.to_string(), |d| d.name.clone());
    let short: String = if name.chars().count() > 26 { name.chars().take(25).chain(['…']).collect() } else { name.clone() };
    let mut pick = None;
    let r = egui::ComboBox::from_id_salt(salt).width(190.0).selected_text(RichText::new(short).size(12.0)).show_ui(ui, |ui| {
        if ui.selectable_label(current.is_none(), default).clicked() {
            pick = Some(None);
        }
        for d in list {
            if ui.selectable_label(current.as_ref() == Some(&d.id), &d.name).clicked() {
                pick = Some(Some(d.id.clone()));
            }
        }
    });
    let r = r.response.on_hover_text(name);
    (pick.filter(|p| p != current), r.clicked())
}

/// Whether an AD936x radio opened at the low (Ethernet FIR) rate should rather run at
/// 2.4 MS/s: only an ADALM-Pluto needs the low rate (its Ethernet is 100 Mbit); other
/// boards on the network, such as the LibreSDR (gigabit, Tezuka firmware), lose the
/// connection with the low rate's FIR filters.
fn needs_full_rate(config: &StationConfig, model: &str) -> bool {
    matches!(config.device, DeviceChoice::Iio(_))
        && config.sample_rate < qo_all::iio::FIR_RATE_LIMIT
        && !model.trim().is_empty()
        && !model.to_lowercase().contains("pluto")
}

fn clock(secs: u64) -> String {
    let s = secs % 86_400;
    format!("{:02}:{:02}:{:02}", s / 3600, s / 60 % 60, s % 60)
}

fn ft_line(m: &Ft8Message) -> String {
    let s = m.slot % 86_400;
    format!("{:02}{:02}{:02} {:+3.0} {:+4.1} {:4.0}  {}", s / 3600, s / 60 % 60, s % 60, m.snr_db, m.dt_s, m.audio_hz, m.text)
}

impl eframe::App for App {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        self.poll(&ui.ctx().clone());
        // Space held = push to talk; Escape drops a latched PTT.
        let typing = ui.ctx().egui_wants_keyboard_input();
        let (space, escape) = ui.input(|i| (i.key_down(egui::Key::Space), i.key_pressed(egui::Key::Escape)));
        if escape {
            self.ptt_latched = false;
            self.tune = false;
        }
        let cw = self.mode == Mode::Cw && self.digital.is_none();
        let ptt = self.armed && !cw && (self.ptt_latched || (space && !typing));
        self.set_ptt(ptt);
        self.sync();
        if let (Some(p), Some(e)) = (self.digital, self.engine.as_ref()) {
            let (dial, uplink) = (self.tx_signal(), self.uplink());
            let armed = self.armed && matches!(self.tx_capable, Some(Ok(())));
            self.ft.update(e, p, armed, dial, uplink, self.lang == Lang::Tr);
        }
        ui.ctx().request_repaint_after(Duration::from_millis(200));

        let bar = egui::Frame::new().fill(CARD()).inner_margin(egui::Margin::symmetric(14, 6));
        egui::Panel::top("header").frame(bar).show(ui, |ui| self.header(ui));
        egui::Panel::top("vfos").frame(egui::Frame::new().fill(BG()).inner_margin(egui::Margin::symmetric(12, 8))).show(ui, |ui| self.vfos(ui));
        if self.show_settings {
            egui::Panel::right("settings")
                .resizable(false)
                .default_size(330.0)
                .frame(egui::Frame::new().fill(CARD()).inner_margin(egui::Margin::same(14)))
                .show(ui, |ui| self.settings_panel(ui));
        }
        // The log, always in view under the cards.
        egui::Panel::bottom("log")
            .resizable(true)
            .default_size(170.0)
            .size_range(110.0..=480.0)
            .frame(egui::Frame::new().fill(BG()).inner_margin(egui::Margin { left: 12, right: 12, top: 0, bottom: 8 }))
            .show(ui, |ui| {
                ui::card(ui, DEEP(), |ui| {
                    ui.set_min_size(vec2(ui.available_width(), ui.available_height()));
                    self.log_ui(ui);
                });
            });
        egui::Panel::bottom("cards")
            .resizable(true)
            .default_size(230.0)
            .size_range(170.0..=420.0)
            .frame(egui::Frame::new().fill(BG()).inner_margin(egui::Margin::symmetric(12, 8)))
            .show(ui, |ui| {
                let h = ui.available_height();
                if let Some(p) = self.digital {
                    self.ft_cards(ui, p, h);
                    return;
                }
                ui.horizontal_top(|ui| {
                    let w = ui.available_width();
                    let side = 380.0f32.min(w * 0.36);
                    let ptt = 200.0;
                    let decoders = (w - side - ptt - 2.0 * ui.spacing().item_spacing.x - 30.0).max(200.0);
                    // Every card stacks its contents top-down, whatever the row's layout.
                    let column = egui::Layout::top_down(egui::Align::Min);
                    ui.allocate_ui_with_layout(vec2(decoders, h), column, |ui| {
                        ui::card(ui, DEEP(), |ui| {
                            ui.set_min_size(vec2(ui.available_width(), h - 18.0));
                            self.decoders(ui);
                        });
                    });
                    ui.allocate_ui_with_layout(vec2(side, h), column, |ui| {
                        ui::card(ui, CARD(), |ui| {
                            ui.set_min_size(vec2(ui.available_width(), h - 18.0));
                            if self.mode == Mode::Cw {
                                self.cw_card(ui);
                            } else {
                                self.sound(ui);
                            }
                        });
                    });
                    ui.allocate_ui_with_layout(vec2(ptt, h), column, |ui| {
                        ui::card(ui, CARD(), |ui| {
                            ui.set_min_size(vec2(ui.available_width(), h - 18.0));
                            self.transmit(ui);
                        });
                    });
                });
            });
        if self.tx_warning {
            self.tx_warning_ui(ui.ctx());
        }
        egui::CentralPanel::default().frame(egui::Frame::new().fill(BG()).inner_margin(egui::Margin::symmetric(12, 0))).show(ui, |ui| self.spectrum(ui));
    }

    fn on_exit(&mut self) {
        self.set_ptt(false);
        self.save();
        self.engine = None;
    }
}
