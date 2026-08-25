#![cfg(unix)]

use std::collections::BTreeMap;
use std::fmt;
use std::fs::{self, File, OpenOptions};
use std::future::Future;
use std::io::{self, BufRead, Read, Write};
use std::net::Shutdown;
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::{FileTypeExt, MetadataExt, OpenOptionsExt, PermissionsExt};
use std::os::unix::net::UnixStream as StdUnixStream;
use std::path::{Path, PathBuf};
use std::time::Duration;

use chrono::Utc;
use serde::{Deserialize, Serialize};
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader as AsyncBufReader};
use tokio::net::{UnixListener, UnixStream};
use tokio::sync::oneshot;
use uuid::Uuid;

use crate::control_protocol::{
    CONTROL_PROTOCOL_VERSION, ControlCommand, ControlError, ControlErrorCode, ControlRequest,
    ControlResponse, ControlResult, MAX_IDEMPOTENCY_KEY_BYTES, MAX_JSON_LINE_BYTES,
};

const CONTROL_DIR_MODE: u32 = 0o700;
const CONTROL_FILE_MODE: u32 = 0o600;
const SOCKET_FILE_NAME: &str = "control.sock";
const TOKEN_FILE_NAME: &str = "token";
const METADATA_FILE_NAME: &str = "control.json";
const OPERATION_JOURNAL_FILE_NAME: &str = "operations.json";
const CLIENT_TIMEOUT: Duration = Duration::from_secs(30);
const REQUEST_READ_TIMEOUT: Duration = Duration::from_secs(5);
const MAX_TOKEN_BYTES: u64 = 1024;
const MAX_OPERATION_JOURNAL_BYTES: u64 = 16 * 1024 * 1024;
const MAX_UNIX_SOCKET_PATH_BYTES: usize = 103;
const OPERATION_JOURNAL_VERSION: u32 = 1;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ControlPaths {
    control_dir: PathBuf,
}

impl ControlPaths {
    pub fn new(control_dir: impl Into<PathBuf>) -> Self {
        Self {
            control_dir: control_dir.into(),
        }
    }

    pub fn control_dir(&self) -> &Path {
        &self.control_dir
    }

    pub fn socket_path(&self) -> PathBuf {
        self.control_dir.join(SOCKET_FILE_NAME)
    }

    pub fn token_path(&self) -> PathBuf {
        self.control_dir.join(TOKEN_FILE_NAME)
    }

    pub fn metadata_path(&self) -> PathBuf {
        self.control_dir.join(METADATA_FILE_NAME)
    }

    pub fn operation_journal_path(&self) -> PathBuf {
        self.control_dir.join(OPERATION_JOURNAL_FILE_NAME)
    }

    fn validate(&self) -> io::Result<()> {
        let socket_bytes = self.socket_path().as_os_str().as_bytes().len();
        if socket_bytes > MAX_UNIX_SOCKET_PATH_BYTES {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                format!(
                    "control socket path is {socket_bytes} bytes; maximum supported is {MAX_UNIX_SOCKET_PATH_BYTES}"
                ),
            ));
        }
        Ok(())
    }
}

pub fn default_control_paths() -> io::Result<ControlPaths> {
    let home = dirs::home_dir().ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::NotFound,
            "could not resolve home directory for Poha control socket",
        )
    })?;
    Ok(ControlPaths::new(
        home.join("Library")
            .join("Application Support")
            .join("Poha")
            .join("control"),
    ))
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ControlMetadata {
    protocol_version: u32,
    server_id: String,
    pid: u32,
    created_at: String,
    socket_path: String,
    token_path: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "camelCase", deny_unknown_fields)]
pub enum StoredControlOperation {
    StartPending {
        session_id: String,
    },
    StartStarted {
        session_id: String,
    },
    StartFailed {
        session_id: Option<String>,
        code: ControlErrorCode,
        message: String,
    },
    StopPending {
        target: crate::control_protocol::StopTarget,
        session_id: String,
    },
    StopStopped {
        target: crate::control_protocol::StopTarget,
        session_id: String,
    },
    StopNoop {
        target: crate::control_protocol::StopTarget,
        session_id: Option<String>,
    },
    StopFailed {
        target: crate::control_protocol::StopTarget,
        session_id: Option<String>,
        code: ControlErrorCode,
        message: String,
    },
}

