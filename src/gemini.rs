//! Read-only Gemini / Cloud Code Assist quota client matching Codex refresh & security invariants.

use crate::app_server::{AppServerError, AppServerFailure, AppServerSnapshot};
use crate::model::{
    AccountSummary, QuotaSnapshot, QuotaWindow, SESSION_MINUTES, SecondaryQuotas, WEEK_MINUTES,
};
use serde::Deserialize;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::ffi::c_void;
use std::io::Read;
use std::os::windows::io::AsRawHandle;
use std::os::windows::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitStatus, Stdio};
use std::ptr;
use std::sync::{Mutex, mpsc};
use std::time::{Duration, Instant};
use windows::Win32::Foundation::{CloseHandle, GlobalFree, HANDLE, HGLOBAL};
use windows::Win32::Networking::WinHttp::*;
use windows::Win32::System::Diagnostics::ToolHelp::{
    CreateToolhelp32Snapshot, TH32CS_SNAPTHREAD, THREADENTRY32, Thread32First, Thread32Next,
};
use windows::Win32::System::JobObjects::{
    AssignProcessToJobObject, CreateJobObjectW, JOB_OBJECT_LIMIT_ACTIVE_PROCESS,
    JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE, JOBOBJECT_EXTENDED_LIMIT_INFORMATION,
    JobObjectExtendedLimitInformation, SetInformationJobObject,
};
use windows::Win32::System::Threading::{
    CREATE_NO_WINDOW, CREATE_SUSPENDED, OpenThread, ResumeThread, THREAD_SUSPEND_RESUME,
};
use windows::core::{PCWSTR, PWSTR, w};
use winreg::RegKey;
use winreg::enums::HKEY_CURRENT_USER;

const LOOPBACK_HOST: PCWSTR = w!("127.0.0.1");
const GET_USER_STATUS_PATH: &str = "/exa.language_server_pb.LanguageServerService/GetUserStatus";
const RETRIEVE_QUOTA_PATH: &str =
    "/exa.language_server_pb.LanguageServerService/RetrieveUserQuotaSummary";
const PROCESS_QUERY: &str = r#"[Console]::OutputEncoding = [Text.UTF8Encoding]::new($false); $items = @(Get-CimInstance Win32_Process | Where-Object { ($_.Name -eq 'language_server.exe' -or $_.Name -eq 'language_server_windows_x64.exe') -and $_.CommandLine -like '*--csrf_token*' } | Select-Object ProcessId, Name, CommandLine); ConvertTo-Json -InputObject $items -Compress"#;
const MAX_DISCOVERY_BYTES: usize = 256 * 1024;
const MAX_RESPONSE_BYTES: usize = 512 * 1024;
const TOTAL_DEADLINE: Duration = Duration::from_secs(20);
const PROCESS_DISCOVERY_TIMEOUT: Duration = Duration::from_secs(8);
const NETSTAT_TIMEOUT: Duration = Duration::from_secs(5);
const READER_CLEANUP_TIMEOUT: Duration = Duration::from_secs(1);
const PROXY_CACHE_TTL: Duration = Duration::from_secs(300);
const AGY_RATE_LIMIT_BACKOFF: Duration = Duration::from_secs(300);
// `agy -p /usage --output-format json` currently omits email/account_id fields.
// When no account identifier is present, cap transient-error cache retention to
// 15 minutes so a switched account cannot keep stale quota across long periods.
const AGY_UNVERIFIED_CACHE_TTL: Duration = Duration::from_secs(15 * 60);
const AGY_ACCOUNT_KEY: &str = "gemini:agy-cli";

#[derive(Default)]
pub struct GeminiState {
    blocked_until: Option<Instant>,
    cached_proxy: Option<(Option<String>, Instant)>,
    last_agy_success: Option<Instant>,
    last_agy_account_key: Option<String>,
}

impl GeminiState {
    fn can_attempt(&mut self, now: Instant) -> bool {
        !self.blocked_until.is_some_and(|until| now < until)
    }

    fn block(&mut self, delay: Duration) {
        self.block_at(Instant::now(), delay);
    }

    fn block_at(&mut self, now: Instant, delay: Duration) {
        let until = now + delay;
        self.blocked_until = Some(self.blocked_until.map_or(until, |old| old.max(until)));
    }

