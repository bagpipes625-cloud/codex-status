use super::auth::Credentials;
use serde_json::Value;
use std::{
    ffi::c_void,
    ptr,
    time::{Duration, Instant},
};
use windows::{
    Win32::Networking::WinHttp::*,
    core::{PCWSTR, w},
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Endpoint {
    Quota,
    Credits,
    Tokens,
}

impl Endpoint {
    pub fn index(self) -> usize {
        match self {
            Self::Quota => 0,
            Self::Credits => 1,
            Self::Tokens => 2,
        }
    }
    fn path(self) -> &'static str {
        match self {
            Self::Quota => "/backend-api/wham/usage",
            Self::Credits => "/backend-api/wham/rate-limit-reset-credits",
            Self::Tokens => "/backend-api/wham/profiles/me",
        }
    }
}

#[derive(Debug)]
pub(super) enum Error {
    Network,
    Invalid,
    Status(u32, Duration),
}
impl From<windows::core::Error> for Error {
    fn from(_: windows::core::Error) -> Self {
        Self::Network
    }
}

struct Handle(*mut c_void);
impl Handle {
    fn new(raw: *mut c_void) -> Result<Self, Error> {
        if raw.is_null() { Err(Error::Network) } else { Ok(Self(raw)) }
    }
}
impl Drop for Handle {
    fn drop(&mut self) {
        unsafe {
            let _ = WinHttpCloseHandle(self.0);
        }
    }
}

// Each worker owns its handles. There is no arbitrary URL, POST, redirect, token refresh,
// certificate bypass or automatic application retry in this transport.
pub(super) fn get(endpoint: Endpoint, auth: &Credentials) -> Result<Value, Error> {
    let deadline = Instant::now() + Duration::from_secs(20);
    let session = unsafe {
        Handle::new(WinHttpOpen(
            w!("CodexStatus read-only fallback"),
            WINHTTP_ACCESS_TYPE_AUTOMATIC_PROXY,
            PCWSTR::null(),
            PCWSTR::null(),
            0,
        ))?
    };
    unsafe {
        WinHttpSetTimeouts(session.0, 5_000, 5_000, 5_000, 20_000)?;
    }
    let connection = unsafe {
        Handle::new(WinHttpConnect(session.0, w!("chatgpt.com"), INTERNET_DEFAULT_HTTPS_PORT, 0))?
    };
    let path: Vec<u16> = endpoint.path().encode_utf16().chain(Some(0)).collect();
    let request = unsafe {
        Handle::new(WinHttpOpenRequest(
            connection.0,
            w!("GET"),
            PCWSTR(path.as_ptr()),
            PCWSTR::null(),
            PCWSTR::null(),
            ptr::null(),
            WINHTTP_FLAG_SECURE,
        ))?
    };
    let disabled = WINHTTP_DISABLE_REDIRECTS | WINHTTP_DISABLE_COOKIES;
    unsafe {
        WinHttpSetOption(
            Some(request.0),
            WINHTTP_OPTION_DISABLE_FEATURE,
            Some(&disabled.to_ne_bytes()),
        )?;
    }
    let headers:Vec<u16>=format!("Authorization: Bearer {}\r\nChatGPT-Account-Id: {}\r\nAccept: application/json\r\nOAI-App-Brand: codex\r\n",auth.token,auth.account_id).encode_utf16().collect();
    timeouts(&request, deadline)?;
    unsafe {
        WinHttpSendRequest(request.0, Some(&headers), None, 0, 0, 0)?;
    }
    timeouts(&request, deadline)?;
    unsafe {
        WinHttpReceiveResponse(request.0, ptr::null_mut())?;
    }
    let mut status = 0u32;
    let mut size = 4;
    let mut index = 0;
    unsafe {
        WinHttpQueryHeaders(
            request.0,
            WINHTTP_QUERY_STATUS_CODE | WINHTTP_QUERY_FLAG_NUMBER,
            PCWSTR::null(),
            Some((&mut status as *mut u32).cast()),
            &mut size,
            &mut index,
        )?;
    }
    if status != 200 {
        return Err(Error::Status(status, retry_after(&request)));
    }
    let mut bytes = Vec::new();
    let mut buffer = [0u8; 8192];
    loop {
        timeouts(&request, deadline)?;
        let mut read = 0;
        unsafe {
            WinHttpReadData(request.0, buffer.as_mut_ptr().cast(), buffer.len() as u32, &mut read)?;
        }
        if read == 0 {
            break;
        }
        if bytes.len() + read as usize > 512 * 1024 {
            return Err(Error::Invalid);
        }
        bytes.extend_from_slice(&buffer[..read as usize]);
    }
    if Instant::now() >= deadline {
        return Err(Error::Network);
    }
    serde_json::from_slice(&bytes).map_err(|_| Error::Invalid)
}

fn timeouts(request: &Handle, deadline: Instant) -> Result<(), Error> {
    let remaining = deadline.saturating_duration_since(Instant::now()).as_millis();
    if remaining == 0 {
        return Err(Error::Network);
    }
    let ms = remaining.min(20_000) as i32;
    unsafe {
        WinHttpSetTimeouts(request.0, ms.min(5_000), ms.min(5_000), ms.min(5_000), ms)?;
    }
    Ok(())
}

fn retry_after(request: &Handle) -> Duration {
    let mut buffer = [0u16; 128];
    let mut size = (buffer.len() * 2) as u32;
    let mut index = 0;
    let ok = unsafe {
        WinHttpQueryHeaders(
            request.0,
            WINHTTP_QUERY_CUSTOM,
            w!("Retry-After"),
            Some(buffer.as_mut_ptr().cast()),
            &mut size,
            &mut index,
        )
    };
    if ok.is_err() {
        return Duration::from_secs(300);
    }
    let value = String::from_utf16_lossy(
        &buffer[..buffer.iter().position(|&v| v == 0).unwrap_or(buffer.len())],
    );
    let seconds = value
        .trim()
        .parse::<u64>()
        .ok()
        .or_else(|| {
            chrono::DateTime::parse_from_rfc2822(value.trim()).ok().map(|date| {
                date.timestamp().saturating_sub(chrono::Utc::now().timestamp()).max(0) as u64
            })
        })
        .unwrap_or(300);
    Duration::from_secs(seconds.clamp(60, 86_400))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn only_three_read_endpoints_exist() {
        for endpoint in [Endpoint::Quota, Endpoint::Credits, Endpoint::Tokens] {
            assert!(endpoint.path().starts_with("/backend-api/wham/"));
            assert!(!endpoint.path().contains("consume"));
        }
    }
}
