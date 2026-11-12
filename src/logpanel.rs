//! The log window: the contacts of the ADIF file, newest at the bottom, a form for
//! entering SSB and CW contacts by hand, and a double click on a row to correct it.

use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use eframe::egui::{self, RichText, Sense, vec2};

use qo_all::adif::{Log, Record, append_record};

use crate::ui::{self, palette::*};

/// What is entered or corrected.
#[derive(Clone, Debug, Default, PartialEq)]
struct Form {
    call: String,
    /// "2026-11-09" and "14:05" (UTC).
    date: String,
    time: String,
    mode: String,
    sent: String,
    rcvd: String,
    name: String,
    grid: String,
}

/// Where the form came from: a new contact, or a record of the log (its index).
#[derive(Clone, Copy, Debug, PartialEq)]
enum Editing {
    New,
    Record(usize),
}

/// Station details and frequencies a new contact is logged with.
pub struct Station<'a> {
    pub my_call: &'a str,
    pub my_grid: &'a str,
    /// Mode in use ("SSB", "CW", "FT8", "FT4") and the default reports for it.
    pub mode: &'a str,
    /// Uplink and downlink of the dial (Hz).
    pub tx_hz: f64,
    pub rx_hz: f64,
}

pub struct LogPanel {
    path: PathBuf,
    log: Log,
    /// When the file was read (it is read again when it changes).
    read: Option<SystemTime>,
    last_check: f64,
    editing: Option<(Editing, Form)>,
    delete_asked: bool,
    filter: String,
    /// The file was saved as it was (".bak") before the first change this run.
    backed_up: bool,
    status: String,
    /// Set when the log changed here (the window reloads the worked stations).
    pub changed: bool,
}

fn now_utc() -> f64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map_or(0.0, |d| d.as_secs_f64())
}

fn modified(path: &Path) -> Option<SystemTime> {
    std::fs::metadata(path).and_then(|m| m.modified()).ok()
}

/// "20261109" → "2026-11-09", "140512" → "14:05".
fn show_date(d: &str) -> String {
    if d.len() == 8 { format!("{}-{}-{}", &d[..4], &d[4..6], &d[6..]) } else { d.to_string() }
}

fn show_time(t: &str) -> String {
    if t.len() >= 4 { format!("{}:{}", &t[..2], &t[2..4]) } else { t.to_string() }
}

/// "2026-11-09" or "20261109" → "20261109", if it is a date.
fn adif_date(text: &str) -> Option<String> {
    let d: String = text.chars().filter(|c| c.is_ascii_digit()).collect();
    let (y, m, day) = (d.get(..4)?.parse::<u32>().ok()?, d.get(4..6)?.parse::<u32>().ok()?, d.get(6..8)?.parse::<u32>().ok()?);
    (d.len() == 8 && (1930..=2100).contains(&y) && (1..=12).contains(&m) && (1..=31).contains(&day)).then_some(d)
}

/// "14:05", "1405" or "140512" → "1405" / "140512", if it is a time.
fn adif_time(text: &str) -> Option<String> {
    let t: String = text.chars().filter(|c| c.is_ascii_digit()).collect();
    let ok = |s: &str, max: u32| s.parse::<u32>().is_ok_and(|v| v <= max);
    match t.len() {
        4 if ok(&t[..2], 23) && ok(&t[2..], 59) => Some(t),
        6 if ok(&t[..2], 23) && ok(&t[2..4], 59) && ok(&t[4..], 59) => Some(t),
        _ => None,
    }
}

fn valid_call(call: &str) -> bool {
    let c = call.trim();
    (3..=12).contains(&c.len()) && c.chars().all(|ch| ch.is_ascii_alphanumeric() || ch == '/') && c.chars().any(|ch| ch.is_ascii_digit())
}

/// Today's date and the time now, as the form shows them.
fn now_form() -> (String, String) {
    let secs = now_utc() as u64;
    let days = (secs / 86_400) as i64;
    // Days to civil date (Howard Hinnant's algorithm).
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + if m <= 2 { 1 } else { 0 };
    let s = secs % 86_400;
    (format!("{y:04}-{m:02}-{d:02}"), format!("{:02}:{:02}", s / 3600, s / 60 % 60))
}

