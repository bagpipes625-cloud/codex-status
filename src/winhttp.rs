//! Shared WinHTTP RAII client for official API reads, local loopback probes, and updater downloads.

use std::ffi::c_void;
use std::ptr;
use std::time::{Duration, Instant};
use windows::Win32::Networking::WinHttp::{
    ERROR_WINHTTP_TIMEOUT, SECURITY_FLAG_IGNORE_CERT_CN_INVALID,
    SECURITY_FLAG_IGNORE_CERT_DATE_INVALID, SECURITY_FLAG_IGNORE_CERT_WRONG_USAGE,
    SECURITY_FLAG_IGNORE_UNKNOWN_CA, WINHTTP_ACCESS_TYPE_AUTOMATIC_PROXY,
    WINHTTP_ACCESS_TYPE_NO_PROXY, WINHTTP_DISABLE_COOKIES, WINHTTP_DISABLE_REDIRECTS,
    WINHTTP_FLAG_SECURE, WINHTTP_OPEN_REQUEST_FLAGS, WINHTTP_OPTION_DISABLE_FEATURE,
    WINHTTP_OPTION_SECURITY_FLAGS, WINHTTP_QUERY_CUSTOM, WINHTTP_QUERY_FLAG_NUMBER,
    WINHTTP_QUERY_STATUS_CODE, WinHttpCloseHandle, WinHttpConnect, WinHttpOpen, WinHttpOpenRequest,
    WinHttpQueryHeaders, WinHttpReadData, WinHttpReceiveResponse, WinHttpSendRequest,
    WinHttpSetOption, WinHttpSetTimeouts,
};
use windows::core::{HRESULT, PCWSTR, w};

const DEFAULT_RETRY_AFTER_SECS: u64 = 300;
const MIN_RETRY_AFTER_SECS: u64 = 60;
const MAX_RETRY_AFTER_SECS: u64 = 86_400;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProxyMode {
    Automatic,
    NoProxy,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Timeouts {
    pub resolve_ms: i32,
    pub connect_ms: i32,
    pub send_ms: i32,
    pub receive_ms: i32,
}

impl Timeouts {
    pub const API: Self =
        Self { resolve_ms: 5_000, connect_ms: 5_000, send_ms: 5_000, receive_ms: 20_000 };
    pub const DOWNLOAD: Self =
        Self { resolve_ms: 5_000, connect_ms: 5_000, send_ms: 10_000, receive_ms: 15_000 };

    pub(crate) fn clamped_to_remaining(self, remaining: Duration) -> Option<Self> {
        let remaining_ms = remaining.as_millis();
        if remaining_ms == 0 {
            return None;
        }
        let budget = remaining_ms.min(i32::MAX as u128) as i32;
        Some(Self {
            resolve_ms: self.resolve_ms.min(budget),
            connect_ms: self.connect_ms.min(budget),
            send_ms: self.send_ms.min(budget),
            receive_ms: self.receive_ms.min(budget),
        })
    }
}

#[derive(Debug)]
pub enum WinHttpError {
    Windows(windows::core::Error),
    Timeout,
    Status { code: u32, retry_after: Duration },
    BodyTooLarge,
    InvalidConfig,
}

impl From<windows::core::Error> for WinHttpError {
    fn from(error: windows::core::Error) -> Self {
        if error.code() == HRESULT::from_win32(ERROR_WINHTTP_TIMEOUT) {
            Self::Timeout
        } else {
            Self::Windows(error)
        }
    }
}

pub struct InternetHandle(*mut c_void);

impl InternetHandle {
    pub fn from_raw(raw: *mut c_void) -> windows::core::Result<Self> {
        if raw.is_null() { Err(windows::core::Error::from_thread()) } else { Ok(Self(raw)) }
    }

    pub fn as_raw(&self) -> *mut c_void {
        self.0
    }
}

impl Drop for InternetHandle {
    fn drop(&mut self) {
        if !self.0.is_null() {
            unsafe {
                let _ = WinHttpCloseHandle(self.0);
            }
        }
    }
}

pub struct Session {
    handle: InternetHandle,
    timeouts: Timeouts,
}

pub struct Request<'a> {
    pub method: &'a str,
    pub host: &'a str,
    pub port: u16,
    pub path: &'a str,
    pub secure: bool,
    pub headers: &'a str,
    pub body: Option<&'a [u8]>,
    pub allow_redirects: bool,
    pub disable_cookies: bool,
    pub allow_loopback_self_signed: bool,
    pub max_response_bytes: usize,
    pub deadline: Instant,
}

