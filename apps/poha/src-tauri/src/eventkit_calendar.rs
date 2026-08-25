use std::fmt;

use serde::{Deserialize, Serialize};

use crate::calendar_source::{CalendarEventInput, CalendarOccurrence, sanitize_calendar_event};

pub const EVENTKIT_LOOKBACK_MS: i64 = 6 * 60 * 60 * 1_000;
pub const EVENTKIT_LOOKAHEAD_MS: i64 = 12 * 60 * 60 * 1_000;
pub const EVENTKIT_MAXIMUM_EVENTS: usize = 128;
const MAXIMUM_RESPONSE_AGE_MS: u64 = 2 * 60 * 1_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum CalendarAuthorizationStatus {
    NotDetermined,
    Restricted,
    Denied,
    FullAccess,
    WriteOnly,
    Unknown,
    Unsupported,
}

impl CalendarAuthorizationStatus {
    fn from_native(code: i32) -> Self {
        match code {
            0 => Self::NotDetermined,
            1 => Self::Restricted,
            2 => Self::Denied,
            3 => Self::FullAccess,
            4 => Self::WriteOnly,
            _ => Self::Unknown,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CalendarConnectorError {
    UnsupportedPlatform,
    PermissionRequestFailed,
    PermissionCallbackClosed,
    AccessRequired { status: CalendarAuthorizationStatus },
    NativeQueryFailed,
    InvalidNativeResponse,
    StaleNativeResponse,
}

impl fmt::Display for CalendarConnectorError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnsupportedPlatform => {
                formatter.write_str("EventKit calendar access is available only on macOS")
            }
            Self::PermissionRequestFailed => {
                formatter.write_str("macOS could not complete the calendar permission request")
            }
            Self::PermissionCallbackClosed => {
                formatter.write_str("the calendar permission request ended without a result")
            }
            Self::AccessRequired { status } => write!(
                formatter,
                "full calendar access is required before querying meetings (status: {status:?})"
            ),
            Self::NativeQueryFailed => {
                formatter.write_str("EventKit could not return the near-current meeting query")
            }
            Self::InvalidNativeResponse => {
                formatter.write_str("EventKit returned an invalid privacy-filtered response")
            }
            Self::StaleNativeResponse => {
                formatter.write_str("EventKit returned a stale near-current meeting query")
            }
        }
    }
}

impl std::error::Error for CalendarConnectorError {}

/// Returns the current EventKit authorization state without prompting.
pub fn calendar_authorization_status() -> CalendarAuthorizationStatus {
    #[cfg(target_os = "macos")]
    {
        // SAFETY: This parameter-free bridge returns a fixed Int32 status code.
        let code = unsafe { native::poha_eventkit_authorization_status() };
        return CalendarAuthorizationStatus::from_native(code);
    }

    #[cfg(not(target_os = "macos"))]
    CalendarAuthorizationStatus::Unsupported
}

/// Requests EventKit full access using the visible macOS system consent sheet.
///
/// The caller should invoke this from a direct, visible user action. Calling it
/// again after the user has decided only returns the current status.
pub async fn request_calendar_access() -> Result<CalendarAuthorizationStatus, CalendarConnectorError>
{
    #[cfg(target_os = "macos")]
    {
        let (sender, receiver) = tokio::sync::oneshot::channel::<PermissionCompletion>();
        let context = Box::into_raw(Box::new(sender)).cast::<std::ffi::c_void>();

        // SAFETY: `context` is owned by the callback, and Swift invokes the
        // callback exactly once whether permission is granted, denied, or the
        // native request errors.
        unsafe {
            native::poha_eventkit_request_full_access(context, permission_callback);
        }

        let completion = receiver
            .await
            .map_err(|_| CalendarConnectorError::PermissionCallbackClosed)?;
        if completion.error_code != 0 {
            return Err(CalendarConnectorError::PermissionRequestFailed);
        }
        return Ok(CalendarAuthorizationStatus::from_native(
            completion.authorization_code,
        ));
    }

    #[cfg(not(target_os = "macos"))]
    Err(CalendarConnectorError::UnsupportedPlatform)
}

/// Queries only the native bridge's fixed near-current window and returns the
/// minimal persistence-safe occurrence model.
pub fn query_near_current_meetings() -> Result<Vec<CalendarOccurrence>, CalendarConnectorError> {
    #[cfg(target_os = "macos")]
    {
        let response = native_query_response()?;
        return sanitize_native_response(response, chrono::Utc::now().timestamp_millis());
    }

    #[cfg(not(target_os = "macos"))]
    Err(CalendarConnectorError::UnsupportedPlatform)
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct NativeCalendarQueryResponse {
    authorization_status: i32,
    queried_at_unix_ms: i64,
    events: Vec<NativeCalendarEvent>,
}

/// Deserialize-only by design: transient native data must never be persisted.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct NativeCalendarEvent {
    starts_at_unix_ms: i64,
    ends_at_unix_ms: i64,
    occurrence_id: String,
    recognized_link: String,
}