fn default_report(mode: &str) -> &'static str {
    match mode {
        "CW" => "599",
        "FT8" | "FT4" => "-10",
        _ => "59",
    }
}

impl LogPanel {
    pub fn new(path: PathBuf) -> Self {
        let mut p = Self {
            path,
            log: Log::default(),
            read: None,
            last_check: 0.0,
            editing: None,
            delete_asked: false,
            filter: String::new(),
            backed_up: false,
            status: String::new(),
            changed: false,
        };
        p.reload();
        p
    }

    /// Another log file (from the settings).
    pub fn set_path(&mut self, path: PathBuf) {
        if path != self.path {
            self.path = path;
            self.backed_up = false;
            self.editing = None;
            self.reload();
        }
    }

    fn reload(&mut self) {
        match Log::load(&self.path) {
            Ok(log) => {
                self.log = log;
                self.read = modified(&self.path);
            }
            Err(e) => self.status = e,
        }
    }

    /// Reads the file again when something else (the FT8 sequence, another program)
    /// changed it; checked once a second.
    fn follow_file(&mut self) {
        let now = now_utc();
        if now - self.last_check < 1.0 {
            return;
        }
        self.last_check = now;
        if modified(&self.path) != self.read && self.editing.is_none() {
            self.reload();
        }
    }

    fn form_of(r: &Record) -> Form {
        Form {
            call: r.get("CALL").unwrap_or_default().to_string(),
            date: show_date(r.get("QSO_DATE").unwrap_or_default()),
            time: show_time(r.get("TIME_ON").unwrap_or_default()),
            mode: r.mode(),
            sent: r.get("RST_SENT").unwrap_or_default().to_string(),
            rcvd: r.get("RST_RCVD").unwrap_or_default().to_string(),
            name: r.get("NAME").unwrap_or_default().to_string(),
            grid: r.get("GRIDSQUARE").unwrap_or_default().to_string(),
        }
    }

    /// Puts the form into a record (new or the one corrected). None, with the reason
    /// in the status, when something is not right.
    fn fill(&mut self, r: &mut Record, f: &Form, tr: bool) -> Option<()> {
        let t = |en: &'static str, tr_: &'static str| if tr { tr_ } else { en };
        let call = f.call.trim().to_ascii_uppercase();
        if !valid_call(&call) {
            self.status = t("Enter a valid call sign", "Geçerli bir çağrı işareti girin").into();
            return None;
        }
        let Some(date) = adif_date(&f.date) else {
            self.status = t("Date as 2026-11-09", "Tarih 2026-11-09 biçiminde").into();
            return None;
        };
        let Some(time) = adif_time(&f.time) else {
            self.status = t("Time (UTC) as 14:05", "Saat (UTC) 14:05 biçiminde").into();
            return None;
        };
        let grid = f.grid.trim().to_ascii_uppercase();
        if !grid.is_empty() && !(grid.len() == 4 || grid.len() == 6) {
            self.status = t("Locator: 4 or 6 characters", "Lokatör: 4 ya da 6 karakter").into();
            return None;
        }
        r.set("CALL", &call);
        r.set("QSO_DATE", &date);
        r.set("TIME_ON", &time);
        if r.get("QSO_DATE_OFF").is_none() {
            r.set("QSO_DATE_OFF", &date);
            r.set("TIME_OFF", &time);
        }
        match f.mode.trim().to_ascii_uppercase().as_str() {
            "FT4" => {
                r.set("MODE", "MFSK");
                r.set("SUBMODE", "FT4");
            }
            m => {
                r.set("MODE", m);
                r.set("SUBMODE", "");
            }
        }
        r.set("RST_SENT", &f.sent);
        r.set("RST_RCVD", &f.rcvd);
        r.set("NAME", &f.name);
        r.set("GRIDSQUARE", &grid);
        Some(())
    }