impl StoredControlOperation {
    pub fn operation_session_id(&self) -> Option<&str> {
        match self {
            Self::StartPending { session_id }
            | Self::StartStarted { session_id }
            | Self::StopPending { session_id, .. }
            | Self::StopStopped { session_id, .. } => Some(session_id),
            Self::StartFailed { session_id, .. }
            | Self::StopNoop { session_id, .. }
            | Self::StopFailed { session_id, .. } => session_id.as_deref(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct OperationJournalFile {
    version: u32,
    operations: BTreeMap<String, StoredControlOperation>,
}

impl OperationJournalFile {
    fn empty() -> Self {
        Self {
            version: OPERATION_JOURNAL_VERSION,
            operations: BTreeMap::new(),
        }
    }
}

#[derive(Debug, Clone)]
pub struct ControlOperationJournal {
    path: PathBuf,
}

impl ControlOperationJournal {
    pub fn open(paths: &ControlPaths) -> io::Result<Self> {
        let journal = Self {
            path: paths.operation_journal_path(),
        };
        let _ = journal.load()?;
        Ok(journal)
    }

    pub fn operation(&self, idempotency_key: &str) -> io::Result<Option<StoredControlOperation>> {
        Ok(self.load()?.operations.get(idempotency_key).cloned())
    }

    pub fn insert(
        &self,
        idempotency_key: &str,
        operation: StoredControlOperation,
    ) -> io::Result<()> {
        validate_journal_key(idempotency_key)?;
        let mut journal = self.load()?;
        if journal.operations.contains_key(idempotency_key) {
            return Err(io::Error::new(
                io::ErrorKind::AlreadyExists,
                format!("control idempotency key already exists: {idempotency_key}"),
            ));
        }
        journal
            .operations
            .insert(idempotency_key.to_string(), operation);
        self.write(&journal)
    }

    pub fn transition(
        &self,
        idempotency_key: &str,
        expected: &StoredControlOperation,
        next: StoredControlOperation,
    ) -> io::Result<()> {
        let mut journal = self.load()?;
        match journal.operations.get(idempotency_key) {
            Some(current) if current == expected => {}
            Some(current) => {
                return Err(invalid_data(format!(
                    "control operation {idempotency_key} changed unexpectedly from {expected:?} to {current:?}"
                )));
            }
            None => {
                return Err(io::Error::new(
                    io::ErrorKind::NotFound,
                    format!("control operation not found: {idempotency_key}"),
                ));
            }
        }
        journal.operations.insert(idempotency_key.to_string(), next);
        self.write(&journal)
    }

    fn load(&self) -> io::Result<OperationJournalFile> {
        match fs::symlink_metadata(&self.path) {
            Ok(_) => {}
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                return Ok(OperationJournalFile::empty());
            }
            Err(error) => return Err(error),
        }
        verify_secure_path(&self.path, false, CONTROL_FILE_MODE)?;
        let bytes = read_bounded_file(&self.path, MAX_OPERATION_JOURNAL_BYTES)?;
        let journal: OperationJournalFile = serde_json::from_slice(&bytes)
            .map_err(|error| invalid_data(format!("invalid control operation journal: {error}")))?;
        if journal.version != OPERATION_JOURNAL_VERSION {
            return Err(invalid_data(format!(
                "unsupported control operation journal version {}; expected {}",
                journal.version, OPERATION_JOURNAL_VERSION
            )));
        }
        Ok(journal)
    }

    fn write(&self, journal: &OperationJournalFile) -> io::Result<()> {
        let bytes = serde_json::to_vec_pretty(journal).map_err(|error| {
            invalid_data(format!(
                "failed encoding control operation journal: {error}"
            ))
        })?;
        if bytes.len() as u64 > MAX_OPERATION_JOURNAL_BYTES {
            return Err(invalid_data(format!(
                "control operation journal exceeds the {MAX_OPERATION_JOURNAL_BYTES}-byte limit; no entries were removed"
            )));
        }
        write_secure_file(&self.path, &bytes)
    }
}

fn validate_journal_key(idempotency_key: &str) -> io::Result<()> {
    if idempotency_key.trim().is_empty() || idempotency_key.len() > MAX_IDEMPOTENCY_KEY_BYTES {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("control idempotency key must contain 1 to {MAX_IDEMPOTENCY_KEY_BYTES} bytes"),
        ));
    }
    Ok(())
}

pub struct ControlServer {
    listener: UnixListener,
    paths: ControlPaths,
    token: String,
    server_id: String,
}

impl ControlServer {
    pub fn bind(paths: ControlPaths) -> io::Result<Self> {
        paths.validate()?;
        secure_control_directory(paths.control_dir())?;
        remove_stale_socket(&paths.socket_path())?;

        let listener = UnixListener::bind(paths.socket_path())?;
        if let Err(error) = set_mode(&paths.socket_path(), CONTROL_FILE_MODE) {
            let _ = fs::remove_file(paths.socket_path());
            return Err(error);
        }

        let server_id = Uuid::new_v4().to_string();
        let token = random_bearer_token();
        if let Err(error) = write_secure_file(&paths.token_path(), token.as_bytes()) {
            let _ = fs::remove_file(paths.socket_path());
            return Err(error);
        }

        let metadata = ControlMetadata {
            protocol_version: CONTROL_PROTOCOL_VERSION,
            server_id: server_id.clone(),
            pid: std::process::id(),
            created_at: Utc::now().to_rfc3339(),
            socket_path: path_string(&paths.socket_path()),
            token_path: path_string(&paths.token_path()),
        };
        let metadata_json = serde_json::to_vec_pretty(&metadata)
            .map_err(|error| invalid_data(format!("failed encoding control metadata: {error}")))?;
        if let Err(error) = write_secure_file(&paths.metadata_path(), &metadata_json) {
            let _ = fs::remove_file(paths.socket_path());
            let _ = fs::remove_file(paths.token_path());
            return Err(error);
        }

        Ok(Self {
            listener,
            paths,
            token,
            server_id,
        })
    }