fn sanitize_native_response(
    response: NativeCalendarQueryResponse,
    observed_now_unix_ms: i64,
) -> Result<Vec<CalendarOccurrence>, CalendarConnectorError> {
    let authorization = CalendarAuthorizationStatus::from_native(response.authorization_status);
    if authorization != CalendarAuthorizationStatus::FullAccess {
        return Err(CalendarConnectorError::AccessRequired {
            status: authorization,
        });
    }
    if response.queried_at_unix_ms.abs_diff(observed_now_unix_ms) > MAXIMUM_RESPONSE_AGE_MS {
        return Err(CalendarConnectorError::StaleNativeResponse);
    }

    let window_start = response
        .queried_at_unix_ms
        .saturating_sub(EVENTKIT_LOOKBACK_MS);
    let window_end = response
        .queried_at_unix_ms
        .saturating_add(EVENTKIT_LOOKAHEAD_MS);
    let mut occurrences = response
        .events
        .into_iter()
        .take(EVENTKIT_MAXIMUM_EVENTS)
        .filter(|event| {
            event.ends_at_unix_ms >= window_start && event.starts_at_unix_ms <= window_end
        })
        .filter_map(|event| {
            sanitize_calendar_event(CalendarEventInput {
                starts_at_unix_ms: event.starts_at_unix_ms,
                ends_at_unix_ms: event.ends_at_unix_ms,
                occurrence_id: &event.occurrence_id,
                url: Some(&event.recognized_link),
                location: None,
                is_all_day: false,
                is_cancelled: false,
            })
        })
        .collect::<Vec<_>>();

    occurrences.sort_by(|left, right| {
        left.starts_at_unix_ms
            .cmp(&right.starts_at_unix_ms)
            .then_with(|| left.ends_at_unix_ms.cmp(&right.ends_at_unix_ms))
            .then_with(|| left.occurrence_id_hash.cmp(&right.occurrence_id_hash))
    });
    occurrences.dedup_by(|left, right| left.occurrence_id_hash == right.occurrence_id_hash);
    Ok(occurrences)
}

#[cfg(target_os = "macos")]
#[derive(Debug)]
struct PermissionCompletion {
    authorization_code: i32,
    error_code: i32,
}

#[cfg(target_os = "macos")]
extern "C" fn permission_callback(
    context: *mut std::ffi::c_void,
    authorization_code: i32,
    error_code: i32,
) {
    if context.is_null() {
        return;
    }

    // SAFETY: Swift returns this exact context once, and the allocation was
    // created by `request_calendar_access` for this callback.
    let sender = unsafe {
        Box::from_raw(context.cast::<tokio::sync::oneshot::Sender<PermissionCompletion>>())
    };
    let _ = sender.send(PermissionCompletion {
        authorization_code,
        error_code,
    });
}

#[cfg(target_os = "macos")]
fn native_query_response() -> Result<NativeCalendarQueryResponse, CalendarConnectorError> {
    // SAFETY: The bridge returns either null or a malloc-owned NUL-terminated
    // UTF-8 string that remains valid until `poha_eventkit_free_string`.
    let pointer = unsafe { native::poha_eventkit_copy_near_current_events_json() };
    if pointer.is_null() {
        return Err(CalendarConnectorError::NativeQueryFailed);
    }
    let native_string = NativeString(pointer);
    // SAFETY: `NativeString` is created only from the Swift bridge contract
    // described above and owns the pointer for this scope.
    let bytes = unsafe { std::ffi::CStr::from_ptr(native_string.0) }.to_bytes();
    serde_json::from_slice(bytes).map_err(|_| CalendarConnectorError::InvalidNativeResponse)
}

#[cfg(target_os = "macos")]
struct NativeString(*mut std::ffi::c_char);

#[cfg(target_os = "macos")]
impl Drop for NativeString {
    fn drop(&mut self) {
        // SAFETY: This pointer came from the matching Swift allocation
        // function and is released exactly once here.
        unsafe { native::poha_eventkit_free_string(self.0) }
    }
}

#[cfg(target_os = "macos")]
mod native {
    pub(super) type PermissionCallback = extern "C" fn(*mut std::ffi::c_void, i32, i32);

