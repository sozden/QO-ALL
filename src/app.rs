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
    Log,
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
    edit_device_iio: bool,
    edit_uri: String,
    edit_lnb_mhz: f64,
    edit_gain: f64,
    edit_auto_gain: bool,
    edit_rate: f64,
    edit_mic: Option<String>,
    edit_speaker: Option<String>,
    usb_found: Vec<(String, String)>,
    mics: Vec<qo_all::audio::OutputDevice>,
    /// FT8 or FT4 (the receiver then works in USB on the FT dial), and the SSB dial to
    /// go back to.
    digital: Option<&'static Protocol>,
    ssb_dial: f64,
    ft: FtPanel,
    /// The log window, and whether it is shown in place of the FT contact's messages.
    log: LogPanel,
    ft_log_tab: bool,
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
        let device = match s.text("device") {
            Some(uri) if uri != "simulator" => DeviceChoice::Iio(iio::normalize_uri(&uri)),
            _ => DeviceChoice::Simulator,
        };
        let defaults = StationConfig::default();
        // Ethernet links carry too little for 2.4 MS/s both ways: the low rate there.
        let rate = s.get("sample_rate").unwrap_or(if matches!(device, DeviceChoice::Iio(ref u) if u.starts_with("ip:")) { LOW_RATE } else { 2.4e6 });
        // Any earlier low rate (1.2 MS/s was too much for an Ethernet link) becomes the
        // one low rate offered now.
        let rate = if rate < qo_all::iio::FIR_RATE_LIMIT { LOW_RATE } else { rate };
        let config = StationConfig {
            device,
            lnb_lo_hz: s.get("lnb_lo_hz").unwrap_or(defaults.lnb_lo_hz),
            rx_gain: if s.get("rx_auto_gain").unwrap_or(false) { None } else { Some(s.get("rx_gain").unwrap_or(40.0)) },
            microphone: s.text("microphone"),
            speaker: s.text("speaker"),
            ..defaults
        }
        .with_rate(rate);
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
                Some("LOG") => DecoderTab::Log,
                _ => DecoderTab::Ft8,
            },
            ft4_on: s.get("ft4").unwrap_or(false),
            show_settings: false,
            edit_device_iio: matches!(config.device, DeviceChoice::Iio(_)),
            edit_uri: match &config.device {
                DeviceChoice::Iio(u) => u.clone(),
                DeviceChoice::Simulator => s.text("last_uri").unwrap_or_else(|| "ip:192.168.2.1".into()),
            },
            edit_lnb_mhz: config.lnb_lo_hz / 1e6,
            edit_gain: config.rx_gain.unwrap_or(40.0),
            edit_auto_gain: config.rx_gain.is_none(),
            edit_rate: config.sample_rate,
            edit_mic: config.microphone.clone(),
            edit_speaker: config.speaker.clone(),
            usb_found: Vec::new(),
            mics: Vec::new(),
            // The last mode, FT8/FT4 included.
            digital: match s.text("digital").as_deref() {
                Some("FT8") => Some(&FT8),
                Some("FT4") => Some(&FT4),
                _ => None,
            },
            ssb_dial: s.get("ssb_dial_hz").unwrap_or(rx_dial),
            log: LogPanel::new(s.text("adif_path").map(std::path::PathBuf::from).unwrap_or_else(ftpanel::default_log)),
            ft_log_tab: s.get("ft_log_tab").unwrap_or(false),
            ft: FtPanel::new(&s),
            edit_call: s.text("my_call").unwrap_or_default(),
            edit_grid: s.text("my_grid").unwrap_or_default(),
            edit_log: s.text("adif_path").unwrap_or_else(|| ftpanel::default_log().display().to_string()),
            speakers: Vec::new(),
            config,
            engine: None,
            model: None,
            tx_capable: None,
            warning: None,
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

    fn poll(&mut self) {
        let Some(e) = &self.engine else { return };
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
                    self.model = Some(model);
                    self.tx_capable = Some(tx);
                    self.warning = warning;
                }
                Event::Frame(f) => self.push_frame(f),
                Event::Rate(r) => self.rate = r,
                Event::Error(e) => self.error = Some(e),
            }
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
        s.set(
            "device",
            match &self.config.device {
                DeviceChoice::Simulator => "simulator".to_string(),
                DeviceChoice::Iio(u) => u.clone(),
            },
        );
        s.set("last_uri", &self.edit_uri);
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
            DecoderTab::Log => "LOG",
        });
        s.set("ft_log_tab", self.ft_log_tab);
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
            match (&self.error, &self.model) {
                (Some(e), _) => {
                    ui::pill(ui, e.clone(), TX(), TX_BG());
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
                Some(BeaconStatus::NotFound) => (self.t("Beacon not found", "Beacon bulunamadı").to_string(), TX(), RAISED()),
                Some(BeaconStatus::OutsideBand) => (self.t("Beacon outside the band", "Beacon bant dışında").to_string(), TX(), RAISED()),
                _ => (self.t("Looking for the beacon…", "Beacon aranıyor…").to_string(), MUTED(), RAISED()),
            };
            ui::pill(ui, text, fg, bg);
            if let Some(w) = &self.warning {
                ui::pill(ui, w.clone(), RX(), RAISED());
            }
            if self.rate > 0.0 && self.rate < self.config.sample_rate * 0.95 {
                ui::pill(ui, format!("{:.2} MS/s", self.rate / 1e6), TX(), RAISED())
                    .on_hover_text(self.t("The radio delivers fewer samples than set", "Cihaz ayarlanandan az örnek gönderiyor"));
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui.add(egui::Button::new(RichText::new(self.t("Settings", "Ayarlar")).color(TEXT()))).clicked() {
                    self.show_settings = !self.show_settings;
                    if self.show_settings {
                        self.mics = qo_all::audio::input_devices();
                        self.speakers = qo_all::audio::output_devices();
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
                if ui::choice(ui, self.t("Mute", "Sessiz"), self.muted, TX()) {
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
        // Top-aligned, so both displays line up whatever each holds.
        ui.horizontal_top(|ui| {
            // Receiver.
            let column = egui::Layout::top_down(egui::Align::Min);
            let rx_change = ui
                .allocate_ui_with_layout(vec2(width, 80.0), column, |ui| {
                    ui::card(ui, DEEP(), |ui| {
                        ui.set_width(ui.available_width());
                        ui::caption(ui, format!("DOWNLINK · RX · {} · {:.0} dBFS", self.mode.name(), level), RX());
                        let change = ui::vfo(ui, self.rx_dial, 34.0, RX());
                        // Under it, as under the transmit side: the band plan's part, and
                        // the RIT when on.
                        let rit = if self.rit_on { format!(" · RIT {:+.0} Hz", self.rit) } else { String::new() };
                        let line = format!("{}{rit}", bandplan::segment(self.rx_listen()).to_uppercase());
                        ui.label(RichText::new(line).monospace().size(13.0).color(RX().gamma_multiply(0.75)));
                        change
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
            let fill = if transmitting { TX_BG() } else { DEEP() };
            // The big number is where the transmission comes down (10 GHz: in split the
            // place among the others), the uplink actually sent (2.4 GHz, corrections
            // included) under it.
            let uplink = self.uplink();
            let down = self.tx_signal();
            let tx_change = ui
                .allocate_ui_with_layout(vec2(width, 80.0), column, |ui| {
                    ui::card(ui, fill, |ui| {
                        ui.set_width(ui.available_width());
                        let shift = if self.linked { String::new() } else { format!(" · shift {:+.2} kHz", shift / 1e3) };
                        ui::caption(ui, format!("TX · USB{shift}"), TX());
                        let change = ui::vfo(ui, down, 34.0, TX());
                        ui.label(RichText::new(format!("UPLINK {:.5} MHz", uplink / 1e6)).monospace().size(13.0).color(TX().gamma_multiply(0.75)));
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
            for m in MODES {
                if ui::choice(ui, m.name(), self.mode == m && self.digital.is_none(), RX()) {
                    self.set_digital(None);
                    self.mode = m;
                }
            }
            for p in [&FT8, &FT4] {
                let on = self.digital.is_some_and(|d| std::ptr::eq(d, p));
                if ui::choice(ui, p.name, on, RX()) && !on {
                    self.set_digital(Some(p));
                }
            }
            ui.add_space(12.0);
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
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.label(
                    RichText::new(self.t(
                        "Click: listen · Right click: transmit here · Wheel: 10 Hz (Shift 100 Hz)",
                        "Tık: dinle · Sağ tık: burada yayın · Tekerlek: 10 Hz (Shift 100 Hz)",
                    ))
                    .size(11.0)
                    .color(DIM()),
                );
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
        let axis_h = 18.0;
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
                painter.rect_filled(r, CornerRadius::ZERO, TX().gamma_multiply(0.08));
            }
        }
        // Frequency grid and axis.
        let step = ui::nice_step(width_hz / (rect.width() as f64 / 110.0));
        let axis = Rect::from_min_max(pos2(rect.left(), spec.bottom()), pos2(rect.right(), wf.top()));
        painter.rect_filled(axis, CornerRadius::ZERO, BG());
        let mut f = (lo_hz / step).ceil() * step;
        while f <= hi_hz {
            let x = x_of(f);
            painter.line_segment([pos2(x, spec.top()), pos2(x, spec.bottom())], Stroke::new(1.0, GRID()));
            let label = if step >= 1e3 { format!("{:.3}", f / 1e6) } else { format!("{:.4}", f / 1e6) };
            painter.text(pos2(x, axis.center().y), Align2::CENTER_CENTER, label, FontId::monospace(11.0), DIM());
            f += step;
        }
        // Spectrum line, filled below.
        let (db_lo, db_hi) = (self.floor - 8.0, self.floor + 55.0);
        let y_of = |db: f32| spec.bottom() - ((db - db_lo) / (db_hi - db_lo)).clamp(0.0, 1.0) * (spec.height() - 14.0);
        if self.line.len() > 1 {
            let n = self.line.len();
            let points: Vec<egui::Pos2> =
                self.line.iter().enumerate().map(|(i, &v)| pos2(rect.left() + (i as f32 + 0.5) / n as f32 * rect.width(), y_of(v))).collect();
            let mut mesh = egui::Mesh::default();
            let fill = BLUE().gamma_multiply(0.14);
            for p in &points {
                mesh.colored_vertex(*p, fill);
                mesh.colored_vertex(pos2(p.x, spec.bottom()), fill);
            }
            for i in 0..(n as u32 - 1) {
                let k = i * 2;
                mesh.add_triangle(k, k + 1, k + 2);
                mesh.add_triangle(k + 1, k + 3, k + 2);
            }
            painter.add(egui::Shape::mesh(mesh));
            painter.add(egui::Shape::line(points, Stroke::new(1.2, BLUE())));
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
            for (tab, name) in [(DecoderTab::Ft8, "FT8"), (DecoderTab::Ft4, "FT4"), (DecoderTab::Cw, "CW"), (DecoderTab::Log, "LOG")] {
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
        if self.tab == DecoderTab::Log {
            self.log_ui(ui);
            return;
        }
        let Some(e) = &self.engine else { return };
        let lines: Vec<String> = match self.tab {
            DecoderTab::Log => Vec::new(),
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
        ui.label(RichText::new(mic_text).size(11.0).color(if matches!(mic, Some(Err(_))) { TX() } else { MUTED() }));
        let bar_color = if status.vox_keyed { TX() } else { GOOD() };
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
                if self.armed { (self.t("TX enabled", "TX etkin"), TX()) } else { (self.t("TX disabled", "TX kapalı"), MUTED()) };
            if ui::choice(ui, arm_text, self.armed, arm_color) && can {
                self.armed = !self.armed;
                if !self.armed {
                    self.ptt_latched = false;
                    self.tune = false;
                }
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
            if ui::choice_response(ui, "Tune", self.tune, TX()).on_hover_text(tip).clicked() && can && self.armed {
                self.tune = !self.tune;
            }
            // In FT8/FT4 the sequence keys the transmitter, in CW the keyer: no PTT.
            if self.digital.is_none() && self.mode != Mode::Cw {
                if ui::ptt_button(ui, self.ptt_latched || on, can && self.armed) && can && self.armed {
                    self.ptt_latched = !self.ptt_latched;
                }
            } else {
                let text = if on { "TX" } else { "RX" };
                let color = if on { TX() } else { DIM() };
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
                TxState::Transmit => TX(),
                TxState::Receive => GOOD(),
                TxState::Blocked(_) => RX(),
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
                ui.label(RichText::new(text).size(11.0).color(TX()));
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
                ui.label(RichText::new(e).size(11.0).color(TX()));
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
            ui.label(RichText::new(self.t("Switch TX on (right) to send", "Göndermek için sağdaki TX'i açın")).size(11.0).color(RX()));
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
            ui.label(RichText::new(format!("{} · {secs:.0} s", self.t("Sending", "Gönderiliyor"))).size(11.0).color(TX()));
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
                    // The contact's messages or the log.
                    ui.horizontal(|ui| {
                        if ui::choice(ui, "QSO", !self.ft_log_tab, RX()) {
                            self.ft_log_tab = false;
                        }
                        if ui::choice(ui, "LOG", self.ft_log_tab, BLUE()) {
                            self.ft_log_tab = true;
                        }
                    });
                    if self.ft_log_tab {
                        self.log_ui(ui);
                    } else {
                        self.ft.qso_pane(ui, tr);
                    }
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
            ui::caption(ui, self.t("RADIO", "CİHAZ"), RX());
            let label = self.t("Simulator (no radio)", "Simülatör (cihazsız)");
            ui.radio_value(&mut self.edit_device_iio, false, label);
            ui.radio_value(&mut self.edit_device_iio, true, "ADALM-Pluto / AD936x (libiio)");
            ui.add_enabled_ui(self.edit_device_iio, |ui| {
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
                        ui.label(RichText::new(description).size(11.0).color(MUTED()));
                    }
                }
                ui.label(
                    RichText::new(self.t("Network: ip:192.168.2.1 · USB: usb:1.2.5", "Ağ: ip:192.168.2.1 · USB: usb:1.2.5")).size(11.0).color(DIM()),
                );
            });
            if let Err(e) = iio::library_status() {
                ui.label(RichText::new(format!("libiio: {e}")).size(11.0).color(TX()));
            }
            ui.add_space(4.0);
            ui.label(self.t("Sample rate (receive and transmit)", "Örnekleme hızı (alma ve gönderme)"));
            let low = self.t("0.6 MS/s · Ethernet (FIR filter in the radio)", "0.6 MS/s · Ethernet (cihazda FIR filtresi)");
            ui.radio_value(&mut self.edit_rate, LOW_RATE, low);
            let high = self.t("2.4 MS/s · USB", "2.4 MS/s · USB");
            ui.radio_value(&mut self.edit_rate, 2.4e6, high);
            ui.add_space(8.0);
            ui::caption(ui, self.t("RECEIVER", "ALICI"), RX());
            ui.horizontal(|ui| {
                ui.label(self.t("LNB LO", "LNB LO"));
                ui.add(egui::DragValue::new(&mut self.edit_lnb_mhz).speed(0.001).suffix(" MHz").max_decimals(3));
            });
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
            ui::caption(ui, self.t("SOUND", "SES"), GOOD());
            let default = self.t("System default", "Sistem varsayılanı");
            let name_of = |list: &[qo_all::audio::OutputDevice], id: &Option<String>| {
                id.as_ref().and_then(|id| list.iter().find(|d| &d.id == id)).map_or(default.to_string(), |d| d.name.clone())
            };
            ui.label(self.t("Microphone", "Mikrofon"));
            egui::ComboBox::from_id_salt("mic").width(ui.available_width() - 10.0).selected_text(name_of(&self.mics, &self.edit_mic)).show_ui(ui, |ui| {
                ui.selectable_value(&mut self.edit_mic, None, default);
                for d in &self.mics {
                    ui.selectable_value(&mut self.edit_mic, Some(d.id.clone()), &d.name);
                }
            });
            ui.label(self.t("Speaker", "Hoparlör"));
            egui::ComboBox::from_id_salt("speaker")
                .width(ui.available_width() - 10.0)
                .selected_text(name_of(&self.speakers, &self.edit_speaker))
                .show_ui(ui, |ui| {
                    ui.selectable_value(&mut self.edit_speaker, None, default);
                    for d in &self.speakers {
                        ui.selectable_value(&mut self.edit_speaker, Some(d.id.clone()), &d.name);
                    }
                });
            ui.add_space(12.0);
            if ui.add(egui::Button::new(RichText::new(self.t("Apply and restart", "Uygula ve yeniden başlat")).color(BG())).fill(RX())).clicked() {
                self.edit_uri = iio::normalize_uri(&self.edit_uri);
                self.config.device = if self.edit_device_iio { DeviceChoice::Iio(self.edit_uri.clone()) } else { DeviceChoice::Simulator };
                self.config.lnb_lo_hz = self.edit_lnb_mhz * 1e6;
                self.config = self.config.clone().with_rate(self.edit_rate);
                self.config.rx_gain = (!self.edit_auto_gain).then_some(self.edit_gain);
                self.config.microphone = self.edit_mic.clone();
                self.config.speaker = self.edit_speaker.clone();
                self.ft.set_station(&self.edit_call, &self.edit_grid, std::path::PathBuf::from(self.edit_log.trim()));
                self.log.set_path(self.ft.log_path.clone());
                let cty = self.edit_cty.trim();
                self.ft.set_cty((!cty.is_empty()).then(|| std::path::PathBuf::from(cty)));
                self.save();
                self.start(ui.ctx());
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
        self.poll();
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
        egui::CentralPanel::default().frame(egui::Frame::new().fill(BG()).inner_margin(egui::Margin::symmetric(12, 0))).show(ui, |ui| self.spectrum(ui));
    }

    fn on_exit(&mut self) {
        self.set_ptt(false);
        self.save();
        self.engine = None;
    }
}
