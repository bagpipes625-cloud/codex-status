use crate::history::UsageLedger;
use crate::model::{QuotaKind, QuotaSnapshot};
use serde::{Deserialize, Serialize};
use std::fs;
use std::io::{self, Write};
use std::os::windows::ffi::OsStrExt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;
use windows::Win32::Storage::FileSystem::{
    MOVEFILE_REPLACE_EXISTING, MOVEFILE_WRITE_THROUGH, MoveFileExW, REPLACEFILE_WRITE_THROUGH,
    ReplaceFileW,
};
use windows::core::PCWSTR;

const APP_DIR: &str = "CodexStatus";
const SMALL_STATE_LIMIT: u64 = 1024 * 1024;
const HISTORY_STATE_LIMIT: u64 = 8 * 1024 * 1024;
const TRAY_LOG_LIMIT: u64 = 256 * 1024;
const STALE_TEMP_AGE: Duration = Duration::from_secs(24 * 60 * 60);
static TEMPORARY_FILE_SEQUENCE: AtomicU64 = AtomicU64::new(0);

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default, rename_all = "camelCase")]
pub struct Settings {
    pub refresh_minutes: u32,
    pub display_quota: QuotaKind,
    pub alert_threshold: Option<u8>,
    pub locale: String,
    pub theme: String,
    pub onboarding_shown: bool,
    pub last_alert_reset: Option<i64>,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            refresh_minutes: 5,
            display_quota: QuotaKind::FiveHour,
            alert_threshold: None,
            locale: "auto".to_owned(),
            theme: "system".to_owned(),
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
        if !matches!(self.theme.as_str(), "system" | "light" | "dark") {
            self.theme = "system".to_owned();
        }
    }
}

#[derive(Debug, Clone)]
pub struct AppStore {
    directory: PathBuf,
    updates_directory: PathBuf,
}

impl AppStore {
    pub fn discover() -> Self {
        let base =
            std::env::var_os("LOCALAPPDATA").map(PathBuf::from).unwrap_or_else(std::env::temp_dir);
        let channel = env!("CODEX_STATUS_CHANNEL");
        let store = Self {
            directory: channel_directory(&base, channel),
            updates_directory: base.join(APP_DIR).join("updates").join(channel),
        };
        store.cleanup_stale_state_files();
        store
    }

    #[cfg(test)]
    pub fn at(directory: PathBuf) -> Self {
        let updates_directory = directory.join("updates").join(env!("CODEX_STATUS_CHANNEL"));
        Self { directory, updates_directory }
    }

    pub fn load_settings(&self) -> Settings {
        let mut settings =
            read_json::<Settings>(&self.directory.join("settings.json"), SMALL_STATE_LIMIT)
                .unwrap_or_default();
        settings.normalize();
        settings
    }

    pub fn save_settings(&self, settings: &Settings) -> io::Result<()> {
        write_json_atomic(&self.directory.join("settings.json"), settings)
    }

    pub fn load_snapshot(&self) -> Option<QuotaSnapshot> {
        read_json(&self.directory.join("snapshot.json"), SMALL_STATE_LIMIT)
    }

    pub fn save_snapshot(&self, snapshot: &QuotaSnapshot) -> io::Result<()> {
        write_json_atomic(&self.directory.join("snapshot.json"), snapshot)
    }

    pub fn load_usage_history(&self) -> UsageLedger {
        let mut history: UsageLedger =
            read_json(&self.directory.join("usage-history.json"), HISTORY_STATE_LIMIT)
                .unwrap_or_default();
        history.prune();
        history
    }

    pub fn save_usage_history(&self, history: &UsageLedger) -> io::Result<()> {
        write_json_atomic(&self.directory.join("usage-history.json"), history)
    }

    pub fn updates_directory(&self) -> PathBuf {
        self.updates_directory.clone()
    }

    pub fn append_tray_error(&self, entry: &str) -> io::Result<()> {
        fs::create_dir_all(&self.directory)?;
        let path = self.directory.join("tray-errors.log");
        if path.metadata().is_ok_and(|metadata| metadata.len() >= TRAY_LOG_LIMIT) {
            let backup = self.directory.join("tray-errors.old.log");
            let _ = fs::remove_file(&backup);
            let _ = fs::rename(&path, backup);
        }
        let mut file = fs::OpenOptions::new().create(true).append(true).open(path)?;
        writeln!(file, "{entry}")
    }

    pub fn write_settings_error(&self, entry: &str) -> io::Result<()> {
        fs::create_dir_all(&self.directory)?;
        fs::write(self.directory.join("settings-error.log"), entry)
    }

    fn cleanup_stale_state_files(&self) {
        let Ok(entries) = fs::read_dir(&self.directory) else { return };
        for entry in entries.flatten() {
            let path = entry.path();
            let Some(name) = entry.file_name().to_str().map(str::to_owned) else {
                continue;
            };
            if !is_state_temporary_name(&name) {
                continue;
            }
            let Ok(metadata) = fs::symlink_metadata(&path) else { continue };
            if !metadata.file_type().is_file()
                || !metadata
                    .modified()
                    .ok()
                    .and_then(|modified| modified.elapsed().ok())
                    .is_some_and(|age| age >= STALE_TEMP_AGE)
            {
                continue;
            }
            let _ = fs::remove_file(path);
        }
    }
}

fn is_state_temporary_name(name: &str) -> bool {
    ["settings.tmp-", "snapshot.tmp-", "usage-history.tmp-"]
        .into_iter()
        .find_map(|prefix| name.strip_prefix(prefix))
        .is_some_and(|suffix| {
            let mut parts = suffix.split('-');
            matches!(
                (parts.next(), parts.next(), parts.next()),
                (Some(pid), Some(sequence), None)
                    if !pid.is_empty()
                        && !sequence.is_empty()
                        && pid.bytes().all(|byte| byte.is_ascii_digit())
                        && sequence.bytes().all(|byte| byte.is_ascii_digit())
            )
        })
}