    fn cached_proxy(&self, now: Instant) -> Option<Option<String>> {
        self.cached_proxy
            .as_ref()
            .filter(|(_, observed)| now.saturating_duration_since(*observed) < PROXY_CACHE_TTL)
            .map(|(proxy, _)| proxy.clone())
    }

    fn store_proxy(&mut self, proxy: Option<String>, now: Instant) {
        self.cached_proxy = Some((proxy, now));
    }

    fn invalidate_proxy(&mut self) {
        self.cached_proxy = None;
    }

    fn record_agy_success(&mut self, account_key: String, now: Instant) {
        self.last_agy_account_key = Some(account_key);
        self.last_agy_success = Some(now);
    }

    fn clear_agy_session(&mut self) {
        self.last_agy_account_key = None;
        self.last_agy_success = None;
    }

    fn fallback_agy_account_key(&self, now: Instant) -> Option<String> {
        let key = self.last_agy_account_key.as_deref()?;
        let observed = self.last_agy_success?;
        (key == AGY_ACCOUNT_KEY
            && now.saturating_duration_since(observed) < AGY_UNVERIFIED_CACHE_TTL)
            .then(|| key.to_owned())
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

fn system32_executable(relative: &str) -> PathBuf {
    if let Some(root) = std::env::var_os("SystemRoot").or_else(|| std::env::var_os("WINDIR")) {
        let candidate = PathBuf::from(root).join("System32").join(relative);
        if candidate.is_file() {
            return candidate;
        }
    }
    Path::new(relative).file_name().map_or_else(|| PathBuf::from(relative), PathBuf::from)
}

fn discover_local_processes() -> Result<Vec<LocalProcess>, AppServerError> {
    let mut cmd = Command::new(system32_executable(r"WindowsPowerShell\v1.0\powershell.exe"));
    cmd.args(["-NoProfile", "-NonInteractive", "-Command", PROCESS_QUERY]);
    let output = run_isolated_command(
        &mut cmd,
        false,
        PROCESS_DISCOVERY_TIMEOUT,
        MAX_DISCOVERY_BYTES,
        8 * 1024,
    )
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
    let mut cmd = Command::new(system32_executable("netstat.exe"));
    cmd.args(["-ano", "-p", "tcp"]);
    let Ok(output) =
        run_isolated_command(&mut cmd, false, NETSTAT_TIMEOUT, MAX_DISCOVERY_BYTES, 4 * 1024)
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

    if let Some(agy) = resolve_agy_binary() {
        return fetch_via_agy_cli(&agy, state);
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

fn resolve_agy_binary() -> Option<PathBuf> {
    if let Some(path_var) = std::env::var_os("PATH") {
        for dir in std::env::split_paths(&path_var) {
            let candidate = dir.join("agy.exe");
            if candidate.is_file() {
                return Some(candidate);
            }
        }
    }
    if let Some(local_app_data) = std::env::var_os("LOCALAPPDATA") {
        let candidate = PathBuf::from(local_app_data).join("agy").join("bin").join("agy.exe");
        if candidate.is_file() {
            return Some(candidate);
        }
    }
    None
}

fn pwstr_to_string_and_free(ptr: PWSTR) -> Option<String> {
    if ptr.is_null() {
        return None;
    }
    let text = unsafe { ptr.to_string().ok() };
    unsafe {
        let _ = GlobalFree(Some(HGLOBAL(ptr.0.cast())));
    }
    text.filter(|s| !s.trim().is_empty())
}

fn resolve_windows_proxy_for_url() -> Option<String> {
    let mut ie_config = WINHTTP_CURRENT_USER_IE_PROXY_CONFIG::default();
    if unsafe { WinHttpGetIEProxyConfigForCurrentUser(&mut ie_config) }.is_err() {
        return read_windows_system_proxy();
    }
    let auto_detect = ie_config.fAutoDetect.as_bool();
    let auto_config_url = pwstr_to_string_and_free(ie_config.lpszAutoConfigUrl);
    let fixed_proxy = pwstr_to_string_and_free(ie_config.lpszProxy);
    let _ = pwstr_to_string_and_free(ie_config.lpszProxyBypass);

    if let Some(proxy) = fixed_proxy.as_deref().and_then(normalize_proxy_server) {
        return Some(proxy);
    }

    if auto_config_url.is_some() || auto_detect {
        let session = unsafe {
            Handle::new(WinHttpOpen(
                w!("CodexStatus Proxy Resolver"),
                WINHTTP_ACCESS_TYPE_NO_PROXY,
                PCWSTR::null(),
                PCWSTR::null(),
                0,
            ))
            .ok()?
        };
        let pac_wide: Option<Vec<u16>> =
            auto_config_url.as_deref().map(|s| s.encode_utf16().chain(Some(0)).collect());
        let mut options =
            WINHTTP_AUTOPROXY_OPTIONS { fAutoLogonIfChallenged: true.into(), ..Default::default() };
        if let Some(ref wide) = pac_wide {
            options.dwFlags = WINHTTP_AUTOPROXY_CONFIG_URL;
            options.lpszAutoConfigUrl = PCWSTR(wide.as_ptr());
        } else {
            options.dwFlags = WINHTTP_AUTOPROXY_AUTO_DETECT;
            options.dwAutoDetectFlags =
                WINHTTP_AUTO_DETECT_TYPE_DHCP | WINHTTP_AUTO_DETECT_TYPE_DNS_A;
        }
        for probe_url in
            [w!("https://antigravity.google/"), w!("https://cloudcode-pa.googleapis.com/")]
        {
            let mut proxy_info = WINHTTP_PROXY_INFO::default();
            let ok = unsafe {
                WinHttpGetProxyForUrl(session.0, probe_url, &mut options, &mut proxy_info).is_ok()
            };
            let resolved = pwstr_to_string_and_free(proxy_info.lpszProxy);
            let _ = pwstr_to_string_and_free(proxy_info.lpszProxyBypass);
            if ok
                && proxy_info.dwAccessType == WINHTTP_ACCESS_TYPE_NAMED_PROXY
                && let Some(proxy) = resolved.as_deref().and_then(normalize_proxy_server)
            {
                return Some(proxy);
            }
        }
    }

    read_windows_system_proxy()
}

fn read_windows_system_proxy() -> Option<String> {
    let hkcu = RegKey::predef(HKEY_CURRENT_USER);
    let key =
        hkcu.open_subkey("Software\\Microsoft\\Windows\\CurrentVersion\\Internet Settings").ok()?;
    let enabled: u32 = key.get_value("ProxyEnable").unwrap_or(0);
    if enabled == 0 {
        return None;
    }
    let server: String = key.get_value("ProxyServer").ok()?;
    normalize_proxy_server(&server)
}

fn normalize_proxy_server(raw: &str) -> Option<String> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return None;
    }
    if trimmed.contains('=') {
        let mut https_val = None;
        let mut http_val = None;
        let mut socks_val = None;
        for part in trimmed.split(';') {
            let part = part.trim();
            if let Some(val) = part.strip_prefix("https=").or_else(|| part.strip_prefix("HTTPS=")) {
                https_val = Some(val.trim());
            } else if let Some(val) =
                part.strip_prefix("http=").or_else(|| part.strip_prefix("HTTP="))
            {
                http_val = Some(val.trim());
            } else if let Some(val) =
                part.strip_prefix("socks=").or_else(|| part.strip_prefix("SOCKS="))
            {
                socks_val = Some(val.trim());
            }
        }
        if let Some(val) = https_val.or(http_val).filter(|v| !v.is_empty()) {
            return Some(if val.contains("://") {
                val.to_owned()
            } else {
                format!("http://{val}")
            });
        }
        if let Some(val) = socks_val.filter(|v| !v.is_empty()) {
            return Some(if val.contains("://") {
                val.to_owned()
            } else {
                format!("socks5://{val}")
            });
        }
        return None;
    }

    for entry in trimmed.split(';') {
        let entry = entry.trim();
        if entry.is_empty() || entry.eq_ignore_ascii_case("DIRECT") {
            continue;
        }
        let mut parts = entry.split_whitespace();
        let first = parts.next()?;
        if first.eq_ignore_ascii_case("PROXY") || first.eq_ignore_ascii_case("HTTP") {
            if let Some(host) = parts.next() {
                return Some(if host.contains("://") {
                    host.to_owned()
                } else {
                    format!("http://{host}")
                });
            }
            continue;
        }
        if first.eq_ignore_ascii_case("HTTPS") {
            if let Some(host) = parts.next() {
                return Some(if host.contains("://") {
                    host.to_owned()
                } else {
                    format!("https://{host}")
                });
            }
            continue;
        }
        if first.eq_ignore_ascii_case("SOCKS") || first.eq_ignore_ascii_case("SOCKS5") {
            if let Some(host) = parts.next() {
                return Some(if host.contains("://") {
                    host.to_owned()
                } else {
                    format!("socks5://{host}")
                });
            }
            continue;
        }
        return Some(if first.contains("://") {
            first.to_owned()
        } else {
            format!("http://{first}")
        });
    }
    None
}

struct SuspendedThread(HANDLE);

impl SuspendedThread {
    fn find(process_id: u32) -> windows::core::Result<Self> {
        unsafe {
            let snapshot = CreateToolhelp32Snapshot(TH32CS_SNAPTHREAD, 0)?;
            let mut entry = THREADENTRY32 {
                dwSize: std::mem::size_of::<THREADENTRY32>() as u32,
                ..Default::default()
            };
            let result = (|| {
                Thread32First(snapshot, &mut entry)?;
                loop {
                    if entry.th32OwnerProcessID == process_id {
                        return OpenThread(THREAD_SUSPEND_RESUME, false, entry.th32ThreadID)
                            .map(Self);
                    }
                    if Thread32Next(snapshot, &mut entry).is_err() {
                        return Err(windows::core::Error::from_thread());
                    }
                }
            })();
            let _ = CloseHandle(snapshot);
            result
        }
    }

    fn resume(&self) -> windows::core::Result<()> {
        let previous_count = unsafe { ResumeThread(self.0) };
        if previous_count == u32::MAX { Err(windows::core::Error::from_thread()) } else { Ok(()) }
    }
}

impl Drop for SuspendedThread {
    fn drop(&mut self) {
        unsafe {
            let _ = CloseHandle(self.0);
        }
    }
}

struct KillOnCloseJob(HANDLE);

impl KillOnCloseJob {
    fn attach(child: &Child, single_process: bool) -> windows::core::Result<Self> {
        unsafe {
            let job = CreateJobObjectW(None, None)?;
            let mut limits = JOBOBJECT_EXTENDED_LIMIT_INFORMATION::default();
            limits.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
            if single_process {
                // Prevent agy.exe from spawning detached background helpers (such as
                // `agy.exe --bg-updater`, which lacks CREATE_NO_WINDOW and flashes
                // Windows Terminal every ~15 minutes on Windows 11).
                limits.BasicLimitInformation.LimitFlags |= JOB_OBJECT_LIMIT_ACTIVE_PROCESS;
                limits.BasicLimitInformation.ActiveProcessLimit = 1;
            }
            if let Err(error) = SetInformationJobObject(
                job,
                JobObjectExtendedLimitInformation,
                (&raw const limits).cast(),
                std::mem::size_of_val(&limits) as u32,
            ) {
                let _ = CloseHandle(job);
                return Err(error);
            }
            let process = HANDLE(child.as_raw_handle());
            if let Err(error) = AssignProcessToJobObject(job, process) {
                let _ = CloseHandle(job);
                return Err(error);
            }
            Ok(Self(job))
        }
    }
}

impl Drop for KillOnCloseJob {
    fn drop(&mut self) {
        unsafe {
            let _ = CloseHandle(self.0);
        }
    }
}

struct CommandOutput {
    status: ExitStatus,
    stdout: Vec<u8>,
    stderr: Vec<u8>,
}

enum IsolatedCommandError {
    Spawn,
    Isolation,
    Timeout,
    Wait,
}

fn terminate_child(child: &mut Child) {
    if child.try_wait().ok().flatten().is_none() {
        let _ = child.kill();
    }
    let _ = child.wait();
}

fn run_isolated_command(
    cmd: &mut Command,
    single_process: bool,
    timeout: Duration,
    max_stdout: usize,
    max_stderr: usize,
) -> Result<CommandOutput, IsolatedCommandError> {
    cmd.stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .creation_flags(CREATE_NO_WINDOW.0 | CREATE_SUSPENDED.0);

    let mut child = cmd.spawn().map_err(|_| IsolatedCommandError::Spawn)?;
    let suspended_thread = match SuspendedThread::find(child.id()) {
        Ok(thread) => thread,
        Err(_) => {
            terminate_child(&mut child);
            return Err(IsolatedCommandError::Isolation);
        }
    };
    let job = match KillOnCloseJob::attach(&child, single_process) {
        Ok(job) => job,
        Err(_) => {
            terminate_child(&mut child);
            return Err(IsolatedCommandError::Isolation);
        }
    };
    if suspended_thread.resume().is_err() {
        drop(job);
        terminate_child(&mut child);
        return Err(IsolatedCommandError::Isolation);
    }
    drop(suspended_thread);

    let (stdout_tx, stdout_rx) = mpsc::sync_channel(1);
    let stdout_handle = child.stdout.take().map(|mut pipe| {
        std::thread::Builder::new()
            .name("codex-status-gemini-stdout".into())
            .stack_size(256 * 1024)
            .spawn(move || {
                let mut buf = Vec::new();
                let _ = (&mut pipe).take(max_stdout as u64 + 1).read_to_end(&mut buf);
                let _ = stdout_tx.send(buf);
            })
            .ok()
    });
    let (stderr_tx, stderr_rx) = mpsc::sync_channel(1);
    let stderr_handle = child.stderr.take().map(|mut pipe| {
        std::thread::Builder::new()
            .name("codex-status-gemini-stderr".into())
            .stack_size(256 * 1024)
            .spawn(move || {
                let mut buf = Vec::new();
                let _ = (&mut pipe).take(max_stderr as u64).read_to_end(&mut buf);
                let _ = stderr_tx.send(buf);
            })
            .ok()
    });

    let deadline = Instant::now() + timeout;
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break Ok(status),
            Ok(None) => {
                if Instant::now() >= deadline {
                    break Err(IsolatedCommandError::Timeout);
                }
                std::thread::sleep(Duration::from_millis(50));
            }
            Err(_) => break Err(IsolatedCommandError::Wait),
        }
    };

