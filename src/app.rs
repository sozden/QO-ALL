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
use qo_all::tune;
use qo_all::vox::{Blocked, TxState, VoxSettings};

use crate::settings::Settings;
use crate::ui::{self, palette::*};

/// The transponder as shown when not zoomed in (Hz).
const FULL_VIEW: (f64, f64) = (10_489.490e6, 10_490.010e6);
/// Zoom spans (Hz); None shows the whole transponder.
const ZOOMS: [Option<f64>; 3] = [None, Some(60e3), Some(12e3)];
const WATERFALL_ROWS: usize = 300;
/// FT8 and FT4 dial on QO-100 (downlink, Hz).
const FT_DIAL_HZ: f64 = 10_489.540e6;
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

pub struct App {
    settings: Settings,
    lang: Lang,
    config: StationConfig,
    engine: Option<Engine>,
    /// What the radio said when it opened, or why it did not.
    model: Option<String>,
    tx_capable: Option<Result<(), String>>,
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

    armed: bool,
    ptt_latched: bool,
    tune: bool,
    attenuation: f64,
    mic_gain_db: f32,
    compress: bool,
    vox: VoxSettings,
    time_out_s: u32,
    volume: f32,

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
    edit_mic: Option<String>,
    edit_speaker: Option<String>,
    usb_found: Vec<(String, String)>,
    mics: Vec<qo_all::audio::OutputDevice>,
    speakers: Vec<qo_all::audio::OutputDevice>,
}

