//! DXCC entities from a call sign, with AD1C's country file (cty.dat) as WSJT-X and JTDX
//! install it. Nothing is downloaded: the file is looked for where those programs keep
//! it, or given in the settings.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

/// A DXCC entity.
#[derive(Clone, Debug, PartialEq)]
pub struct Entity {
    pub name: String,
    /// Its main prefix, e.g. "TA".
    pub prefix: String,
    pub continent: String,
}

/// The country file, ready for look-ups.
#[derive(Clone, Debug, Default)]
pub struct Cty {
    entities: Vec<Entity>,
    /// Whole calls ("=" entries) and prefixes → entity index.
    exact: HashMap<String, usize>,
    prefixes: HashMap<String, usize>,
    longest: usize,
}

/// A prefix or call without the zone, position, continent and time zone overrides that
/// may follow it: "(14)", "[28]", "<51.0/-10.0>", "{EU}", "~-1.0~".
fn bare(token: &str) -> String {
    let mut out = String::new();
    let mut close = None;
    for c in token.chars() {
        match (close, c) {
            (Some(end), c) if c == end => close = None,
            (Some(_), _) => {}
            (None, '(') => close = Some(')'),
            (None, '[') => close = Some(']'),
            (None, '<') => close = Some('>'),
            (None, '{') => close = Some('}'),
            (None, '~') => close = Some('~'),
            (None, c) => out.push(c),
        }
    }
    out.trim().to_ascii_uppercase()
}

impl Cty {
    pub fn parse(text: &str) -> Self {
        let mut cty = Cty::default();
        let mut current: Option<usize> = None;
        for line in text.lines() {
            if !line.starts_with(' ') && !line.starts_with('\t') && line.contains(':') {
                let fields: Vec<&str> = line.split(':').map(str::trim).collect();
                if fields.len() < 8 {
                    current = None;
                    continue;
                }
                // "*" marks entities on the WAE list only (European Turkey, ...): their
                // prefixes are left out, so the calls fall to the DXCC entity.
                if fields[7].starts_with('*') {
                    current = None;
                    continue;
                }
                cty.entities.push(Entity { name: fields[0].to_string(), prefix: fields[7].to_string(), continent: fields[3].to_string() });
                current = Some(cty.entities.len() - 1);
                continue;
            }
            let Some(entity) = current else { continue };
            for token in line.split([',', ';']) {
                let token = token.trim();
                if token.is_empty() {
                    continue;
                }
                match token.strip_prefix('=') {
                    Some(call) => {
                        cty.exact.insert(bare(call), entity);
                    }
                    None => {
                        let p = bare(token);
                        cty.longest = cty.longest.max(p.len());
                        cty.prefixes.insert(p, entity);
                    }
                }
            }
        }
        cty
    }

    pub fn load(path: &Path) -> Option<Self> {
        let text = std::fs::read(path).ok()?;
        let cty = Self::parse(&String::from_utf8_lossy(&text));
        (!cty.entities.is_empty()).then_some(cty)
    }

    pub fn len(&self) -> usize {
        self.entities.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entities.is_empty()
    }

    /// The entity of a call sign. Portable forms are handled: "EA8/DL1ABC" counts as
    /// EA8, "DL1ABC/P" or "DL1ABC/QRP" as DL1ABC.
    pub fn entity(&self, call: &str) -> Option<&Entity> {
        let call = call.trim().trim_matches(|c| c == '<' || c == '>').to_ascii_uppercase();
        if let Some(&i) = self.exact.get(&call) {
            return self.entities.get(i);
        }
        let base = match call.split('/').collect::<Vec<_>>().as_slice() {
            [one] => one.to_string(),
            [a, b] => {
                let suffix = |s: &str| matches!(s, "P" | "M" | "MM" | "AM" | "QRP" | "A" | "B") || s.chars().all(|c| c.is_ascii_digit());
                if suffix(b) {
                    a.to_string()
                } else if a.len() < b.len() {
                    a.to_string()
                } else {
                    b.to_string()
                }
            }
            [a, b, ..] => if a.len() < b.len() { a.to_string() } else { b.to_string() },
            [] => return None,
        };
        (1..=base.len().min(self.longest)).rev().find_map(|n| self.prefixes.get(&base[..n])).and_then(|&i| self.entities.get(i))
    }
}

