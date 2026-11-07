//! Where the program keeps its files.

use std::path::PathBuf;

/// Settings (Windows: %APPDATA%\QO-ALL, elsewhere ~/.config/qo-all).
pub fn config_dir() -> Option<PathBuf> {
    if cfg!(windows) {
        Some(PathBuf::from(std::env::var_os("APPDATA")?).join("QO-ALL"))
    } else {
        Some(PathBuf::from(std::env::var_os("HOME")?).join(".config").join("qo-all"))
    }
}

/// Recordings kept for diagnosis (Windows: %LOCALAPPDATA%\QO-ALL, elsewhere ~/.cache/qo-all).
pub fn cache_dir() -> Option<PathBuf> {
    if cfg!(windows) {
        Some(PathBuf::from(std::env::var_os("LOCALAPPDATA")?).join("QO-ALL"))
    } else {
        Some(PathBuf::from(std::env::var_os("HOME")?).join(".cache").join("qo-all"))
    }
}
