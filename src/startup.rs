use std::io;
use std::path::Path;
use winreg::RegKey;
use winreg::enums::{HKEY_CURRENT_USER, KEY_READ, KEY_WRITE};

const RUN_KEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Run";
#[cfg(codex_status_channel = "stable")]
const VALUE_NAME: &str = "CodexStatus";
#[cfg(codex_status_channel = "beta")]
const VALUE_NAME: &str = "CodexStatus Beta";
#[cfg(codex_status_channel = "development")]
const VALUE_NAME: &str = "CodexStatus Development";
#[cfg(codex_status_channel = "portable")]
const VALUE_NAME: &str = "CodexStatus Portable";

pub fn is_enabled() -> bool {
    let Ok(executable) = std::env::current_exe() else {
        return false;
    };
    let expected = startup_command(&executable);
    let current_user = RegKey::predef(HKEY_CURRENT_USER);
    current_user
        .open_subkey_with_flags(RUN_KEY, KEY_READ)
        .ok()
        .and_then(|key| key.get_value::<String, _>(VALUE_NAME).ok())
        .is_some_and(|command| command.to_lowercase() == expected.to_lowercase())
}

pub fn enable(executable: &Path) -> io::Result<()> {
    let current_user = RegKey::predef(HKEY_CURRENT_USER);
    let (key, _) = current_user.create_subkey_with_flags(RUN_KEY, KEY_WRITE)?;
    let command = startup_command(executable);
    key.set_value(VALUE_NAME, &command)
}

pub fn disable() -> io::Result<()> {
    let current_user = RegKey::predef(HKEY_CURRENT_USER);
    let key = current_user.open_subkey_with_flags(RUN_KEY, KEY_WRITE)?;
    match key.delete_value(VALUE_NAME) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error),
    }
}

fn startup_command(executable: &Path) -> String {
    format!("\"{}\" --background", executable.display())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn startup_command_quotes_paths_and_uses_background_mode() {
        assert_eq!(
            startup_command(Path::new(r"F:\Apps With Spaces\CodexStatus.exe")),
            r#""F:\Apps With Spaces\CodexStatus.exe" --background"#
        );
    }

    #[test]
    fn release_channels_use_independent_startup_values() {
        let expected = if cfg!(codex_status_channel = "stable") {
            "CodexStatus"
        } else if cfg!(codex_status_channel = "beta") {
            "CodexStatus Beta"
        } else if cfg!(codex_status_channel = "portable") {
            "CodexStatus Portable"
        } else {
            "CodexStatus Development"
        };
        assert_eq!(VALUE_NAME, expected);
    }
}