    fn save_form(&mut self, which: Editing, f: &Form, st: &Station, tr: bool) -> bool {
        let t = |en: &'static str, tr_: &'static str| if tr { tr_ } else { en };
        match which {
            Editing::New => {
                let mut r = Record::default();
                if self.fill(&mut r, f, tr).is_none() {
                    return false;
                }
                r.set("BAND", "13cm");
                r.set("FREQ", &format!("{:.6}", st.tx_hz / 1e6));
                r.set("BAND_RX", "3cm");
                r.set("FREQ_RX", &format!("{:.6}", st.rx_hz / 1e6));
                r.set("PROP_MODE", "SAT");
                r.set("SAT_NAME", "QO-100");
                r.set("STATION_CALLSIGN", st.my_call);
                r.set("MY_GRIDSQUARE", st.my_grid);
                match append_record(&self.path, &r) {
                    Ok(()) => self.status = format!("{} {}", t("Logged", "Loglandı:"), r.get("CALL").unwrap_or_default()),
                    Err(e) => {
                        self.status = e;
                        return false;
                    }
                }
            }
            Editing::Record(i) => {
                let Some(mut r) = self.log.records.get(i).cloned() else { return false };
                if self.fill(&mut r, f, tr).is_none() {
                    return false;
                }
                // The file may have changed meanwhile: the correction goes into what is
                // there now, at the same record.
                if let Ok(fresh) = Log::load(&self.path)
                    && fresh.records.len() == self.log.records.len()
                {
                    self.log = fresh;
                }
                self.log.records[i] = r;
                match self.log.save(&self.path, !self.backed_up) {
                    Ok(()) => {
                        self.backed_up = true;
                        self.status = t("Corrected", "Düzeltildi").into();
                    }
                    Err(e) => {
                        self.status = e;
                        return false;
                    }
                }
            }
        }
        self.changed = true;
        self.reload();
        true
    }

    fn delete(&mut self, i: usize, tr: bool) {
        if i < self.log.records.len() {
            self.log.records.remove(i);
            match self.log.save(&self.path, !self.backed_up) {
                Ok(()) => {
                    self.backed_up = true;
                    self.status = if tr { "Silindi".into() } else { "Deleted".into() };
                    self.changed = true;
                }
                Err(e) => self.status = e,
            }
            self.reload();
        }
    }

