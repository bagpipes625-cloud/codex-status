//! Read-only Gemini / Cloud Code Assist quota client matching Codex refresh & security invariants.

use crate::app_server::{AppServerError, AppServerFailure, AppServerSnapshot};
use crate::model::{
    AccountSummary, QuotaSnapshot, QuotaWindow, SESSION_MINUTES, SecondaryQuotas, WEEK_MINUTES,
};
use serde::Deserialize;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::ffi::c_void;
use std::os::windows::process::CommandExt;
use std::process::Command;
use std::ptr;
use std::sync::Mutex;
use std::time::{Duration, Instant};
use windows::Win32::Networking::WinHttp::*;
use windows::Win32::System::Threading::CREATE_NO_WINDOW;
use windows::core::{PCWSTR, w};

const LOOPBACK_HOST: PCWSTR = w!("127.0.0.1");
const GET_USER_STATUS_PATH: &str = "/exa.language_server_pb.LanguageServerService/GetUserStatus";
const RETRIEVE_QUOTA_PATH: &str =
    "/exa.language_server_pb.LanguageServerService/RetrieveUserQuotaSummary";
const PROCESS_QUERY: &str = r#"$items = @(Get-CimInstance Win32_Process | Where-Object { ($_.Name -eq 'language_server.exe' -or $_.Name -eq 'language_server_windows_x64.exe') -and $_.CommandLine -like '*--csrf_token*' } | Select-Object ProcessId, Name, CommandLine); ConvertTo-Json -InputObject $items -Compress"#;
const MAX_DISCOVERY_BYTES: usize = 256 * 1024;
const MAX_RESPONSE_BYTES: usize = 512 * 1024;
const TOTAL_DEADLINE: Duration = Duration::from_secs(20);

#[derive(Default)]
pub struct GeminiState {
    blocked_until: Option<Instant>,
}

impl GeminiState {
    fn can_attempt(&mut self, now: Instant) -> bool {
        !self.blocked_until.is_some_and(|until| now < until)
    }

    fn block(&mut self, delay: Duration) {
        let until = Instant::now() + delay;
        self.blocked_until = Some(self.blocked_until.map_or(until, |old| old.max(until)));
    }
}

// Never Debug/Serialize the command line or CSRF token. They live only for one refresh.
struct LocalProcess {
    pid: u32,
    csrf_token: String,
    ports: Vec<u16>,
}

#[derive(Deserialize)]
#[serde(rename_all = "PascalCase")]
struct ProcessRecord {
    process_id: u32,
    name: String,
    command_line: Option<String>,
}

fn discover_local_processes() -> Result<Vec<LocalProcess>, AppServerError> {
    let output = Command::new("powershell.exe")
        .args(["-NoProfile", "-NonInteractive", "-Command", PROCESS_QUERY])
        .creation_flags(CREATE_NO_WINDOW.0)
        .output()
        .map_err(|_| local_error("Could not inspect the running Antigravity process"))?;
    if !output.status.success() || output.stdout.len() > MAX_DISCOVERY_BYTES {
        return Err(local_error("Could not inspect the running Antigravity process"));
    }
    let records: Vec<ProcessRecord> = serde_json::from_slice(&output.stdout)
        .map_err(|_| local_error("Invalid Antigravity process discovery response"))?;
    let mut processes = Vec::new();
    for record in records {
        if !record.name.eq_ignore_ascii_case("language_server.exe")
            && !record.name.eq_ignore_ascii_case("language_server_windows_x64.exe")
        {
            continue;
        }
        let Some(command_line) = record.command_line else { continue };
        let Some(csrf_token) = command_argument(&command_line, "--csrf_token") else { continue };
        if !header_value(&csrf_token, 16_384) {
            continue;
        }
        let mut ports = listening_ports(record.process_id);
        if let Some(port) = command_argument(&command_line, "--extension_server_port")
            .and_then(|value| value.parse::<u16>().ok())
            .filter(|port| *port != 0)
            && !ports.contains(&port)
        {
            ports.push(port);
        }
        if !ports.is_empty() {
            processes.push(LocalProcess { pid: record.process_id, csrf_token, ports });
        }
    }
    if processes.is_empty() {
        Err(local_error("Antigravity language server is not running"))
    } else {
        processes.sort_by_key(|process| process.pid);
        Ok(processes)
    }
}

