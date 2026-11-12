//! The FT8/FT4 panel, laid out as WSJT-X users know it: band activity, the contact's
//! messages, and the controls with the six standard messages and automatic sequencing.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use eframe::egui::{self, Color32, CornerRadius, RichText, Sense, vec2};

use std::collections::HashSet;

use qo_all::adif::{self, Contact, Status, Worked};
use qo_all::dxcc::Cty;
use qo_all::engine::{DigitalJob, Engine};
use qo_all::ft8::qso::{Heard, Parsed, Payload, Sequencer, Step, format_report, parse};
use qo_all::ft8::worker::Ft8Message;
use qo_all::ft8::{FT8, Protocol, encode};

use crate::settings::Settings;
use crate::ui::{self, palette::*};

/// Row colours of the band activity: dark rows with white text at night, light rows with
/// dark text on paper.
fn shade(night: [u8; 3], paper: [u8; 3]) -> Color32 {
    let c = if light() { paper } else { night };
    Color32::from_rgb(c[0], c[1], c[2])
}
#[allow(non_snake_case)]
fn TO_ME_BG() -> Color32 { shade([0x5a, 0x1e, 0x24], [0xf3, 0xcf, 0xc7]) }
#[allow(non_snake_case)]
fn NEW_CQ_BG() -> Color32 { shade([0x15, 0x3d, 0x28], [0xdc, 0xeb, 0xc9]) }
#[allow(non_snake_case)]
fn NEW_GRID_BG() -> Color32 { shade([0x34, 0x24, 0x5c], [0xe3, 0xd9, 0xf2]) }
#[allow(non_snake_case)]
fn WORKED() -> Color32 { shade([0x6a, 0x7b, 0x93], [0x9c, 0x8f, 0x78]) }
/// Alert colours: a new DXCC entity, a new grid square.
#[allow(non_snake_case)]
fn NEW_DXCC_BG() -> Color32 { shade([0x6b, 0x40, 0x0c], [0xf5, 0xd9, 0xa8]) }
#[allow(non_snake_case)]
fn NEW_SQUARE_BG() -> Color32 { shade([0x0d, 0x4a, 0x52], [0xcf, 0xe6, 0xe8]) }
/// Text on the coloured rows.
#[allow(non_snake_case)]
fn ROW_TEXT() -> Color32 { STRONG() }
pub fn alert_color() -> Color32 {
    shade([0xff, 0xb3, 0x4d], [0xb0, 0x5a, 0x00])
}
/// How long an alert stays in the panel's title (s).
const ALERT_SHOWN_S: f64 = 30.0;

/// Which alerts are on: shown (row colour and a line in the title) and heard (a beep).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Alerts {
    pub dxcc_show: bool,
    pub dxcc_sound: bool,
    pub grid_show: bool,
    pub grid_sound: bool,
}
/// Messages shown in the band activity.
const SHOWN: usize = 150;
/// FT8 and FT4 tones span this much audio (Hz).
fn signal_width(p: &Protocol) -> f64 {
    p.tones as f64 * p.tone_spacing()
}

fn now_utc() -> f64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map_or(0.0, |d| d.as_secs_f64())
}

fn hms(secs: u64) -> String {
    let s = secs % 86_400;
    format!("{:02}{:02}{:02}", s / 3600, s / 60 % 60, s % 60)
}

/// A transmission handed to the engine.
struct Scheduled {
    id: u64,
    slot: u64,
    text: String,
    step: Step,
    start_utc: f64,
}