impl App {
    pub fn new(cc: &eframe::CreationContext) -> Self {
        ui::apply_theme(&cc.egui_ctx);
        let s = Settings::load();
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
            Some(uri) if uri != "simulator" => DeviceChoice::Iio(uri),
            _ => DeviceChoice::Simulator,
        };
        let defaults = StationConfig::default();
        let config = StationConfig {
            device,
            lnb_lo_hz: s.get("lnb_lo_hz").unwrap_or(defaults.lnb_lo_hz),
            rx_gain: if s.get("rx_auto_gain").unwrap_or(false) { None } else { Some(s.get("rx_gain").unwrap_or(40.0)) },
            microphone: s.text("microphone"),
            speaker: s.text("speaker"),
            ..defaults
        };
        let logo = qo_all::logo::rgba(96, false);
        let logo = cc.egui_ctx.load_texture("logo", ColorImage::from_rgba_unmultiplied([96, 96], &logo), TextureOptions::LINEAR);
        let mode = s.text("mode").and_then(|m| Mode::from_name(&m)).unwrap_or(Mode::Usb);
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
            armed: false,
            ptt_latched: false,
            tune: false,
            attenuation: s.get("tx_attenuation_db").unwrap_or(tx_defaults.attenuation_db),
            mic_gain_db: s.get("mic_gain_db").unwrap_or(0.0),
            compress: s.get("compress").unwrap_or(false),
            vox,
            time_out_s: s.get("time_out_s").unwrap_or(180),
            volume: s.get("volume").unwrap_or(0.7),
            zoom: s.get("zoom").unwrap_or(0).min(ZOOMS.len() - 1),
            zoom_center: rx_dial,
            line: Vec::new(),
            line_band: FULL_VIEW,
            floor: -100.0,
            waterfall: ColorImage::new([FRAME_POINTS, WATERFALL_ROWS], vec![DEEP; FRAME_POINTS * WATERFALL_ROWS]),
            waterfall_tex: None,
            logo,
            tab: DecoderTab::Ft8,
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
            edit_mic: config.microphone.clone(),
            edit_speaker: config.speaker.clone(),
            usb_found: Vec::new(),
            mics: Vec::new(),
            speakers: Vec::new(),
            config,
            engine: None,
            model: None,
            tx_capable: None,
            error: None,
            rate: 0.0,
            settings: s,
        };
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
        self.error = None;
        self.rate = 0.0;
        self.armed = false;
        self.ptt_latched = false;
        self.tune = false;
        self.waterfall.pixels.fill(DEEP);
        let listen = ListenSettings { dial_hz: 0.0, mode: self.mode, bandwidth_hz: self.mode.default_bandwidth(), audio_rate: 48e3, volume: self.volume };
        let ctx = ctx.clone();
        let engine = Engine::start(self.config.clone(), listen, TxSettings::default(), self.volume, move || ctx.request_repaint());
        self.engine = Some(engine);
        self.sync();
    }

    fn correction_now(&self) -> f64 {
        self.engine.as_ref().and_then(|e| e.correction()).unwrap_or(self.correction)
    }

    fn tx_downlink(&self) -> f64 {
        if self.linked { self.rx_dial } else { self.tx_dial }
    }

    fn uplink(&self) -> f64 {
        bandplan::uplink(self.tx_downlink()) + self.tx_correction
    }

    /// Band plan check of the transmitted signal.
    fn tx_band(&self) -> Result<(), Refusal> {
        let d = self.tx_downlink();
        let (lo, hi) = match self.mode {
            Mode::Lsb => (d - qo_all::tx::AUDIO_HIGH, d - qo_all::tx::AUDIO_LOW),
            _ => (d + qo_all::tx::AUDIO_LOW, d + qo_all::tx::AUDIO_HIGH),
        };
        bandplan::check(lo, hi)
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
            l.dial_hz = to_if(self.rx_dial);
            l.mode = self.mode;
            l.bandwidth_hz = self.mode.default_bandwidth();
            l.volume = self.volume;
        });
        e.volume.set(self.volume);
        {
            let mut tx = e.tx.lock().unwrap();
            tx.mode = if self.mode == Mode::Lsb { Mode::Lsb } else { Mode::Usb };
            tx.uplink_hz = self.uplink();
            tx.attenuation_db = self.attenuation;
            tx.mic_gain_db = self.mic_gain_db;
            tx.compress = self.compress;
            tx.vox = self.vox;
            // CW transmission comes in a later version.
            tx.armed = self.armed && self.mode != Mode::Cw;
            tx.tune = self.tune && tx.armed;
            tx.band = self.tx_band();
            tx.time_out = Duration::from_secs(self.time_out_s as u64);
        }
        e.ft8.set_dial(Some(to_if(FT_DIAL_HZ)));
        e.ft4.set_dial(self.ft4_on.then(|| to_if(FT_DIAL_HZ)));
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
        if let Some(c) = e.correction() {
            self.correction = c;
        }
        for ev in events {
            match ev {
                Event::Opened { model, tx } => {
                    self.model = Some(model);
                    self.tx_capable = Some(tx);
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
        s.set("zoom", self.zoom);
        s.set("ft4", self.ft4_on);
        s.save();
    }

    /// Moves the receiver (and a linked transmitter) to `hz`.
    fn tune_rx(&mut self, hz: f64) {
        self.rx_dial = (hz / 10.0).round() * 10.0;
        if let Some(span) = ZOOMS[self.zoom] {
            if (self.rx_dial - self.zoom_center).abs() > span * 0.4 {
                self.zoom_center = self.rx_dial;
                self.waterfall.pixels.fill(DEEP);
            }
        }
    }

    // ---- Top bar ------------------------------------------------------------------

    fn header(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            ui.image((self.logo.id(), vec2(30.0, 30.0)));
            ui.label(RichText::new("QO").size(19.0).strong().color(Color32::WHITE));
            ui.add_space(-4.0);
            ui.label(RichText::new("ALL").size(19.0).strong().color(RX));
            ui.add_space(10.0);
            match (&self.error, &self.model) {
                (Some(e), _) => {
                    ui::pill(ui, e.clone(), TX, Color32::from_rgb(0x3a, 0x14, 0x18));
                }
                (None, Some(m)) => {
                    let duplex = match &self.tx_capable {
                        Some(Ok(())) => self.t("full duplex", "full duplex"),
                        _ => self.t("receive only", "yalnızca alıcı"),
                    };
                    ui::pill(ui, format!("{m} · {duplex}"), BLUE, RAISED);
                }
                (None, None) => {
                    ui::pill(ui, self.t("Connecting…", "Bağlanıyor…"), MUTED, RAISED);
                }
            }
            let beacon = self.engine.as_ref().map(|e| *e.beacon.lock().unwrap());
            let (text, fg, bg) = match beacon.map(|b| b.status) {
                Some(BeaconStatus::Locked { .. }) => (
                    format!("{} · LNB {:+.0} Hz", self.t("Beacon locked", "Beacon kilitli"), self.correction),
                    GOOD,
                    Color32::from_rgb(0x12, 0x30, 0x1f),
                ),
                Some(BeaconStatus::NotFound) => (self.t("Beacon not found", "Beacon bulunamadı").to_string(), TX, RAISED),
                Some(BeaconStatus::OutsideBand) => (self.t("Beacon outside the band", "Beacon bant dışında").to_string(), TX, RAISED),
                _ => (self.t("Looking for the beacon…", "Beacon aranıyor…").to_string(), MUTED, RAISED),
            };
            ui::pill(ui, text, fg, bg);
            if self.rate > 0.0 && self.rate < self.config.sample_rate * 0.95 {
                ui::pill(ui, format!("{:.2} MS/s", self.rate / 1e6), TX, RAISED)
                    .on_hover_text(self.t("The radio delivers fewer samples than set", "Cihaz ayarlanandan az örnek gönderiyor"));
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui.add(egui::Button::new(RichText::new(self.t("Settings", "Ayarlar")).color(TEXT))).clicked() {
                    self.show_settings = !self.show_settings;
                    if self.show_settings {
                        self.mics = qo_all::audio::input_devices();
                        self.speakers = qo_all::audio::output_devices();
                    }
                }
                let next = if self.lang == Lang::Tr { "EN" } else { "TR" };
                if ui.button(next).clicked() {
                    self.lang = if self.lang == Lang::Tr { Lang::En } else { Lang::Tr };
                    self.save();
                }
                let now = SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs());
                ui.label(RichText::new(format!("{} UTC", clock(now))).monospace().size(14.0).color(MUTED));
            });
        });
    }

    // ---- Frequency displays -------------------------------------------------------

    fn vfos(&mut self, ui: &mut egui::Ui) {
        let status = self.engine.as_ref().map(|e| e.tx_status.lock().unwrap().clone()).unwrap_or_default();
        let transmitting = status.state == TxState::Transmit;
        let level = self.engine.as_ref().map_or(-150.0, |e| e.listener.level_db());
        let width = (ui.available_width() - 60.0) / 2.0;
        ui.horizontal(|ui| {
            // Receiver.
            let rx_change = ui
                .allocate_ui(vec2(width, 80.0), |ui| {
                    ui::card(ui, DEEP, |ui| {
                        ui.set_width(ui.available_width());
                        ui::caption(ui, format!("DOWNLINK · RX · {} · {:.0} dBFS", self.mode.name(), level), RX);
                        ui::vfo(ui, self.rx_dial, 34.0, RX)
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
            let fill = if transmitting { Color32::from_rgb(0x3a, 0x14, 0x18) } else { DEEP };
            let uplink = self.uplink();
            let tx_change = ui
                .allocate_ui(vec2(width, 80.0), |ui| {
                    ui::card(ui, fill, |ui| {
                        ui.set_width(ui.available_width());
                        let mode = if self.mode == Mode::Lsb { "LSB" } else { "USB" };
                        let shift = if self.linked { String::new() } else { format!(" · shift {:+.2} kHz", shift / 1e3) };
                        ui::caption(ui, format!("UPLINK · TX · {mode}{shift}"), TX);
                        ui::vfo(ui, uplink, 34.0, TX)
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
            for m in Mode::ALL {
                if ui::choice(ui, m.name(), self.mode == m, RX) {
                    self.mode = m;
                }
            }
            ui.add_space(12.0);
            let lock = if self.auto_lock { self.t("Lock: automatic", "Kilit: otomatik") } else { self.t("Lock: manual", "Kilit: elle") };
            if ui::choice(ui, lock, self.auto_lock, BLUE) {
                self.auto_lock = !self.auto_lock;
            }
            ui.add_space(12.0);
            for (i, label) in ["520 kHz", "60 kHz", "12 kHz"].iter().enumerate() {
                if ui::choice(ui, label, self.zoom == i, BLUE) {
                    self.zoom = i;
                    self.zoom_center = self.rx_dial;
                    self.waterfall.pixels.fill(DEEP);
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
                    .color(DIM),
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
        painter.rect_filled(rect, CornerRadius::same(8), DEEP);
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
                painter.rect_filled(r, CornerRadius::ZERO, TX.gamma_multiply(0.08));
            }
        }
        // Frequency grid and axis.
        let step = ui::nice_step(width_hz / (rect.width() as f64 / 110.0));
        let axis = Rect::from_min_max(pos2(rect.left(), spec.bottom()), pos2(rect.right(), wf.top()));
        painter.rect_filled(axis, CornerRadius::ZERO, BG);
        let mut f = (lo_hz / step).ceil() * step;
        while f <= hi_hz {
            let x = x_of(f);
            painter.line_segment([pos2(x, spec.top()), pos2(x, spec.bottom())], Stroke::new(1.0, GRID));
            let label = if step >= 1e3 { format!("{:.3}", f / 1e6) } else { format!("{:.4}", f / 1e6) };
            painter.text(pos2(x, axis.center().y), Align2::CENTER_CENTER, label, FontId::monospace(11.0), DIM);
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
            let fill = BLUE.gamma_multiply(0.14);
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
            painter.add(egui::Shape::line(points, Stroke::new(1.2, BLUE)));
        }
        // FT8 segment marker.
        let ft = x_of(FT_DIAL_HZ);
        if (rect.left()..rect.right()).contains(&ft) {
            painter.text(pos2(ft + 3.0, spec.top() + 4.0), Align2::LEFT_TOP, "FT8", FontId::proportional(11.0), DIM);
        }
        // Receiver passband and dial.
        let (pa, pb) = self.mode.passband(self.mode.default_bandwidth());
        let rx_band = Rect::from_min_max(pos2(x_of(self.rx_dial + pa), rect.top()), pos2(x_of(self.rx_dial + pb), rect.bottom()));
        let rx_band = if rx_band.width() < 3.0 { rx_band.expand2(vec2((3.0 - rx_band.width()) / 2.0, 0.0)) } else { rx_band };
        painter.rect_filled(rx_band, CornerRadius::ZERO, RX.gamma_multiply(0.2));
        let rx_x = x_of(self.rx_dial);
        painter.line_segment([pos2(rx_x, rect.top()), pos2(rx_x, rect.bottom())], Stroke::new(1.5, RX));
        painter.text(pos2(rx_x + 5.0, rect.top() + 3.0), Align2::LEFT_TOP, "RX", FontId::proportional(12.0), RX);
        // Transmitter dial.
        let tx_x = x_of(self.tx_downlink());
        ui::dashed_vline(&painter, tx_x, rect.top(), rect.bottom(), Stroke::new(1.5, TX));
        painter.text(pos2(tx_x - 5.0, rect.top() + 3.0), Align2::RIGHT_TOP, "TX", FontId::proportional(12.0), TX);

        // Hover readout.
        if let Some(p) = response.hover_pos() {
            let hz = hz_of(p.x);
            painter.line_segment([pos2(p.x, spec.top()), pos2(p.x, spec.bottom())], Stroke::new(1.0, MUTED.gamma_multiply(0.5)));
            let text = format!("{:.3} MHz · {}", hz / 1e6, bandplan::segment(hz));
            painter.text(pos2(p.x + 8.0, spec.bottom() - 6.0), Align2::LEFT_BOTTOM, text, FontId::monospace(11.0), TEXT);
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
            && let Some(p) = response.interact_pointer_pos()
        {
            let hz = place(self, p.x);
            self.tune_rx(hz);
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
                if ui::choice(ui, name, self.tab == tab, BLUE) {
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
                ui.label(RichText::new(text).size(11.0).color(DIM));
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
                    let mut text = cw.text.clone();
                    if text.len() > 600 {
                        text = text[text.len() - 600..].to_string();
                    }
                    vec![format!("{:.0} WPM {}", cw.wpm, if cw.key_down { "■" } else { "" }), text]
                }
                _ => vec![self.t("Switch to CW and click a CW signal.", "CW'ye geçip bir CW sinyaline tıklayın.").to_string()],
            },
        };
        egui::ScrollArea::vertical().auto_shrink([false, false]).stick_to_bottom(true).show(ui, |ui| {
            for l in lines {
                ui.label(RichText::new(l).monospace().size(12.0).color(TEXT));
            }
        });
    }

    fn sound(&mut self, ui: &mut egui::Ui) {
        let status = self.engine.as_ref().map(|e| e.tx_status.lock().unwrap().clone()).unwrap_or_default();
        ui.horizontal(|ui| {
            ui::caption(ui, self.t("SOUND AND VOX", "SES VE VOX"), TX);
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let (text, on) = if self.vox.enabled { (self.t("VOX on", "VOX açık"), true) } else { (self.t("VOX off", "VOX kapalı"), false) };
                if ui::choice(ui, text, on, GOOD) {
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
        ui.label(RichText::new(mic_text).size(11.0).color(if matches!(mic, Some(Err(_))) { TX } else { MUTED }));
        let bar_color = if status.vox_keyed { TX } else { GOOD };
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
                if self.armed { (self.t("TX enabled", "TX etkin"), TX) } else { (self.t("TX disabled", "TX kapalı"), MUTED) };
            if ui::choice(ui, arm_text, self.armed, arm_color) && can {
                self.armed = !self.armed;
                if !self.armed {
                    self.ptt_latched = false;
                    self.tune = false;
                }
            }
            ui.add_space(2.0);
            ui.label(RichText::new(self.t("Output power", "Çıkış gücü")).size(11.0).color(MUTED));
            let mut power = -self.attenuation;
            ui.spacing_mut().slider_width = 70.0;
            ui.add(egui::Slider::new(&mut power, -MAX_ATTENUATION..=0.0).suffix(" dB").step_by(0.5));
            self.attenuation = -power;
            ui.add_space(2.0);
            let tune_text = self.t("Tune 1 kHz", "Ton 1 kHz");
            if ui::choice(ui, tune_text, self.tune, TX) && can && self.armed {
                self.tune = !self.tune;
            }
            let on = status.state == TxState::Transmit;
            if ui::ptt_button(ui, self.ptt_latched || on, can && self.armed) && can && self.armed {
                self.ptt_latched = !self.ptt_latched;
            }
            let state = match status.state {
                TxState::Transmit => match status.remaining {
                    Some(r) => format!("TX · {}:{:02} · ALC {:.0} dB", r.as_secs() / 60, r.as_secs() % 60, status.alc_db),
                    None => "TX".into(),
                },
                TxState::Receive => "RX".into(),
                TxState::Blocked(b) => self.blocked_text(b).to_string(),
            };
            let color = match status.state {
                TxState::Transmit => TX,
                TxState::Receive => GOOD,
                TxState::Blocked(_) => RX,
            };
            ui.label(RichText::new(state).size(12.0).color(color));
            if let Some(e) = &status.error {
                ui.label(RichText::new(e).size(11.0).color(TX));
            }
            if self.mode == Mode::Cw {
                ui.label(RichText::new(self.t("CW sending comes later", "CW gönderimi sonraki sürümde")).size(11.0).color(DIM));
            }
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
            ui::caption(ui, self.t("RADIO", "CİHAZ"), RX);
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
                        ui.label(RichText::new(description).size(11.0).color(MUTED));
                    }
                }
                ui.label(
                    RichText::new(self.t("Network: ip:192.168.2.1 · USB: usb:1.2.5", "Ağ: ip:192.168.2.1 · USB: usb:1.2.5")).size(11.0).color(DIM),
                );
            });
            if let Err(e) = iio::library_status() {
                ui.label(RichText::new(format!("libiio: {e}")).size(11.0).color(TX));
            }
            ui.add_space(8.0);
            ui::caption(ui, self.t("RECEIVER", "ALICI"), RX);
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
            ui::caption(ui, self.t("TRANSMITTER", "VERİCİ"), TX);
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
            ui::caption(ui, self.t("SOUND", "SES"), GOOD);
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
            if ui.add(egui::Button::new(RichText::new(self.t("Apply and restart", "Uygula ve yeniden başlat")).color(BG)).fill(RX)).clicked() {
                self.config.device = if self.edit_device_iio { DeviceChoice::Iio(self.edit_uri.trim().to_string()) } else { DeviceChoice::Simulator };
                self.config.lnb_lo_hz = self.edit_lnb_mhz * 1e6;
                self.config.rx_gain = (!self.edit_auto_gain).then_some(self.edit_gain);
                self.config.microphone = self.edit_mic.clone();
                self.config.speaker = self.edit_speaker.clone();
                self.save();
                self.start(ui.ctx());
                self.show_settings = false;
            }
            ui.add_space(8.0);
            ui.label(RichText::new(format!("QO-ALL {} · GPL-3.0-or-later", env!("CARGO_PKG_VERSION"))).size(11.0).color(DIM));
        });
    }
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
        let ptt = self.armed && (self.ptt_latched || (space && !typing));
        self.set_ptt(ptt);
        self.sync();
        ui.ctx().request_repaint_after(Duration::from_millis(200));

        let bar = egui::Frame::new().fill(CARD).inner_margin(egui::Margin::symmetric(14, 6));
        egui::Panel::top("header").frame(bar).show(ui, |ui| self.header(ui));
        egui::Panel::top("vfos").frame(egui::Frame::new().fill(BG).inner_margin(egui::Margin::symmetric(12, 8))).show(ui, |ui| self.vfos(ui));
        if self.show_settings {
            egui::Panel::right("settings")
                .resizable(false)
                .default_size(330.0)
                .frame(egui::Frame::new().fill(CARD).inner_margin(egui::Margin::same(14)))
                .show(ui, |ui| self.settings_panel(ui));
        }
        egui::Panel::bottom("cards")
            .resizable(true)
            .default_size(230.0)
            .size_range(170.0..=420.0)
            .frame(egui::Frame::new().fill(BG).inner_margin(egui::Margin::symmetric(12, 8)))
            .show(ui, |ui| {
                let h = ui.available_height();
                ui.horizontal_top(|ui| {
                    let w = ui.available_width();
                    let side = 380.0f32.min(w * 0.36);
                    let ptt = 200.0;
                    let decoders = (w - side - ptt - 2.0 * ui.spacing().item_spacing.x - 30.0).max(200.0);
                    // Every card stacks its contents top-down, whatever the row's layout.
                    let column = egui::Layout::top_down(egui::Align::Min);
                    ui.allocate_ui_with_layout(vec2(decoders, h), column, |ui| {
                        ui::card(ui, DEEP, |ui| {
                            ui.set_min_size(vec2(ui.available_width(), h - 18.0));
                            self.decoders(ui);
                        });
                    });
                    ui.allocate_ui_with_layout(vec2(side, h), column, |ui| {
                        ui::card(ui, CARD, |ui| {
                            ui.set_min_size(vec2(ui.available_width(), h - 18.0));
                            self.sound(ui);
                        });
                    });
                    ui.allocate_ui_with_layout(vec2(ptt, h), column, |ui| {
                        ui::card(ui, CARD, |ui| {
                            ui.set_min_size(vec2(ui.available_width(), h - 18.0));
                            self.transmit(ui);
                        });
                    });
                });
            });
        egui::CentralPanel::default().frame(egui::Frame::new().fill(BG).inner_margin(egui::Margin::symmetric(12, 0))).show(ui, |ui| self.spectrum(ui));
    }

    fn on_exit(&mut self) {
        self.set_ptt(false);
        self.save();
        self.engine = None;
    }
}