    pub fn paths(&self) -> &ControlPaths {
        &self.paths
    }

    pub async fn serve<H, F>(self, handler: H) -> io::Result<()>
    where
        H: Fn(ControlCommand) -> F + Send + Sync + 'static,
        F: Future<Output = Result<ControlResult, ControlError>> + Send + 'static,
    {
        loop {
            let (stream, _) = match self.listener.accept().await {
                Ok(accepted) => accepted,
                Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
                Err(error) => return Err(error),
            };
            if let Err(error) = handle_connection(stream, &self.token, &handler).await {
                tracing::warn!(error = %error, "control socket request failed");
            }
        }
    }

    pub async fn serve_until<H, F>(
        self,
        mut shutdown: oneshot::Receiver<()>,
        handler: H,
    ) -> io::Result<()>
    where
        H: Fn(ControlCommand) -> F + Send + Sync + 'static,
        F: Future<Output = Result<ControlResult, ControlError>> + Send + 'static,
    {
        loop {
            tokio::select! {
                _ = &mut shutdown => return Ok(()),
                accepted = self.listener.accept() => {
                    let (stream, _) = match accepted {
                        Ok(accepted) => accepted,
                        Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
                        Err(error) => return Err(error),
                    };
                    if let Err(error) = handle_connection(stream, &self.token, &handler).await {
                        tracing::warn!(error = %error, "control socket request failed");
                    }
                }
            }
        }
    }
}

impl Drop for ControlServer {
    fn drop(&mut self) {
        let metadata_matches = read_control_metadata(&self.paths.metadata_path())
            .is_ok_and(|metadata| metadata.server_id == self.server_id);
        if metadata_matches {
            let _ = fs::remove_file(self.paths.socket_path());
            let _ = fs::remove_file(self.paths.token_path());
            let _ = fs::remove_file(self.paths.metadata_path());
        }
    }
}

async fn handle_connection<H, F>(
    stream: UnixStream,
    expected_token: &str,
    handler: &H,
) -> io::Result<()>
where
    H: Fn(ControlCommand) -> F,
    F: Future<Output = Result<ControlResult, ControlError>>,
{
    if let Err(message) = verify_peer_uid(stream.as_raw_fd()) {
        return write_async_response(
            stream,
            &ControlResponse::failure(
                None,
                ControlError::new(ControlErrorCode::PeerUidMismatch, message),
            ),
        )
        .await;
    }

    let (read_half, mut write_half) = stream.into_split();
    let line =
        match tokio::time::timeout(REQUEST_READ_TIMEOUT, read_async_json_line(read_half)).await {
            Ok(Ok(line)) => line,
            Ok(Err(error)) => {
                let response = ControlResponse::failure(
                    None,
                    ControlError::new(ControlErrorCode::InvalidRequest, error.to_string()),
                );
                return write_async_response_half(&mut write_half, &response).await;
            }
            Err(_) => {
                let response = ControlResponse::failure(
                    None,
                    ControlError::new(
                        ControlErrorCode::InvalidRequest,
                        "timed out waiting for a complete control request",
                    ),
                );
                return write_async_response_half(&mut write_half, &response).await;
            }
        };

    let request: ControlRequest = match serde_json::from_slice(&line) {
        Ok(request) => request,
        Err(error) => {
            let response = ControlResponse::failure(
                None,
                ControlError::new(
                    ControlErrorCode::InvalidRequest,
                    format!("invalid control request JSON: {error}"),
                ),
            );
            return write_async_response_half(&mut write_half, &response).await;
        }
    };

    if !constant_time_eq(request.bearer_token.as_bytes(), expected_token.as_bytes()) {
        let response = ControlResponse::failure(
            Some(request.request_id),
            ControlError::new(ControlErrorCode::Unauthorized, "invalid bearer token"),
        );
        return write_async_response_half(&mut write_half, &response).await;
    }

    if let Err(error) = request.validate() {
        let response = ControlResponse::failure(Some(request.request_id), error);
        return write_async_response_half(&mut write_half, &response).await;
    }

    let request_id = request.request_id;
    let response = match handler(request.operation).await {
        Ok(data) => ControlResponse::success(request_id, data),
        Err(error) => ControlResponse::failure(Some(request_id), error),
    };
    write_async_response_half(&mut write_half, &response).await
}

async fn read_async_json_line(read_half: tokio::net::unix::OwnedReadHalf) -> io::Result<Vec<u8>> {
    let mut reader = AsyncBufReader::new(read_half).take((MAX_JSON_LINE_BYTES + 1) as u64);
    let mut line = Vec::new();
    let count = reader.read_until(b'\n', &mut line).await?;
    validate_json_line(&mut line, count)
}

async fn write_async_response(stream: UnixStream, response: &ControlResponse) -> io::Result<()> {
    let (_, mut write_half) = stream.into_split();
    write_async_response_half(&mut write_half, response).await
}

async fn write_async_response_half(
    write_half: &mut tokio::net::unix::OwnedWriteHalf,
    response: &ControlResponse,
) -> io::Result<()> {
    let bytes = response_json_line(response)?;
    write_half.write_all(&bytes).await?;
    write_half.shutdown().await
}

pub struct ControlClient {
    paths: ControlPaths,
    token: String,
}

impl ControlClient {
    pub fn discover(paths: ControlPaths) -> Result<Self, ControlCallError> {
        paths.validate().map_err(ControlCallError::unavailable)?;
        verify_secure_path(paths.control_dir(), true, CONTROL_DIR_MODE)
            .map_err(ControlCallError::unavailable)?;
        verify_secure_path(&paths.metadata_path(), false, CONTROL_FILE_MODE)
            .map_err(ControlCallError::unavailable)?;
        verify_secure_path(&paths.token_path(), false, CONTROL_FILE_MODE)
            .map_err(ControlCallError::unavailable)?;
        verify_secure_socket_path(&paths.socket_path()).map_err(ControlCallError::unavailable)?;

        let metadata =
            read_control_metadata(&paths.metadata_path()).map_err(ControlCallError::protocol)?;
        if metadata.protocol_version != CONTROL_PROTOCOL_VERSION {
            return Err(ControlCallError::Protocol(format!(
                "control metadata protocol version {} is unsupported",
                metadata.protocol_version
            )));
        }
        if metadata.socket_path != path_string(&paths.socket_path())
            || metadata.token_path != path_string(&paths.token_path())
        {
            return Err(ControlCallError::Protocol(
                "control metadata points outside the expected control directory".to_string(),
            ));
        }

        let token = read_bounded_file(&paths.token_path(), MAX_TOKEN_BYTES)
            .map_err(ControlCallError::protocol)?;
        let token = String::from_utf8(token).map_err(|error| {
            ControlCallError::Protocol(format!("invalid token encoding: {error}"))
        })?;
        let token = token.trim().to_string();
        if token.is_empty() {
            return Err(ControlCallError::Protocol(
                "control bearer token is empty".to_string(),
            ));
        }

        Ok(Self { paths, token })
    }

