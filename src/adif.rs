//! The log: finished contacts appended to an ADIF file (which other programs may share,
//! e.g. WSJT-X's wsjtx_log.adi), and the stations in it, for colouring the decodes.

use std::collections::{HashMap, HashSet};
use std::io::Write;
use std::path::{Path, PathBuf};

/// A contact to log.
#[derive(Clone, Debug, PartialEq)]
pub struct Contact {
    pub call: String,
    pub grid: Option<String>,
    /// "FT8", "FT4", "SSB", "CW".
    pub mode: String,
    pub rst_sent: String,
    pub rst_rcvd: String,
    /// Start and end (Unix seconds, UTC).
    pub start: u64,
    pub end: u64,
    /// Uplink and downlink frequencies (Hz).
    pub tx_hz: f64,
    pub rx_hz: f64,
    pub my_call: String,
    pub my_grid: String,
    /// The other operator's name, if given.
    pub name: Option<String>,
}

fn field(name: &str, value: &str) -> String {
    format!("<{name}:{}>{value} ", value.len())
}

/// Calendar date and time of Unix seconds: ("YYYYMMDD", "HHMMSS").
fn date_time(secs: u64) -> (String, String) {
    // Days to civil date (Howard Hinnant's algorithm).
    let days = (secs / 86_400) as i64;
    let s = secs % 86_400;
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + if m <= 2 { 1 } else { 0 };
    (format!("{y:04}{m:02}{d:02}"), format!("{:02}{:02}{:02}", s / 3600, s / 60 % 60, s % 60))
}

/// One ADIF record, with the satellite fields LoTW expects for QO-100.
pub fn record(c: &Contact) -> String {
    let (date_on, time_on) = date_time(c.start);
    let (date_off, time_off) = date_time(c.end);
    let mut r = String::new();
    r += &field("CALL", &c.call);
    if let Some(g) = &c.grid {
        r += &field("GRIDSQUARE", g);
    }
    if let Some(n) = c.name.as_ref().filter(|n| !n.trim().is_empty()) {
        r += &field("NAME", n.trim());
    }
    match c.mode.as_str() {
        "FT4" => {
            r += &field("MODE", "MFSK");
            r += &field("SUBMODE", "FT4");
        }
        m => r += &field("MODE", m),
    }
    r += &field("RST_SENT", &c.rst_sent);
    r += &field("RST_RCVD", &c.rst_rcvd);
    r += &field("QSO_DATE", &date_on);
    r += &field("TIME_ON", &time_on);
    r += &field("QSO_DATE_OFF", &date_off);
    r += &field("TIME_OFF", &time_off);
    r += &field("BAND", "13cm");
    r += &field("FREQ", &format!("{:.6}", c.tx_hz / 1e6));
    r += &field("BAND_RX", "3cm");
    r += &field("FREQ_RX", &format!("{:.6}", c.rx_hz / 1e6));
    r += &field("PROP_MODE", "SAT");
    r += &field("SAT_NAME", "QO-100");
    r += &field("STATION_CALLSIGN", &c.my_call);
    r += &field("MY_GRIDSQUARE", &c.my_grid);
    r += "<EOR>\n";
    r
}