fn hash_account_email(email: &str) -> String {
    let normalized = email.trim().to_ascii_lowercase();
    let digest = Sha256::digest(format!("gemini:{normalized}").as_bytes());
    format!("gemini:{}", digest.iter().map(|b| format!("{b:02x}")).collect::<String>())
}

fn command_argument(command_line: &str, flag: &str) -> Option<String> {
    let tokens = split_command_line(command_line);
    for (index, token) in tokens.iter().enumerate() {
        if token.eq_ignore_ascii_case(flag) {
            return tokens.get(index + 1).filter(|value| !value.is_empty()).cloned();
        }
        if let Some(value) = token.strip_prefix(&format!("{flag}="))
            && !value.is_empty()
        {
            return Some(value.to_owned());
        }
    }
    None
}

fn split_command_line(command_line: &str) -> Vec<String> {
    let mut tokens = Vec::new();
    let mut token = String::new();
    let mut quote = None;
    for ch in command_line.chars() {
        match (quote, ch) {
            (Some(active), current) if current == active => quote = None,
            (None, '"' | '\'') => quote = Some(ch),
            (None, current) if current.is_whitespace() => {
                if !token.is_empty() {
                    tokens.push(std::mem::take(&mut token));
                }
            }
            _ => token.push(ch),
        }
    }
    if !token.is_empty() {
        tokens.push(token);
    }
    tokens
}

fn listening_ports(pid: u32) -> Vec<u16> {
    let Ok(output) = Command::new("netstat.exe")
        .args(["-ano", "-p", "tcp"])
        .creation_flags(CREATE_NO_WINDOW.0)
        .output()
    else {
        return Vec::new();
    };
    if !output.status.success() || output.stdout.len() > MAX_DISCOVERY_BYTES {
        return Vec::new();
    }
    let pid = pid.to_string();
    let mut ports = Vec::new();
    for line in String::from_utf8_lossy(&output.stdout).lines() {
        let fields: Vec<_> = line.split_whitespace().collect();
        if fields.len() < 5 || !fields[3].eq_ignore_ascii_case("LISTENING") || fields[4] != pid {
            continue;
        }
        if let Some(port) = fields[1]
            .rsplit(':')
            .next()
            .and_then(|value| value.parse::<u16>().ok())
            .filter(|port| *port != 0)
            && !ports.contains(&port)
        {
            ports.push(port);
        }
    }
    ports
}

fn header_value(value: &str, max_len: usize) -> bool {
    !value.is_empty()
        && value.len() <= max_len
        && !value.bytes().any(|b| b == b'\r' || b == b'\n' || b == 0)
}

struct Handle(*mut c_void);
impl Handle {
    fn new(raw: *mut c_void) -> Result<Self, HttpError> {
        if raw.is_null() { Err(HttpError::Network) } else { Ok(Self(raw)) }
    }
}
impl Drop for Handle {
    fn drop(&mut self) {
        unsafe {
            let _ = WinHttpCloseHandle(self.0);
        }
    }
}

#[derive(Debug)]
enum HttpError {
    Network,
    Invalid,
    Status(u32, Duration),
}

impl From<windows::core::Error> for HttpError {
    fn from(_: windows::core::Error) -> Self {
        Self::Network
    }
}