    // Close the job first to terminate the entire process tree so any inherited
    // stdout/stderr pipes close before boundedly reclaiming the reader threads.
    drop(job);
    if status.is_err() {
        terminate_child(&mut child);
    }
    let stdout = stdout_rx.recv_timeout(READER_CLEANUP_TIMEOUT).unwrap_or_default();
    let stderr = stderr_rx.recv_timeout(READER_CLEANUP_TIMEOUT).unwrap_or_default();
    drop(stdout_handle);
    drop(stderr_handle);

    let status = status?;
    Ok(CommandOutput { status, stdout, stderr })
}

fn run_agy_json(
    binary: &Path,
    slash_cmd: &str,
    proxy: Option<&str>,
    timeout: Duration,
) -> Result<Value, AppServerError> {
    let mut cmd = Command::new(binary);
    cmd.args(["-p", slash_cmd, "--output-format", "json", "--print-timeout", "15s"])
        .current_dir(std::env::temp_dir())
        .env("AGY_DISABLE_AUTO_UPDATE", "1")
        .env("AGY_CLI_DISABLE_AUTO_UPDATE", "1");

    if let Some(proxy) = proxy {
        cmd.env("HTTP_PROXY", proxy);
        cmd.env("HTTPS_PROXY", proxy);
    }

    let output = run_isolated_command(&mut cmd, true, timeout, MAX_RESPONSE_BYTES, 16 * 1024)
        .map_err(|error| match error {
            IsolatedCommandError::Spawn => local_error("Could not launch Antigravity CLI (agy)"),
            IsolatedCommandError::Isolation => {
                local_error("Could not isolate Antigravity CLI (agy) process")
            }
            IsolatedCommandError::Timeout => local_error("Antigravity CLI (agy) timed out"),
            IsolatedCommandError::Wait => local_error("Failed waiting for Antigravity CLI (agy)"),
        })?;

    if output.stdout.len() > MAX_RESPONSE_BYTES {
        return Err(local_error("Antigravity CLI (agy) response exceeded size limit"));
    }

    if !output.status.success() {
        let detail = String::from_utf8_lossy(&output.stderr).to_ascii_lowercase();
        if detail.contains("login") || detail.contains("auth") || detail.contains("sign in") {
            return Err(local_error("Sign in to Antigravity CLI (agy), then refresh"));
        }
        if is_rate_limit_text(&detail) {
            return Err(local_error("Antigravity CLI (agy) was rate-limited"));
        }
        return Err(local_error("Antigravity CLI (agy) failed to fetch quota"));
    }

    let value: Value = serde_json::from_slice(&output.stdout)
        .map_err(|_| local_error("Invalid JSON from Antigravity CLI (agy)"))?;
    if let Some(st) = value.get("status").and_then(Value::as_str)
        && !st.eq_ignore_ascii_case("SUCCESS")
    {
        let err_text = value
            .get("error")
            .or_else(|| value.get("message"))
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_ascii_lowercase();
        if is_rate_limit_text(&err_text) {
            return Err(local_error("Antigravity CLI (agy) was rate-limited"));
        }
        return Err(local_error("Antigravity CLI (agy) reported an error"));
    }
    Ok(value)
}

