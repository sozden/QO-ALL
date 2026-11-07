//! Settings kept between runs, as "key=value" lines in the user's configuration folder
//! (Windows: %APPDATA%\QO-ALL\settings.txt, elsewhere ~/.config/qo-all/settings.txt).

use std::collections::BTreeMap;
use std::fmt::Display;
use std::str::FromStr;

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Settings(BTreeMap<String, String>);

impl Settings {
    pub fn parse(text: &str) -> Self {
        Self(
            text.lines()
                .filter_map(|l| l.split_once('='))
                .map(|(k, v)| (k.trim().to_string(), v.trim().to_string()))
                .filter(|(k, _)| !k.is_empty())
                .collect(),
        )
    }

    pub fn to_text(&self) -> String {
        self.0.iter().map(|(k, v)| format!("{k}={v}\n")).collect()
    }

    pub fn get<T: FromStr>(&self, key: &str) -> Option<T> {
        self.0.get(key).and_then(|v| v.parse().ok())
    }

    pub fn text(&self, key: &str) -> Option<String> {
        self.0.get(key).filter(|v| !v.is_empty()).cloned()
    }

    pub fn set(&mut self, key: &str, value: impl Display) {
        self.0.insert(key.to_string(), value.to_string());
    }

    pub fn set_opt(&mut self, key: &str, value: Option<impl Display>) {
        match value {
            Some(v) => self.set(key, v),
            None => {
                self.0.remove(key);
            }
        }
    }

    fn path() -> Option<std::path::PathBuf> {
        qo_all::paths::config_dir().map(|d| d.join("settings.txt"))
    }

    pub fn load() -> Self {
        Self::path().and_then(|p| std::fs::read_to_string(p).ok()).map(|t| Self::parse(&t)).unwrap_or_default()
    }

    /// Failures are ignored: losing the settings is not worth interrupting the operator.
    pub fn save(&self) {
        if let Some(p) = Self::path() {
            if let Some(dir) = p.parent() {
                let _ = std::fs::create_dir_all(dir);
            }
            let _ = std::fs::write(p, self.to_text());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip() {
        let mut s = Settings::default();
        s.set("rx_dial_hz", 10_489_725_000.0);
        s.set("device", "ip:192.168.2.1");
        s.set_opt("microphone", Some("wasapi:{0.0.1.00000000}.{abc}"));
        s.set_opt("rx_gain", None::<f64>);
        let back = Settings::parse(&s.to_text());
        assert_eq!(back, s);
        assert_eq!(back.get::<f64>("rx_dial_hz"), Some(10_489_725_000.0));
        assert_eq!(back.text("microphone").as_deref(), Some("wasapi:{0.0.1.00000000}.{abc}"));
        assert_eq!(back.get::<f64>("rx_gain"), None);
    }
}
