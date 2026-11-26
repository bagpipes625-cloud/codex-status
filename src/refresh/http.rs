use super::auth::Credentials;
use crate::winhttp::{ProxyMode, Request, Session, Timeouts, WinHttpError};
use serde_json::Value;
use std::time::{Duration, Instant};
use windows::Win32::Networking::WinHttp::INTERNET_DEFAULT_HTTPS_PORT;

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

impl From<WinHttpError> for Error {
    fn from(error: WinHttpError) -> Self {
        match error {
            WinHttpError::Status { code, retry_after } => Self::Status(code, retry_after),
            WinHttpError::BodyTooLarge | WinHttpError::InvalidConfig => Self::Invalid,
            WinHttpError::Windows(_) | WinHttpError::Timeout => Self::Network,
        }
    }
}

// Each worker owns its handles. There is no arbitrary URL, POST, redirect, token refresh,
// certificate bypass or automatic application retry in this transport.
pub(super) fn get(endpoint: Endpoint, auth: &Credentials) -> Result<Value, Error> {
    let session =
        Session::open("CodexStatus read-only fallback", ProxyMode::Automatic, Timeouts::API)?;
    let headers = format!(
        "Authorization: Bearer {}\r\nChatGPT-Account-Id: {}\r\nAccept: application/json\r\nOAI-App-Brand: codex\r\n",
        auth.token, auth.account_id
    );
    let bytes = session.send(&Request {
        method: "GET",
        host: "chatgpt.com",
        port: INTERNET_DEFAULT_HTTPS_PORT,
        path: endpoint.path(),
        secure: true,
        headers: &headers,
        body: None,
        allow_redirects: false,
        disable_cookies: true,
        allow_loopback_self_signed: false,
        max_response_bytes: 512 * 1024,
        deadline: Instant::now() + Duration::from_secs(20),
    })?;
    serde_json::from_slice(&bytes).map_err(|_| Error::Invalid)
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
