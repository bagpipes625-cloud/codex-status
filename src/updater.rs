use serde::Deserialize;
use sha2::{Digest, Sha256};
use std::ffi::{OsStr, c_void};
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::os::windows::fs::OpenOptionsExt;
use std::os::windows::io::AsRawHandle;
use std::os::windows::process::CommandExt;
use std::path::{Component, Path, PathBuf};
use std::process::Command;
use std::ptr;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use windows::Win32::Foundation::{CloseHandle, ERROR_INVALID_PARAMETER, HANDLE, WAIT_OBJECT_0};
use windows::Win32::Networking::WinHttp::{
    INTERNET_DEFAULT_HTTPS_PORT, WINHTTP_ACCESS_TYPE_AUTOMATIC_PROXY, WINHTTP_FLAG_SECURE,
    WINHTTP_QUERY_FLAG_NUMBER, WINHTTP_QUERY_STATUS_CODE, WinHttpCloseHandle, WinHttpConnect,
    WinHttpOpen, WinHttpOpenRequest, WinHttpQueryDataAvailable, WinHttpQueryHeaders,
    WinHttpReadData, WinHttpReceiveResponse, WinHttpSendRequest, WinHttpSetTimeouts,
};
use windows::Win32::Storage::FileSystem::{
    BY_HANDLE_FILE_INFORMATION, CreateFileW, FILE_ATTRIBUTE_DIRECTORY,
    FILE_ATTRIBUTE_REPARSE_POINT, FILE_FLAG_BACKUP_SEMANTICS, FILE_FLAG_DELETE_ON_CLOSE,
    FILE_FLAG_OPEN_REPARSE_POINT, FILE_READ_ATTRIBUTES, FILE_SHARE_READ, FILE_SHARE_WRITE,
    GetFileInformationByHandle, MOVEFILE_REPLACE_EXISTING, MOVEFILE_WRITE_THROUGH, MoveFileExW,
    OPEN_EXISTING,
};
use windows::Win32::System::Threading::{
    CREATE_NO_WINDOW, OpenProcess, PROCESS_SYNCHRONIZE, WaitForSingleObject,
};
use windows::Win32::UI::WindowsAndMessaging::{MB_ICONERROR, MB_OK, MessageBoxW};
use windows::core::{HRESULT, PCWSTR, w};

const RELEASE_API: &str =
    "https://api.github.com/repos/bagpipes625-cloud/codex-status/releases/latest";
const RELEASE_ASSET_PREFIX: &str =
    "https://github.com/bagpipes625-cloud/codex-status/releases/download/";
const MAX_METADATA_BYTES: usize = 512 * 1024;
const MAX_EXECUTABLE_BYTES: usize = 32 * 1024 * 1024;
const UPDATE_WAIT_MS: u32 = 30_000;
const HTTP_REQUEST_DEADLINE: Duration = Duration::from_secs(60);
const STALE_PENDING_AGE: Duration = Duration::from_secs(24 * 60 * 60);