    pub fn call(&self, operation: ControlCommand) -> Result<ControlResult, ControlCallError> {
        operation.validate().map_err(ControlCallError::Rejected)?;
        let request_id = Uuid::new_v4().to_string();
        let request = ControlRequest {
            protocol_version: CONTROL_PROTOCOL_VERSION,
            request_id: request_id.clone(),
            bearer_token: self.token.clone(),
            operation,
        };
        let bytes = request_json_line(&request).map_err(ControlCallError::protocol)?;

        let mut stream = StdUnixStream::connect(self.paths.socket_path())
            .map_err(ControlCallError::unavailable)?;
        stream
            .set_read_timeout(Some(CLIENT_TIMEOUT))
            .map_err(ControlCallError::unavailable)?;
        stream
            .set_write_timeout(Some(CLIENT_TIMEOUT))
            .map_err(ControlCallError::unavailable)?;
        verify_peer_uid(stream.as_raw_fd()).map_err(ControlCallError::Unavailable)?;
        stream
            .write_all(&bytes)
            .map_err(ControlCallError::unavailable)?;
        stream.flush().map_err(ControlCallError::unavailable)?;
        stream
            .shutdown(Shutdown::Write)
            .map_err(ControlCallError::unavailable)?;

        let mut reader = io::BufReader::new(stream).take((MAX_JSON_LINE_BYTES + 1) as u64);
        let mut line = Vec::new();
        let count = reader
            .read_until(b'\n', &mut line)
            .map_err(ControlCallError::unavailable)?;
        let line = validate_json_line(&mut line, count).map_err(ControlCallError::protocol)?;
        let response: ControlResponse = serde_json::from_slice(&line).map_err(|error| {
            ControlCallError::Protocol(format!("invalid control response JSON: {error}"))
        })?;

        if response.protocol_version != CONTROL_PROTOCOL_VERSION {
            return Err(ControlCallError::Protocol(format!(
                "control response protocol version {} is unsupported",
                response.protocol_version
            )));
        }
        if response.request_id.as_deref() != Some(request_id.as_str()) {
            return Err(ControlCallError::Protocol(
                "control response requestId did not match the request".to_string(),
            ));
        }
        match (response.ok, response.data, response.error) {
            (true, Some(data), None) => Ok(data),
            (false, None, Some(error)) => Err(ControlCallError::Rejected(error)),
            _ => Err(ControlCallError::Protocol(
                "control response must contain exactly one of data or error".to_string(),
            )),
        }
    }
}

#[derive(Debug)]
pub enum ControlCallError {
    Unavailable(String),
    Protocol(String),
    Rejected(ControlError),
}

impl ControlCallError {
    pub fn code(&self) -> &'static str {
        match self {
            Self::Unavailable(_) => "controlUnavailable",
            Self::Protocol(_) => "controlProtocolError",
            Self::Rejected(error) => match error.code {
                ControlErrorCode::InvalidRequest => "controlInvalidRequest",
                ControlErrorCode::UnsupportedVersion => "controlUnsupportedVersion",
                ControlErrorCode::Unauthorized | ControlErrorCode::PeerUidMismatch => {
                    "controlUnauthorized"
                }
                ControlErrorCode::Conflict => "controlConflict",
                ControlErrorCode::NotFound => "controlNotFound",
                ControlErrorCode::PermissionDenied => "controlPermissionDenied",
                ControlErrorCode::Internal => "controlInternalError",
            },
        }
    }

