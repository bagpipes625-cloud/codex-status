use std::io;
use std::path::Path;
use winreg::RegKey;
use winreg::enums::{HKEY_CURRENT_USER, KEY_READ, KEY_WRITE};

const RUN_KEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Run";
const VALUE_NAME: &str = "CodexStatus";

pub fn is_enabled() -> bool {
    let current_user = RegKey::predef(HKEY_CURRENT_USER);
    current_user
        .open_subkey_with_flags(RUN_KEY, KEY_READ)
        .ok()
        .and_then(|key| key.get_value::<String, _>(VALUE_NAME).ok())
        .is_some()
}

pub fn enable(executable: &Path) -> io::Result<()> {
    let current_user = RegKey::predef(HKEY_CURRENT_USER);
    let (key, _) = current_user.create_subkey_with_flags(RUN_KEY, KEY_WRITE)?;
    let command = format!("\"{}\" --background", executable.display());
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