pub fn fetch(state: &Mutex<GeminiState>) -> Result<AppServerSnapshot, AppServerFailure> {
    let allowed = {
        let mut guard = state.lock().unwrap_or_else(|e| e.into_inner());
        guard.can_attempt(Instant::now())
    };
    if !allowed {
        return Err(AppServerFailure {
            error: AppServerError::Rpc {
                method: "gemini/quota",
                message: "Gemini refresh temporarily throttled".into(),
            },
            account_key: None,
            token_usage: None,
        });
    }

    let processes = discover_local_processes().map_err(failure)?;
    let deadline = Instant::now() + TOTAL_DEADLINE;
    let mut last_status = None;
    for process in processes {
        for port in process.ports {
            for secure in [true, false] {
                let quota_value = match post_local_json(
                    port,
                    secure,
                    RETRIEVE_QUOTA_PATH,
                    &json!({}),
                    &process.csrf_token,
                    deadline,
                ) {
                    Ok(value) => value,
                    Err(HttpError::Status(status, delay)) => {
                        last_status = Some(status);
                        if status == 429
                            && let Ok(mut guard) = state.lock()
                        {
                            guard.block(delay);
                        }
                        continue;
                    }
                    Err(HttpError::Network | HttpError::Invalid) => continue,
                };
                let user_status = post_local_json(
                    port,
                    secure,
                    GET_USER_STATUS_PATH,
                    &json!({
                        "metadata": {
                            "ideName": "antigravity",
                            "extensionName": "antigravity",
                            "locale": "en"
                        }
                    }),
                    &process.csrf_token,
                    deadline,
                )
                .ok();
                let plan_type = user_status.as_ref().and_then(extract_plan_type);
                let account_key = user_status.as_ref().and_then(extract_account_key);
                let snapshot =
                    parse_quota_snapshot(&quota_value, plan_type, chrono::Utc::now().timestamp())
                        .map_err(|error| AppServerFailure {
                        error,
                        account_key: account_key.clone(),
                        token_usage: None,
                    })?;
                return Ok(AppServerSnapshot {
                    quota: snapshot,
                    token_usage: None,
                    account_key,
                    supplement_allowed: false,
                });
            }
        }
    }

    Err(failure(last_status.map_or_else(
        || local_error("Could not reach the local Antigravity quota service"),
        status_error,
    )))
}

fn failure(error: AppServerError) -> AppServerFailure {
    AppServerFailure { error, account_key: None, token_usage: None }
}

fn local_error(message: &str) -> AppServerError {
    AppServerError::Rpc { method: "gemini/local", message: message.to_owned() }
}

fn status_error(status: u32) -> AppServerError {
    let message = match status {
        401 | 403 => "Local Antigravity quota access was rejected".to_owned(),
        429 => "Local Antigravity quota check was rate-limited".to_owned(),
        _ => format!("Local Antigravity quota service returned HTTP {status}"),
    };
    local_error(&message)
}

fn post_local_json(
    port: u16,
    secure: bool,
    path_str: &str,
    body: &Value,
    csrf_token: &str,
    deadline: Instant,
) -> Result<Value, HttpError> {
    let session = unsafe {
        Handle::new(WinHttpOpen(
            w!("CodexStatus Antigravity local read-only"),
            WINHTTP_ACCESS_TYPE_NO_PROXY,
            PCWSTR::null(),
            PCWSTR::null(),
            0,
        ))?
    };
    unsafe {
        WinHttpSetTimeouts(session.0, 5_000, 5_000, 5_000, 20_000)?;
    }
    let connection = unsafe { Handle::new(WinHttpConnect(session.0, LOOPBACK_HOST, port, 0))? };
    let path: Vec<u16> = path_str.encode_utf16().chain(Some(0)).collect();
    let request = unsafe {
        Handle::new(WinHttpOpenRequest(
            connection.0,
            w!("POST"),
            PCWSTR(path.as_ptr()),
            PCWSTR::null(),
            PCWSTR::null(),
            ptr::null(),
            if secure { WINHTTP_FLAG_SECURE } else { WINHTTP_OPEN_REQUEST_FLAGS(0) },
        ))?
    };
    let disabled = WINHTTP_DISABLE_REDIRECTS | WINHTTP_DISABLE_COOKIES;
    unsafe {
        WinHttpSetOption(
            Some(request.0),
            WINHTTP_OPTION_DISABLE_FEATURE,
            Some(&disabled.to_ne_bytes()),
        )?;
        if secure {
            // This exception is scoped to a fixed loopback host. Antigravity's local
            // language server uses an ephemeral self-signed certificate.
            let security_flags = SECURITY_FLAG_IGNORE_UNKNOWN_CA
                | SECURITY_FLAG_IGNORE_CERT_CN_INVALID
                | SECURITY_FLAG_IGNORE_CERT_DATE_INVALID
                | SECURITY_FLAG_IGNORE_CERT_WRONG_USAGE;
            WinHttpSetOption(
                Some(request.0),
                WINHTTP_OPTION_SECURITY_FLAGS,
                Some(&security_flags.to_ne_bytes()),
            )?;
        }
    }
    let payload = serde_json::to_vec(body).map_err(|_| HttpError::Invalid)?;
    let headers: Vec<u16> = format!(
        "X-Codeium-Csrf-Token: {csrf_token}\r\nConnect-Protocol-Version: 1\r\nContent-Type: application/json\r\nAccept: application/json\r\n"
    )
    .encode_utf16()
    .collect();
    timeouts(&request, deadline)?;
    unsafe {
        WinHttpSendRequest(
            request.0,
            Some(&headers),
            Some(payload.as_ptr().cast()),
            payload.len() as u32,
            payload.len() as u32,
            0,
        )?;
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
        return Err(HttpError::Status(status, retry_after(&request)));
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
        if bytes.len() + read as usize > MAX_RESPONSE_BYTES {
            return Err(HttpError::Invalid);
        }
        bytes.extend_from_slice(&buffer[..read as usize]);
    }
    if Instant::now() >= deadline {
        return Err(HttpError::Network);
    }
    serde_json::from_slice(&bytes).map_err(|_| HttpError::Invalid)
}