fn is_rate_limit_text(text: &str) -> bool {
    text.split(|c: char| !c.is_ascii_alphanumeric()).any(|part| part == "429")
        || text.contains("rate limit")
        || text.contains("rate-limit")
        || text.contains("too many requests")
        || text.contains("resource_exhausted")
}

fn agy_failure(error: AppServerError, state: &Mutex<GeminiState>) -> AppServerFailure {
    let is_auth = matches!(&error, AppServerError::Rpc { message, .. } if {
        let lower = message.to_ascii_lowercase();
        lower.contains("sign in") || lower.contains("login") || lower.contains("auth")
    });
    let account_key = if let Ok(mut guard) = state.lock() {
        if is_auth {
            guard.clear_agy_session();
            None
        } else {
            guard.fallback_agy_account_key(Instant::now())
        }
    } else {
        None
    };
    AppServerFailure { error, account_key, token_usage: None }
}

fn fetch_via_agy_cli(
    binary: &Path,
    state: &Mutex<GeminiState>,
) -> Result<AppServerSnapshot, AppServerFailure> {
    let has_env_proxy = std::env::var_os("HTTPS_PROXY")
        .or_else(|| std::env::var_os("https_proxy"))
        .or_else(|| std::env::var_os("HTTP_PROXY"))
        .or_else(|| std::env::var_os("http_proxy"))
        .or_else(|| std::env::var_os("ALL_PROXY"))
        .or_else(|| std::env::var_os("all_proxy"))
        .is_some();
    let proxy = if has_env_proxy {
        None
    } else {
        let now = Instant::now();
        let cached = state.lock().ok().and_then(|guard| guard.cached_proxy(now));
        match cached {
            Some(proxy) => proxy,
            None => {
                let resolved = resolve_windows_proxy_for_url();
                if let Ok(mut guard) = state.lock() {
                    guard.store_proxy(resolved.clone(), now);
                }
                resolved
            }
        }
    };
    let usage_value =
        match run_agy_json(binary, "/usage", proxy.as_deref(), Duration::from_secs(20)) {
            Ok(value) => value,
            Err(error) => {
                if let Ok(mut guard) = state.lock() {
                    if !has_env_proxy {
                        guard.invalidate_proxy();
                    }
                    if is_rate_limit_text(&error.to_string().to_ascii_lowercase()) {
                        guard.block(AGY_RATE_LIMIT_BACKOFF);
                    }
                }
                return Err(agy_failure(error, state));
            }
        };
    let snapshot = parse_quota_snapshot(&usage_value, None, chrono::Utc::now().timestamp())
        .map_err(|error| agy_failure(error, state))?;
    // Prefer a hashed account identifier if `agy` includes one; otherwise fall back to
    // the shared `AGY_ACCOUNT_KEY` whose error-time reuse is bounded by AGY_UNVERIFIED_CACHE_TTL.
    let account_key =
        extract_agy_account_key(&usage_value).unwrap_or_else(|| AGY_ACCOUNT_KEY.to_owned());
    if let Ok(mut guard) = state.lock() {
        guard.record_agy_success(account_key.clone(), Instant::now());
    }
    Ok(AppServerSnapshot {
        quota: snapshot,
        token_usage: None,
        account_key: Some(account_key),
        supplement_allowed: false,
    })
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

fn extract_agy_account_key(value: &Value) -> Option<String> {
    extract_account_key(value).or_else(|| {
        [
            "/command/data/email",
            "/command/data/account_id",
            "/command/data/accountId",
            "/account_id",
            "/accountId",
        ]
        .into_iter()
        .find_map(|pointer| value.pointer(pointer).and_then(Value::as_str))
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(hash_account_email)
    })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PoolKind {
    Gemini,
    Claude,
}

fn bucket_identifier(bucket: &Value) -> Option<&str> {
    bucket.get("bucketId").or_else(|| bucket.get("id")).and_then(Value::as_str)
}

fn classify_bucket(bucket: &Value) -> Option<PoolKind> {
    let bucket_id = bucket_identifier(bucket)?;
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
    match bucket_identifier(bucket) {
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
    let field = bucket.get("resetTime").or_else(|| bucket.get("reset_time"))?;
    let ts = if let Some(ts) = field.as_i64() {
        Some(ts)
    } else {
        field
            .as_str()
            .and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok())
            .map(|dt| dt.timestamp())
    };
    ts.filter(|value| *value > 0)
}

fn parse_bucket_window(bucket: &Value) -> Option<QuotaWindow> {
    // QuotaSummary is protobuf JSON. A known bucket can omit its zero-valued
    // remainingFraction / remaining_fraction, which means exhausted rather than unknown.
    let remaining_fraction = bucket
        .get("remainingFraction")
        .or_else(|| bucket.get("remaining_fraction"))
        .map_or(Some(0.0), Value::as_f64)?;
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

    let groups = value
        .pointer("/command/data/groups")
        .or_else(|| value.get("groups"))
        .or_else(|| value.pointer("/response/groups"))
        .and_then(Value::as_array);
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
    fn parses_agy_cli_usage_and_credits_json() {
        let usage = json!({
            "status": "SUCCESS",
            "command": {
                "name": "usage",
                "data": {
                    "groups": [
                        {
                            "name": "Gemini Models",
                            "buckets": [
                                {
                                    "id": "gemini-weekly",
                                    "window": "weekly",
                                    "remaining_fraction": 0.805,
                                    "reset_time": "2026-11-25T00:49:58Z"
                                },
                                {
                                    "id": "gemini-5h",
                                    "window": "5h",
                                    "remaining_fraction": 0.975,
                                    "reset_time": "2026-11-21T06:12:31Z"
                                }
                            ]
                        },
                        {
                            "name": "Claude and GPT models",
                            "buckets": [
                                {
                                    "id": "3p-weekly",
                                    "window": "weekly",
                                    "remaining_fraction": 1,
                                    "reset_time": "2026-11-28T01:47:20Z"
                                },
                                {
                                    "id": "3p-5h",
                                    "window": "5h",
                                    "remaining_fraction": 1,
                                    "reset_time": "2026-11-21T06:47:20Z"
                                }
                            ]
                        }
                    ]
                }
            }
        });
        let snap = parse_quota_snapshot(&usage, None, 1_795_225_600).unwrap();
        assert_eq!(snap.weekly.as_ref().unwrap().display_percent(), 81);
        assert_eq!(snap.session.as_ref().unwrap().display_percent(), 98);
        let claude = snap.account.claude_quotas.as_ref().unwrap();
        assert_eq!(claude.weekly.as_ref().unwrap().display_percent(), 100);
        assert_eq!(claude.five_hour.as_ref().unwrap().display_percent(), 100);
    }

    #[test]
    fn parse_reset_time_treats_epoch_zero_and_non_positive_as_none() {
        assert_eq!(parse_reset_time(&json!({ "reset_time": "1970-01-01T00:00:00Z" })), None);
        assert_eq!(parse_reset_time(&json!({ "resetTime": "1970-01-01T00:00:00Z" })), None);
        assert_eq!(parse_reset_time(&json!({ "reset_time": 0 })), None);
        assert_eq!(parse_reset_time(&json!({ "reset_time": -1 })), None);
        assert_eq!(
            parse_reset_time(&json!({ "reset_time": "2026-11-17T17:14:00Z" })),
            Some(1_794_935_640)
        );
        assert_eq!(parse_reset_time(&json!({ "reset_time": 1_794_935_640 })), Some(1_794_935_640));
    }

    #[test]
    fn normalizes_windows_proxy_server_values() {
        assert_eq!(
            normalize_proxy_server("127.0.0.1:7897").as_deref(),
            Some("http://127.0.0.1:7897")
        );
        assert_eq!(
            normalize_proxy_server("http=127.0.0.1:8080;https=127.0.0.1:8443").as_deref(),
            Some("http://127.0.0.1:8443")
        );
        assert_eq!(
            normalize_proxy_server("PROXY 127.0.0.1:7897; DIRECT").as_deref(),
            Some("http://127.0.0.1:7897")
        );
        assert_eq!(
            normalize_proxy_server("SOCKS 127.0.0.1:7897; DIRECT").as_deref(),
            Some("socks5://127.0.0.1:7897")
        );
        assert_eq!(
            normalize_proxy_server("socks5://127.0.0.1:1080").as_deref(),
            Some("socks5://127.0.0.1:1080")
        );
        assert_eq!(normalize_proxy_server("DIRECT"), None);
        assert_eq!(normalize_proxy_server("   "), None);
    }

    #[test]
    fn gemini_state_caches_proxy_within_ttl_and_invalidates_on_reset() {
        let mut state = GeminiState::default();
        let start = Instant::now();
        assert_eq!(state.cached_proxy(start), None);

        state.store_proxy(Some("http://127.0.0.1:7897".into()), start);
        assert_eq!(
            state.cached_proxy(start + Duration::from_secs(60)),
            Some(Some("http://127.0.0.1:7897".into()))
        );
        assert_eq!(state.cached_proxy(start + PROXY_CACHE_TTL), None);

        state.store_proxy(None, start);
        assert_eq!(state.cached_proxy(start + Duration::from_secs(10)), Some(None));
        state.invalidate_proxy();
        assert_eq!(state.cached_proxy(start + Duration::from_secs(10)), None);
    }

    #[test]
    fn recognizes_agy_rate_limit_signals_and_enforces_backoff() {
        for text in [
            "HTTP 429 too many requests",
            "error: rate limit exceeded",
            "status: RESOURCE_EXHAUSTED",
        ] {
            assert!(is_rate_limit_text(&text.to_ascii_lowercase()), "{text}");
        }
        assert!(!is_rate_limit_text("connection timed out"));

        let mut state = GeminiState::default();
        let now = Instant::now();
        assert!(state.can_attempt(now));
        state.block_at(now, AGY_RATE_LIMIT_BACKOFF);
        assert!(!state.can_attempt(now + Duration::from_secs(60)));
        assert!(state.can_attempt(now + AGY_RATE_LIMIT_BACKOFF));
    }

    #[test]
    fn process_query_forces_utf8_output_and_system32_resolves_windows_tools() {
        assert!(
            PROCESS_QUERY
                .starts_with("[Console]::OutputEncoding = [Text.UTF8Encoding]::new($false);")
        );
        let ps = system32_executable(r"WindowsPowerShell\v1.0\powershell.exe");
        assert!(ps.to_string_lossy().to_ascii_lowercase().ends_with("powershell.exe"));
        let netstat = system32_executable("netstat.exe");
        assert!(netstat.to_string_lossy().to_ascii_lowercase().ends_with("netstat.exe"));
    }

    #[test]
    fn agy_account_key_hashes_explicit_identity_and_expires_unverified_fallback() {
        assert_eq!(
            extract_agy_account_key(&json!({"status": "SUCCESS", "command": {"data": {}}})),
            None
        );
        let hashed = extract_agy_account_key(&json!({
            "command": {"data": {"account_id": "acct-42"}}
        }))
        .unwrap();
        assert!(hashed.starts_with("gemini:"));
        assert!(!hashed.contains("acct-42"));

        let mut state = GeminiState::default();
        let now = Instant::now();
        assert_eq!(state.fallback_agy_account_key(now), None);

        state.record_agy_success(AGY_ACCOUNT_KEY.to_owned(), now);
        assert_eq!(
            state.fallback_agy_account_key(now + Duration::from_secs(60)).as_deref(),
            Some(AGY_ACCOUNT_KEY)
        );
        assert_eq!(state.fallback_agy_account_key(now + AGY_UNVERIFIED_CACHE_TTL), None);

        state.clear_agy_session();
        assert_eq!(state.fallback_agy_account_key(now + Duration::from_secs(10)), None);
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
