use crate::model::{
    ParseError, QuotaSnapshot, TokenUsageSnapshot, parse_snapshot, parse_token_usage,
};
use serde::Deserialize;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::collections::{HashMap, hash_map::Entry};
use std::env;
use std::ffi::{OsStr, OsString};
use std::io::{Read, Write};
use std::os::windows::io::AsRawHandle;
use std::os::windows::process::CommandExt;
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};
use windows::Win32::Foundation::{CloseHandle, HANDLE};
use windows::Win32::System::Diagnostics::ToolHelp::{
    CreateToolhelp32Snapshot, TH32CS_SNAPTHREAD, THREADENTRY32, Thread32First, Thread32Next,
};
use windows::Win32::System::JobObjects::{
    AssignProcessToJobObject, CreateJobObjectW, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
    JOBOBJECT_EXTENDED_LIMIT_INFORMATION, JobObjectExtendedLimitInformation,
    SetInformationJobObject,
};
use windows::Win32::System::Threading::{
    CREATE_NO_WINDOW, CREATE_SUSPENDED, OpenThread, ResumeThread, THREAD_SUSPEND_RESUME,
};

const REQUEST_TIMEOUT: Duration = Duration::from_secs(8);
const OPTIONAL_USAGE_GRACE: Duration = Duration::from_millis(300);
const READER_CLEANUP_TIMEOUT: Duration = Duration::from_secs(1);
const ERROR_LIMIT: usize = 240;
const STDOUT_LINE_LIMIT: usize = 256 * 1024;