/// Appends a contact; a new (or empty) file gets an ADIF header first.
pub fn append(path: &Path, c: &Contact) -> Result<(), String> {
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let fresh = std::fs::metadata(path).map_or(true, |m| m.len() == 0);
    let mut f = std::fs::OpenOptions::new().create(true).append(true).open(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let mut text = String::new();
    if fresh {
        text += &format!("QO-ALL ADIF log\n{}{}<EOH>\n", field("ADIF_VER", "3.1.4"), field("PROGRAMID", "QO-ALL"));
    }
    text += &record(c);
    f.write_all(text.as_bytes()).map_err(|e| format!("{}: {e}", path.display()))
}

/// One record, its fields in their order (names in upper case).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Record(pub Vec<(String, String)>);

impl Record {
    pub fn get(&self, name: &str) -> Option<&str> {
        self.0.iter().find(|(n, _)| n == name).map(|(_, v)| v.as_str())
    }

    /// Sets a field (an empty value removes it); a new field goes at the end.
    pub fn set(&mut self, name: &str, value: &str) {
        let value = value.trim();
        match self.0.iter().position(|(n, _)| n == name) {
            Some(i) if value.is_empty() => {
                self.0.remove(i);
            }
            Some(i) => self.0[i].1 = value.to_string(),
            None if value.is_empty() => {}
            None => self.0.push((name.to_string(), value.to_string())),
        }
    }

    pub fn text(&self) -> String {
        let mut r: String = self.0.iter().map(|(n, v)| field(n, v)).collect();
        r += "<EOR>\n";
        r
    }

    /// "FT8", "FT4" (MFSK/FT4), "SSB", "CW"...
    pub fn mode(&self) -> String {
        match (self.get("MODE"), self.get("SUBMODE")) {
            (Some("MFSK"), Some(sub)) => sub.to_string(),
            (Some(m), _) => m.to_string(),
            (None, _) => String::new(),
        }
    }
}

/// A whole ADIF file: its header kept as it was, and its records.
#[derive(Clone, Debug, Default)]
pub struct Log {
    pub header: String,
    pub records: Vec<Record>,
}

impl Log {
    pub fn parse(text: &str) -> Self {
        let (header, body) = match text.to_ascii_uppercase().find("<EOH>") {
            Some(i) => (text[..i + 5].to_string(), &text[i + 5..]),
            None => (String::new(), text),
        };
        let mut records = Vec::new();
        let mut current = Vec::new();
        let mut rest = body;
        while let Some(open) = rest.find('<') {
            let Some(close) = rest[open..].find('>') else { break };
            let tag = &rest[open + 1..open + close];
            rest = &rest[open + close + 1..];
            let mut parts = tag.split(':');
            let name = parts.next().unwrap_or_default().trim().to_ascii_uppercase();
            if name == "EOR" {
                records.push(Record(std::mem::take(&mut current)));
                continue;
            }
            let Some(len) = parts.next().and_then(|l| l.trim().parse::<usize>().ok()) else { continue };
            let mut end = len.min(rest.len());
            while !rest.is_char_boundary(end) {
                end -= 1;
            }
            current.push((name, rest[..end].to_string()));
            rest = &rest[end..];
        }
        Self { header, records }
    }

    /// The log of a file; an empty one if there is none yet.
    pub fn load(path: &Path) -> Result<Self, String> {
        match std::fs::read(path) {
            Ok(bytes) => Ok(Self::parse(&String::from_utf8_lossy(&bytes))),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Self::default()),
            Err(e) => Err(format!("{}: {e}", path.display())),
        }
    }

    pub fn text(&self) -> String {
        let header = if self.header.is_empty() {
            format!("QO-ALL ADIF log\n{}{}<EOH>", field("ADIF_VER", "3.1.4"), field("PROGRAMID", "QO-ALL"))
        } else {
            self.header.clone()
        };
        let mut out = header + "\n";
        for r in &self.records {
            out += &r.text();
        }
        out
    }

    /// Writes the log back. The file as it was is kept once beside it (".bak") before
    /// the first change of a run; the new one goes in through a temporary file, so a
    /// failure never leaves half a log.
    pub fn save(&self, path: &Path, keep_backup: bool) -> Result<(), String> {
        let err = |e: std::io::Error| format!("{}: {e}", path.display());
        if keep_backup && path.is_file() {
            let mut bak = path.as_os_str().to_owned();
            bak.push(".bak");
            std::fs::copy(path, PathBuf::from(bak)).map_err(err)?;
        }
        let mut tmp = path.as_os_str().to_owned();
        tmp.push(".tmp");
        let tmp = PathBuf::from(tmp);
        std::fs::write(&tmp, self.text()).map_err(err)?;
        std::fs::rename(&tmp, path).map_err(err)
    }
}