    unsafe extern "C" {
        pub(super) fn poha_eventkit_authorization_status() -> i32;
        pub(super) fn poha_eventkit_request_full_access(
            context: *mut std::ffi::c_void,
            callback: PermissionCallback,
        );
        pub(super) fn poha_eventkit_copy_near_current_events_json() -> *mut std::ffi::c_char;
        pub(super) fn poha_eventkit_free_string(pointer: *mut std::ffi::c_char);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::calendar_source::MeetingProvider;

    #[test]
    fn native_payload_becomes_only_safe_calendar_occurrences() {
        let now = 1_700_000_000_000;
        let json = format!(
            r#"{{
                "authorizationStatus": 3,
                "queriedAtUnixMs": {now},
                "events": [{{
                    "startsAtUnixMs": {},
                    "endsAtUnixMs": {},
                    "occurrenceId": "private-event-id|{}",
                    "recognizedLink": "https://zoom.us/j/REDACTED"
                }}]
            }}"#,
            now - 60_000,
            now + 60_000,
            now - 60_000,
        );
        let response: NativeCalendarQueryResponse =
            serde_json::from_str(&json).expect("native response");
        let occurrences = sanitize_native_response(response, now).expect("safe occurrences");

        assert_eq!(occurrences.len(), 1);
        assert_eq!(occurrences[0].provider, MeetingProvider::Zoom);
        let serialized = serde_json::to_string(&occurrences).expect("serialize safe result");
        for forbidden in [
            "private-event-id",
            "recognizedLink",
            "attendee",
            "notes",
            "location",
            "url",
        ] {
            assert!(!serialized.contains(forbidden));
        }
        assert!(serialized.contains("occurrenceIdHash"));
    }

    #[test]
    fn sanitizer_revalidates_links_and_enforces_near_current_window() {
        let now = 1_700_000_000_000;
        let response = NativeCalendarQueryResponse {
            authorization_status: 3,
            queried_at_unix_ms: now,
            events: vec![
                NativeCalendarEvent {
                    starts_at_unix_ms: now,
                    ends_at_unix_ms: now + 60_000,
                    occurrence_id: "lookalike".to_string(),
                    recognized_link: "https://evilzoom.us/j/REDACTED".to_string(),
                },
                NativeCalendarEvent {
                    starts_at_unix_ms: now + EVENTKIT_LOOKAHEAD_MS + 1,
                    ends_at_unix_ms: now + EVENTKIT_LOOKAHEAD_MS + 60_000,
                    occurrence_id: "future".to_string(),
                    recognized_link: "https://meet.google.com/aaa-bbbb-ccc".to_string(),
                },
            ],
        };

        assert!(
            sanitize_native_response(response, now)
                .expect("filtered response")
                .is_empty()
        );
    }

    #[test]
    fn query_requires_full_access_and_fresh_native_response() {
        let now = 1_700_000_000_000;
        let denied = NativeCalendarQueryResponse {
            authorization_status: 2,
            queried_at_unix_ms: now,
            events: Vec::new(),
        };
        assert_eq!(
            sanitize_native_response(denied, now),
            Err(CalendarConnectorError::AccessRequired {
                status: CalendarAuthorizationStatus::Denied,
            })
        );

        let stale = NativeCalendarQueryResponse {
            authorization_status: 3,
            queried_at_unix_ms: now - MAXIMUM_RESPONSE_AGE_MS as i64 - 1,
            events: Vec::new(),
        };
        assert_eq!(
            sanitize_native_response(stale, now),
            Err(CalendarConnectorError::StaleNativeResponse)
        );
    }

    #[test]
    fn authorization_codes_are_exhaustively_mapped() {
        assert_eq!(
            CalendarAuthorizationStatus::from_native(0),
            CalendarAuthorizationStatus::NotDetermined
        );
        assert_eq!(
            CalendarAuthorizationStatus::from_native(1),
            CalendarAuthorizationStatus::Restricted
        );
        assert_eq!(
            CalendarAuthorizationStatus::from_native(2),
            CalendarAuthorizationStatus::Denied
        );
        assert_eq!(
            CalendarAuthorizationStatus::from_native(3),
            CalendarAuthorizationStatus::FullAccess
        );
        assert_eq!(
            CalendarAuthorizationStatus::from_native(4),
            CalendarAuthorizationStatus::WriteOnly
        );
        assert_eq!(
            CalendarAuthorizationStatus::from_native(99),
            CalendarAuthorizationStatus::Unknown
        );
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn native_authorization_status_symbol_is_callable_without_prompting() {
        assert_ne!(
            calendar_authorization_status(),
            CalendarAuthorizationStatus::Unsupported
        );
    }
}