/// Where WSJT-X and JTDX keep the country file, newest copies first.
pub fn find_file() -> Option<PathBuf> {
    let mut places = Vec::new();
    if let Some(local) = std::env::var_os("LOCALAPPDATA").map(PathBuf::from) {
        places.push(local.join("WSJT-X").join("cty.dat"));
        places.push(local.join("JTDX").join("cty.dat"));
    }
    places.push(PathBuf::from(r"C:\WSJT\wsjtx\share\wsjtx\cty.dat"));
    for pf in ["ProgramFiles", "ProgramFiles(x86)"] {
        if let Some(p) = std::env::var_os(pf).map(PathBuf::from) {
            places.push(p.join("WSJT-X").join("share").join("wsjtx").join("cty.dat"));
        }
    }
    if let Some(home) = std::env::var_os("HOME").map(PathBuf::from) {
        places.push(home.join(".local").join("share").join("WSJT-X").join("cty.dat"));
    }
    places.push(PathBuf::from("/usr/share/wsjtx/cty.dat"));
    // JTDX keeps it under its versioned folder.
    for root in [r"C:\JTDX64", r"C:\JTDX"] {
        if let Ok(dirs) = std::fs::read_dir(root) {
            for d in dirs.flatten() {
                places.push(d.path().join("share").join("jtdx").join("cty.dat"));
            }
        }
    }
    places.into_iter().find(|p| p.is_file())
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = "\
Fed. Rep. of Germany:     14:  28:  EU:   51.00:   -10.00:    -1.0:  DL:
    DA,DB,DC,DD,DE,DF,DG,DH,DI,DJ,DK,DL,DM,DN,DO,DP,DQ,DR,=DL0ABC(15);
Canary Islands:           33:  36:  AF:   28.32:    15.85:     0.0:  EA8:
    AM8,AN8,AO8,EA8,EB8,EC8,ED8,EE8,EF8,EG8,EH8;
Spain:                    14:  37:  EU:   40.37:     4.88:    -1.0:  EA:
    AM,AN,AO,EA,EB,EC,ED,EE,EF,EG,EH;
European Turkey:          20:  39:  EU:   41.02:   -28.97:    -2.0:  *TA1:
    TA1,TB1,TC1,YM1;
Turkey:                   20:  39:  AS:   39.18:   -35.65:    -2.0:  TA:
    TA,TB,TC,YM,=TA4SO[39];
United States:            05:  08:  NA:   37.60:    91.87:     5.0:  K:
    AA,AB,K,N,W,=KH6/W1ABC{NA};
";

    #[test]
    fn looks_up_entities() {
        let c = Cty::parse(SAMPLE);
        assert_eq!(c.len(), 5, "the WAE-only entity is left out");
        let name = |call: &str| c.entity(call).map(|e| e.name.clone());
        assert_eq!(name("DL1ABC").as_deref(), Some("Fed. Rep. of Germany"));
        assert_eq!(name("TA4SO").as_deref(), Some("Turkey"));
        assert_eq!(name("TA1XYZ").as_deref(), Some("Turkey"), "European Turkey is not a DXCC entity");
        assert_eq!(name("EA8ABC").as_deref(), Some("Canary Islands"), "longest prefix wins");
        assert_eq!(name("EA1ABC").as_deref(), Some("Spain"));
        assert_eq!(name("EA8/DL1ABC").as_deref(), Some("Canary Islands"));
        assert_eq!(name("DL1ABC/P").as_deref(), Some("Fed. Rep. of Germany"));
        assert_eq!(name("<W9XYZ>").as_deref(), Some("United States"));
        assert_eq!(name("KH6/W1ABC").as_deref(), Some("United States"), "exact entry with overrides");
        assert_eq!(name("QQ1ABC"), None);
    }

    #[test]
    fn overrides_are_stripped() {
        assert_eq!(bare("=DL0ABC(15)"), "=DL0ABC");
        assert_eq!(bare("KH6/W1ABC{NA}<21.0/157.0>~-10~"), "KH6/W1ABC");
    }

    /// With an installed country file: `cargo test --lib real_cty -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn real_cty() {
        let Some(path) = find_file() else { return };
        let c = Cty::load(&path).unwrap();
        println!("{} entities from {}", c.len(), path.display());
        for call in ["TA4SO", "DL1ABC", "SP3MZ", "ON8AJ", "OE3PKB", "EA8XYZ", "PA/F4MFS", "R4FDW", "VK2ABC"] {
            println!("{call}: {:?}", c.entity(call).map(|e| &e.name));
        }
        assert!(c.entity("TA4SO").unwrap().name.contains("Turkey"));
    }
}