pub struct FtPanel {
    pub seq: Sequencer,
    /// "Enable Tx": the sequence may transmit.
    pub enabled: bool,
    pub auto_seq: bool,
    /// Audio frequency of tone 0 above the dial (Hz).
    pub audio_hz: f64,
    /// CQ in odd (second) slots.
    pub cq_odd: bool,
    pub log_path: PathBuf,
    pub worked: Worked,
    processed: Option<(&'static str, u64)>,
    scheduled: Option<Scheduled>,
    acked: Option<u64>,
    missed: Option<u64>,
    next_id: u64,
    last_tx_slot: Option<u64>,
    qso_lines: Vec<(String, Color32)>,
    status: String,
    audio: HashMap<(String, &'static str, u64), Arc<Vec<f32>>>,
    /// The country file (from WSJT-X/JTDX or the settings), the entities in the log, the
    /// alerts and what was already alerted this session.
    pub cty: Option<Cty>,
    pub cty_path: Option<PathBuf>,
    worked_entities: HashSet<String>,
    pub alerts: Alerts,
    alerted: HashSet<String>,
    banner: Option<(String, f64)>,
}

/// Default log: next to the settings.
pub fn default_log() -> PathBuf {
    qo_all::paths::config_dir().unwrap_or_default().join("qo-all.adi")
}

/// WSJT-X's own log, if there is one (to share it).
pub fn wsjtx_log() -> Option<PathBuf> {
    let base = if cfg!(windows) {
        PathBuf::from(std::env::var_os("LOCALAPPDATA")?).join("WSJT-X")
    } else {
        PathBuf::from(std::env::var_os("HOME")?).join(".local").join("share").join("WSJT-X")
    };
    let p = base.join("wsjtx_log.adi");
    p.is_file().then_some(p)
}

impl FtPanel {
    pub fn new(s: &Settings) -> Self {
        let log_path = s.text("adif_path").map(PathBuf::from).unwrap_or_else(default_log);
        let mut seq = Sequencer::new(&s.text("my_call").unwrap_or_default(), &s.text("my_grid").unwrap_or_default());
        seq.max_repeats = s.get("ft_max_repeats").unwrap_or(6);
        let cty_path = s.text("cty_path").map(PathBuf::from).filter(|p| p.is_file()).or_else(qo_all::dxcc::find_file);
        let alerts = Alerts {
            dxcc_show: s.get("alert_dxcc_show").unwrap_or(true),
            dxcc_sound: s.get("alert_dxcc_sound").unwrap_or(true),
            grid_show: s.get("alert_grid_show").unwrap_or(true),
            grid_sound: s.get("alert_grid_sound").unwrap_or(false),
        };
        let mut panel = Self {
            seq,
            enabled: false,
            auto_seq: s.get("ft_auto_seq").unwrap_or(true),
            audio_hz: s.get("ft_audio_hz").unwrap_or(1_500.0),
            cq_odd: s.get("ft_cq_odd").unwrap_or(false),
            worked: Worked::load(&log_path),
            log_path,
            processed: None,
            scheduled: None,
            acked: None,
            missed: None,
            next_id: 1,
            last_tx_slot: None,
            qso_lines: Vec::new(),
            status: String::new(),
            audio: HashMap::new(),
            cty: cty_path.as_deref().and_then(Cty::load),
            cty_path,
            worked_entities: HashSet::new(),
            alerts,
            alerted: HashSet::new(),
            banner: None,
        };
        panel.count_entities();
        panel
    }

    /// The DXCC entities of the calls in the log.
    fn count_entities(&mut self) {
        self.worked_entities = match &self.cty {
            Some(c) => self.worked.calls().filter_map(|call| c.entity(call)).map(|e| e.name.clone()).collect(),
            None => HashSet::new(),
        };
    }

    /// Another country file.
    pub fn set_cty(&mut self, path: Option<PathBuf>) {
        let path = path.filter(|p| p.is_file()).or_else(qo_all::dxcc::find_file);
        self.cty = path.as_deref().and_then(Cty::load);
        self.cty_path = path;
        self.count_entities();
    }

    /// Entities worked, and those the country file knows (for the settings).
    pub fn entity_counts(&self) -> (usize, usize) {
        (self.worked_entities.len(), self.cty.as_ref().map_or(0, Cty::len))
    }

    /// What is new about a station: a DXCC entity, a grid square.
    fn novelty(&self, call: &str, grid: Option<&str>) -> (bool, bool) {
        let entity = self.cty.as_ref().and_then(|c| c.entity(call));
        let new_dxcc = entity.is_some_and(|e| !self.worked_entities.contains(&e.name));
        let new_grid = grid.is_some_and(|g| !self.worked.grid_worked(g));
        (new_dxcc, new_grid)
    }

    pub fn save(&self, s: &mut Settings) {
        s.set("my_call", &self.seq.my_call);
        s.set("my_grid", &self.seq.my_grid);
        s.set("adif_path", self.log_path.display());
        s.set("ft_auto_seq", self.auto_seq);
        s.set("ft_audio_hz", self.audio_hz);
        s.set("ft_cq_odd", self.cq_odd);
        s.set("ft_max_repeats", self.seq.max_repeats);
        s.set_opt("cty_path", self.cty_path.as_ref().map(|p| p.display()));
        s.set("alert_dxcc_show", self.alerts.dxcc_show);
        s.set("alert_dxcc_sound", self.alerts.dxcc_sound);
        s.set("alert_grid_show", self.alerts.grid_show);
        s.set("alert_grid_sound", self.alerts.grid_sound);
    }

    /// New station details or log file (from the settings).
    /// The log changed elsewhere (entered or corrected by hand): the worked stations
    /// are read again.
    pub fn reload_worked(&mut self) {
        self.worked = Worked::load(&self.log_path);
        self.count_entities();
    }

    pub fn set_station(&mut self, call: &str, grid: &str, log: PathBuf) {
        let max = self.seq.max_repeats;
        self.seq = Sequencer::new(call, grid);
        self.seq.max_repeats = max;
        self.worked = Worked::load(&log);
        self.log_path = log;
        self.count_entities();
    }

    pub fn signal_width(p: &Protocol) -> f64 {
        signal_width(p)
    }

    /// Stops at once: the sequence, and a transmission going out.
    pub fn halt(&mut self, e: Option<&Engine>) {
        self.enabled = false;
        self.seq.halt();
        self.scheduled = None;
        if let Some(e) = e {
            let mut d = e.digital.lock().unwrap();
            d.job = None;
            d.halt = true;
        }
    }

    fn line(&mut self, text: String, color: Color32) {
        self.qso_lines.push((text, color));
        if self.qso_lines.len() > 200 {
            self.qso_lines.drain(..50);
        }
    }

    fn involves_me(&self, text: &str) -> bool {
        let me = &self.seq.my_call;
        let dx = self.seq.dx.as_ref().map(|d| d.call.as_str());
        match parse(text) {
            Parsed::Directed { to, from, .. } => to == *me || from == *me || Some(from.as_str()) == dx || Some(to.as_str()) == dx,
            Parsed::Cq { from, .. } => from == *me || Some(from.as_str()) == dx,
            Parsed::Other => false,
        }
    }

    /// Runs every frame in FT8/FT4: new decodes into the sequence, transmissions that
    /// started, finished contacts into the log, and the next transmission scheduled.
    /// `dial` and `uplink` (downlink and uplink of the suppressed carrier, Hz) go into
    /// the log.
    pub fn update(&mut self, e: &Engine, p: &'static Protocol, armed: bool, dial: f64, uplink: f64, tr: bool) {
        let t = |en: &'static str, tr_: &'static str| if tr { tr_ } else { en };
        let now = now_utc();
        let worker = if std::ptr::eq(p, &FT8) { &e.ft8 } else { &e.ft4 };
        // New decodes.
        if let Some(slot) = worker.status().last_slot
            && self.processed != Some((p.name, slot))
        {
            self.processed = Some((p.name, slot));
            let messages: Vec<Ft8Message> = worker.messages().into_iter().filter(|m| m.slot == slot).collect();
            let odd = ((slot as f64 / p.slot_s).round() as u64) % 2 == 1;
            for m in &messages {
                if self.involves_me(&m.text) {
                    let color = if matches!(parse(&m.text), Parsed::Directed { ref to, .. } if *to == self.seq.my_call) { TX() } else { TEXT() };
                    self.line(format!("{} {:+3.0} {:4.0}  {}", hms(m.slot), m.snr_db, m.audio_hz, m.text), color);
                }
            }
            // New DXCC entities and grid squares calling CQ: shown and/or heard, once
            // per station and session.
            let mut beep = false;
            for m in &messages {
                let Parsed::Cq { from, grid } = parse(&m.text) else { continue };
                if from == self.seq.my_call || self.alerted.contains(&from) {
                    continue;
                }
                let (new_dxcc, new_grid) = self.novelty(&from, grid.as_deref());
                let a = self.alerts;
                let what = if new_dxcc && (a.dxcc_show || a.dxcc_sound) {
                    let name = self.cty.as_ref().and_then(|c| c.entity(&from)).map_or(String::new(), |e| e.name.clone());
                    beep |= a.dxcc_sound;
                    Some((format!("{}: {name} ({from})", t("New DXCC", "Yeni DXCC")), a.dxcc_show))
                } else if new_grid && (a.grid_show || a.grid_sound) {
                    beep |= a.grid_sound;
                    Some((format!("{}: {} ({from})", t("New locator", "Yeni lokatör"), grid.clone().unwrap_or_default()), a.grid_show))
                } else {
                    None
                };
                if let Some((text, show)) = what {
                    self.alerted.insert(from.clone());
                    if show {
                        self.banner = Some((text, now));
                    }
                }
            }
            if beep {
                e.listener.beep();
            }
            if self.auto_seq && !self.seq.my_call.is_empty() {
                let heard: Vec<Heard> = messages.iter().map(|m| Heard { text: m.text.clone(), snr: m.snr_db.round() as i32, odd }).collect();
                let busy_before = self.seq.dx.as_ref().map(|d| d.call.clone());
                self.seq.heard(&heard, now as u64);
                // Someone answered: transmitting goes on by itself (as with WSJT-X's
                // auto-sequence) as long as Tx was enabled before.
                if self.seq.dx.as_ref().map(|d| d.call.clone()) != busy_before && !self.seq.done && self.enabled {
                    self.status = format!("{} {}", t("Working", "QSO:"), self.seq.dx.as_ref().map_or("", |d| d.call.as_str()));
                }
            }
        }
        // Transmissions that started or were dropped.
        let st = e.tx_status.lock().unwrap().clone();
        if let Some(id) = st.digital_started
            && self.acked != Some(id)
        {
            self.acked = Some(id);
            if let Some(sc) = self.scheduled.take().filter(|s| s.id == id) {
                self.last_tx_slot = Some(sc.slot);
                self.line(format!("{} TX      {}", hms(sc.slot * p.slot_s as u64), sc.text), RX());
                // Counts only if the sequence did not move on meanwhile.
                if sc.step == self.seq.next {
                    self.seq.sent(now as u64);
                }
            }
        }
        if let Some(id) = st.digital_missed
            && self.missed != Some(id)
        {
            self.missed = Some(id);
            self.status = t("Too late for the slot: the next one is used", "Dilime geç kalındı: sonraki kullanılacak").into();
            self.scheduled = None;
        }
        // A finished contact goes into the log.
        if let Some(f) = self.seq.take_finished() {
            let c = Contact {
                call: f.partner.call.clone(),
                grid: f.partner.grid.clone(),
                mode: p.name.to_string(),
                rst_sent: format_report(f.partner.sent.unwrap_or(-10)),
                rst_rcvd: format_report(f.partner.rcvd.unwrap_or(-10)),
                start: f.partner.start,
                end: f.end,
                tx_hz: uplink + self.audio_hz,
                rx_hz: dial + self.audio_hz,
                my_call: self.seq.my_call.clone(),
                my_grid: self.seq.my_grid.clone(),
                name: None,
            };
            match adif::append(&self.log_path, &c) {
                Ok(()) => {
                    self.worked.add(&c.call, c.grid.as_deref());
                    if let Some(entity) = self.cty.as_ref().and_then(|cty| cty.entity(&c.call)) {
                        self.worked_entities.insert(entity.name.clone());
                    }
                    self.line(format!("LOG {} {} {} {}/{}", c.call, c.grid.clone().unwrap_or_default(), p.name, c.rst_sent, c.rst_rcvd), GOOD());
                    self.status = format!("{} {}", t("Logged", "Loglandı:"), c.call);
                }
                Err(err) => self.status = format!("{}: {err}", t("Log not written", "Log yazılamadı")),
            }
        }
        // The next transmission.
        let want = self.enabled && armed && !self.seq.my_call.is_empty() && !self.seq.done;
        if !want {
            if self.scheduled.take().is_some() {
                e.digital.lock().unwrap().job = None;
            }
            return;
        }
        let slot_s = p.slot_s;
        let cur = (now / slot_s).floor() as u64;
        let into = now - cur as f64 * slot_s;
        // As in WSJT-X, a call picked (or an answer due) in the own slot goes out at once,
        // its beginning cut so the rest keeps the slot's timing. Up to about a third of
        // the message can be missing and it still decodes at the other end (FT8: 4.5 of
        // 12.6 s, FT4: 1.8 of 5 s); later than that the next own slot is used.
        let (whole, max_late) = if std::ptr::eq(p, &FT8) { (1.0, 4.5) } else { (0.5, 1.8) };
        let odd = |n: u64| n % 2 == 1;
        let mut target = cur;
        if !(odd(cur) == self.seq.tx_odd && into < 0.5 + max_late - 0.2 && self.last_tx_slot != Some(cur)) {
            target = cur + 1;
            if odd(target) != self.seq.tx_odd {
                target += 1;
            }
            if self.last_tx_slot == Some(target) {
                target += 2;
            }
        }
        let text = self.seq.message(self.seq.next);
        if self.scheduled.as_ref().is_some_and(|s| s.slot == target && s.text == text) {
            return;
        }
        let key = (text.clone(), p.name, self.audio_hz.round() as u64);
        let audio = match self.audio.get(&key) {
            Some(a) => a.clone(),
            None => match encode::tones(p, &text) {
                Some(tones) => {
                    let a = Arc::new(encode::waveform(p, &tones, self.audio_hz, e.mic_rate, 0.9));
                    if self.audio.len() > 64 {
                        self.audio.clear();
                    }
                    self.audio.insert(key, a.clone());
                    a
                }
                None => {
                    self.status = format!("{}: {text}", t("Cannot encode", "Kodlanamıyor"));
                    self.enabled = false;
                    return;
                }
            },
        };
        let id = self.next_id;
        self.next_id += 1;
        let start_utc = target as f64 * slot_s + 0.5;
        e.digital.lock().unwrap().job = Some(DigitalJob { id, start_utc, max_late_s: max_late, whole_within_s: whole, audio });
        self.scheduled = Some(Scheduled { id, slot: target, text, step: self.seq.next, start_utc });
    }

    /// Background and text of a band activity row.
    fn colors(&self, text: &str) -> (Option<Color32>, Color32) {
        let me = &self.seq.my_call;
        match parse(text) {
            Parsed::Directed { to, .. } if !me.is_empty() && to == *me => (Some(TO_ME_BG()), ROW_TEXT()),
            Parsed::Directed { from, .. } | Parsed::Cq { from, .. } if !me.is_empty() && from == *me => (None, RX()),
            Parsed::Cq { from, grid } if self.alerts.dxcc_show && self.novelty(&from, grid.as_deref()).0 => {
                (Some(NEW_DXCC_BG()), ROW_TEXT())
            }
            Parsed::Cq { from, grid } if self.alerts.grid_show && self.novelty(&from, grid.as_deref()).1 => {
                (Some(NEW_SQUARE_BG()), ROW_TEXT())
            }
            Parsed::Cq { from, grid } => match self.worked.status(&from, grid.as_deref()) {
                Status::New => (Some(NEW_CQ_BG()), ROW_TEXT()),
                Status::NewGrid => (Some(NEW_GRID_BG()), ROW_TEXT()),
                Status::Worked => (None, WORKED()),
            },
            Parsed::Directed { from, payload: Payload::Grid(g), .. } => match self.worked.status(&from, Some(&g)) {
                Status::Worked => (None, WORKED()),
                Status::NewGrid => (None, Color32::from_rgb(0xb4, 0x9c, 0xff)),
                Status::New => (None, TEXT()),
            },
            _ => (None, TEXT()),
        }
    }

    /// Band activity: all decodes, by slot; a double click picks a station.
    pub fn activity(&mut self, ui: &mut egui::Ui, e: &Engine, p: &'static Protocol, tr: bool) {
        let t = |en: &'static str, tr_: &'static str| if tr { tr_ } else { en };
        // The legend follows the title, and goes on to the next line when narrow.
        ui.horizontal_wrapped(|ui| {
            ui::caption(ui, format!("{} · {}", p.name, t("BAND ACTIVITY", "BANT ETKİNLİĞİ")), BLUE());
            ui.add_space(8.0);
            ui::pill(ui, t("new", "yeni"), ROW_TEXT(), NEW_CQ_BG());
            ui::pill(ui, t("new locator", "yeni lokatör"), ROW_TEXT(), NEW_GRID_BG());
            ui::pill(ui, t("worked", "çalışılmış"), WORKED(), RAISED());
            ui::pill(ui, t("to me", "bana"), ROW_TEXT(), TO_ME_BG());
            if self.alerts.dxcc_show && self.cty.is_some() {
                ui::pill(ui, t("new DXCC", "yeni DXCC"), ROW_TEXT(), NEW_DXCC_BG());
            }
            if self.alerts.grid_show {
                ui::pill(ui, t("new square", "yeni kare"), ROW_TEXT(), NEW_SQUARE_BG());
            }
            if let Some((text, at)) = &self.banner
                && now_utc() - at < ALERT_SHOWN_S
            {
                ui.label(RichText::new(format!("★ {text}")).size(13.0).strong().color(alert_color()));
            }
        });
        let worker = if std::ptr::eq(p, &FT8) { &e.ft8 } else { &e.ft4 };
        let messages = worker.messages();
        let shown = &messages[messages.len().saturating_sub(SHOWN)..];
        let mut picked = None;
        egui::ScrollArea::vertical().auto_shrink([false, false]).stick_to_bottom(true).show(ui, |ui| {
            ui.spacing_mut().item_spacing.y = 0.0;
            let mut last_slot = None;
            for m in shown {
                if last_slot.is_some_and(|s| s != m.slot) {
                    let (r, _) = ui.allocate_exact_size(vec2(ui.available_width(), 5.0), Sense::hover());
                    ui.painter().line_segment([r.left_center(), r.right_center()], egui::Stroke::new(1.0, BORDER()));
                }
                last_slot = Some(m.slot);
                let (bg, fg) = self.colors(&m.text);
                let mut text = format!("{} {:+3.0} {:+4.1} {:4.0}  {}", hms(m.slot), m.snr_db, m.dt_s, m.audio_hz, m.text);
                if let (Parsed::Cq { from, .. }, Some(c)) = (parse(&m.text), &self.cty)
                    && let Some(entity) = c.entity(&from)
                {
                    text += &format!("   {}", entity.name);
                }
                let frame = egui::Frame::new().fill(bg.unwrap_or(Color32::TRANSPARENT)).corner_radius(CornerRadius::same(3)).inner_margin(egui::Margin::symmetric(4, 1));
                let r = frame
                    .show(ui, |ui| {
                        ui.set_width(ui.available_width());
                        // One line per message: what does not fit is cut off with "…".
                        ui.add(egui::Label::new(RichText::new(text).monospace().size(12.0).color(fg)).sense(Sense::click()).selectable(false).truncate())
                    })
                    .inner;
                if r.double_clicked() {
                    picked = Some(m.clone());
                }
                r.on_hover_text(t("Double click: call this station", "Çift tık: bu istasyonu çağır"));
            }
        });
        if let Some(m) = picked {
            if self.seq.my_call.is_empty() {
                self.status = t("Enter your call sign in the settings first", "Önce Ayarlar'dan çağrı işaretinizi girin").into();
                return;
            }
            let odd = ((m.slot as f64 / p.slot_s).round() as u64) % 2 == 1;
            let h = Heard { text: m.text.clone(), snr: m.snr_db.round() as i32, odd };
            if self.seq.pick(&h, now_utc() as u64) {
                self.enabled = true;
                self.last_tx_slot = None;
                self.status = format!("{} {}", t("Calling", "Çağrılıyor:"), self.seq.dx.as_ref().map_or("", |d| d.call.as_str()));
            }
        }
    }

    /// The contact's messages and what was sent.
    pub fn qso_pane(&mut self, ui: &mut egui::Ui, tr: bool) {
        ui::caption(ui, if tr { "QSO" } else { "QSO" }, RX());
        egui::ScrollArea::vertical().id_salt("qso_lines").auto_shrink([false, false]).stick_to_bottom(true).show(ui, |ui| {
            ui.spacing_mut().item_spacing.y = 1.0;
            for (l, c) in &self.qso_lines {
                ui.add(egui::Label::new(RichText::new(l).monospace().size(12.0).color(*c)).truncate());
            }
        });
    }

    /// DX, the six messages, and the buttons.
    pub fn controls(&mut self, ui: &mut egui::Ui, e: Option<&Engine>, p: &'static Protocol, armed: bool, tr: bool) {
        let t = |en: &'static str, tr_: &'static str| if tr { tr_ } else { en };
        let dx = self.seq.dx.clone();
        ui.horizontal(|ui| {
            ui::caption(ui, "DX", TX());
            match &dx {
                Some(d) => {
                    ui.label(RichText::new(format!("{} {}", d.call, d.grid.clone().unwrap_or_default())).monospace().size(14.0).color(STRONG()));
                    let rcvd = d.rcvd.map_or("—".to_string(), format_report);
                    let sent = d.sent.map_or("—".to_string(), format_report);
                    ui.label(RichText::new(format!("{} {sent} · {} {rcvd}", t("sent", "gön."), t("rcvd", "alı."))).size(11.0).color(MUTED()));
                }
                None => {
                    ui.label(RichText::new(t("— (CQ or double click a station)", "— (CQ ya da istasyona çift tık)")).size(11.0).color(DIM()));
                }
            }
        });
        ui.horizontal(|ui| {
            if ui::choice(ui, "CQ", self.seq.next == Step::Tx6 && !self.seq.done, BLUE()) {
                if self.seq.my_call.is_empty() {
                    self.status = t("Enter your call sign in the settings first", "Önce Ayarlar'dan çağrı işaretinizi girin").into();
                } else {
                    self.seq.call_cq(self.cq_odd);
                    self.enabled = true;
                    self.last_tx_slot = None;
                }
            }
            let (label, on) = if self.enabled { (t("Tx enabled", "Tx açık"), true) } else { (t("Enable Tx", "Tx'i aç"), false) };
            if ui::choice(ui, label, on, TX()) {
                self.enabled = !self.enabled;
                if self.enabled {
                    self.seq.done = false;
                }
            }
            if ui::choice(ui, t("Halt", "Durdur"), false, TX()) {
                self.halt(e);
            }
        });
        ui.horizontal(|ui| {
            ui.checkbox(&mut self.auto_seq, t("Auto seq", "Oto sıra"));
            ui.checkbox(&mut self.cq_odd, t("CQ in 2nd slot", "CQ 2. dilimde"))
                .on_hover_text(t("Odd slots (WSJT-X: Tx even/1st off)", "Tek dilimler (WSJT-X: Tx even/1st kapalı)"));
            ui.label(RichText::new("Tx").size(11.0).color(MUTED()));
            let max = 2_700.0 - signal_width(p);
            ui.add(egui::DragValue::new(&mut self.audio_hz).speed(5.0).range(300.0..=max).suffix(" Hz"));
        });
        ui.add_space(2.0);
        let mut chosen = None;
        for step in Step::ALL {
            let text = self.seq.message(step);
            let active = self.seq.next == step;
            // Without a partner only the CQ makes sense.
            let usable = step == Step::Tx6 || self.seq.dx.is_some();
            let label = format!("{:?}  {text}", step);
            let color = if active { STRONG() } else if usable { MUTED() } else { DIM() };
            let r = ui.add_enabled(usable, egui::Button::selectable(active, RichText::new(label).monospace().size(12.0).color(color)));
            if r.clicked() {
                chosen = Some(step);
            }
        }
        if let Some(step) = chosen {
            self.seq.next = step;
            self.seq.repeats = 0;
            self.seq.done = false;
            if step == Step::Tx6 {
                self.seq.tx_odd = self.cq_odd;
            }
        }
        ui.add_space(2.0);
        let now = now_utc();
        let state = match (&self.scheduled, armed, self.enabled) {
            (_, false, true) => t("Switch TX on (right) to transmit", "Yayın için sağdaki TX'i açın").to_string(),
            (Some(s), _, _) if s.start_utc > now => format!("{} {:?} · {:.0} s", t("Next", "Sıradaki"), s.step, s.start_utc - now),
            (Some(s), _, _) => format!("{} {:?}", t("Starting", "Başlıyor"), s.step),
            (None, _, true) if self.seq.done => t("Sequence finished", "Sıra bitti").to_string(),
            _ => String::new(),
        };
        let st = if self.status.is_empty() { state } else if state.is_empty() { self.status.clone() } else { format!("{state} · {}", self.status) };
        ui.label(RichText::new(st).size(11.0).color(RX()));
        let _ = e;
    }
}
