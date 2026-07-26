use crate::model::QuotaSnapshot;
use serde::{Deserialize, Serialize};
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

const APP_DIR: &str = "CodexStatus";

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default, rename_all = "camelCase")]
pub struct Settings {
    pub refresh_minutes: u32,
    pub alert_threshold: Option<u8>,
    pub locale: String,
    pub onboarding_shown: bool,
    pub last_alert_reset: Option<i64>,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            refresh_minutes: 5,
            alert_threshold: None,
            locale: "auto".to_owned(),
            onboarding_shown: false,
            last_alert_reset: None,
        }
    }
}

impl Settings {
    pub fn normalize(&mut self) {
        if !matches!(self.refresh_minutes, 1 | 5 | 15) {
            self.refresh_minutes = 5;
        }
        if !matches!(self.alert_threshold, None | Some(10 | 20 | 30)) {
            self.alert_threshold = None;
        }
        if !matches!(self.locale.as_str(), "auto" | "en" | "zh-CN") {
            self.locale = "auto".to_owned();
        }
    }
}

#[derive(Debug, Clone)]
pub struct AppStore {
    directory: PathBuf,
}

impl AppStore {
    pub fn discover() -> Self {
        let base =
            std::env::var_os("LOCALAPPDATA").map(PathBuf::from).unwrap_or_else(std::env::temp_dir);
        Self { directory: base.join(APP_DIR) }
    }

    #[cfg(test)]
    pub fn at(directory: PathBuf) -> Self {
        Self { directory }
    }

    pub fn load_settings(&self) -> Settings {
        let mut settings =
            read_json::<Settings>(&self.directory.join("settings.json")).unwrap_or_default();
        settings.normalize();
        settings
    }

    pub fn save_settings(&self, settings: &Settings) -> io::Result<()> {
        write_json_atomic(&self.directory.join("settings.json"), settings)
    }

    pub fn load_snapshot(&self) -> Option<QuotaSnapshot> {
        read_json(&self.directory.join("snapshot.json"))
    }

    pub fn save_snapshot(&self, snapshot: &QuotaSnapshot) -> io::Result<()> {
        write_json_atomic(&self.directory.join("snapshot.json"), snapshot)
    }
}

fn read_json<T: for<'de> Deserialize<'de>>(path: &Path) -> Option<T> {
    let bytes = fs::read(path).ok()?;
    serde_json::from_slice(&bytes).ok()
}

fn write_json_atomic<T: Serialize>(path: &Path, value: &T) -> io::Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let temporary = path.with_extension("tmp");
    let bytes = serde_json::to_vec_pretty(value).map_err(io::Error::other)?;
    fs::write(&temporary, bytes)?;
    if path.exists() {
        fs::remove_file(path)?;
    }
    fs::rename(temporary, path)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    #[test]
    fn normalizes_untrusted_settings() {
        let mut settings = Settings {
            refresh_minutes: 2,
            alert_threshold: Some(99),
            locale: "invalid".to_owned(),
            ..Settings::default()
        };
        settings.normalize();
        assert_eq!(settings, Settings::default());
    }

    #[test]
    fn round_trips_settings_atomically() {
        let suffix = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos();
        let directory = std::env::temp_dir().join(format!("codex-status-settings-{suffix}"));
        let store = AppStore::at(directory.clone());
        let settings =
            Settings { refresh_minutes: 15, alert_threshold: Some(20), ..Settings::default() };
        store.save_settings(&settings).unwrap();
        assert_eq!(store.load_settings(), settings);
        fs::remove_dir_all(directory).unwrap();
    }
}