impl Session {
    pub fn open(
        user_agent: &str,
        proxy: ProxyMode,
        timeouts: Timeouts,
    ) -> Result<Self, WinHttpError> {
        let agent = wide_null(user_agent);
        let access_type = match proxy {
            ProxyMode::Automatic => WINHTTP_ACCESS_TYPE_AUTOMATIC_PROXY,
            ProxyMode::NoProxy => WINHTTP_ACCESS_TYPE_NO_PROXY,
        };
        let handle = unsafe {
            InternetHandle::from_raw(WinHttpOpen(
                PCWSTR(agent.as_ptr()),
                access_type,
                PCWSTR::null(),
                PCWSTR::null(),
                0,
            ))?
        };
        unsafe {
            WinHttpSetTimeouts(
                handle.as_raw(),
                timeouts.resolve_ms,
                timeouts.connect_ms,
                timeouts.send_ms,
                timeouts.receive_ms,
            )?;
        }
        Ok(Self { handle, timeouts })
    }

    pub fn raw_handle(&self) -> *mut c_void {
        self.handle.as_raw()
    }

    pub fn send(&self, request: &Request<'_>) -> Result<Vec<u8>, WinHttpError> {
        if request.allow_loopback_self_signed && !is_loopback_host(request.host) {
            return Err(WinHttpError::InvalidConfig);
        }
        let host = wide_null(request.host);
        let connection = unsafe {
            InternetHandle::from_raw(WinHttpConnect(
                self.handle.as_raw(),
                PCWSTR(host.as_ptr()),
                request.port,
                0,
            ))?
        };
        let method = wide_null(request.method);
        let path = wide_null(request.path);
        let flags =
            if request.secure { WINHTTP_FLAG_SECURE } else { WINHTTP_OPEN_REQUEST_FLAGS(0) };
        let req = unsafe {
            InternetHandle::from_raw(WinHttpOpenRequest(
                connection.as_raw(),
                PCWSTR(method.as_ptr()),
                PCWSTR(path.as_ptr()),
                PCWSTR::null(),
                PCWSTR::null(),
                ptr::null(),
                flags,
            ))?
        };

        let disabled = disable_feature_flags(request.allow_redirects, request.disable_cookies);
        if disabled != 0 {
            unsafe {
                WinHttpSetOption(
                    Some(req.as_raw()),
                    WINHTTP_OPTION_DISABLE_FEATURE,
                    Some(&disabled.to_ne_bytes()),
                )?;
            }
        }
        if request.secure && request.allow_loopback_self_signed {
            // Scoped strictly to loopback hosts verified above (e.g. Antigravity's
            // ephemeral local language server certificate).
            let security_flags = SECURITY_FLAG_IGNORE_UNKNOWN_CA
                | SECURITY_FLAG_IGNORE_CERT_CN_INVALID
                | SECURITY_FLAG_IGNORE_CERT_DATE_INVALID
                | SECURITY_FLAG_IGNORE_CERT_WRONG_USAGE;
            unsafe {
                WinHttpSetOption(
                    Some(req.as_raw()),
                    WINHTTP_OPTION_SECURITY_FLAGS,
                    Some(&security_flags.to_ne_bytes()),
                )?;
            }
        }

        let headers_wide: Vec<u16> = request.headers.encode_utf16().collect();
        let headers_slice = (!headers_wide.is_empty()).then_some(headers_wide.as_slice());
        let (body_ptr, body_len) = match request.body {
            Some(bytes) => {
                let len = u32::try_from(bytes.len()).map_err(|_| WinHttpError::InvalidConfig)?;
                (Some(bytes.as_ptr().cast()), len)
            }
            None => (None, 0),
        };

        apply_deadline_timeouts(&req, self.timeouts, request.deadline)?;
        unsafe {
            WinHttpSendRequest(req.as_raw(), headers_slice, body_ptr, body_len, body_len, 0)?;
        }
        apply_deadline_timeouts(&req, self.timeouts, request.deadline)?;
        unsafe {
            WinHttpReceiveResponse(req.as_raw(), ptr::null_mut())?;
        }

        let mut status = 0_u32;
        let mut size = std::mem::size_of::<u32>() as u32;
        let mut index = 0_u32;
        unsafe {
            WinHttpQueryHeaders(
                req.as_raw(),
                WINHTTP_QUERY_STATUS_CODE | WINHTTP_QUERY_FLAG_NUMBER,
                PCWSTR::null(),
                Some((&mut status as *mut u32).cast()),
                &mut size,
                &mut index,
            )?;
        }
        if status != 200 {
            return Err(WinHttpError::Status {
                code: status,
                retry_after: query_retry_after(&req),
            });
        }

        let mut bytes = Vec::new();
        let mut buffer = [0_u8; 8192];
        loop {
            apply_deadline_timeouts(&req, self.timeouts, request.deadline)?;
            let mut read = 0_u32;
            unsafe {
                WinHttpReadData(
                    req.as_raw(),
                    buffer.as_mut_ptr().cast(),
                    buffer.len() as u32,
                    &mut read,
                )?;
            }
            if read == 0 {
                break;
            }
            if bytes.len().saturating_add(read as usize) > request.max_response_bytes {
                return Err(WinHttpError::BodyTooLarge);
            }
            bytes.extend_from_slice(&buffer[..read as usize]);
        }
        if Instant::now() >= request.deadline {
            return Err(WinHttpError::Timeout);
        }
        Ok(bytes)
    }
}