/// Appends a record; a new (or empty) file gets an ADIF header first.
pub fn append_record(path: &Path, r: &Record) -> Result<(), String> {
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let fresh = std::fs::metadata(path).map_or(true, |m| m.len() == 0);
    let mut f = std::fs::OpenOptions::new().create(true).append(true).open(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let mut text = String::new();
    if fresh {
        text += &format!("QO-ALL ADIF log
{}{}<EOH>
", field("ADIF_VER", "3.1.4"), field("PROGRAMID", "QO-ALL"));
    }
    text += &r.text();
    f.write_all(text.as_bytes()).map_err(|e| format!("{}: {e}", path.display()))
}

/// Records of an ADIF text: field name (upper case) → value.
pub fn parse(text: &str) -> Vec<HashMap<String, String>> {
    let mut records = Vec::new();
    let mut current = HashMap::new();
    let mut rest = text;
    // A header ends with <EOH>; without one the records start at once.
    if let Some(i) = rest.to_ascii_uppercase().find("<EOH>") {
        rest = &rest[i + 5..];
    }
    while let Some(open) = rest.find('<') {
        let Some(close) = rest[open..].find('>') else { break };
        let tag = &rest[open + 1..open + close];
        rest = &rest[open + close + 1..];
        let mut parts = tag.split(':');
        let name = parts.next().unwrap_or_default().trim().to_ascii_uppercase();
        if name == "EOR" {
            records.push(std::mem::take(&mut current));
            continue;
        }
        let Some(len) = parts.next().and_then(|l| l.trim().parse::<usize>().ok()) else { continue };
        // The length counts bytes; a value cut inside a character is taken up to it.
        let mut end = len.min(rest.len());
        while !rest.is_char_boundary(end) {
            end -= 1;
        }
        current.insert(name, rest[..end].to_string());
        rest = &rest[end..];
    }
    records
}

/// How a station stands against the log.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Status {
    /// Never worked.
    New,
    /// Worked, from this grid (or the grid is not known).
    Worked,
    /// Worked, but from another grid: a new locator.
    NewGrid,
}

/// The stations of the log and the grids each was worked from.
#[derive(Clone, Debug, Default)]
pub struct Worked {
    calls: HashMap<String, HashSet<String>>,
    /// Every grid square worked, whoever from.
    grids: HashSet<String>,
}

fn grid4(g: &str) -> String {
    g.trim().chars().take(4).collect::<String>().to_ascii_uppercase()
}

impl Worked {
    /// The stations of an ADIF file (empty if it cannot be read).
    pub fn load(path: &Path) -> Self {
        let mut w = Self::default();
        if let Ok(text) = std::fs::read(path) {
            for r in parse(&String::from_utf8_lossy(&text)) {
                if let Some(call) = r.get("CALL") {
                    w.add(call, r.get("GRIDSQUARE").map(String::as_str));
                }
            }
        }
        w
    }

    pub fn add(&mut self, call: &str, grid: Option<&str>) {
        let set = self.calls.entry(call.trim().to_ascii_uppercase()).or_default();
        if let Some(g) = grid.filter(|g| g.trim().len() >= 4) {
            set.insert(grid4(g));
            self.grids.insert(grid4(g));
        }
    }

    pub fn status(&self, call: &str, grid: Option<&str>) -> Status {
        match self.calls.get(&call.trim().to_ascii_uppercase()) {
            None => Status::New,
            Some(grids) => match grid.filter(|g| g.trim().len() >= 4) {
                Some(g) if !grids.is_empty() && !grids.contains(&grid4(g)) => Status::NewGrid,
                _ => Status::Worked,
            },
        }
    }

    pub fn len(&self) -> usize {
        self.calls.len()
    }

    /// Whether a grid square (its first four characters) was ever worked.
    pub fn grid_worked(&self, grid: &str) -> bool {
        self.grids.contains(&grid4(grid))
    }

    /// The calls in the log.
    pub fn calls(&self) -> impl Iterator<Item = &str> {
        self.calls.keys().map(String::as_str)
    }

    pub fn is_empty(&self) -> bool {
        self.calls.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn contact() -> Contact {
        Contact {
            call: "DL1ABC".into(),
            grid: Some("JO62".into()),
            mode: "FT8".into(),
            rst_sent: "-12".into(),
            rst_rcvd: "-09".into(),
            start: 1_794_067_200 + 3_600 + 62,
            end: 1_794_067_200 + 3_600 + 125,
            tx_hz: 2_400.041_5e6,
            rx_hz: 10_489.541_5e6,
            my_call: "TA4SO".into(),
            my_grid: "KM46".into(),
            name: None,
        }
    }

    #[test]
    fn dates() {
        assert_eq!(date_time(0), ("19700101".into(), "000000".into()));
        // 2026-11-07 18:10:32 UTC.
        assert_eq!(date_time(1_794_075_032), ("20261107".into(), "181032".into()));
        assert_eq!(date_time(951_782_400), ("20000229".into(), "000000".into()));
    }

    #[test]
    fn record_has_the_satellite_fields() {
        let r = record(&contact());
        for f in ["<CALL:6>DL1ABC", "<GRIDSQUARE:4>JO62", "<MODE:3>FT8", "<RST_SENT:3>-12", "<TIME_ON:6>170102", "<BAND:4>13cm", "<FREQ:11>2400.041500", "<BAND_RX:3>3cm", "<FREQ_RX:12>10489.541500", "<PROP_MODE:3>SAT", "<SAT_NAME:6>QO-100", "<STATION_CALLSIGN:5>TA4SO", "<EOR>"] {
            assert!(r.contains(f), "{f} missing in {r}");
        }
        let ft4 = record(&Contact { mode: "FT4".into(), ..contact() });
        assert!(ft4.contains("<MODE:4>MFSK") && ft4.contains("<SUBMODE:3>FT4"));
    }

    #[test]
    fn appended_file_reads_back() {
        let dir = std::env::temp_dir().join(format!("qoall-adif-{}", std::process::id()));
        let path = dir.join("log.adi");
        let _ = std::fs::remove_file(&path);
        append(&path, &contact()).unwrap();
        append(&path, &Contact { call: "EA7HJ".into(), grid: None, ..contact() }).unwrap();
        let text = std::fs::read_to_string(&path).unwrap();
        assert_eq!(text.matches("<EOH>").count(), 1);
        let records = parse(&text);
        assert_eq!(records.len(), 2);
        assert_eq!(records[0]["CALL"], "DL1ABC");
        assert_eq!(records[1]["FREQ_RX"], "10489.541500");
        let w = Worked::load(&path);
        assert_eq!(w.status("dl1abc", Some("JO62ab")), Status::Worked);
        assert_eq!(w.status("DL1ABC", Some("JO63")), Status::NewGrid);
        assert_eq!(w.status("DL1ABC", None), Status::Worked);
        assert_eq!(w.status("EA7HJ", Some("IM76")), Status::Worked, "no grid in the log: nothing to compare");
        assert_eq!(w.status("OE3XYZ", Some("JN88")), Status::New);
        assert!(w.grid_worked("jo62") && !w.grid_worked("JN88"));
        assert_eq!(w.calls().count(), 2);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn log_edits_keep_the_rest() {
        let text = "WSJT-X ADIF Export<eoh>\n<call:5>SP9UNU <gridsquare:4>JO90 <mode:3>FT8 <app_x:3>abc <eor>\n<call:6>DL1ABC <mode:3>SSB <rst_sent:2>59 <eor>\n";
        let mut log = Log::parse(text);
        assert_eq!(log.header, "WSJT-X ADIF Export<eoh>");
        assert_eq!(log.records.len(), 2);
        assert_eq!(log.records[0].mode(), "FT8");
        log.records[1].set("NAME", "Hans");
        log.records[1].set("RST_SENT", "57");
        log.records[0].set("GRIDSQUARE", "");
        let again = Log::parse(&log.text());
        assert_eq!(again.records[1].get("NAME"), Some("Hans"));
        assert_eq!(again.records[1].get("RST_SENT"), Some("57"));
        assert_eq!(again.records[0].get("GRIDSQUARE"), None);
        assert_eq!(again.records[0].get("APP_X"), Some("abc"), "unknown fields stay");
        assert!(again.text().starts_with("WSJT-X ADIF Export<eoh>"));
        // Written through a temporary file, with a backup of the old one.
        let dir = std::env::temp_dir().join(format!("qoall-log-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("log.adi");
        std::fs::write(&path, text).unwrap();
        log.save(&path, true).unwrap();
        assert_eq!(std::fs::read_to_string(dir.join("log.adi.bak")).unwrap(), text);
        assert_eq!(Log::load(&path).unwrap().records[1].get("NAME"), Some("Hans"));
        assert!(Log::load(&dir.join("none.adi")).unwrap().records.is_empty());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn name_goes_into_the_record() {
        let r = record(&Contact { mode: "SSB".into(), name: Some("Hans".into()), ..contact() });
        assert!(r.contains("<NAME:4>Hans") && r.contains("<MODE:3>SSB"));
    }

    #[test]
    fn reads_a_wsjtx_style_log() {
        let text = "WSJT-X ADIF Export<eoh>\n<call:5>SP9UNU <gridsquare:4>JO90 <mode:3>FT8 <qso_date:8>20261101 <eor>\n<call:6>R4FDW/P <mode:4>MFSK <submode:3>FT4 <eor>\n";
        let r = parse(text);
        assert_eq!(r.len(), 2);
        assert_eq!(r[0]["GRIDSQUARE"], "JO90");
        assert_eq!(r[1]["CALL"], "R4FDW/");
    }
}