    pub fn ui(&mut self, ui: &mut egui::Ui, st: &Station, tr: bool) {
        let t = |en: &'static str, tr_: &'static str| if tr { tr_ } else { en };
        self.follow_file();
        if let Some((which, mut form)) = self.editing.clone() {
            self.form_ui(ui, which, &mut form, st, tr);
            return;
        }
        ui.horizontal(|ui| {
            ui::caption(ui, format!("LOG · {} QSO", self.log.records.len()), BLUE());
            if ui::choice(ui, t("New QSO", "Yeni QSO"), false, GOOD()) {
                let (date, time) = now_form();
                let rpt = default_report(st.mode).to_string();
                self.editing = Some((Editing::New, Form { date, time, mode: st.mode.to_string(), sent: rpt.clone(), rcvd: rpt, ..Form::default() }));
                self.status.clear();
            }
            ui.add(egui::TextEdit::singleline(&mut self.filter).desired_width(110.0).hint_text(t("search call", "çağrı ara")));
            if !self.status.is_empty() {
                ui.label(RichText::new(&self.status).size(11.0).color(RX()));
            }
        });
        let filter = self.filter.trim().to_ascii_uppercase();
        let shown: Vec<usize> =
            (0..self.log.records.len()).filter(|&i| filter.is_empty() || self.log.records[i].get("CALL").unwrap_or_default().to_ascii_uppercase().contains(&filter)).collect();
        let mut picked = None;
        let widths = [86.0, 46.0, 96.0, 44.0, 44.0, 44.0, 110.0, 64.0];
        let head = [t("Date", "Tarih"), "UTC", t("Call", "Çağrı"), t("Mode", "Mod"), t("Sent", "Ver."), t("Rcvd", "Alı."), t("Name", "İsim"), t("Locator", "Lokatör")];
        let row = |ui: &mut egui::Ui, cells: [String; 8], strong: bool| {
            let mut resp: Option<egui::Response> = None;
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 6.0;
                for (cell, w) in cells.iter().zip(widths) {
                    let text = RichText::new(cell).monospace().size(12.0).color(if strong { MUTED() } else { TEXT() });
                    let r = ui.add_sized(vec2(w, 16.0), egui::Label::new(text).truncate().sense(Sense::click()).selectable(false));
                    resp = Some(match resp.take() {
                        Some(p) => p.union(r),
                        None => r,
                    });
                }
            });
            resp.expect("cells")
        };
        row(ui, head.map(str::to_string), true);
        egui::ScrollArea::vertical().id_salt("log_rows").auto_shrink([false, false]).stick_to_bottom(true).show(ui, |ui| {
            ui.spacing_mut().item_spacing.y = 1.0;
            for &i in &shown {
                let r = &self.log.records[i];
                let cells = [
                    show_date(r.get("QSO_DATE").unwrap_or_default()),
                    show_time(r.get("TIME_ON").unwrap_or_default()),
                    r.get("CALL").unwrap_or_default().to_string(),
                    r.mode(),
                    r.get("RST_SENT").unwrap_or_default().to_string(),
                    r.get("RST_RCVD").unwrap_or_default().to_string(),
                    r.get("NAME").unwrap_or_default().to_string(),
                    r.get("GRIDSQUARE").unwrap_or_default().to_string(),
                ];
                let resp = row(ui, cells, false).on_hover_text(t("Double click: correct", "Çift tık: düzelt"));
                if resp.double_clicked() {
                    picked = Some(i);
                }
            }
        });
        if let Some(i) = picked {
            self.editing = Some((Editing::Record(i), Self::form_of(&self.log.records[i])));
            self.delete_asked = false;
            self.status.clear();
        }
    }

    fn form_ui(&mut self, ui: &mut egui::Ui, which: Editing, form: &mut Form, st: &Station, tr: bool) {
        let t = |en: &'static str, tr_: &'static str| if tr { tr_ } else { en };
        let title = match which {
            Editing::New => t("NEW QSO", "YENİ QSO"),
            Editing::Record(_) => t("CORRECT QSO", "QSO DÜZELT"),
        };
        ui::caption(ui, title, GOOD());
        // Two rows, so it fits the log's strip at the bottom of the window.
        // Large fields with a large font: easy to type into during a QSO.
        egui::Grid::new("log_form").num_columns(8).spacing(vec2(12.0, 8.0)).show(ui, |ui| {
            let field = |ui: &mut egui::Ui, label: &str, value: &mut String, w: f32| {
                ui.label(RichText::new(label).size(14.0).color(MUTED()));
                // Fixed size: an empty field in a grid would otherwise shrink.
                ui.add_sized(
                    vec2(w, 34.0),
                    egui::TextEdit::singleline(value).font(egui::FontId::proportional(18.0)).margin(vec2(8.0, 6.0)).desired_width(w),
                );
            };
            field(ui, t("Call", "Çağrı"), &mut form.call, 190.0);
            field(ui, t("Name", "İsim"), &mut form.name, 230.0);
            field(ui, t("Date", "Tarih"), &mut form.date, 150.0);
            field(ui, "UTC", &mut form.time, 100.0);
            ui.end_row();
            field(ui, t("Sent", "Verilen"), &mut form.sent, 190.0);
            field(ui, t("Rcvd", "Alınan"), &mut form.rcvd, 230.0);
            ui.label(RichText::new(t("Mode", "Mod")).size(14.0).color(MUTED()));
            egui::ComboBox::from_id_salt("log_mode")
                .selected_text(RichText::new(&form.mode).size(18.0))
                .width(150.0)
                .height(240.0)
                .show_ui(ui, |ui| {
                    for m in ["SSB", "CW", "FT8", "FT4"] {
                        ui.selectable_value(&mut form.mode, m.to_string(), RichText::new(m).size(16.0));
                    }
                });
            field(ui, t("Locator", "Lokatör"), &mut form.grid, 100.0);
            ui.end_row();
        });
        ui.add_space(4.0);
        ui.horizontal(|ui| {
            if ui::choice(ui, t("Save", "Kaydet"), false, GOOD()) && self.save_form(which, form, st, tr) {
                self.editing = None;
                return;
            }
            if ui::choice(ui, t("Cancel", "Vazgeç"), false, MUTED()) {
                self.editing = None;
                self.status.clear();
                return;
            }
            if let Editing::Record(i) = which {
                let label = if self.delete_asked { t("Delete for good?", "Kesin silinsin mi?") } else { t("Delete", "Sil") };
                if ui::choice(ui, label, self.delete_asked, TX()) {
                    if self.delete_asked {
                        self.delete(i, tr);
                        self.editing = None;
                        return;
                    }
                    self.delete_asked = true;
                }
            }
            if !self.status.is_empty() {
                ui.label(RichText::new(&self.status).size(11.0).color(RX()));
            }
        });
        if let Some((_, f)) = self.editing.as_mut() {
            *f = form.clone();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dates_and_times() {
        assert_eq!(adif_date("2026-11-09").as_deref(), Some("20261109"));
        assert_eq!(adif_date("20261109").as_deref(), Some("20261109"));
        assert_eq!(adif_date("2026-13-09"), None);
        assert_eq!(adif_time("14:05").as_deref(), Some("1405"));
        assert_eq!(adif_time("140512").as_deref(), Some("140512"));
        assert_eq!(adif_time("25:00"), None);
        assert_eq!(show_date("20261109"), "2026-11-09");
        assert_eq!(show_time("140512"), "14:05");
        assert!(valid_call("TA4SO") && valid_call("PA/F4MFS") && !valid_call("HELLO") && !valid_call("T4"));
        let (d, t) = now_form();
        assert!(adif_date(&d).is_some() && adif_time(&t).is_some());
    }

    #[test]
    fn new_and_corrected_contacts() {
        let dir = std::env::temp_dir().join(format!("qoall-logpanel-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("log.adi");
        let mut p = LogPanel::new(path.clone());
        let st = Station { my_call: "TA4SO", my_grid: "KM47", mode: "SSB", tx_hz: 2_400.225e6, rx_hz: 10_489.725e6 };
        let form = Form {
            call: "dl1abc".into(),
            date: "2026-11-09".into(),
            time: "14:05".into(),
            mode: "SSB".into(),
            sent: "59".into(),
            rcvd: "57".into(),
            name: "Hans".into(),
            grid: "jo62".into(),
        };
        assert!(p.save_form(Editing::New, &form, &st, false));
        assert_eq!(p.log.records.len(), 1);
        let r = &p.log.records[0];
        assert_eq!((r.get("CALL"), r.get("GRIDSQUARE"), r.get("TIME_ON"), r.get("SAT_NAME")), (Some("DL1ABC"), Some("JO62"), Some("1405"), Some("QO-100")));
        // Correct it: the name and the report.
        let mut f2 = LogPanel::form_of(r);
        f2.name = "Hans-Peter".into();
        f2.rcvd = "55".into();
        assert!(p.save_form(Editing::Record(0), &f2, &st, false));
        assert_eq!(p.log.records[0].get("NAME"), Some("Hans-Peter"));
        assert_eq!(p.log.records[0].get("FREQ_RX"), Some("10489.725000"), "the rest stays");
        assert!(dir.join("log.adi.bak").is_file());
        // A wrong entry is refused.
        assert!(!p.save_form(Editing::New, &Form { call: "X".into(), ..form }, &st, false));
        p.delete(0, false);
        assert!(p.log.records.is_empty());
        let _ = std::fs::remove_dir_all(dir);
    }
}