    fn unavailable(error: impl fmt::Display) -> Self {
        Self::Unavailable(error.to_string())
    }

    fn protocol(error: impl fmt::Display) -> Self {
        Self::Protocol(error.to_string())
    }
}

impl fmt::Display for ControlCallError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Unavailable(message) | Self::Protocol(message) => formatter.write_str(message),
            Self::Rejected(error) => formatter.write_str(&error.message),
        }
    }
}

impl std::error::Error for ControlCallError {}

fn secure_control_directory(path: &Path) -> io::Result<()> {
    match fs::symlink_metadata(path) {
        Ok(metadata) => {
            if metadata.file_type().is_symlink() || !metadata.is_dir() {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    format!("control path is not a real directory: {}", path.display()),
                ));
            }
            verify_owner(&metadata)?;
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => fs::create_dir_all(path)?,
        Err(error) => return Err(error),
    }
    set_mode(path, CONTROL_DIR_MODE)?;
    verify_secure_path(path, true, CONTROL_DIR_MODE)
}

fn verify_secure_path(path: &Path, directory: bool, expected_mode: u32) -> io::Result<()> {
    let metadata = fs::symlink_metadata(path)?;
    if metadata.file_type().is_symlink()
        || (directory && !metadata.is_dir())
        || (!directory && !metadata.is_file())
    {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            format!("insecure control path type: {}", path.display()),
        ));
    }
    verify_owner(&metadata)?;
    let actual_mode = metadata.permissions().mode() & 0o777;
    if actual_mode != expected_mode {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            format!(
                "insecure permissions {:o} on {}; expected {:o}",
                actual_mode,
                path.display(),
                expected_mode
            ),
        ));
    }
    Ok(())
}

fn verify_secure_socket_path(path: &Path) -> io::Result<()> {
    let metadata = fs::symlink_metadata(path)?;
    if metadata.file_type().is_symlink() || !metadata.file_type().is_socket() {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            format!("insecure control socket path type: {}", path.display()),
        ));
    }
    verify_owner(&metadata)?;
    let actual_mode = metadata.permissions().mode() & 0o777;
    if actual_mode != CONTROL_FILE_MODE {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            format!(
                "insecure permissions {:o} on {}; expected {:o}",
                actual_mode,
                path.display(),
                CONTROL_FILE_MODE
            ),
        ));
    }
    Ok(())
}

