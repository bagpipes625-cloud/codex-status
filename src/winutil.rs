//! Shared Win32 utility helpers.

use std::ffi::OsStr;
use std::os::windows::ffi::OsStrExt;

/// Encodes a string or path as a NUL-terminated UTF-16 buffer for Win32 wide APIs.
pub fn wide0(value: impl AsRef<OsStr>) -> Vec<u16> {
    value.as_ref().encode_wide().chain(std::iter::once(0)).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    #[test]
    fn wide0_encodes_strings_and_paths_with_nul_terminator() {
        assert_eq!(wide0(""), vec![0]);
        assert_eq!(wide0("AB"), vec![b'A' as u16, b'B' as u16, 0]);
        assert_eq!(
            wide0(Path::new("C:\\a")),
            vec![b'C' as u16, b':' as u16, b'\\' as u16, b'a' as u16, 0]
        );
        let cjk = wide0("额度");
        assert_eq!(cjk.last(), Some(&0));
        assert_eq!(String::from_utf16(&cjk[..cjk.len() - 1]).unwrap(), "额度");
    }
}
