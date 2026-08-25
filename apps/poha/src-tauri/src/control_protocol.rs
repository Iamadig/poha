use serde::{Deserialize, Serialize};

pub const CONTROL_PROTOCOL_VERSION: u32 = 1;
pub const MAX_JSON_LINE_BYTES: usize = 64 * 1024;
const MAX_REQUEST_ID_BYTES: usize = 128;
const MAX_IDEMPOTENCY_KEY_BYTES: usize = 256;
const MAX_SESSION_ID_BYTES: usize = 256;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ControlRequest {
    pub protocol_version: u32,
    pub request_id: String,
    pub bearer_token: String,
    pub operation: ControlCommand,
}

impl ControlRequest {
    pub fn validate(&self) -> Result<(), ControlError> {
        if self.protocol_version != CONTROL_PROTOCOL_VERSION {
            return Err(ControlError::new(
                ControlErrorCode::UnsupportedVersion,
                format!(
                    "unsupported control protocol version {}; expected {}",
                    self.protocol_version, CONTROL_PROTOCOL_VERSION
                ),
            ));
        }
        validate_nonempty_bounded(
            "requestId",
            &self.request_id,
            MAX_REQUEST_ID_BYTES,
            ControlErrorCode::InvalidRequest,
        )?;
        self.operation.validate()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "command", rename_all = "camelCase", deny_unknown_fields)]
pub enum ControlCommand {
    Status,
    Start { idempotency_key: String },
    Stop { target: StopTarget },
}

impl ControlCommand {
    pub fn validate(&self) -> Result<(), ControlError> {
        match self {
            Self::Status => Ok(()),
            Self::Start { idempotency_key } => validate_nonempty_bounded(
                "idempotencyKey",
                idempotency_key,
                MAX_IDEMPOTENCY_KEY_BYTES,
                ControlErrorCode::InvalidRequest,
            ),
            Self::Stop { target } => target.validate(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase", deny_unknown_fields)]
pub enum StopTarget {
    Current,
    Session { session_id: String },
}

impl StopTarget {
    fn validate(&self) -> Result<(), ControlError> {
        match self {
            Self::Current => Ok(()),
            Self::Session { session_id } => validate_nonempty_bounded(
                "sessionId",
                session_id,
                MAX_SESSION_ID_BYTES,
                ControlErrorCode::InvalidRequest,
            ),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum RecordingPhase {
    Idle,
    Recording,
    Finalizing,
    Done,
    Error,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RecordingState {
    pub phase: RecordingPhase,
    pub active_session_id: Option<String>,
}

impl RecordingState {
    pub fn new(phase: RecordingPhase, active_session_id: Option<String>) -> Self {
        Self {
            phase,
            active_session_id,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ControlAction {
    Status,
    Started,
    StartReplayed,
    AlreadyRecording,
    Stopped,
    StopReplayed,
    AlreadyStopped,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ControlResult {
    pub action: ControlAction,
    #[serde(flatten)]
    pub state: RecordingState,
}

impl ControlResult {
    pub fn new(action: ControlAction, state: RecordingState) -> Self {
        Self { action, state }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ControlErrorCode {
    InvalidRequest,
    UnsupportedVersion,
    Unauthorized,
    PeerUidMismatch,
    Conflict,
    NotFound,
    PermissionDenied,
    Internal,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ControlError {
    pub code: ControlErrorCode,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub state: Option<RecordingState>,
}

impl ControlError {
    pub fn new(code: ControlErrorCode, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
            state: None,
        }
    }

    pub fn with_state(mut self, state: RecordingState) -> Self {
        self.state = Some(state);
        self
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ControlResponse {
    pub protocol_version: u32,
    pub request_id: Option<String>,
    pub ok: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data: Option<ControlResult>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<ControlError>,
}

impl ControlResponse {
    pub fn success(request_id: String, data: ControlResult) -> Self {
        Self {
            protocol_version: CONTROL_PROTOCOL_VERSION,
            request_id: Some(request_id),
            ok: true,
            data: Some(data),
            error: None,
        }
    }

    pub fn failure(request_id: Option<String>, error: ControlError) -> Self {
        Self {
            protocol_version: CONTROL_PROTOCOL_VERSION,
            request_id,
            ok: false,
            data: None,
            error: Some(error),
        }
    }
}

fn validate_nonempty_bounded(
    field: &str,
    value: &str,
    max_bytes: usize,
    code: ControlErrorCode,
) -> Result<(), ControlError> {
    if value.trim().is_empty() {
        return Err(ControlError::new(
            code,
            format!("{field} must not be empty"),
        ));
    }
    if value.len() > max_bytes {
        return Err(ControlError::new(
            code,
            format!("{field} exceeds the {max_bytes}-byte limit"),
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn start_request_round_trips_as_versioned_json() {
        let request = ControlRequest {
            protocol_version: CONTROL_PROTOCOL_VERSION,
            request_id: "request-1".to_string(),
            bearer_token: "secret".to_string(),
            operation: ControlCommand::Start {
                idempotency_key: "meeting-2026-08-25".to_string(),
            },
        };

        let json = serde_json::to_string(&request).expect("serialize request");
        let decoded: ControlRequest = serde_json::from_str(&json).expect("decode request");

        assert_eq!(decoded, request);
        assert!(decoded.validate().is_ok());
        assert!(json.contains("\"protocolVersion\":1"));
        assert!(json.contains("\"command\":\"start\""));
    }

    #[test]
    fn start_requires_nonempty_idempotency_key() {
        let request = ControlRequest {
            protocol_version: CONTROL_PROTOCOL_VERSION,
            request_id: "request-1".to_string(),
            bearer_token: "secret".to_string(),
            operation: ControlCommand::Start {
                idempotency_key: "  ".to_string(),
            },
        };

        let error = request.validate().expect_err("invalid request");
        assert_eq!(error.code, ControlErrorCode::InvalidRequest);
        assert!(error.message.contains("idempotencyKey"));
    }

    #[test]
    fn stop_session_requires_session_id() {
        let command = ControlCommand::Stop {
            target: StopTarget::Session {
                session_id: String::new(),
            },
        };

        let error = command.validate().expect_err("invalid command");
        assert_eq!(error.code, ControlErrorCode::InvalidRequest);
        assert!(error.message.contains("sessionId"));
    }
}