fn disable_feature_flags(allow_redirects: bool, disable_cookies: bool) -> u32 {
    let mut flags = 0_u32;
    if !allow_redirects {
        flags |= WINHTTP_DISABLE_REDIRECTS;
    }
    if disable_cookies {
        flags |= WINHTTP_DISABLE_COOKIES;
    }
    flags
}

fn is_loopback_host(host: &str) -> bool {
    matches!(host, "127.0.0.1" | "::1") || host.eq_ignore_ascii_case("localhost")
}

fn apply_deadline_timeouts(
    request: &InternetHandle,
    base: Timeouts,
    deadline: Instant,
) -> Result<(), WinHttpError> {
    let remaining = deadline.saturating_duration_since(Instant::now());
    let Some(clamped) = base.clamped_to_remaining(remaining) else {
        return Err(WinHttpError::Timeout);
    };
    unsafe {
        WinHttpSetTimeouts(
            request.as_raw(),
            clamped.resolve_ms,
            clamped.connect_ms,
            clamped.send_ms,
            clamped.receive_ms,
        )?;
    }
    Ok(())
}

fn query_retry_after(request: &InternetHandle) -> Duration {
    let mut buffer = [0_u16; 128];
    let mut size = (buffer.len() * 2) as u32;
    let mut index = 0_u32;
    let ok = unsafe {
        WinHttpQueryHeaders(
            request.as_raw(),
            WINHTTP_QUERY_CUSTOM,
            w!("Retry-After"),
            Some(buffer.as_mut_ptr().cast()),
            &mut size,
            &mut index,
        )
    };
    if ok.is_err() {
        return Duration::from_secs(DEFAULT_RETRY_AFTER_SECS);
    }
    let len = buffer.iter().position(|&v| v == 0).unwrap_or(buffer.len());
    let value = String::from_utf16_lossy(&buffer[..len]);
    parse_retry_after(&value, chrono::Utc::now().timestamp())
}