fn read_json<T: for<'de> Deserialize<'de>>(path: &Path, limit: u64) -> Option<T> {
    if path.metadata().ok()?.len() > limit {
        return None;
    }
    let bytes = fs::read(path).ok()?;
    serde_json::from_slice(&bytes).ok()
}

fn write_json_atomic<T: Serialize>(path: &Path, value: &T) -> io::Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let sequence = TEMPORARY_FILE_SEQUENCE.fetch_add(1, Ordering::Relaxed);
    let temporary = path.with_extension(format!("tmp-{}-{sequence}", std::process::id()));
    let bytes = serde_json::to_vec(value).map_err(io::Error::other)?;
    let result = (|| {
        let mut file = fs::OpenOptions::new().write(true).create_new(true).open(&temporary)?;
        file.write_all(&bytes)?;
        file.sync_all()?;
        drop(file);
        replace_file(&temporary, path)
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    result
}

fn channel_directory(base: &Path, channel: &str) -> PathBuf {
    let root = base.join(APP_DIR);
    if channel == "stable" { root } else { root.join("channels").join(channel) }
}

fn replace_file(temporary: &Path, destination: &Path) -> io::Result<()> {
    let destination_exists = destination.exists();
    let temporary = wide0(temporary);
    let destination = wide0(destination);
    let result = unsafe {
        if destination_exists {
            ReplaceFileW(
                PCWSTR(destination.as_ptr()),
                PCWSTR(temporary.as_ptr()),
                PCWSTR::null(),
                REPLACEFILE_WRITE_THROUGH,
                None,
                None,
            )
        } else {
            MoveFileExW(
                PCWSTR(temporary.as_ptr()),
                PCWSTR(destination.as_ptr()),
                MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH,
            )
        }
    };
    result.map_err(|_| io::Error::last_os_error())
}

fn wide0(path: &Path) -> Vec<u16> {
    path.as_os_str().encode_wide().chain(std::iter::once(0)).collect()
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
            theme: "sepia".to_owned(),
            ..Settings::default()
        };
        settings.normalize();
        assert_eq!(settings, Settings::default());
    }

    #[test]
    fn old_settings_default_to_the_five_hour_quota() {
        let settings: Settings = serde_json::from_value(serde_json::json!({
            "refreshMinutes": 5,
            "locale": "auto",
            "theme": "system"
        }))
        .unwrap();
        assert_eq!(settings.display_quota, QuotaKind::FiveHour);
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

    #[test]
    fn round_trips_usage_history_atomically() {
        let suffix = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos();
        let directory = std::env::temp_dir().join(format!("codex-status-history-{suffix}"));
        let store = AppStore::at(directory.clone());
        let history = UsageLedger::default();
        store.save_usage_history(&history).unwrap();
        assert_eq!(store.load_usage_history(), history);
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn stable_keeps_its_existing_state_directory_and_other_channels_are_isolated() {
        let base = Path::new(r"C:\Users\Example\AppData\Local");
        assert_eq!(channel_directory(base, "stable"), base.join(APP_DIR));
        assert_eq!(
            channel_directory(base, "development"),
            base.join(APP_DIR).join("channels").join("development")
        );
        assert_eq!(
            channel_directory(base, "portable"),
            base.join(APP_DIR).join("channels").join("portable")
        );
    }

    #[test]
    fn failed_atomic_replace_removes_its_temporary_file() {
        let directory =
            std::env::temp_dir().join(format!("codex-status-settings-{}", std::process::id()));
        let destination = directory.join("settings.json");
        fs::create_dir_all(&destination).unwrap();

        assert!(write_json_atomic(&destination, &Settings::default()).is_err());
        let leftovers = fs::read_dir(&directory)
            .unwrap()
            .flatten()
            .filter(|entry| entry.file_name().to_string_lossy().starts_with("settings.tmp-"))
            .count();
        assert_eq!(leftovers, 0);

        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn oversized_history_file_is_rejected_before_reading() {
        let suffix = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos();
        let directory = std::env::temp_dir().join(format!("codex-status-history-limit-{suffix}"));
        fs::create_dir_all(&directory).unwrap();
        let file = fs::File::create(directory.join("usage-history.json")).unwrap();
        file.set_len(HISTORY_STATE_LIMIT + 1).unwrap();
        let store = AppStore::at(directory.clone());
        assert_eq!(store.load_usage_history(), UsageLedger::default());
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn tray_error_log_rotates_at_the_fixed_limit() {
        let suffix = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos();
        let directory = std::env::temp_dir().join(format!("codex-status-tray-log-{suffix}"));
        fs::create_dir_all(&directory).unwrap();
        fs::File::create(directory.join("tray-errors.log"))
            .unwrap()
            .set_len(TRAY_LOG_LIMIT)
            .unwrap();
        let store = AppStore::at(directory.clone());
        store.append_tray_error("new failure").unwrap();
        assert!(directory.join("tray-errors.old.log").is_file());
        assert_eq!(fs::read_to_string(directory.join("tray-errors.log")).unwrap(), "new failure\n");
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn stale_cleanup_matches_only_our_exact_temporary_names() {
        assert!(is_state_temporary_name("settings.tmp-123-0"));
        assert!(is_state_temporary_name("snapshot.tmp-7-42"));
        assert!(is_state_temporary_name("usage-history.tmp-1-9"));
        assert!(!is_state_temporary_name("notes.tmp-backup"));
        assert!(!is_state_temporary_name("settings.tmp-old-0"));
        assert!(!is_state_temporary_name("settings.tmp-1-2-extra"));
    }
}