fn timeouts(request: &Handle, deadline: Instant) -> Result<(), HttpError> {
    let remaining = deadline.saturating_duration_since(Instant::now()).as_millis();
    if remaining == 0 {
        return Err(HttpError::Network);
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

fn extract_plan_type(value: &Value) -> Option<String> {
    value
        .pointer("/userStatus/userTier/name")
        .or_else(|| value.pointer("/userStatus/userTier/id"))
        .or_else(|| value.pointer("/userStatus/planName"))
        .or_else(|| value.pointer("/userStatus/planStatus/planInfo/planName"))
        .or_else(|| value.pointer("/userStatus/planStatus/planInfo/planType"))
        .or_else(|| value.pointer("/planName"))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_owned)
}

fn extract_account_key(value: &Value) -> Option<String> {
    value
        .pointer("/userStatus/email")
        .or_else(|| value.get("email"))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(hash_account_email)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PoolKind {
    Gemini,
    Claude,
}

fn classify_bucket(bucket: &Value) -> Option<PoolKind> {
    let bucket_id = bucket.get("bucketId").and_then(Value::as_str)?;
    if bucket_id == "gemini-5h" || bucket_id == "gemini-weekly" {
        Some(PoolKind::Gemini)
    } else if bucket_id == "3p-5h" || bucket_id == "3p-weekly" {
        Some(PoolKind::Claude)
    } else {
        None
    }
}

fn parse_window_minutes(bucket: &Value) -> Option<u64> {
    match bucket.get("window").and_then(Value::as_str) {
        Some("5h") => return Some(SESSION_MINUTES),
        Some("weekly") => return Some(WEEK_MINUTES),
        _ => {}
    }
    match bucket.get("bucketId").and_then(Value::as_str) {
        Some("gemini-5h" | "3p-5h") => return Some(SESSION_MINUTES),
        Some("gemini-weekly" | "3p-weekly") => return Some(WEEK_MINUTES),
        _ => {}
    }
    if let Some(dur) = bucket.get("windowDuration").and_then(Value::as_str) {
        let trimmed = dur.trim();
        if let Some(secs_str) = trimmed.strip_suffix('s')
            && let Ok(secs) = secs_str.parse::<u64>()
        {
            return Some((secs / 60).max(1));
        }
    }
    if let Some(secs) = bucket.get("windowDurationSeconds").and_then(Value::as_u64) {
        return Some((secs / 60).max(1));
    }
    match bucket.get("bucketType").and_then(Value::as_str) {
        Some("rolling") => Some(SESSION_MINUTES),
        Some("weekly") => Some(WEEK_MINUTES),
        _ => None,
    }
}

fn parse_reset_time(bucket: &Value) -> Option<i64> {
    if let Some(ts) = bucket.get("resetTime").and_then(Value::as_i64) {
        return Some(ts);
    }
    bucket
        .get("resetTime")
        .and_then(Value::as_str)
        .and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok())
        .map(|dt| dt.timestamp())
}

fn parse_bucket_window(bucket: &Value) -> Option<QuotaWindow> {
    // QuotaSummary is protobuf JSON. A known bucket can omit its zero-valued
    // remainingFraction, which means exhausted rather than unknown.
    let remaining_fraction = bucket.get("remainingFraction").map_or(Some(0.0), Value::as_f64)?;
    if !remaining_fraction.is_finite() {
        return None;
    }
    let remaining_percent = (remaining_fraction.clamp(0.0, 1.0)) * 100.0;
    let used_percent = (100.0 - remaining_percent).clamp(0.0, 100.0);
    let window_minutes = parse_window_minutes(bucket)?;
    let resets_at = parse_reset_time(bucket);
    Some(QuotaWindow { used_percent, remaining_percent, window_minutes, resets_at })
}

fn assign_window(
    five_hour: &mut Option<QuotaWindow>,
    weekly: &mut Option<QuotaWindow>,
    window: QuotaWindow,
) {
    if window.window_minutes == SESSION_MINUTES && five_hour.is_none() {
        *five_hour = Some(window);
    } else if window.window_minutes == WEEK_MINUTES && weekly.is_none() {
        *weekly = Some(window);
    }
}

pub(crate) fn parse_quota_snapshot(
    value: &Value,
    plan_type: Option<String>,
    fetched_at: i64,
) -> Result<QuotaSnapshot, AppServerError> {
    let mut gem_5h = None;
    let mut gem_wk = None;
    let mut claude_5h = None;
    let mut claude_wk = None;

    let groups =
        value.get("groups").or_else(|| value.pointer("/response/groups")).and_then(Value::as_array);
    if let Some(groups) = groups {
        for bucket in groups
            .iter()
            .filter_map(|group| group.get("buckets").and_then(Value::as_array))
            .flatten()
        {
            if let (Some(pool), Some(window)) =
                (classify_bucket(bucket), parse_bucket_window(bucket))
            {
                match pool {
                    PoolKind::Gemini => assign_window(&mut gem_5h, &mut gem_wk, window),
                    PoolKind::Claude => assign_window(&mut claude_5h, &mut claude_wk, window),
                }
            }
        }
    }

    if gem_5h.is_none() && gem_wk.is_none() {
        return Err(AppServerError::Rpc {
            method: "gemini/quota",
            message: "Gemini quota response contained no recognizable rate-limit windows".into(),
        });
    }

    let claude_quotas = (claude_5h.is_some() || claude_wk.is_some())
        .then_some(SecondaryQuotas { five_hour: claude_5h, weekly: claude_wk });

    Ok(QuotaSnapshot {
        weekly: gem_wk,
        session: gem_5h,
        account: AccountSummary { plan_type, claude_quotas, ..AccountSummary::default() },
        fetched_at,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_dual_gemini_and_claude_quotas() {
        let payload = json!({
            "groups": [
                {
                    "displayName": "Gemini Models",
                    "buckets": [
                        {
                            "bucketId": "gemini-5h",
                            "window": "5h",
                            "remainingFraction": 0.85,
                            "resetTime": "2026-11-17T17:14:00Z"
                        },
                        {
                            "bucketId": "gemini-weekly",
                            "window": "weekly",
                            "remainingFraction": 0.66,
                            "resetTime": "2026-11-24T01:49:00Z"
                        }
                    ]
                },
                {
                    "displayName": "Claude and GPT models",
                    "buckets": [
                        {
                            "bucketId": "3p-5h",
                            "window": "5h",
                            "remainingFraction": 0.42,
                            "resetTime": "2026-11-17T16:30:00Z"
                        },
                        {
                            "bucketId": "3p-weekly",
                            "window": "weekly",
                            "remainingFraction": 0.78,
                            "resetTime": "2026-11-22T00:00:00Z"
                        }
                    ]
                }
            ]
        });

        let snap = parse_quota_snapshot(&payload, Some("AI Pro".into()), 1_700_000_000).unwrap();
        assert_eq!(snap.session.as_ref().unwrap().display_percent(), 85);
        assert_eq!(snap.weekly.as_ref().unwrap().display_percent(), 66);
        assert_eq!(snap.account.plan_type.as_deref(), Some("AI Pro"));
        let claude = snap.account.claude_quotas.as_ref().unwrap();
        assert_eq!(claude.five_hour.as_ref().unwrap().display_percent(), 42);
        assert_eq!(claude.weekly.as_ref().unwrap().display_percent(), 78);
        assert_eq!(snap.session.as_ref().unwrap().resets_at, Some(1_794_935_640));
        assert_eq!(claude.weekly.as_ref().unwrap().resets_at, Some(1_795_305_600));
    }

    #[test]
    fn parses_wrapped_summary_and_omitted_zero_fraction() {
        let payload = json!({
            "response": {
                "groups": [{
                    "displayName": "Gemini Models",
                    "buckets": [
                        {
                            "bucketId": "gemini-5h",
                            "window": "5h",
                            "resetTime": "2026-11-17T17:14:00Z"
                        },
                        {
                            "bucketId": "gemini-weekly",
                            "window": "weekly",
                            "remainingFraction": 0.9,
                            "resetTime": "2026-11-24T00:00:00Z"
                        },
                        {
                            "bucketId": "gemini-argon-wtu-weekly",
                            "window": "weekly",
                            "remainingFraction": 0.1
                        }
                    ]
                }]
            }
        });
        let snap = parse_quota_snapshot(&payload, None, 1_700_000_000).unwrap();
        assert_eq!(snap.session.as_ref().unwrap().display_percent(), 0);
        assert_eq!(snap.weekly.as_ref().unwrap().display_percent(), 90);
        assert!(snap.account.claude_quotas.is_none());
    }

    #[test]
    fn does_not_treat_third_party_group_as_gemini() {
        let payload = json!({
            "groups": [{
                "displayName": "Claude and GPT models",
                "buckets": [{
                    "bucketId": "3p-5h",
                    "window": "5h",
                    "remainingFraction": 0.5
                }]
            }]
        });
        assert!(parse_quota_snapshot(&payload, None, 1_700_000_000).is_err());
    }

    #[test]
    fn extracts_local_account_fields_without_exposing_email() {
        let payload = json!({
            "userStatus": {
                "planName": "AI Pro",
                "email": "User@Example.com"
            }
        });
        assert_eq!(extract_plan_type(&payload).as_deref(), Some("AI Pro"));
        let key = extract_account_key(&payload).unwrap();
        assert!(key.starts_with("gemini:"));
        assert!(!key.to_ascii_lowercase().contains("user@example.com"));
    }

    #[test]
    fn parses_antigravity_process_arguments_without_logging_token() {
        let line = r#"C:\Antigravity\language_server.exe --extension_server_port=6108 --csrf_token "secret-token_123" --other value"#;
        assert_eq!(command_argument(line, "--extension_server_port").as_deref(), Some("6108"));
        assert_eq!(command_argument(line, "--csrf_token").as_deref(), Some("secret-token_123"));
        assert!(command_argument(line, "--missing").is_none());
    }

    #[test]
    #[ignore = "explicit opt-in only: reads quota from the running local Antigravity process"]
    fn read_only_live_probe() {
        assert_eq!(std::env::var("CODEX_STATUS_GEMINI_LIVE_TEST").as_deref(), Ok("1"));
        let result = fetch(&Mutex::new(GeminiState::default())).expect("local quota fetch");
        let quota = result.quota;
        let claude = quota.account.claude_quotas.expect("Claude quota group");
        assert!(quota.session.is_some(), "missing Gemini 5h quota");
        assert!(quota.weekly.is_some(), "missing Gemini weekly quota");
        assert!(claude.five_hour.is_some(), "missing Claude 5h quota");
        assert!(claude.weekly.is_some(), "missing Claude weekly quota");
    }
}