pub(crate) fn parse_retry_after(value: &str, now_timestamp: i64) -> Duration {
    let trimmed = value.trim();
    let seconds = trimmed
        .parse::<u64>()
        .ok()
        .or_else(|| {
            chrono::DateTime::parse_from_rfc2822(trimmed)
                .ok()
                .map(|date| date.timestamp().saturating_sub(now_timestamp).max(0) as u64)
        })
        .unwrap_or(DEFAULT_RETRY_AFTER_SECS);
    Duration::from_secs(seconds.clamp(MIN_RETRY_AFTER_SECS, MAX_RETRY_AFTER_SECS))
}

fn wide_null(value: &str) -> Vec<u16> {
    value.encode_utf16().chain(std::iter::once(0)).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clamps_timeouts_to_remaining_deadline_budget() {
        assert_eq!(Timeouts::API.clamped_to_remaining(Duration::ZERO), None);
        assert_eq!(
            Timeouts::API.clamped_to_remaining(Duration::from_millis(2_500)),
            Some(Timeouts {
                resolve_ms: 2_500,
                connect_ms: 2_500,
                send_ms: 2_500,
                receive_ms: 2_500,
            })
        );
        assert_eq!(
            Timeouts::DOWNLOAD.clamped_to_remaining(Duration::from_secs(60)),
            Some(Timeouts::DOWNLOAD)
        );
        assert_eq!(
            Timeouts::DOWNLOAD.clamped_to_remaining(Duration::from_secs(8)),
            Some(Timeouts {
                resolve_ms: 5_000,
                connect_ms: 5_000,
                send_ms: 8_000,
                receive_ms: 8_000,
            })
        );
    }

    #[test]
    fn parses_and_clamps_retry_after_seconds_and_http_dates() {
        let now = 1_700_000_000;
        assert_eq!(parse_retry_after("120", now), Duration::from_secs(120));
        assert_eq!(parse_retry_after(" 5 ", now), Duration::from_secs(60));
        assert_eq!(parse_retry_after("999999", now), Duration::from_secs(86_400));
        assert_eq!(parse_retry_after("invalid", now), Duration::from_secs(300));
        // RFC2822 timestamp 600s after `now` (2023-11-14T22:23:20Z).
        assert_eq!(
            parse_retry_after("Tue, 14 Nov 2023 22:23:20 GMT", now),
            Duration::from_secs(600)
        );
        // Past RFC2822 date clamps to minimum 60s.
        assert_eq!(
            parse_retry_after("Tue, 14 Nov 2023 22:00:00 GMT", now),
            Duration::from_secs(60)
        );
    }

    #[test]
    fn loopback_self_signed_policy_rejects_remote_hosts() {
        assert!(is_loopback_host("127.0.0.1"));
        assert!(is_loopback_host("localhost"));
        assert!(is_loopback_host("LOCALHOST"));
        assert!(is_loopback_host("::1"));
        assert!(!is_loopback_host("chatgpt.com"));
        assert!(!is_loopback_host("api.github.com"));

        let session = Session::open("CodexStatus test", ProxyMode::NoProxy, Timeouts::API).unwrap();
        let error = session
            .send(&Request {
                method: "GET",
                host: "chatgpt.com",
                port: 443,
                path: "/",
                secure: true,
                headers: "",
                body: None,
                allow_redirects: false,
                disable_cookies: true,
                allow_loopback_self_signed: true,
                max_response_bytes: 1024,
                deadline: Instant::now() + Duration::from_secs(1),
            })
            .unwrap_err();
        assert!(matches!(error, WinHttpError::InvalidConfig));
    }

    #[test]
    fn disable_feature_flags_honor_redirect_and_cookie_policies() {
        assert_eq!(
            disable_feature_flags(false, true),
            WINHTTP_DISABLE_REDIRECTS | WINHTTP_DISABLE_COOKIES
        );
        assert_eq!(disable_feature_flags(true, true), WINHTTP_DISABLE_COOKIES);
        assert_eq!(disable_feature_flags(false, false), WINHTTP_DISABLE_REDIRECTS);
        assert_eq!(disable_feature_flags(true, false), 0);
    }
}