fn remove_stale_socket(path: &Path) -> io::Result<()> {
    let metadata = match fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error),
    };
    if metadata.file_type().is_symlink() || !metadata.file_type().is_socket() {
        return Err(io::Error::new(
            io::ErrorKind::AlreadyExists,
            format!("refusing to replace non-socket path: {}", path.display()),
        ));
    }
    verify_owner(&metadata)?;

    match StdUnixStream::connect(path) {
        Ok(_) => Err(io::Error::new(
            io::ErrorKind::AddrInUse,
            format!(
                "Poha control server is already running at {}",
                path.display()
            ),
        )),
        Err(error)
            if matches!(
                error.kind(),
                io::ErrorKind::ConnectionRefused | io::ErrorKind::NotFound
            ) =>
        {
            fs::remove_file(path)
        }
        Err(error) => Err(error),
    }
}

fn write_secure_file(path: &Path, contents: &[u8]) -> io::Result<()> {
    let file_name = path
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "invalid control file name"))?;
    let temporary = path.with_file_name(format!(".{file_name}.{}.tmp", Uuid::new_v4()));
    let result = (|| {
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(CONTROL_FILE_MODE)
            .open(&temporary)?;
        file.write_all(contents)?;
        fs::set_permissions(&temporary, fs::Permissions::from_mode(CONTROL_FILE_MODE))?;
        file.sync_all()?;
        drop(file);
        fs::rename(&temporary, path)?;
        set_mode(path, CONTROL_FILE_MODE)?;
        sync_parent_directory(path)
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    result
}

fn sync_parent_directory(path: &Path) -> io::Result<()> {
    let parent = path.parent().ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("control path has no parent directory: {}", path.display()),
        )
    })?;
    File::open(parent)?.sync_all()
}

fn read_control_metadata(path: &Path) -> io::Result<ControlMetadata> {
    let bytes = read_bounded_file(path, MAX_JSON_LINE_BYTES as u64)?;
    serde_json::from_slice(&bytes)
        .map_err(|error| invalid_data(format!("invalid control metadata: {error}")))
}

fn read_bounded_file(path: &Path, max_bytes: u64) -> io::Result<Vec<u8>> {
    let file = File::open(path)?;
    let metadata = file.metadata()?;
    if metadata.len() > max_bytes {
        return Err(invalid_data(format!(
            "{} exceeds the {max_bytes}-byte limit",
            path.display()
        )));
    }
    let mut bytes = Vec::with_capacity(metadata.len() as usize);
    file.take(max_bytes + 1).read_to_end(&mut bytes)?;
    if bytes.len() as u64 > max_bytes {
        return Err(invalid_data(format!(
            "{} exceeds the {max_bytes}-byte limit",
            path.display()
        )));
    }
    Ok(bytes)
}

fn request_json_line(request: &ControlRequest) -> io::Result<Vec<u8>> {
    let mut bytes = serde_json::to_vec(request)
        .map_err(|error| invalid_data(format!("failed encoding control request: {error}")))?;
    bytes.push(b'\n');
    ensure_line_size(bytes)
}

fn response_json_line(response: &ControlResponse) -> io::Result<Vec<u8>> {
    let mut bytes = serde_json::to_vec(response)
        .map_err(|error| invalid_data(format!("failed encoding control response: {error}")))?;
    bytes.push(b'\n');
    ensure_line_size(bytes)
}

fn ensure_line_size(bytes: Vec<u8>) -> io::Result<Vec<u8>> {
    if bytes.len() > MAX_JSON_LINE_BYTES {
        return Err(invalid_data(format!(
            "JSON line exceeds the {MAX_JSON_LINE_BYTES}-byte limit"
        )));
    }
    Ok(bytes)
}

fn validate_json_line(line: &mut Vec<u8>, count: usize) -> io::Result<Vec<u8>> {
    if count == 0 {
        return Err(io::Error::new(
            io::ErrorKind::UnexpectedEof,
            "control connection closed before a JSON line was received",
        ));
    }
    if line.len() > MAX_JSON_LINE_BYTES {
        return Err(invalid_data(format!(
            "JSON line exceeds the {MAX_JSON_LINE_BYTES}-byte limit"
        )));
    }
    if line.last() != Some(&b'\n') {
        return Err(invalid_data(format!(
            "control request must end with a newline within {MAX_JSON_LINE_BYTES} bytes"
        )));
    }
    line.pop();
    if line.last() == Some(&b'\r') {
        line.pop();
    }
    if line.is_empty() {
        return Err(invalid_data("control JSON line must not be empty"));
    }
    Ok(std::mem::take(line))
}