#[derive(Debug, thiserror::Error)]
pub enum UpdateError {
    #[error("Windows update service failed: {0}")]
    Windows(#[from] windows::core::Error),
    #[error("Update network response was invalid")]
    InvalidResponse,
    #[error("Update metadata could not be parsed: {0}")]
    Json(#[from] serde_json::Error),
    #[error("Update file could not be prepared: {0}")]
    Io(#[from] std::io::Error),
    #[error("Downloaded update did not match its GitHub digest")]
    DigestMismatch,
    #[error("Update helper did not receive a safe target")]
    UnsafeTarget,
    #[error("The executable location does not allow in-place updates")]
    TargetNotWritable,
    #[error("Updates are unavailable for this development channel")]
    UnsupportedChannel,
    #[error("The running CodexStatus process did not exit in time")]
    ParentStillRunning,
    #[error("The update request exceeded its time limit")]
    RequestTimedOut,
}

#[derive(Debug, Clone)]
pub struct StagedUpdate {
    pub executable: PathBuf,
    digest: String,
    size: u64,
}

#[derive(Debug, Deserialize)]
struct Release {
    tag_name: String,
    #[serde(default)]
    draft: bool,
    #[serde(default)]
    prerelease: bool,
    #[serde(default)]
    assets: Vec<ReleaseAsset>,
}

#[derive(Debug, Clone, Deserialize)]
struct ReleaseAsset {
    name: String,
    browser_download_url: String,
    size: u64,
    digest: Option<String>,
}

struct InternetHandle(*mut c_void);

impl InternetHandle {
    fn new(value: *mut c_void) -> Result<Self, UpdateError> {
        if value.is_null() {
            Err(windows::core::Error::from_thread().into())
        } else {
            Ok(Self(value))
        }
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

struct HttpClient {
    session: InternetHandle,
}

impl HttpClient {
    fn new() -> Result<Self, UpdateError> {
        let agent = wide0(format!("CodexStatus/{}", env!("CARGO_PKG_VERSION")));
        let session = unsafe {
            InternetHandle::new(WinHttpOpen(
                PCWSTR(agent.as_ptr()),
                WINHTTP_ACCESS_TYPE_AUTOMATIC_PROXY,
                PCWSTR::null(),
                PCWSTR::null(),
                0,
            ))?
        };
        unsafe {
            WinHttpSetTimeouts(session.0, 5_000, 5_000, 10_000, 15_000)?;
        }
        Ok(Self { session })
    }

    fn get(&self, url: &str, accept: &str, limit: usize) -> Result<Vec<u8>, UpdateError> {
        let deadline = Instant::now() + HTTP_REQUEST_DEADLINE;
        let (host, path) = split_https_url(url).ok_or(UpdateError::InvalidResponse)?;
        let host = wide0(host);
        let path = wide0(path);
        let connection = unsafe {
            InternetHandle::new(WinHttpConnect(
                self.session.0,
                PCWSTR(host.as_ptr()),
                INTERNET_DEFAULT_HTTPS_PORT,
                0,
            ))?
        };
        let request = unsafe {
            InternetHandle::new(WinHttpOpenRequest(
                connection.0,
                w!("GET"),
                PCWSTR(path.as_ptr()),
                PCWSTR::null(),
                PCWSTR::null(),
                ptr::null(),
                WINHTTP_FLAG_SECURE,
            ))?
        };
        let headers: Vec<u16> = format!("Accept: {accept}\r\nX-GitHub-Api-Version: 2022-11-28\r\n")
            .encode_utf16()
            .collect();
        unsafe {
            WinHttpSendRequest(request.0, Some(&headers), None, 0, 0, 0)?;
            WinHttpReceiveResponse(request.0, ptr::null_mut())?;
        }

        let mut status = 0_u32;
        let mut status_size = std::mem::size_of::<u32>() as u32;
        let mut index = 0_u32;
        unsafe {
            WinHttpQueryHeaders(
                request.0,
                WINHTTP_QUERY_STATUS_CODE | WINHTTP_QUERY_FLAG_NUMBER,
                PCWSTR::null(),
                Some((&mut status as *mut u32).cast()),
                &mut status_size,
                &mut index,
            )?;
        }
        if status != 200 {
            return Err(UpdateError::InvalidResponse);
        }

        let mut body = Vec::new();
        loop {
            if Instant::now() >= deadline {
                return Err(UpdateError::RequestTimedOut);
            }
            let mut available = 0_u32;
            unsafe {
                WinHttpQueryDataAvailable(request.0, &mut available)?;
            }
            if available == 0 {
                break;
            }
            let available = available as usize;
            if body.len().saturating_add(available) > limit {
                return Err(UpdateError::InvalidResponse);
            }
            let start = body.len();
            body.resize(start + available, 0);
            let mut read = 0_u32;
            unsafe {
                WinHttpReadData(
                    request.0,
                    body[start..].as_mut_ptr().cast(),
                    available as u32,
                    &mut read,
                )?;
            }
            body.truncate(start + read as usize);
            if Instant::now() >= deadline {
                return Err(UpdateError::RequestTimedOut);
            }
            if read == 0 {
                break;
            }
        }
        Ok(body)
    }
}

pub fn check_and_stage(updates_directory: &Path) -> Result<Option<StagedUpdate>, UpdateError> {
    let channel = update_asset_channel().ok_or(UpdateError::UnsupportedChannel)?;
    let target = std::env::current_exe()?;
    validate_target_for_update(&target)?;
    cleanup_stale_pending_updates(&target);
    let staging = StagingTree::open(updates_directory)?;
    staging.cleanup();

    let client = HttpClient::new()?;
    let metadata = client.get(RELEASE_API, "application/vnd.github+json", MAX_METADATA_BYTES)?;
    let selection = select_asset(&metadata, env!("CARGO_PKG_VERSION"), channel)?;
    let Some((version, asset, digest)) = selection else {
        return Ok(None);
    };

    let bytes = client.get(
        &asset.browser_download_url,
        "application/octet-stream",
        MAX_EXECUTABLE_BYTES,
    )?;
    if bytes.len() as u64 != asset.size || bytes.len() < 64 * 1024 || !bytes.starts_with(b"MZ") {
        return Err(UpdateError::InvalidResponse);
    }
    if sha256_hex(&bytes) != digest {
        return Err(UpdateError::DigestMismatch);
    }

    let (version_directory, _version_lock) = staging.prepare_version(&version)?;
    probe_directory_writable(&version_directory)?;
    let executable = version_directory.join("CodexStatus.exe");
    let temporary = version_directory.join("CodexStatus.download");
    create_staged_file(&temporary, &bytes)?;
    if executable.exists() {
        fs::remove_file(&executable)?;
    }
    fs::rename(temporary, &executable)?;
    Ok(Some(StagedUpdate { executable, digest, size: bytes.len() as u64 }))
}

fn create_staged_file(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    let mut file = OpenOptions::new().write(true).create_new(true).open(path)?;
    file.write_all(bytes)?;
    file.sync_all()
}

struct StagingTree {
    _app_lock: DirectoryLock,
    _updates_lock: DirectoryLock,
    _channel_lock: DirectoryLock,
    updates_root: PathBuf,
    channel_directory: PathBuf,
}

impl StagingTree {
    fn open(updates_directory: &Path) -> Result<Self, UpdateError> {
        let updates_root = updates_directory.parent().ok_or(UpdateError::UnsafeTarget)?;
        let app_directory = updates_root.parent().ok_or(UpdateError::UnsafeTarget)?;
        if updates_root.file_name() != Some(OsStr::new("updates"))
            || app_directory.file_name() != Some(OsStr::new("CodexStatus"))
        {
            return Err(UpdateError::UnsafeTarget);
        }
        Self::open_at(app_directory, updates_directory, env!("CODEX_STATUS_CHANNEL"))
    }

    fn open_at(
        app_directory: &Path,
        updates_directory: &Path,
        channel: &str,
    ) -> Result<Self, UpdateError> {
        let updates_root = app_directory.join("updates");
        if updates_directory != updates_root.join(channel) {
            return Err(UpdateError::UnsafeTarget);
        }
        ensure_directory(app_directory)?;
        let app_lock = DirectoryLock::open(app_directory)?;
        ensure_directory(&updates_root)?;
        let updates_lock = DirectoryLock::open(&updates_root)?;
        ensure_directory(updates_directory)?;
        let channel_lock = DirectoryLock::open(updates_directory)?;
        Ok(Self {
            _app_lock: app_lock,
            _updates_lock: updates_lock,
            _channel_lock: channel_lock,
            updates_root,
            channel_directory: updates_directory.to_owned(),
        })
    }

    fn cleanup(&self) {
        cleanup_version_directories(&self.updates_root);
        cleanup_version_directories(&self.channel_directory);
    }

    fn prepare_version(&self, version: &str) -> Result<(PathBuf, DirectoryLock), UpdateError> {
        let directory = self.channel_directory.join(format!("v{version}"));
        if directory.parent() != Some(self.channel_directory.as_path()) {
            return Err(UpdateError::UnsafeTarget);
        }
        ensure_directory(&directory)?;
        let lock = DirectoryLock::open(&directory)?;
        Ok((directory, lock))
    }
}

fn ensure_directory(path: &Path) -> Result<(), UpdateError> {
    match fs::create_dir(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => Ok(()),
        Err(error) => Err(error.into()),
    }
}

#[cfg(test)]
fn cleanup_staged_updates_under(
    app_directory: &Path,
    updates_directory: &Path,
    channel: &str,
) -> Result<(), UpdateError> {
    let staging = StagingTree::open_at(app_directory, updates_directory, channel)?;
    staging.cleanup();
    Ok(())
}

fn cleanup_version_directories(root: &Path) {
    let Ok(entries) = fs::read_dir(root) else {
        return;
    };
    for entry in entries.flatten() {
        let name = entry.file_name();
        let Some(name) = name.to_str() else {
            continue;
        };
        if !is_staged_version_name(name) {
            continue;
        }
        let path = entry.path();
        if path.parent() != Some(root) {
            continue;
        }
        let Ok(_version_lock) = DirectoryLock::open(&path) else {
            continue;
        };
        cleanup_version_directory(&path);
    }
}

struct DirectoryLock(HANDLE);

impl DirectoryLock {
    fn open(path: &Path) -> Result<Self, UpdateError> {
        let path = wide0(path.as_os_str());
        let handle = unsafe {
            CreateFileW(
                PCWSTR(path.as_ptr()),
                FILE_READ_ATTRIBUTES.0,
                FILE_SHARE_READ | FILE_SHARE_WRITE,
                None,
                OPEN_EXISTING,
                FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT,
                None,
            )?
        };
        let locked = Self(handle);
        let mut information = BY_HANDLE_FILE_INFORMATION::default();
        unsafe {
            GetFileInformationByHandle(handle, &mut information)?;
        }
        if information.dwFileAttributes & FILE_ATTRIBUTE_DIRECTORY.0 == 0
            || information.dwFileAttributes & FILE_ATTRIBUTE_REPARSE_POINT.0 != 0
        {
            return Err(UpdateError::UnsafeTarget);
        }
        Ok(locked)
    }
}

impl Drop for DirectoryLock {
    fn drop(&mut self) {
        unsafe {
            let _ = CloseHandle(self.0);
        }
    }
}

fn cleanup_version_directory(directory: &Path) {
    let Ok(entries) = fs::read_dir(directory) else {
        return;
    };
    let mut files = Vec::new();
    for entry in entries {
        let Ok(entry) = entry else {
            return;
        };
        let Ok(file_type) = entry.file_type() else {
            return;
        };
        let name = entry.file_name();
        let Some(name) = name.to_str() else {
            return;
        };
        if !file_type.is_file() || !matches!(name, "CodexStatus.exe" | "CodexStatus.download") {
            return;
        }
        files.push(entry.path());
    }
    for file in files {
        if fs::remove_file(file).is_err() {
            return;
        }
    }
    let _ = fs::remove_dir(directory);
}

fn is_staged_version_name(name: &str) -> bool {
    let Some(version) = name.strip_prefix('v') else {
        return false;
    };
    !version.is_empty()
        && version.bytes().all(|byte| byte.is_ascii_digit() || byte == b'.')
        && parse_version(version).is_some()
}

fn select_asset(
    metadata: &[u8],
    current_version: &str,
    channel: UpdateAssetChannel,
) -> Result<Option<(String, ReleaseAsset, String)>, UpdateError> {
    let release: Release = serde_json::from_slice(metadata)?;
    if release.draft || release.prerelease {
        return Ok(None);
    }
    let version = release.tag_name.strip_prefix('v').unwrap_or(&release.tag_name);
    let latest = parse_version(version).ok_or(UpdateError::InvalidResponse)?;
    let current = parse_version(current_version).ok_or(UpdateError::InvalidResponse)?;
    if latest <= current {
        return Ok(None);
    }

    let expected_name = channel.asset_name(version);
    let asset = release
        .assets
        .into_iter()
        .find(|asset| asset.name == expected_name)
        .ok_or(UpdateError::InvalidResponse)?;
    if !asset.browser_download_url.starts_with(RELEASE_ASSET_PREFIX)
        || asset.browser_download_url.contains("/../")
        || asset.size == 0
        || asset.size > MAX_EXECUTABLE_BYTES as u64
    {
        return Err(UpdateError::InvalidResponse);
    }
    let digest = asset
        .digest
        .as_deref()
        .and_then(|value| value.strip_prefix("sha256:"))
        .ok_or(UpdateError::InvalidResponse)?
        .to_ascii_lowercase();
    if digest.len() != 64 || !digest.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err(UpdateError::InvalidResponse);
    }
    Ok(Some((version.to_owned(), asset, digest)))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum UpdateAssetChannel {
    Installed,
    Portable,
}

impl UpdateAssetChannel {
    fn asset_name(self, version: &str) -> String {
        match self {
            Self::Installed => format!("CodexStatus-v{version}-windows-x64.exe"),
            Self::Portable => format!("CodexStatus-v{version}-windows-x64-portable.exe"),
        }
    }
}

fn update_asset_channel() -> Option<UpdateAssetChannel> {
    update_asset_channel_for(env!("CODEX_STATUS_CHANNEL"))
}

fn update_asset_channel_for(channel: &str) -> Option<UpdateAssetChannel> {
    match channel {
        "stable" => Some(UpdateAssetChannel::Installed),
        "portable" => Some(UpdateAssetChannel::Portable),
        "beta" | "development" => None,
        _ => None,
    }
}

pub fn launch_staged_update(update: &StagedUpdate) -> Result<(), UpdateError> {
    let target = std::env::current_exe()?;
    let _directory_locks = lock_path_ancestors(&update.executable)?;
    let _verified_file = verify_staged_file(update)?;
    Command::new(&update.executable)
        .arg("--apply-update")
        .arg(std::process::id().to_string())
        .arg(target)
        .creation_flags(CREATE_NO_WINDOW.0)
        .spawn()?;
    Ok(())
}

fn lock_path_ancestors(path: &Path) -> Result<Vec<DirectoryLock>, UpdateError> {
    let parent = path.parent().ok_or(UpdateError::UnsafeTarget)?;
    let mut ancestors: Vec<_> = parent.ancestors().collect();
    ancestors.reverse();
    ancestors.into_iter().map(DirectoryLock::open).collect()
}

fn verify_staged_file(update: &StagedUpdate) -> Result<File, UpdateError> {
    let mut file = OpenOptions::new()
        .read(true)
        .share_mode(FILE_SHARE_READ.0)
        .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT.0)
        .open(&update.executable)?;
    let mut information = BY_HANDLE_FILE_INFORMATION::default();
    unsafe {
        GetFileInformationByHandle(HANDLE(file.as_raw_handle()), &mut information)?;
    }
    if information.dwFileAttributes & (FILE_ATTRIBUTE_DIRECTORY.0 | FILE_ATTRIBUTE_REPARSE_POINT.0)
        != 0
    {
        return Err(UpdateError::UnsafeTarget);
    }
    let metadata = file.metadata()?;
    if !metadata.is_file()
        || metadata.len() != update.size
        || metadata.len() < 64 * 1024
        || metadata.len() > MAX_EXECUTABLE_BYTES as u64
    {
        return Err(UpdateError::InvalidResponse);
    }
    let mut hasher = Sha256::new();
    let mut header = [0_u8; 2];
    file.read_exact(&mut header)?;
    if header != *b"MZ" {
        return Err(UpdateError::InvalidResponse);
    }
    hasher.update(header);
    let mut buffer = [0_u8; 64 * 1024];
    loop {
        let read = file.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }
    let actual: String = hasher.finalize().iter().map(|byte| format!("{byte:02x}")).collect();
    if actual != update.digest {
        return Err(UpdateError::DigestMismatch);
    }
    Ok(file)
}

pub fn apply_update_silently(parent_pid: u32, target: &Path) {
    let recovery_allowed = validate_target(target).is_ok() && target.is_file();
    if let Err(error) = apply_update(parent_pid, target) {
        let restart_error = if recovery_allowed {
            launch_target(target).err()
        } else {
            Some(std::io::Error::new(
                std::io::ErrorKind::PermissionDenied,
                "the update target is not a supported executable",
            ))
        };
        record_update_failure(target, &error, restart_error.as_ref());
        if let Some(restart_error) = restart_error {
            show_update_recovery_error(target, &error, &restart_error);
        }
    }
}

fn record_update_failure(
    target: &Path,
    update_error: &UpdateError,
    restart_error: Option<&std::io::Error>,
) {
    let base =
        std::env::var_os("LOCALAPPDATA").map(PathBuf::from).unwrap_or_else(std::env::temp_dir);
    let root = base.join("CodexStatus");
    let directory = if env!("CODEX_STATUS_CHANNEL") == "stable" {
        root
    } else {
        root.join("channels").join(env!("CODEX_STATUS_CHANNEL"))
    };
    let _ = fs::create_dir_all(&directory);
    let restart = restart_error
        .map(ToString::to_string)
        .unwrap_or_else(|| "restarted successfully".to_owned());
    let report =
        format!("target={}\nupdate_error={update_error}\nrestart={restart}\n", target.display());
    let _ = fs::write(directory.join("update-error.log"), report);
}

fn show_update_recovery_error(
    target: &Path,
    update_error: &UpdateError,
    restart_error: &std::io::Error,
) {
    let message = format!(
        "CodexStatus could not finish the update or restart.\n\nUpdate: {update_error}\nRestart: \
         {restart_error}\n\nPlease start CodexStatus manually from:\n{}",
        target.display()
    );
    let message = wide0(message);
    unsafe {
        let _ = MessageBoxW(
            None,
            PCWSTR(message.as_ptr()),
            w!("CodexStatus update failed"),
            MB_OK | MB_ICONERROR,
        );
    }
}

fn apply_update(parent_pid: u32, target: &Path) -> Result<(), UpdateError> {
    validate_target(target)?;
    match unsafe { OpenProcess(PROCESS_SYNCHRONIZE, false, parent_pid) } {
        Ok(process) => {
            let waited = unsafe { WaitForSingleObject(process, UPDATE_WAIT_MS) };
            unsafe {
                let _ = CloseHandle(process);
            }
            if waited != WAIT_OBJECT_0 {
                return Err(UpdateError::ParentStillRunning);
            }
        }
        Err(error) if error.code() == HRESULT::from_win32(ERROR_INVALID_PARAMETER.0) => {}
        Err(error) => return Err(error.into()),
    }

    let staged = std::env::current_exe()?;
    if staged == target {
        return Err(UpdateError::UnsafeTarget);
    }
    cleanup_stale_pending_updates(target);
    let nonce = SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_nanos();
    let pending = target.with_extension(format!("update-{}-{nonce}", std::process::id()));
    copy_to_new_file(&staged, &pending)?;
    let pending_wide = wide0(pending.as_os_str());
    let target_wide = wide0(target.as_os_str());
    let replacement = unsafe {
        MoveFileExW(
            PCWSTR(pending_wide.as_ptr()),
            PCWSTR(target_wide.as_ptr()),
            MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH,
        )
    };
    if let Err(error) = replacement {
        let _ = fs::remove_file(&pending);
        return Err(error.into());
    }
    launch_target(target)?;
    Ok(())
}

fn cleanup_stale_pending_updates(target: &Path) {
    let Some(parent) = target.parent() else { return };
    let Some(stem) = target.file_stem().and_then(OsStr::to_str) else { return };
    let prefix = format!("{stem}.update-");
    let Ok(entries) = fs::read_dir(parent) else { return };
    for entry in entries.flatten() {
        let name = entry.file_name();
        let Some(suffix) = name.to_str().and_then(|name| name.strip_prefix(&prefix)) else {
            continue;
        };
        if suffix.is_empty()
            || !suffix
                .split('-')
                .all(|part| !part.is_empty() && part.bytes().all(|byte| byte.is_ascii_digit()))
        {
            continue;
        }
        let path = entry.path();
        let Ok(metadata) = fs::symlink_metadata(&path) else { continue };
        if !metadata.file_type().is_file()
            || !metadata
                .modified()
                .ok()
                .and_then(|modified| modified.elapsed().ok())
                .is_some_and(|age| age >= STALE_PENDING_AGE)
        {
            continue;
        }
        let _ = fs::remove_file(path);
    }
}

fn copy_to_new_file(source: &Path, destination: &Path) -> std::io::Result<()> {
    let result = (|| {
        let mut source = File::open(source)?;
        let mut destination = OpenOptions::new().write(true).create_new(true).open(destination)?;
        std::io::copy(&mut source, &mut destination)?;
        destination.sync_all()
    })();
    if result.is_err() {
        let _ = fs::remove_file(destination);
    }
    result.map(|_| ())
}

fn launch_target(target: &Path) -> Result<(), std::io::Error> {
    Command::new(target).arg("--background").creation_flags(CREATE_NO_WINDOW.0).spawn().map(|_| ())
}

fn validate_target_for_update(target: &Path) -> Result<(), UpdateError> {
    validate_target(target)?;
    if fs::metadata(target)?.permissions().readonly() {
        return Err(UpdateError::TargetNotWritable);
    }
    let parent = target.parent().ok_or(UpdateError::UnsafeTarget)?;
    probe_directory_writable(parent).map_err(|_| UpdateError::TargetNotWritable)
}

fn probe_directory_writable(directory: &Path) -> std::io::Result<()> {
    let probe = directory.join(format!(".codexstatus-update-probe-{}.tmp", std::process::id()));
    OpenOptions::new()
        .write(true)
        .create_new(true)
        .custom_flags(FILE_FLAG_DELETE_ON_CLOSE.0)
        .open(probe)
        .map(drop)
}

fn validate_target(target: &Path) -> Result<(), UpdateError> {
    if !target.is_absolute() || target.components().any(|item| item == Component::ParentDir) {
        return Err(UpdateError::UnsafeTarget);
    }
    let name = target.file_name().and_then(OsStr::to_str).unwrap_or_default().to_ascii_lowercase();
    let supported =
        (name.starts_with("codexstatus") && name.ends_with(".exe")) || name == "codex-status.exe";
    if !supported {
        return Err(UpdateError::UnsafeTarget);
    }
    Ok(())
}

fn parse_version(value: &str) -> Option<(u64, u64, u64)> {
    let mut parts = value.split('.');
    let major = parts.next()?.parse().ok()?;
    let minor = parts.next()?.parse().ok()?;
    let patch = parts.next()?.parse().ok()?;
    if parts.next().is_some() {
        return None;
    }
    Some((major, minor, patch))
}

fn split_https_url(url: &str) -> Option<(&str, &str)> {
    let remainder = url.strip_prefix("https://")?;
    let slash = remainder.find('/')?;
    let host = &remainder[..slash];
    let path = &remainder[slash..];
    if host.is_empty() || host.contains([':', '@', '\\']) || !path.starts_with('/') {
        return None;
    }
    Some((host, path))
}

fn sha256_hex(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    digest.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn wide0(value: impl AsRef<OsStr>) -> Vec<u16> {
    use std::os::windows::ffi::OsStrExt;
    value.as_ref().encode_wide().chain(std::iter::once(0)).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn temporary_directory(name: &str) -> PathBuf {
        let suffix = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos();
        std::env::temp_dir().join(format!("codex-status-{name}-{suffix}"))
    }

    fn metadata(tag: &str, digest: &str, channel: UpdateAssetChannel) -> Vec<u8> {
        let version = tag.strip_prefix('v').unwrap_or(tag);
        let name = channel.asset_name(version);
        serde_json::to_vec(&json!({
            "tag_name": tag,
            "draft": false,
            "prerelease": false,
            "assets": [{
                "name": name,
                "browser_download_url": format!(
                    "https://github.com/bagpipes625-cloud/codex-status/releases/download/{tag}/{}",
                    channel.asset_name(version)
                ),
                "size": 100_000,
                "digest": format!("sha256:{digest}")
            }]
        }))
        .unwrap()
    }

    #[test]
    fn selects_only_newer_stable_releases_with_matching_channel_assets() {
        let digest = "a".repeat(64);
        let selected = select_asset(
            &metadata("v0.5.0", &digest, UpdateAssetChannel::Installed),
            "0.4.0",
            UpdateAssetChannel::Installed,
        )
        .unwrap()
        .unwrap();
        assert_eq!(selected.0, "0.5.0");
        assert_eq!(selected.2, digest);
        assert!(
            select_asset(
                &metadata("v0.4.0", &digest, UpdateAssetChannel::Installed),
                "0.4.0",
                UpdateAssetChannel::Installed,
            )
            .unwrap()
            .is_none()
        );
    }

    #[test]
    fn rejects_wrong_channel_domain_digest_and_unsafe_versions() {
        let digest = "b".repeat(64);
        let portable = metadata("v0.5.0", &digest, UpdateAssetChannel::Portable);
        assert!(select_asset(&portable, "0.4.0", UpdateAssetChannel::Installed).is_err());

        let mut wrong_domain: serde_json::Value =
            serde_json::from_slice(&metadata("v0.5.0", &digest, UpdateAssetChannel::Installed))
                .unwrap();
        wrong_domain["assets"][0]["browser_download_url"] =
            json!("https://example.com/CodexStatus-v0.5.0-windows-x64.exe");
        assert!(
            select_asset(
                &serde_json::to_vec(&wrong_domain).unwrap(),
                "0.4.0",
                UpdateAssetChannel::Installed,
            )
            .is_err()
        );
        assert!(
            select_asset(
                &metadata("v0.5.0", "short", UpdateAssetChannel::Installed),
                "0.4.0",
                UpdateAssetChannel::Installed,
            )
            .is_err()
        );
        assert_eq!(parse_version("1.2"), None);
        assert_eq!(parse_version("1.2.3-beta"), None);
    }

    #[test]
    fn release_channels_are_isolated() {
        assert_eq!(update_asset_channel_for("stable"), Some(UpdateAssetChannel::Installed));
        assert_eq!(update_asset_channel_for("portable"), Some(UpdateAssetChannel::Portable));
        assert_eq!(update_asset_channel_for("development"), None);
        assert_eq!(update_asset_channel_for("beta"), None);
    }

    #[test]
    fn accepts_only_simple_https_urls() {
        assert_eq!(
            split_https_url("https://api.github.com/repos/bagpipes625-cloud/codex-status"),
            Some(("api.github.com", "/repos/bagpipes625-cloud/codex-status"))
        );
        assert!(split_https_url("http://api.github.com/test").is_none());
        assert!(split_https_url("https://user@api.github.com/test").is_none());
    }

    #[test]
    fn hashes_bytes_as_lowercase_sha256() {
        assert_eq!(
            sha256_hex(b"CodexStatus"),
            "1348bd7daee4282c641059f8cdd9fe96ae24f501c0cd32fbdabb8c1e60eea85c"
        );
    }

    #[test]
    fn staged_update_is_reverified_and_locked_against_replacement() {
        let directory = temporary_directory("update-reverify");
        fs::create_dir_all(&directory).unwrap();
        let executable = directory.join("CodexStatus.exe");
        let mut bytes = vec![0_u8; 64 * 1024];
        bytes[..2].copy_from_slice(b"MZ");
        fs::write(&executable, &bytes).unwrap();
        let update = StagedUpdate {
            executable: executable.clone(),
            digest: sha256_hex(&bytes),
            size: bytes.len() as u64,
        };

        let verified = verify_staged_file(&update).unwrap();
        assert!(fs::write(&executable, &bytes).is_err());
        drop(verified);
        bytes[2] = 1;
        fs::write(&executable, &bytes).unwrap();
        assert!(matches!(verify_staged_file(&update), Err(UpdateError::DigestMismatch)));
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn update_cleanup_handles_channel_and_legacy_layouts_without_touching_unknown_content() {
        let root = temporary_directory("update-cleanup");
        let app_directory = root.join("CodexStatus");
        let updates = app_directory.join("updates");
        let stable = updates.join("stable");
        let legacy_version = updates.join("v0.4.4");
        let channel_version = stable.join("v0.5.0");
        let protected_version = updates.join("v1.0.0");
        let unrelated = updates.join("downloads");
        fs::create_dir_all(&legacy_version).unwrap();
        fs::create_dir_all(&channel_version).unwrap();
        fs::create_dir_all(&protected_version).unwrap();
        fs::create_dir_all(&unrelated).unwrap();
        fs::write(legacy_version.join("CodexStatus.exe"), b"old").unwrap();
        fs::write(channel_version.join("CodexStatus.download"), b"partial").unwrap();
        fs::write(protected_version.join("notes.txt"), b"keep").unwrap();
        fs::write(unrelated.join("CodexStatus.exe"), b"keep").unwrap();

        cleanup_staged_updates_under(&app_directory, &stable, "stable").unwrap();

        assert!(!legacy_version.exists());
        assert!(!channel_version.exists());
        assert!(protected_version.join("notes.txt").is_file());
        assert!(unrelated.join("CodexStatus.exe").is_file());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn staged_version_directory_names_are_strict() {
        assert!(is_staged_version_name("v0.5.0"));
        assert!(is_staged_version_name("v12.34.56"));
        assert!(!is_staged_version_name("0.5.0"));
        assert!(!is_staged_version_name("v0.5"));
        assert!(!is_staged_version_name("v0.5.0-beta"));
        assert!(!is_staged_version_name("v../0.5.0"));
    }

    #[test]
    fn update_cleanup_refuses_a_reparse_point_app_root() {
        let root = temporary_directory("update-cleanup-reparse");
        let outside = root.join("outside");
        let app_link = root.join("CodexStatus");
        let legacy_version = outside.join("updates").join("v0.4.4");
        fs::create_dir_all(&legacy_version).unwrap();
        fs::write(legacy_version.join("CodexStatus.exe"), b"keep").unwrap();
        let junction = Command::new("cmd")
            .args(["/D", "/C", "mklink", "/J"])
            .arg(&app_link)
            .arg(&outside)
            .output()
            .unwrap();
        assert!(
            junction.status.success(),
            "could not create junction: {}",
            String::from_utf8_lossy(&junction.stderr)
        );

        let stable = app_link.join("updates").join("stable");
        assert!(matches!(
            cleanup_staged_updates_under(&app_link, &stable, "stable"),
            Err(UpdateError::UnsafeTarget)
        ));

        assert!(legacy_version.join("CodexStatus.exe").is_file());
        fs::remove_dir(&app_link).unwrap();
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn staged_download_refuses_an_existing_hardlink() {
        let root = temporary_directory("update-hardlink");
        let version = root.join("v0.5.1");
        let outside = root.join("outside.exe");
        let temporary = version.join("CodexStatus.download");
        fs::create_dir_all(&version).unwrap();
        fs::write(&outside, b"keep").unwrap();
        fs::hard_link(&outside, &temporary).unwrap();

        assert!(create_staged_file(&temporary, b"replacement").is_err());
        assert_eq!(fs::read(&outside).unwrap(), b"keep");

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn final_update_copy_refuses_an_existing_hardlink() {
        let root = temporary_directory("update-final-hardlink");
        let source = root.join("staged.exe");
        let outside = root.join("outside.exe");
        let pending = root.join("CodexStatus.update-1");
        fs::create_dir_all(&root).unwrap();
        fs::write(&source, b"replacement").unwrap();
        fs::write(&outside, b"keep").unwrap();
        fs::hard_link(&outside, &pending).unwrap();

        assert!(copy_to_new_file(&source, &pending).is_err());
        assert_eq!(fs::read(&outside).unwrap(), b"keep");

        fs::remove_dir_all(root).unwrap();
    }
}