#[derive(Debug, thiserror::Error)]
pub enum AppServerError {
    #[error("Codex is not installed or is not available on PATH")]
    CodexNotFound,
    #[error("Node.js is required for this Codex installation")]
    NodeNotFound,
    #[error("Unsupported Codex wrapper: {0}")]
    UnsupportedWrapper(String),
    #[error("Could not start Codex app-server: {0}")]
    Spawn(#[source] std::io::Error),
    #[error("Could not isolate the Codex app-server process tree: {0}")]
    ProcessIsolation(String),
    #[error("Could not communicate with Codex app-server: {0}")]
    Io(#[source] std::io::Error),
    #[error("Codex app-server did not respond within 8 seconds")]
    Timeout,
    #[error("Codex app-server closed before returning quota data")]
    Closed,
    #[error("Codex app-server rejected {method}: {message}")]
    Rpc { method: &'static str, message: String },
    #[error(transparent)]
    Parse(#[from] ParseError),
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct CommandSpec {
    program: PathBuf,
    args: Vec<OsString>,
}

#[derive(Debug, Clone)]
pub struct AppServerClient {
    commands: Result<Vec<CommandSpec>, String>,
}

#[derive(Debug, Clone)]
pub struct AppServerSnapshot {
    pub quota: QuotaSnapshot,
    pub token_usage: Option<TokenUsageSnapshot>,
    pub account_key: Option<String>,
    pub(crate) supplement_allowed: bool,
}

#[derive(Debug, thiserror::Error)]
#[error("{error}")]
pub struct AppServerFailure {
    #[source]
    pub error: AppServerError,
    pub account_key: Option<String>,
    pub(crate) token_usage: Option<TokenUsageSnapshot>,
}

impl AppServerFailure {
    fn unscoped(error: AppServerError) -> Self {
        Self { error, account_key: None, token_usage: None }
    }
}

#[derive(Debug, thiserror::Error)]
#[error("{error}")]
pub struct ResetFailure {
    pub error: AppServerError,
    pub may_have_sent: bool,
}

impl AppServerClient {
    pub fn consume_reset_credit(
        &self,
        attempt: &crate::reset::ResetAttempt,
    ) -> Result<crate::reset::ResetOutcome, ResetFailure> {
        if !attempt.valid() {
            return Err(ResetFailure {
                may_have_sent: false,
                error: AppServerError::Rpc {
                    method: "reset",
                    message: "Invalid reset attempt".into(),
                },
            });
        }
        let commands = self.commands.as_ref().map_err(|_| ResetFailure {
            error: AppServerError::CodexNotFound,
            may_have_sent: false,
        })?;
        for (index, command) in commands.iter().enumerate() {
            let mut may_have_sent = false;
            let result = with_session(command, |stdin, receiver| {
                consume_in_session(stdin, receiver, attempt, &mut may_have_sent)
            });
            match result {
                Err(AppServerError::Spawn(ref e))
                    if index + 1 < commands.len()
                        && matches!(
                            e.kind(),
                            std::io::ErrorKind::PermissionDenied | std::io::ErrorKind::NotFound
                        ) =>
                {
                    continue;
                }
                other => return other.map_err(|error| ResetFailure { error, may_have_sent }),
            }
        }
        Err(ResetFailure { error: AppServerError::CodexNotFound, may_have_sent: false })
    }

    pub fn new() -> Self {
        Self { commands: resolve_commands().map_err(|error| error.to_string()) }
    }

    pub fn fetch(&self) -> Result<AppServerSnapshot, AppServerFailure> {
        let commands = self.commands.as_ref().map_err(|message| {
            let error = if message.contains("Node.js") {
                AppServerError::NodeNotFound
            } else if message.contains("wrapper") {
                AppServerError::UnsupportedWrapper(message.clone())
            } else {
                AppServerError::CodexNotFound
            };
            AppServerFailure::unscoped(error)
        })?;
        let mut last_spawn_error = None;
        for command in commands {
            match fetch_with_command(command) {
                Err(AppServerFailure { error: AppServerError::Spawn(source), .. })
                    if matches!(
                        source.kind(),
                        std::io::ErrorKind::PermissionDenied | std::io::ErrorKind::NotFound
                    ) =>
                {
                    last_spawn_error = Some(AppServerError::Spawn(source));
                }
                result => return result,
            }
        }
        Err(AppServerFailure::unscoped(last_spawn_error.unwrap_or(AppServerError::CodexNotFound)))
    }
}

impl Default for AppServerClient {
    fn default() -> Self {
        Self::new()
    }
}

fn fetch_with_command(command: &CommandSpec) -> Result<AppServerSnapshot, AppServerFailure> {
    let mut verified_account_key = None;
    let mut token_usage = None;
    fetch_with_command_scoped(command, &mut verified_account_key, &mut token_usage)
        .map_err(|error| AppServerFailure { error, account_key: verified_account_key, token_usage })
}

fn fetch_with_command_scoped(
    command: &CommandSpec,
    verified_account_key: &mut Option<String>,
    token_usage: &mut Option<TokenUsageSnapshot>,
) -> Result<AppServerSnapshot, AppServerError> {
    let mut supplement_allowed = true;
    let mut rpc_allowed = true;
    let result = with_session_observed(
        command,
        |stdin, receiver| {
            write_json(
                stdin,
                &json!({"method":"account/read","id":1,"params":{"refreshToken":false}}),
            )?;
            write_json(stdin, &json!({"method":"account/rateLimits/read","id":2}))?;
            write_json(stdin, &json!({"method":"account/usage/read","id":3}))?;
            let responses = receive_account_responses(
                receiver,
                REQUEST_TIMEOUT,
                verified_account_key,
                &mut rpc_allowed,
                token_usage,
            )?;
            let (snapshot, account_key) = parse_account_responses(&responses);
            *verified_account_key = account_key;
            snapshot
        },
        &mut supplement_allowed,
    );
    supplement_allowed &= rpc_allowed;
    match result {
        Ok(mut snapshot) => {
            snapshot.supplement_allowed &= supplement_allowed;
            Ok(snapshot)
        }
        Err(_) if !supplement_allowed => Err(AppServerError::Rpc {
            method: "account/rateLimits/read",
            message: "Authentication or rate limit response; fallback suppressed".into(),
        }),
        Err(error) => Err(error),
    }
}

fn with_session<T>(
    command: &CommandSpec,
    operation: impl FnOnce(
        &mut std::process::ChildStdin,
        &mpsc::Receiver<String>,
    ) -> Result<T, AppServerError>,
) -> Result<T, AppServerError> {
    with_session_observed(command, operation, &mut true)
}

fn with_session_observed<T>(
    command: &CommandSpec,
    operation: impl FnOnce(
        &mut std::process::ChildStdin,
        &mpsc::Receiver<String>,
    ) -> Result<T, AppServerError>,
    supplement_allowed: &mut bool,
) -> Result<T, AppServerError> {
    let (mut child, suspended_thread) = spawn_suspended(command)?;
    let job = match JobGuard::assign(&child) {
        Ok(job) => job,
        Err(error) => {
            terminate(&mut child);
            return Err(AppServerError::ProcessIsolation(error.to_string()));
        }
    };
    if let Err(error) = suspended_thread.resume() {
        terminate(&mut child);
        return Err(AppServerError::ProcessIsolation(error.to_string()));
    }
    let stdout = child.stdout.take().ok_or(AppServerError::Closed)?;
    let stderr = child.stderr.take().ok_or(AppServerError::Closed)?;
    let mut stdin = child.stdin.take().ok_or(AppServerError::Closed)?;
    let (sender, receiver) = mpsc::channel::<String>();
    let reader = thread::Builder::new()
        .name("codex-status-stdout".to_owned())
        .stack_size(256 * 1024)
        .spawn(move || {
            forward_bounded_lines(stdout, sender);
        })
        .map_err(AppServerError::Io)?;
    let (error_sender, error_receiver) = mpsc::channel();
    let error_reader = thread::Builder::new()
        .name("codex-status-stderr".to_owned())
        .stack_size(256 * 1024)
        .spawn(move || {
            let _ = error_sender.send(read_capped(stderr, 4096));
        })
        .map_err(AppServerError::Io)?;

    let result = (|| {
        write_json(
            &mut stdin,
            &json!({
                "method": "initialize",
                "id": 0,
                "params": {"clientInfo": {
                    "name": "codex_status",
                    "title": "CodexStatus",
                    "version": env!("CARGO_PKG_VERSION")
                }}
            }),
        )?;
        let initialize = receive_response(&receiver, 0, "initialize", REQUEST_TIMEOUT)?;
        response_result(&initialize, "initialize")?;

        write_json(&mut stdin, &json!({"method": "initialized", "params": {}}))?;
        operation(&mut stdin, &receiver)
    })();

    drop(stdin);
    // Closing the job first terminates the complete app-server process tree. A
    // descendant can inherit stdout/stderr, so joining readers before this
    // close could otherwise block forever even after the direct child exits.
    drop(job);
    terminate(&mut child);
    let stderr = error_receiver.recv_timeout(READER_CLEANUP_TIMEOUT).unwrap_or_default();
    *supplement_allowed = !crate::refresh::blocked_error(&stderr);
    // Dropping a JoinHandle detaches it. The job close normally makes both
    // readers finish immediately; the bounded receive also protects the rare
    // case where assigning the process to a job was rejected by Windows.
    drop(receiver);
    drop(reader);
    drop(error_reader);

    match result {
        Err(AppServerError::Closed) if !stderr.trim().is_empty() => {
            Err(AppServerError::Rpc { method: "app-server", message: sanitize(&stderr) })
        }
        other => other,
    }
}

fn consume_in_session(
    writer: &mut impl Write,
    receiver: &mpsc::Receiver<String>,
    attempt: &crate::reset::ResetAttempt,
    may_have_sent: &mut bool,
) -> Result<crate::reset::ResetOutcome, AppServerError> {
    // Verify identity in this same isolated session before the only mutating RPC.
    write_json(writer, &json!({"method":"account/read","id":1,"params":{"refreshToken":false}}))?;
    let response = receive_response(receiver, 1, "account/read", REQUEST_TIMEOUT)?;
    if account_key(response_result(&response, "account/read")?).as_ref()
        != Some(&attempt.account_key)
    {
        return Err(AppServerError::Rpc {
            method: "reset",
            message: "Account changed; reset cancelled".into(),
        });
    }
    const METHOD: &str = "account/rateLimitResetCredit/consume";
    // A failed write can still have delivered part/all of the request.
    *may_have_sent = true;
    write_json(
        writer,
        &json!({"method":METHOD,"id":2,"params":{
            "idempotencyKey":attempt.idempotency_key,"creditId":attempt.credit.id
        }}),
    )?;
    let response = receive_response(receiver, 2, METHOD, REQUEST_TIMEOUT)?;
    let result = response_result(&response, METHOD)?;
    use crate::reset::ResetOutcome;
    match result.get("outcome").and_then(Value::as_str) {
        Some("reset") => Ok(ResetOutcome::Reset),
        Some("alreadyRedeemed") => Ok(ResetOutcome::AlreadyRedeemed),
        Some("nothingToReset") => Ok(ResetOutcome::NothingToReset),
        Some("noCredit") => Ok(ResetOutcome::NoCredit),
        _ => {
            Err(AppServerError::Rpc { method: METHOD, message: "Unrecognized reset result".into() })
        }
    }
}

#[cfg(test)]
mod reset_tests {
    use super::*;
    use crate::reset::{ResetAttempt, ResetOutcome};
    fn fixture() -> (Value, ResetAttempt) {
        let account = json!({"account":{"type":"chatgpt","email":"fixture@example.invalid"}});
        let attempt = ResetAttempt::new(
            account_key(&account).unwrap(),
            crate::model::ResetCredit { id: Some("fixture-credit".into()), expires_at: None },
        )
        .unwrap();
        (account, attempt)
    }
    #[test]
    fn consume_only_after_identity_check_and_reuses_exact_key() {
        for (wire, expected) in [
            ("reset", ResetOutcome::Reset),
            ("alreadyRedeemed", ResetOutcome::AlreadyRedeemed),
            ("nothingToReset", ResetOutcome::NothingToReset),
            ("noCredit", ResetOutcome::NoCredit),
        ] {
            let (account, attempt) = fixture();
            for _ in 0..2 {
                let (tx, rx) = mpsc::channel();
                tx.send(json!({"id":1,"result":account}).to_string()).unwrap();
                tx.send(json!({"id":2,"result":{"outcome":wire}}).to_string()).unwrap();
                let mut buffer = Vec::new();
                let mut sent = false;
                assert_eq!(
                    consume_in_session(&mut buffer, &rx, &attempt, &mut sent).unwrap(),
                    expected
                );
                assert!(sent);
                let lines: Vec<Value> = String::from_utf8(buffer)
                    .unwrap()
                    .lines()
                    .map(|l| serde_json::from_str(l).unwrap())
                    .collect();
                assert_eq!(lines.len(), 2);
                assert_eq!(lines[0]["method"], "account/read");
                assert_eq!(lines[1]["params"]["creditId"], "fixture-credit");
                assert_eq!(lines[1]["params"]["idempotencyKey"], attempt.idempotency_key);
            }
        }
    }
    #[test]
    fn changed_or_missing_account_never_sends_mutation() {
        let (_, attempt) = fixture();
        for account in [
            json!({"account":null}),
            json!({"account":{"type":"chatgpt","email":"other@example.invalid"}}),
        ] {
            let (tx, rx) = mpsc::channel();
            tx.send(json!({"id":1,"result":account}).to_string()).unwrap();
            let mut buffer = Vec::new();
            let mut sent = false;
            assert!(consume_in_session(&mut buffer, &rx, &attempt, &mut sent).is_err());
            assert!(!sent);
            assert!(!String::from_utf8(buffer).unwrap().contains("consume"));
        }
    }
    #[test]
    fn unsupported_or_unknown_result_stays_uncertain() {
        let (account, attempt) = fixture();
        for response in [
            json!({"id":2,"error":{"code":-32601,"message":"unsupported"}}),
            json!({"id":2,"result":{"outcome":"future"}}),
        ] {
            let (tx, rx) = mpsc::channel();
            tx.send(json!({"id":1,"result":account}).to_string()).unwrap();
            tx.send(response.to_string()).unwrap();
            let mut sent = false;
            assert!(consume_in_session(&mut Vec::new(), &rx, &attempt, &mut sent).is_err());
            assert!(sent);
        }
    }
}

fn parse_account_responses(
    responses: &HashMap<u64, RpcResponse>,
) -> (Result<AppServerSnapshot, AppServerError>, Option<String>) {
    let account = match responses
        .get(&1)
        .ok_or(AppServerError::Closed)
        .and_then(|response| response_result(response, "account/read"))
    {
        Ok(account) => account,
        Err(error) => return (Err(error), None),
    };
    let account_key = account_key(account);
    let snapshot = (|| {
        let limits = response_result(
            responses.get(&2).ok_or(AppServerError::Closed)?,
            "account/rateLimits/read",
        )?;
        let quota = parse_snapshot(account, limits, chrono::Utc::now().timestamp())?;
        let token_usage = responses.get(&3).and_then(token_usage_response);
        Ok(AppServerSnapshot {
            quota,
            token_usage,
            account_key: account_key.clone(),
            supplement_allowed: !responses.values().any(|response| {
                response
                    .error
                    .as_ref()
                    .is_some_and(|error| crate::refresh::blocked_error(&error.message))
            }),
        })
    })();
    (snapshot, account_key)
}

fn spawn_suspended(spec: &CommandSpec) -> Result<(Child, SuspendedThread), AppServerError> {
    let mut command = Command::new(&spec.program);
    command
        .args(&spec.args)
        .arg("app-server")
        .arg("--stdio")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .creation_flags(CREATE_NO_WINDOW.0 | CREATE_SUSPENDED.0);
    let mut child = command.spawn().map_err(AppServerError::Spawn)?;
    match SuspendedThread::find(child.id()) {
        Ok(thread) => Ok((child, thread)),
        Err(error) => {
            terminate(&mut child);
            Err(AppServerError::ProcessIsolation(error.to_string()))
        }
    }
}

fn terminate(child: &mut Child) {
    if child.try_wait().ok().flatten().is_none() {
        let _ = child.kill();
    }
    let _ = child.wait();
}

fn write_json(writer: &mut impl Write, value: &Value) -> Result<(), AppServerError> {
    writeln!(writer, "{value}").map_err(AppServerError::Io)?;
    writer.flush().map_err(AppServerError::Io)
}

fn receive_response(
    receiver: &mpsc::Receiver<String>,
    id: u64,
    _method: &'static str,
    timeout: Duration,
) -> Result<RpcResponse, AppServerError> {
    let deadline = Instant::now() + timeout;
    loop {
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            return Err(AppServerError::Timeout);
        }
        let line = receiver.recv_timeout(remaining).map_err(|error| match error {
            mpsc::RecvTimeoutError::Timeout => AppServerError::Timeout,
            mpsc::RecvTimeoutError::Disconnected => AppServerError::Closed,
        })?;
        if let Ok(response) = serde_json::from_str::<RpcResponse>(&line) {
            if response.id == Some(id) {
                return Ok(response);
            }
        }
    }
}

#[cfg(test)]
fn receive_required(
    receiver: &mpsc::Receiver<String>,
    required_ids: &[u64],
    optional_ids: &[u64],
    timeout: Duration,
) -> Result<HashMap<u64, RpcResponse>, AppServerError> {
    receive_required_with(receiver, required_ids, optional_ids, timeout, |_| {})
}

fn receive_required_with(
    receiver: &mpsc::Receiver<String>,
    required_ids: &[u64],
    optional_ids: &[u64],
    timeout: Duration,
    mut observe: impl FnMut(&RpcResponse),
) -> Result<HashMap<u64, RpcResponse>, AppServerError> {
    let deadline = Instant::now() + timeout;
    let mut responses = HashMap::new();
    while required_ids.iter().any(|id| !responses.contains_key(id)) {
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            return Err(AppServerError::Timeout);
        }
        let line = receiver.recv_timeout(remaining).map_err(|error| match error {
            mpsc::RecvTimeoutError::Timeout => AppServerError::Timeout,
            mpsc::RecvTimeoutError::Disconnected => AppServerError::Closed,
        })?;
        if let Ok(response) = serde_json::from_str::<RpcResponse>(&line) {
            if let Some(id) =
                response.id.filter(|id| required_ids.contains(id) || optional_ids.contains(id))
            {
                observe(&response);
                responses.insert(id, response);
            }
        }
    }
    Ok(responses)
}

fn receive_account_responses(
    receiver: &mpsc::Receiver<String>,
    timeout: Duration,
    verified_account_key: &mut Option<String>,
    supplement_allowed: &mut bool,
    token_usage: &mut Option<TokenUsageSnapshot>,
) -> Result<HashMap<u64, RpcResponse>, AppServerError> {
    let deadline = Instant::now() + timeout;
    let mut responses = receive_required_with(receiver, &[1, 2], &[3], timeout, |response| {
        if response.id == Some(3) {
            *token_usage = token_usage_response(response);
        }
        if response
            .error
            .as_ref()
            .is_some_and(|error| crate::refresh::blocked_error(&error.message))
        {
            *supplement_allowed = false;
        }
        if response.id == Some(1)
            && let Ok(account) = response_result(response, "account/read")
        {
            *verified_account_key = account_key(account);
        }
    })?;
    if let Entry::Vacant(entry) = responses.entry(3) {
        let remaining =
            deadline.saturating_duration_since(Instant::now()).min(OPTIONAL_USAGE_GRACE);
        if !remaining.is_zero()
            && let Ok(response) = receive_response(receiver, 3, "account/usage/read", remaining)
        {
            if response
                .error
                .as_ref()
                .is_some_and(|error| crate::refresh::blocked_error(&error.message))
            {
                *supplement_allowed = false;
            }
            *token_usage = token_usage_response(&response);
            entry.insert(response);
        }
    }
    Ok(responses)
}

fn token_usage_response(response: &RpcResponse) -> Option<TokenUsageSnapshot> {
    response_result(response, "account/usage/read")
        .ok()
        .filter(|value| value.get("dailyUsageBuckets").is_some_and(Value::is_array))
        .map(parse_token_usage)
}

fn response_result<'a>(
    response: &'a RpcResponse,
    method: &'static str,
) -> Result<&'a Value, AppServerError> {
    if let Some(error) = &response.error {
        return Err(AppServerError::Rpc { method, message: sanitize(&error.message) });
    }
    response.result.as_ref().ok_or(AppServerError::Closed)
}

fn sanitize(value: &str) -> String {
    value
        .chars()
        .filter(|character| !matches!(character, '\r' | '\n' | '\t'))
        .take(ERROR_LIMIT)
        .collect()
}

pub(crate) fn account_key(account_result: &Value) -> Option<String> {
    let account = account_result.get("account")?;
    let identity = account
        .get("email")
        .and_then(Value::as_str)
        .or_else(|| account.get("id").and_then(Value::as_str))?
        .trim()
        .to_lowercase();
    if identity.is_empty() {
        return None;
    }
    let account_type = account.get("type").and_then(Value::as_str).unwrap_or("unknown");
    let digest = Sha256::digest(format!("{account_type}\0{identity}").as_bytes());
    Some(digest.iter().map(|byte| format!("{byte:02x}")).collect())
}

fn read_capped(mut reader: impl Read, limit: usize) -> String {
    let mut bytes = Vec::with_capacity(limit);
    let mut buffer = [0_u8; 1024];
    while let Ok(read) = reader.read(&mut buffer) {
        if read == 0 {
            break;
        }
        let retained = limit.saturating_sub(bytes.len()).min(read);
        bytes.extend_from_slice(&buffer[..retained]);
    }
    String::from_utf8_lossy(&bytes).into_owned()
}

fn forward_bounded_lines(mut reader: impl Read, sender: mpsc::Sender<String>) {
    let mut chunk = [0_u8; 4096];
    let mut line = Vec::with_capacity(4096);
    let mut overflow = false;
    let mut forwarded = [false; 4];
    while let Ok(read) = reader.read(&mut chunk) {
        if read == 0 {
            break;
        }
        for &byte in &chunk[..read] {
            if byte == b'\n' {
                if !overflow {
                    if line.last() == Some(&b'\r') {
                        line.pop();
                    }
                    let value = String::from_utf8_lossy(&line).into_owned();
                    if let Some(id) = expected_response_id(&value)
                        && !forwarded[id]
                    {
                        forwarded[id] = true;
                        if sender.send(value).is_err() {
                            return;
                        }
                    }
                }
                line.clear();
                overflow = false;
            } else if !overflow {
                if line.len() < STDOUT_LINE_LIMIT {
                    line.push(byte);
                } else {
                    line.clear();
                    overflow = true;
                }
            }
        }
    }
    if !overflow && !line.is_empty() {
        let value = String::from_utf8_lossy(&line).into_owned();
        if let Some(id) = expected_response_id(&value)
            && !forwarded[id]
        {
            let _ = sender.send(value);
        }
    }
}

fn expected_response_id(line: &str) -> Option<usize> {
    let value = serde_json::from_str::<Value>(line).ok()?;
    let object = value.as_object()?;
    if !object.contains_key("result") && !object.contains_key("error") {
        return None;
    }
    let id = object.get("id")?.as_u64()?;
    usize::try_from(id).ok().filter(|id| *id < 4)
}

#[derive(Debug, Deserialize)]
struct RpcResponse {
    id: Option<u64>,
    result: Option<Value>,
    error: Option<RpcError>,
}

#[derive(Debug, Deserialize)]
struct RpcError {
    message: String,
}

fn resolve_commands() -> Result<Vec<CommandSpec>, AppServerError> {
    if let Some(path) = env::var_os("CODEX_STATUS_CODEX").map(PathBuf::from) {
        return command_from_path(path).map(|command| vec![command]);
    }

    let directories = path_directories();
    let local_executables = local_codex_executables();
    resolve_commands_from(&directories, &local_executables)
}

fn resolve_commands_from(
    directories: &[PathBuf],
    local_executables: &[PathBuf],
) -> Result<Vec<CommandSpec>, AppServerError> {
    let mut commands = Vec::new();
    // Public native installs are preferred. Store package internals can appear on PATH while
    // denying CreateProcess to unpackaged apps, so those candidates are tried last.
    for directory in directories {
        let executable = directory.join("codex.exe");
        if executable.is_file()
            && !executable.to_string_lossy().to_ascii_lowercase().contains("\\windowsapps\\")
        {
            commands.push(CommandSpec { program: executable, args: Vec::new() });
        }
    }
    // Codex Desktop keeps an executable specifically for local app-server integrations outside
    // PATH. These stable per-user locations remain usable when the packaged WindowsApps binary
    // denies CreateProcess to an unpackaged tray application.
    for executable in local_executables {
        if executable.is_file() && !commands.iter().any(|command| command.program == *executable) {
            commands.push(CommandSpec { program: executable.clone(), args: Vec::new() });
        }
    }
    for directory in directories {
        let wrapper = directory.join("codex.cmd");
        if wrapper.is_file() {
            if let Ok(command) = command_from_path(wrapper) {
                commands.push(command);
            }
        }
    }
    for directory in directories {
        let executable = directory.join("codex.exe");
        if executable.is_file()
            && executable.to_string_lossy().to_ascii_lowercase().contains("\\windowsapps\\")
        {
            commands.push(CommandSpec { program: executable, args: Vec::new() });
        }
    }
    if commands.is_empty() { Err(AppServerError::CodexNotFound) } else { Ok(commands) }
}

fn local_codex_executables() -> Vec<PathBuf> {
    let mut roots = Vec::new();
    if let Some(root) = env::var_os("CODEX_HOME").map(PathBuf::from) {
        roots.push(root);
    }
    if let Some(profile) = env::var_os("USERPROFILE").map(PathBuf::from) {
        let root = profile.join(".codex");
        if !roots.contains(&root) {
            roots.push(root);
        }
    }

    let mut executables = Vec::new();
    for root in roots {
        executables.push(root.join("bin").join("codex.exe"));
        executables.push(root.join("plugins").join(".plugin-appserver").join("codex.exe"));
        executables.push(root.join(".sandbox-bin").join("codex.exe"));
    }
    executables
}

fn command_from_path(path: PathBuf) -> Result<CommandSpec, AppServerError> {
    let extension = path.extension().and_then(OsStr::to_str).unwrap_or_default();
    if extension.eq_ignore_ascii_case("exe") || extension.is_empty() {
        return Ok(CommandSpec { program: path, args: Vec::new() });
    }
    if extension.eq_ignore_ascii_case("js") {
        let node = find_node().ok_or(AppServerError::NodeNotFound)?;
        return Ok(CommandSpec { program: node, args: vec![path.into_os_string()] });
    }
    if extension.eq_ignore_ascii_case("cmd") {
        let directory = path
            .parent()
            .ok_or_else(|| AppServerError::UnsupportedWrapper(path.display().to_string()))?;
        let script = directory
            .join("node_modules")
            .join("@openai")
            .join("codex")
            .join("bin")
            .join("codex.js");
        if !script.is_file() {
            return Err(AppServerError::UnsupportedWrapper(path.display().to_string()));
        }
        let node = directory.join("node.exe");
        let node =
            if node.is_file() { node } else { find_node().ok_or(AppServerError::NodeNotFound)? };
        return Ok(CommandSpec { program: node, args: vec![script.into_os_string()] });
    }
    Err(AppServerError::UnsupportedWrapper(path.display().to_string()))
}

fn find_node() -> Option<PathBuf> {
    path_directories()
        .into_iter()
        .map(|directory| directory.join("node.exe"))
        .find(|path| path.is_file())
}

fn path_directories() -> Vec<PathBuf> {
    env::var_os("PATH").map(|path| env::split_paths(&path).collect()).unwrap_or_default()
}

struct JobGuard(HANDLE);

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

impl JobGuard {
    fn assign(child: &Child) -> windows::core::Result<Self> {
        unsafe {
            let job = CreateJobObjectW(None, None)?;
            let mut limits = JOBOBJECT_EXTENDED_LIMIT_INFORMATION::default();
            limits.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
            if let Err(error) = SetInformationJobObject(
                job,
                JobObjectExtendedLimitInformation,
                &limits as *const _ as *const _,
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

impl Drop for JobGuard {
    fn drop(&mut self) {
        unsafe {
            let _ = CloseHandle(self.0);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn root(name: &str) -> PathBuf {
        let suffix = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos();
        env::temp_dir().join(format!("codex-status-{name}-{suffix}"))
    }

    #[test]
    fn resolves_npm_wrapper_without_running_a_shell() {
        let directory = root("npm");
        let script = directory
            .join("node_modules")
            .join("@openai")
            .join("codex")
            .join("bin")
            .join("codex.js");
        fs::create_dir_all(script.parent().unwrap()).unwrap();
        fs::write(directory.join("codex.cmd"), "").unwrap();
        fs::write(directory.join("node.exe"), "").unwrap();
        fs::write(&script, "").unwrap();

        let spec = command_from_path(directory.join("codex.cmd")).unwrap();
        assert_eq!(spec.program, directory.join("node.exe"));
        assert_eq!(spec.args, vec![script.into_os_string()]);
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn rejects_unknown_wrapper() {
        let error = command_from_path(PathBuf::from(r"C:\tools\codex.ps1")).unwrap_err();
        assert!(matches!(error, AppServerError::UnsupportedWrapper(_)));
    }

    #[test]
    fn sanitizes_multiline_errors() {
        assert_eq!(sanitize("first\nsecond\tthird"), "firstsecondthird");
    }

    #[test]
    fn account_keys_are_stable_case_insensitive_and_do_not_expose_email() {
        let first = json!({"account": {"type": "chatgpt", "email": "User@Example.com"}});
        let second = json!({"account": {"type": "chatgpt", "email": "user@example.com"}});
        let key = account_key(&first).unwrap();
        assert_eq!(Some(key.as_str()), account_key(&second).as_deref());
        assert_eq!(key.len(), 64);
        assert!(!key.contains("example"));
    }

    #[test]
    fn optional_auth_errors_suppress_fallback_even_before_required_timeout() {
        for status in [401, 403, 429] {
            let (sender, receiver) = mpsc::channel();
            sender.send(json!({"id":1,"result":{"account":{"type":"chatgpt","email":"fixture@example.invalid"}}}).to_string()).unwrap();
            sender
                .send(
                    json!({"id":3,"error":{"code":-32603,"message":format!("HTTP {status}")}})
                        .to_string(),
                )
                .unwrap();
            drop(sender);
            let mut allowed = true;
            assert!(
                receive_account_responses(
                    &receiver,
                    Duration::from_millis(20),
                    &mut None,
                    &mut allowed,
                    &mut None
                )
                .is_err()
            );
            assert!(!allowed);
        }
    }

    #[test]
    fn token_buckets_distinguish_missing_from_explicit_empty() {
        for (value, present) in [
            (json!({}), false),
            (json!({"dailyUsageBuckets":null}), false),
            (json!({"dailyUsageBuckets":[]}), true),
        ] {
            let response: RpcResponse =
                serde_json::from_value(json!({"id":3,"result":value})).unwrap();
            assert_eq!(token_usage_response(&response).is_some(), present);
        }
    }

    #[test]
    fn grace_period_retains_tokens_when_quota_rpc_failed() {
        let (sender, receiver) = mpsc::channel();
        sender.send(json!({"id":1,"result":{"account":{"type":"chatgpt","email":"fixture@example.invalid"}}}).to_string()).unwrap();
        sender
            .send(json!({"id":2,"error":{"code":-32603,"message":"temporary failure"}}).to_string())
            .unwrap();
        let late = thread::spawn(move || {
            thread::sleep(Duration::from_millis(10));
            sender.send(json!({"id":3,"result":{"dailyUsageBuckets":[{"startDate":"2026-09-04","tokens":42}]}}).to_string()).unwrap();
        });
        let mut retained = None;
        let responses = receive_account_responses(
            &receiver,
            Duration::from_secs(1),
            &mut None,
            &mut true,
            &mut retained,
        )
        .unwrap();
        late.join().unwrap();
        assert!(parse_account_responses(&responses).0.is_err());
        assert_eq!(retained.unwrap().daily_buckets[0].tokens, 42);
    }

    #[test]
    fn rate_limit_failure_preserves_the_verified_account_identity() {
        let responses = HashMap::from([
            (
                1,
                serde_json::from_value(json!({
                    "id": 1,
                    "result": {"account": {"type": "chatgpt", "email": "user@example.com"}}
                }))
                .unwrap(),
            ),
            (
                2,
                serde_json::from_value(json!({
                    "id": 2,
                    "error": {"message": "rate limits unavailable"}
                }))
                .unwrap(),
            ),
        ]);
        let (snapshot, verified_account) = parse_account_responses(&responses);
        assert!(snapshot.is_err());
        assert_eq!(verified_account.as_deref().map(str::len), Some(64));
    }

    #[test]
    fn puts_inaccessible_store_candidates_after_npm() {
        let directory = root("path with spaces");
        let npm = directory.join("npm");
        let script =
            npm.join("node_modules").join("@openai").join("codex").join("bin").join("codex.js");
        let store = directory.join("WindowsApps").join("OpenAI.Codex");
        fs::create_dir_all(script.parent().unwrap()).unwrap();
        fs::create_dir_all(&store).unwrap();
        fs::write(npm.join("codex.cmd"), "").unwrap();
        fs::write(npm.join("node.exe"), "").unwrap();
        fs::write(&script, "").unwrap();
        fs::write(store.join("codex.exe"), "").unwrap();

        let commands = resolve_commands_from(&[store.clone(), npm.clone()], &[]).unwrap();
        assert_eq!(commands[0].program, npm.join("node.exe"));
        assert_eq!(commands[1].program, store.join("codex.exe"));
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn puts_desktop_app_server_before_inaccessible_store_candidate() {
        let directory = root("desktop-app-server");
        let local = directory.join(".codex").join("plugins").join(".plugin-appserver");
        let store = directory.join("WindowsApps").join("OpenAI.Codex");
        fs::create_dir_all(&local).unwrap();
        fs::create_dir_all(&store).unwrap();
        fs::write(local.join("codex.exe"), "").unwrap();
        fs::write(store.join("codex.exe"), "").unwrap();

        let commands =
            resolve_commands_from(std::slice::from_ref(&store), &[local.join("codex.exe")])
                .unwrap();
        assert_eq!(commands[0].program, local.join("codex.exe"));
        assert_eq!(commands[1].program, store.join("codex.exe"));
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn receive_timeout_is_bounded() {
        let (_sender, receiver) = mpsc::channel();
        let error = receive_response(&receiver, 1, "test", Duration::from_millis(1)).unwrap_err();
        assert!(matches!(error, AppServerError::Timeout));
    }

    #[test]
    fn required_responses_do_not_wait_for_optional_usage() {
        let (sender, receiver) = mpsc::channel();
        sender.send(r#"{"id":1,"result":{}}"#.to_owned()).unwrap();
        sender.send(r#"{"id":2,"result":{}}"#.to_owned()).unwrap();
        let responses = receive_required(&receiver, &[1, 2], &[3], Duration::from_secs(1)).unwrap();
        assert!(responses.contains_key(&1));
        assert!(responses.contains_key(&2));
        assert!(!responses.contains_key(&3));
    }

    #[test]
    fn required_response_collection_preserves_early_optional_usage() {
        let (sender, receiver) = mpsc::channel();
        for id in [3, 1, 2] {
            sender.send(format!(r#"{{"id":{id},"result":{{}}}}"#)).unwrap();
        }
        let responses = receive_required(&receiver, &[1, 2], &[3], Duration::from_secs(1)).unwrap();
        assert!(responses.contains_key(&3));
    }

    #[test]
    fn account_response_collection_waits_for_late_usage_within_the_grace_period() {
        let (sender, receiver) = mpsc::channel();
        sender.send(r#"{"id":1,"result":{}}"#.to_owned()).unwrap();
        sender.send(r#"{"id":2,"result":{}}"#.to_owned()).unwrap();
        let late_sender = sender.clone();
        let late = thread::spawn(move || {
            thread::sleep(Duration::from_millis(20));
            late_sender.send(r#"{"id":3,"result":{}}"#.to_owned()).unwrap();
        });
        let responses = receive_account_responses(
            &receiver,
            Duration::from_millis(200),
            &mut None,
            &mut true,
            &mut None,
        )
        .unwrap();
        late.join().unwrap();
        assert!(responses.contains_key(&3));
    }

    #[test]
    fn account_response_collection_rejects_usage_after_the_grace_period() {
        let (sender, receiver) = mpsc::channel();
        sender.send(r#"{"id":1,"result":{}}"#.to_owned()).unwrap();
        sender.send(r#"{"id":2,"result":{}}"#.to_owned()).unwrap();
        let late_sender = sender.clone();
        let late = thread::spawn(move || {
            thread::sleep(OPTIONAL_USAGE_GRACE + Duration::from_millis(100));
            late_sender.send(r#"{"id":3,"result":{}}"#.to_owned()).unwrap();
        });
        let responses = receive_account_responses(
            &receiver,
            Duration::from_secs(2),
            &mut None,
            &mut true,
            &mut None,
        )
        .unwrap();
        late.join().unwrap();
        assert!(!responses.contains_key(&3));
    }

    #[test]
    fn account_identity_survives_a_later_required_response_failure() {
        let (sender, receiver) = mpsc::channel();
        sender
            .send(
                r#"{"id":1,"result":{"account":{"type":"chatgpt","email":"user@example.com"}}}"#
                    .to_owned(),
            )
            .unwrap();
        drop(sender);
        let mut verified_account = None;
        let result = receive_account_responses(
            &receiver,
            Duration::from_secs(1),
            &mut verified_account,
            &mut true,
            &mut None,
        );
        assert!(matches!(result, Err(AppServerError::Closed)));
        assert_eq!(verified_account.as_deref().map(str::len), Some(64));
    }

    #[test]
    fn capped_error_capture_keeps_draining_the_reader() {
        let source = b"0123456789";
        let mut reader = std::io::Cursor::new(source);
        assert_eq!(read_capped(&mut reader, 4), "0123");
        assert_eq!(reader.position(), source.len() as u64);
    }

    #[test]
    fn stdout_forwarder_discards_oversized_lines_and_preserves_following_messages() {
        let mut source = Vec::new();
        for index in 0..100 {
            source
                .extend_from_slice(format!(r#"{{"method":"notice","index":{index}}}"#).as_bytes());
            source.push(b'\n');
        }
        source.extend(std::iter::repeat_n(b'x', STDOUT_LINE_LIMIT + 1));
        source.extend_from_slice(
            b"\n{\"id\":1,\"method\":\"server/request\"}\n\
              {\"id\":1,\"result\":{\"first\":true}}\r\n\
              {\"id\":1,\"result\":{\"duplicate\":true}}\n\
              {\"id\":2,\"result\":{}}\n\
              {\"id\":3,\"result\":{}}\n",
        );
        let (sender, receiver) = mpsc::channel();
        forward_bounded_lines(std::io::Cursor::new(source), sender);
        assert_eq!(
            receiver.into_iter().collect::<Vec<_>>(),
            vec![
                r#"{"id":1,"result":{"first":true}}"#,
                r#"{"id":2,"result":{}}"#,
                r#"{"id":3,"result":{}}"#,
            ]
        );
    }
}