fn random_bearer_token() -> String {
    format!("{}{}", Uuid::new_v4().simple(), Uuid::new_v4().simple())
}

fn constant_time_eq(left: &[u8], right: &[u8]) -> bool {
    let mut difference = left.len() ^ right.len();
    let max_len = left.len().max(right.len());
    for index in 0..max_len {
        let left_byte = left.get(index).copied().unwrap_or(0);
        let right_byte = right.get(index).copied().unwrap_or(0);
        difference |= usize::from(left_byte ^ right_byte);
    }
    difference == 0
}

fn set_mode(path: &Path, mode: u32) -> io::Result<()> {
    fs::set_permissions(path, fs::Permissions::from_mode(mode))
}

fn path_string(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}

fn invalid_data(message: impl Into<String>) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message.into())
}

#[cfg(target_os = "macos")]
use std::os::fd::{AsRawFd, RawFd};

#[cfg(target_os = "macos")]
unsafe extern "C" {
    fn geteuid() -> u32;
    fn getpeereid(socket: i32, effective_uid: *mut u32, effective_gid: *mut u32) -> i32;
}

#[cfg(target_os = "macos")]
fn verify_owner(metadata: &fs::Metadata) -> io::Result<()> {
    let current_uid = unsafe { geteuid() };
    if metadata.uid() != current_uid {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            format!(
                "control path owner UID {} does not match current UID {current_uid}",
                metadata.uid()
            ),
        ));
    }
    Ok(())
}

#[cfg(not(target_os = "macos"))]
fn verify_owner(_metadata: &fs::Metadata) -> io::Result<()> {
    Ok(())
}

#[cfg(target_os = "macos")]
fn verify_peer_uid(socket: RawFd) -> Result<(), String> {
    let mut peer_uid = 0u32;
    let mut peer_gid = 0u32;
    let result = unsafe { getpeereid(socket, &mut peer_uid, &mut peer_gid) };
    if result != 0 {
        return Err(format!("getpeereid failed: {}", io::Error::last_os_error()));
    }
    let current_uid = unsafe { geteuid() };
    if peer_uid != current_uid {
        return Err(format!(
            "control peer UID {peer_uid} does not match current UID {current_uid}"
        ));
    }
    Ok(())
}

#[cfg(not(target_os = "macos"))]
fn verify_peer_uid(_socket: std::os::fd::RawFd) -> Result<(), String> {
    Ok(())
}

