//! The log: finished contacts appended to an ADIF file (which other programs may share,
//! e.g. WSJT-X's wsjtx_log.adi), and the stations in it, for colouring the decodes.

use std::collections::{HashMap, HashSet};
use std::io::Write;
use std::path::Path;

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
        let _ = std::fs::remove_dir_all(dir);
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