#[cfg(not(target_os = "macos"))]
use std::os::fd::AsRawFd;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::control_protocol::{ControlAction, RecordingPhase, RecordingState, StopTarget};

    #[test]
    fn bearer_tokens_are_random_and_64_hex_chars_wide() {
        let first = random_bearer_token();
        let second = random_bearer_token();

        assert_eq!(first.len(), 64);
        assert_ne!(first, second);
        assert!(first.bytes().all(|byte| byte.is_ascii_hexdigit()));
    }

    #[test]
    fn oversized_json_line_is_rejected() {
        let mut line = vec![b'x'; MAX_JSON_LINE_BYTES + 1];
        let count = line.len();
        let error = validate_json_line(&mut line, count).expect_err("oversized line");
        assert_eq!(error.kind(), io::ErrorKind::InvalidData);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn server_and_client_round_trip_with_secure_files() {
        let temp = tempfile::tempdir_in("/private/tmp").expect("temp dir");
        let paths = ControlPaths::new(temp.path().join("control"));
        let server = ControlServer::bind(paths.clone()).expect("bind server");

        assert_eq!(mode(paths.control_dir()), CONTROL_DIR_MODE);
        assert_eq!(mode(&paths.socket_path()), CONTROL_FILE_MODE);
        assert_eq!(mode(&paths.token_path()), CONTROL_FILE_MODE);
        assert_eq!(mode(&paths.metadata_path()), CONTROL_FILE_MODE);

        let (shutdown_tx, shutdown_rx) = oneshot::channel();
        let task = tokio::spawn(server.serve_until(shutdown_rx, |command| async move {
            let (action, state) = match command {
                ControlCommand::Status => (
                    ControlAction::Status,
                    RecordingState::new(RecordingPhase::Idle, None),
                ),
                ControlCommand::Start { .. } => (
                    ControlAction::Started,
                    RecordingState::new(RecordingPhase::Recording, Some("session-1".to_string())),
                ),
                ControlCommand::Stop {
                    target: StopTarget::Current { .. },
                    ..
                }
                | ControlCommand::Stop {
                    target: StopTarget::Session { .. },
                    ..
                } => (
                    ControlAction::Stopped,
                    RecordingState::new(RecordingPhase::Finalizing, None),
                ),
                ControlCommand::StopRetry { .. } => (
                    ControlAction::StopReplayed,
                    RecordingState::new(RecordingPhase::Finalizing, None),
                ),
            };
            Ok(ControlResult::new(action, state))
        }));

        let call_paths = paths.clone();
        let result = tokio::task::spawn_blocking(move || {
            ControlClient::discover(call_paths)
                .expect("discover client")
                .call(ControlCommand::Start {
                    idempotency_key: "start-1".to_string(),
                })
                .expect("call start")
        })
        .await
        .expect("join client");

        assert_eq!(result.action, ControlAction::Started);
        assert_eq!(result.state.phase, RecordingPhase::Recording);
        assert_eq!(result.state.active_session_id.as_deref(), Some("session-1"));

        shutdown_tx.send(()).expect("send shutdown");
        task.await.expect("join server").expect("server result");
        assert!(!paths.socket_path().exists());
        assert!(!paths.token_path().exists());
        assert!(!paths.metadata_path().exists());
    }

    #[test]
    fn operation_journal_is_durable_and_never_reuses_keys() {
        let temp = tempfile::tempdir_in("/private/tmp").expect("temp dir");
        let paths = ControlPaths::new(temp.path().join("control"));
        secure_control_directory(paths.control_dir()).expect("secure control dir");
        let journal = ControlOperationJournal::open(&paths).expect("open journal");
        let pending = StoredControlOperation::StartPending {
            session_id: "session-1".to_string(),
        };
        journal
            .insert("start-key", pending.clone())
            .expect("persist pending");

        let reopened = ControlOperationJournal::open(&paths).expect("reopen journal");
        assert_eq!(
            reopened.operation("start-key").expect("read pending"),
            Some(pending.clone())
        );
        assert_eq!(mode(&paths.operation_journal_path()), CONTROL_FILE_MODE);

        let started = StoredControlOperation::StartStarted {
            session_id: "session-1".to_string(),
        };
        reopened
            .transition("start-key", &pending, started.clone())
            .expect("persist started");
        assert_eq!(
            ControlOperationJournal::open(&paths)
                .expect("reopen transitioned journal")
                .operation("start-key")
                .expect("read started"),
            Some(started)
        );

        let duplicate = reopened
            .insert(
                "start-key",
                StoredControlOperation::StartPending {
                    session_id: "session-2".to_string(),
                },
            )
            .expect_err("duplicate key must fail");
        assert_eq!(duplicate.kind(), io::ErrorKind::AlreadyExists);
        assert!(
            fs::read_dir(paths.control_dir())
                .expect("read control dir")
                .filter_map(Result::ok)
                .all(|entry| !entry.file_name().to_string_lossy().ends_with(".tmp"))
        );
    }

    #[test]
    fn stop_journal_binds_current_to_observed_session() {
        let temp = tempfile::tempdir_in("/private/tmp").expect("temp dir");
        let paths = ControlPaths::new(temp.path().join("control"));
        secure_control_directory(paths.control_dir()).expect("secure control dir");
        let journal = ControlOperationJournal::open(&paths).expect("open journal");
        let pending = StoredControlOperation::StopPending {
            target: StopTarget::Current {
                observed_session_id: Some("observed-session".to_string()),
            },
            session_id: "observed-session".to_string(),
        };
        journal
            .insert("stop-key", pending.clone())
            .expect("persist pending stop");

        let reloaded = journal
            .operation("stop-key")
            .expect("reload stop operation")
            .expect("stored stop operation");
        assert_eq!(reloaded, pending);
        assert_eq!(reloaded.operation_session_id(), Some("observed-session"));

        let stopped = StoredControlOperation::StopStopped {
            target: StopTarget::Current {
                observed_session_id: Some("observed-session".to_string()),
            },
            session_id: "observed-session".to_string(),
        };
        journal
            .transition("stop-key", &pending, stopped.clone())
            .expect("persist completed stop");
        assert_eq!(
            ControlOperationJournal::open(&paths)
                .expect("reopen completed stop")
                .operation("stop-key")
                .expect("reload completed stop"),
            Some(stopped)
        );
    }

    fn mode(path: &Path) -> u32 {
        fs::symlink_metadata(path)
            .expect("metadata")
            .permissions()
            .mode()
            & 0o777
    }
}
