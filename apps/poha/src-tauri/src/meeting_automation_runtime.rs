use std::collections::BTreeSet;
use tauri::Manager;
use tauri_plugin_notification::NotificationExt;

use crate::calendar_source::{CalendarOccurrence, MeetingProvider, select_occurrence};
use crate::controller::PermissionState;
use crate::eventkit_calendar::{
    CalendarAuthorizationStatus, calendar_authorization_status, query_near_current_meetings,
    request_calendar_access,
};
use crate::meeting_detection::{
    AutomationDecision, AutomationMode, MeetingAutomationPolicy, MeetingEvidence,
    NativeMeetingEvidence,
};
use crate::native_meeting_activity::{
    ActiveMeetingApplication, try_collect_active_meeting_applications,
};
use crate::{
    AppState, get_controller, refresh_tray, start_recording_for_meeting, update_and_save_settings,
    update_controller,
};

const POLL_INTERVAL: std::time::Duration = std::time::Duration::from_secs(3);
const COLLECTOR_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(5);
const CALENDAR_REFRESH_MS: i64 = 30_000;
const PROMPT_MAX_AGE_MS: i64 = 12_000;
const SCHEDULE_EARLY_TOLERANCE_MS: i64 = 10 * 60 * 1_000;
const SCHEDULE_LATE_TOLERANCE_MS: i64 = 5 * 60 * 1_000;

#[derive(Debug, Clone)]
struct PendingPrompt {
    generation: u64,
    evidence: MeetingEvidence,
    scheduled_occurrence: Option<CalendarOccurrence>,
    last_seen_unix_ms: i64,
}

#[derive(Debug, Default)]
pub(crate) struct MeetingAutomationRuntime {
    policy: MeetingAutomationPolicy,
    pending_prompt: Option<PendingPrompt>,
    calendar_cache: Vec<CalendarOccurrence>,
    calendar_refreshed_at_unix_ms: Option<i64>,
    calendar_generation: u64,
    next_prompt_generation: u64,
}

pub(crate) fn install(app: &tauri::AppHandle) {
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        let mut tick = tokio::time::interval(POLL_INTERVAL);
        tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            tick.tick().await;
            if let Err(error) = poll_once(app.clone()).await {
                tracing::debug!(%error, "meeting automation poll failed closed");
            }
        }
    });
}

pub(crate) fn set_mode(app: &tauri::AppHandle, mode: AutomationMode) -> Result<(), String> {
    with_runtime(app, |runtime| {
        let mode_changed = runtime.policy.config().mode != mode;
        runtime.policy.set_mode(mode);
        if mode == AutomationMode::Off {
            runtime.pending_prompt = None;
            if mode_changed {
                invalidate_calendar_context(runtime);
            } else {
                clear_calendar_context(runtime);
            }
        }
    })?;
    if mode == AutomationMode::Off {
        clear_prompt_ui(app);
    }
    Ok(())
}

pub(crate) fn calendar_permission_state() -> PermissionState {
    permission_state(calendar_authorization_status())
}

pub(crate) async fn enable_calendar_from_user_action(app: tauri::AppHandle) -> Result<(), String> {
    enable_calendar(app, None).await
}

pub(crate) fn calendar_generation(app: &tauri::AppHandle) -> Result<u64, String> {
    with_runtime(app, |runtime| runtime.calendar_generation)
}

pub(crate) async fn enable_calendar_for_auto_mode(
    app: tauri::AppHandle,
    expected_generation: u64,
) -> Result<(), String> {
    enable_calendar(app, Some(expected_generation)).await
}

async fn enable_calendar(
    app: tauri::AppHandle,
    expected_auto_generation: Option<u64>,
) -> Result<(), String> {
    let start = with_runtime(&app, |runtime| -> Result<Option<(u64, bool)>, String> {
        if let Some(expected_generation) = expected_auto_generation {
            let auto_still_selected = get_controller(&app)?.settings.meeting_automation_mode
                == AutomationMode::AutoScheduled;
            if runtime.calendar_generation != expected_generation || !auto_still_selected {
                return Ok(None);
            }
        }
        let cleared_prompt = invalidate_calendar_context(runtime);
        Ok(Some((runtime.calendar_generation, cleared_prompt)))
    })??;
    let Some((generation, cleared_prompt)) = start else {
        return Err("automatic meeting mode changed before calendar access began".to_string());
    };
    if cleared_prompt {
        clear_prompt_ui(&app);
    }

    let status = request_calendar_access()
        .await
        .map_err(|error| error.to_string())?;
    let authorized = status == CalendarAuthorizationStatus::FullAccess;
    let commit = with_runtime(&app, |runtime| -> Result<Option<bool>, String> {
        if runtime.calendar_generation != generation {
            return Ok(None);
        }
        update_and_save_settings(&app, |controller| {
            controller.permission_snapshot.calendar = permission_state(status);
            controller.settings.calendar_integration_enabled = authorized;
            controller.status_detail = if authorized {
                "Calendar matching enabled".to_string()
            } else {
                "Calendar access was not granted".to_string()
            };
            Ok(())
        })?;
        runtime.policy.reset_observations();
        let cleared_prompt = if authorized {
            runtime.calendar_refreshed_at_unix_ms = None;
            false
        } else {
            invalidate_calendar_context(runtime)
        };
        Ok(Some(cleared_prompt))
    })??;
    let Some(cleared_prompt) = commit else {
        return Err("calendar preference changed before access completed".to_string());
    };
    if cleared_prompt {
        clear_prompt_ui(&app);
    }
    refresh_tray(&app);
    if authorized {
        Ok(())
    } else {
        Err(format!(
            "full calendar access is required (status: {status:?})"
        ))
    }
}

pub(crate) fn disable_calendar(app: &tauri::AppHandle) -> Result<(), String> {
    let cleared_prompt = with_runtime(app, |runtime| -> Result<bool, String> {
        let cleared_prompt = invalidate_calendar_context(runtime);
        update_and_save_settings(app, |controller| {
            controller.settings.calendar_integration_enabled = false;
            controller.status_detail = "Calendar matching disabled".to_string();
            Ok(())
        })?;
        Ok(cleared_prompt)
    })??;
    if cleared_prompt {
        clear_prompt_ui(app);
    }
    refresh_tray(app);
    Ok(())
}

pub(crate) async fn accept_pending_prompt(app: tauri::AppHandle) -> Result<String, String> {
    let now_unix_ms = chrono::Utc::now().timestamp_millis();
    let pending = with_runtime(&app, |runtime| runtime.pending_prompt.clone())?
        .ok_or_else(|| "no detected meeting is awaiting confirmation".to_string())?;
    if now_unix_ms.saturating_sub(pending.last_seen_unix_ms) > PROMPT_MAX_AGE_MS {
        dismiss_pending_prompt(&app)?;
        return Err("the detected meeting is no longer fresh".to_string());
    }

    let controller = get_controller(&app)?;
    ensure_start_is_allowed(&controller, false)?;
    let (evidence, calendar) = collect_inputs(&app, now_unix_ms, true).await?;
    let validated_now_unix_ms = chrono::Utc::now().timestamp_millis();
    if validated_now_unix_ms.saturating_sub(pending.last_seen_unix_ms) > PROMPT_MAX_AGE_MS {
        dismiss_pending_prompt(&app)?;
        return Err("the detected meeting is no longer fresh".to_string());
    }
    if !evidence
        .iter()
        .any(|current| same_evidence_identity(current, &pending.evidence))
    {
        dismiss_pending_prompt(&app)?;
        return Err("the detected meeting is no longer active".to_string());
    }
    if let Some(expected) = pending.scheduled_occurrence.as_ref() {
        let still_current = pending.evidence.provider().is_some_and(|provider| {
            select_occurrence(
                &calendar,
                provider,
                validated_now_unix_ms,
                SCHEDULE_EARLY_TOLERANCE_MS,
                SCHEDULE_LATE_TOLERANCE_MS,
            )
            .is_some_and(|current| current.occurrence_id_hash == expected.occurrence_id_hash)
        });
        if !still_current {
            dismiss_pending_prompt(&app)?;
            return Err("the scheduled meeting is no longer current".to_string());
        }
    }

    let accepted = with_runtime(&app, |runtime| {
        if runtime
            .pending_prompt
            .as_ref()
            .is_some_and(|current| current.generation == pending.generation)
        {
            runtime.pending_prompt = None;
            true
        } else {
            false
        }
    })?;
    if !accepted {
        return Err("a newer meeting prompt replaced this one".to_string());
    }
    clear_prompt_ui(&app);
    let latest = get_controller(&app)?;
    ensure_start_is_allowed(&latest, false)?;
    let required_mode = latest.settings.meeting_automation_mode;
    if required_mode == AutomationMode::Off {
        return Err("meeting detection was turned off before recording started".to_string());
    }
    let detail = recording_detail(
        &pending.evidence,
        pending.scheduled_occurrence.as_ref(),
        false,
    );
    match start_recording_for_meeting(
        app.clone(),
        pending.evidence.clone(),
        &detail,
        required_mode,
        false,
    )
    .await
    {
        Ok(session_id) => {
            notify(&app, "Poha is recording", &detail);
            Ok(session_id)
        }
        Err(error) => {
            apply_global_cooldown(&app)?;
            Err(error)
        }
    }
}

pub(crate) fn dismiss_pending_prompt(app: &tauri::AppHandle) -> Result<(), String> {
    let now_unix_ms = chrono::Utc::now().timestamp_millis();
    with_runtime(app, |runtime| {
        if let Some(prompt) = runtime.pending_prompt.take() {
            runtime.policy.dismiss(&prompt.evidence, now_unix_ms);
        }
    })?;
    clear_prompt_ui(app);
    Ok(())
}

/// Manual and quiet-audio stops use a global cooldown so vendor helper
/// processes cannot immediately re-trigger the same call under another ID.
pub(crate) fn recording_stopping(app: &tauri::AppHandle) -> Result<(), String> {
    apply_global_cooldown(app)?;
    clear_pending_prompt(app)
}

pub(crate) fn recording_started(app: &tauri::AppHandle) -> Result<(), String> {
    clear_pending_prompt(app)
}

pub(crate) fn permissions_changed(
    app: &tauri::AppHandle,
    calendar_permission_changed: bool,
) -> Result<(), String> {
    let calendar_authorized =
        calendar_authorization_status() == CalendarAuthorizationStatus::FullAccess;
    let calendar_available =
        get_controller(app)?.settings.calendar_integration_enabled && calendar_authorized;
    let cleared_prompt = with_runtime(app, |runtime| {
        runtime.policy.reset_observations();
        if calendar_available {
            false
        } else if calendar_authorized {
            clear_calendar_context(runtime)
        } else if calendar_permission_changed {
            invalidate_calendar_context(runtime)
        } else {
            clear_calendar_context(runtime)
        }
    })?;
    if cleared_prompt {
        clear_prompt_ui(app);
    }
    Ok(())
}

async fn poll_once(app: tauri::AppHandle) -> Result<(), String> {
    let controller = get_controller(&app)?;
    let mode = controller.settings.meeting_automation_mode;
    set_mode(&app, mode)?;
    refresh_calendar_permission_ui(&app)?;
    if mode == AutomationMode::Off {
        return Ok(());
    }
    if controller.has_active_capture() {
        clear_pending_prompt(&app)?;
        return Ok(());
    }

    let now_unix_ms = chrono::Utc::now().timestamp_millis();
    let (evidence, calendar) = collect_inputs(&app, now_unix_ms, false).await?;
    refresh_or_clear_prompt(&app, &evidence, now_unix_ms)?;
    let decision = with_runtime(&app, |runtime| {
        runtime.policy.observe(&evidence, &calendar, now_unix_ms)
    })?;

    match decision {
        AutomationDecision::Start {
            evidence,
            scheduled_occurrence,
        } => {
            let latest = get_controller(&app)?;
            if let Err(error) = ensure_start_is_allowed(&latest, true) {
                update_controller(&app, |controller| {
                    if !controller.has_active_capture() {
                        controller.status_detail =
                            "Meeting detected — recording permissions needed".to_string();
                    }
                })?;
                refresh_tray(&app);
                notify(&app, "Poha needs recording permissions", &error);
                return Ok(());
            }
            if latest.settings.meeting_automation_mode != AutomationMode::AutoScheduled
                || !latest.settings.calendar_integration_enabled
            {
                return Ok(());
            }
            let expected_evidence = MeetingEvidence::Native(evidence.clone());
            let (fresh_evidence, fresh_calendar) = collect_inputs(&app, now_unix_ms, true).await?;
            let fresh_now_unix_ms = chrono::Utc::now().timestamp_millis();
            if !automatic_candidate_is_current(
                &evidence,
                &scheduled_occurrence,
                &fresh_evidence,
                &fresh_calendar,
                fresh_now_unix_ms,
            ) {
                with_runtime(&app, |runtime| runtime.policy.reset_observations())?;
                return Ok(());
            }

            let latest = get_controller(&app)?;
            ensure_start_is_allowed(&latest, true)?;
            if !latest.settings.calendar_integration_enabled {
                return Ok(());
            }
            let detail = recording_detail(&expected_evidence, Some(&scheduled_occurrence), true);
            match start_recording_for_meeting(
                app.clone(),
                expected_evidence,
                &detail,
                AutomationMode::AutoScheduled,
                true,
            )
            .await
            {
                Ok(_) => notify(&app, "Poha started recording", &detail),
                Err(error) => {
                    apply_global_cooldown(&app)?;
                    tracing::warn!(%error, "automatic meeting recording did not start");
                }
            }
        }
        AutomationDecision::Prompt {
            evidence,
            scheduled_occurrence,
            ..
        } => {
            show_prompt(&app, evidence, scheduled_occurrence, now_unix_ms)?;
        }
        AutomationDecision::NoAction(_) => {}
    }
    Ok(())
}

async fn collect_inputs(
    app: &tauri::AppHandle,
    now_unix_ms: i64,
    force_calendar_refresh: bool,
) -> Result<(Vec<MeetingEvidence>, Vec<CalendarOccurrence>), String> {
    let controller = get_controller(app)?;
    let calendar_enabled = controller.settings.calendar_integration_enabled
        && calendar_authorization_status() == CalendarAuthorizationStatus::FullAccess;
    let calendar = if calendar_enabled {
        match calendar_occurrences(app, now_unix_ms, force_calendar_refresh).await {
            Ok(calendar) => calendar,
            Err(error) => {
                tracing::debug!(%error, "calendar matching failed closed for this poll");
                with_runtime(app, |runtime| {
                    clear_calendar_context(runtime);
                    runtime.calendar_refreshed_at_unix_ms = Some(now_unix_ms);
                })?;
                Vec::new()
            }
        }
    } else {
        with_runtime(app, |runtime| {
            clear_calendar_context(runtime);
        })?;
        Vec::new()
    };

    let applications = match tokio::time::timeout(
        COLLECTOR_TIMEOUT,
        tokio::task::spawn_blocking(try_collect_active_meeting_applications),
    )
    .await
    {
        Ok(Ok(Ok(applications))) => applications,
        Ok(Ok(Err(error))) => {
            tracing::debug!(%error, "native meeting detector failed closed for this poll");
            Vec::new()
        }
        Ok(Err(error)) => {
            tracing::debug!(%error, "native meeting detector task failed closed");
            Vec::new()
        }
        Err(_) => {
            tracing::debug!("native meeting detector timed out and failed closed");
            Vec::new()
        }
    };
    let evidence = evidence_from_applications(&applications, &calendar, now_unix_ms);
    Ok((evidence, calendar))
}

async fn calendar_occurrences(
    app: &tauri::AppHandle,
    now_unix_ms: i64,
    force_refresh: bool,
) -> Result<Vec<CalendarOccurrence>, String> {
    let (cached, generation) = with_runtime(app, |runtime| {
        let fresh = runtime
            .calendar_refreshed_at_unix_ms
            .is_some_and(|at| now_unix_ms.saturating_sub(at) < CALENDAR_REFRESH_MS);
        let cached = if fresh && !force_refresh {
            Some(runtime.calendar_cache.clone())
        } else {
            None
        };
        (cached, runtime.calendar_generation)
    })?;
    if let Some(cached) = cached {
        return Ok(cached);
    }

    let queried = tokio::time::timeout(
        COLLECTOR_TIMEOUT,
        tokio::task::spawn_blocking(query_near_current_meetings),
    )
    .await
    .map_err(|_| "calendar query timed out".to_string())?
    .map_err(|error| format!("calendar query task failed: {error}"))?
    .map_err(|error| error.to_string())?;
    let stored = with_runtime(app, |runtime| {
        if runtime.calendar_generation != generation {
            return false;
        }
        let still_enabled = get_controller(app)
            .map(|controller| controller.settings.calendar_integration_enabled)
            .unwrap_or(false)
            && calendar_authorization_status() == CalendarAuthorizationStatus::FullAccess;
        if !still_enabled {
            invalidate_calendar_context(runtime);
            return false;
        }
        runtime.calendar_cache = queried.clone();
        runtime.calendar_refreshed_at_unix_ms = Some(now_unix_ms);
        true
    })?;
    if stored {
        Ok(queried)
    } else {
        Err("calendar preference changed while the query was running".to_string())
    }
}

fn evidence_from_applications(
    applications: &[ActiveMeetingApplication],
    calendar: &[CalendarOccurrence],
    now_unix_ms: i64,
) -> Vec<MeetingEvidence> {
    let matching_providers = [
        MeetingProvider::Zoom,
        MeetingProvider::Teams,
        MeetingProvider::Meet,
        MeetingProvider::Webex,
    ]
    .into_iter()
    .filter(|provider| {
        select_occurrence(
            calendar,
            *provider,
            now_unix_ms,
            SCHEDULE_EARLY_TOLERANCE_MS,
            SCHEDULE_LATE_TOLERANCE_MS,
        )
        .is_some()
    })
    .collect::<Vec<_>>();
    let unique_browser_provider = match matching_providers.as_slice() {
        [provider] => Some(*provider),
        _ => None,
    };

    applications
        .iter()
        .filter_map(|application| match application {
            ActiveMeetingApplication::Native(_) => application.to_meeting_evidence(None),
            ActiveMeetingApplication::Browser { .. } => {
                application.to_meeting_evidence(unique_browser_provider)
            }
        })
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect()
}

fn show_prompt(
    app: &tauri::AppHandle,
    evidence: MeetingEvidence,
    scheduled_occurrence: Option<CalendarOccurrence>,
    now_unix_ms: i64,
) -> Result<(), String> {
    let title = prompt_title(&evidence, scheduled_occurrence.as_ref());
    with_runtime(app, |runtime| {
        runtime.next_prompt_generation = runtime.next_prompt_generation.wrapping_add(1).max(1);
        runtime.pending_prompt = Some(PendingPrompt {
            generation: runtime.next_prompt_generation,
            evidence,
            scheduled_occurrence,
            last_seen_unix_ms: now_unix_ms,
        });
    })?;
    update_controller(app, |controller| {
        controller.pending_meeting_prompt_title = Some(title.clone());
        if !controller.has_active_capture() {
            controller.status_detail = "Meeting detected — confirmation needed".to_string();
        }
    })?;
    refresh_tray(app);
    notify(
        app,
        "Meeting detected",
        &format!("Choose “Start: {title}” or “Not Now” from the Poha menu."),
    );
    Ok(())
}

fn refresh_or_clear_prompt(
    app: &tauri::AppHandle,
    current_evidence: &[MeetingEvidence],
    now_unix_ms: i64,
) -> Result<(), String> {
    let cleared = with_runtime(app, |runtime| {
        let stale = runtime
            .pending_prompt
            .as_ref()
            .is_some_and(|prompt| !current_evidence.contains(&prompt.evidence));
        if stale {
            runtime.pending_prompt = None;
        } else if let Some(prompt) = runtime.pending_prompt.as_mut() {
            prompt.last_seen_unix_ms = now_unix_ms;
        }
        stale
    })?;
    if cleared {
        clear_prompt_ui(app);
    }
    Ok(())
}

fn clear_pending_prompt(app: &tauri::AppHandle) -> Result<(), String> {
    with_runtime(app, |runtime| runtime.pending_prompt = None)?;
    clear_prompt_ui(app);
    Ok(())
}

fn clear_prompt_ui(app: &tauri::AppHandle) {
    let changed = update_controller(app, |controller| {
        controller.pending_meeting_prompt_title.take().is_some()
    })
    .unwrap_or(false);
    if changed {
        refresh_tray(app);
    }
}

fn apply_global_cooldown(app: &tauri::AppHandle) -> Result<(), String> {
    let now_unix_ms = chrono::Utc::now().timestamp_millis();
    with_runtime(app, |runtime| runtime.policy.manual_stop(None, now_unix_ms))
}

fn ensure_start_is_allowed(
    controller: &crate::controller::AppController,
    require_auto_scheduled: bool,
) -> Result<(), String> {
    if !controller.can_start_recording() {
        return Err(format!(
            "cannot start while status is {}",
            controller.phase.status_text()
        ));
    }
    if controller.permission_snapshot.microphone != PermissionState::Authorized
        || controller.permission_snapshot.system_audio != PermissionState::Authorized
    {
        return Err("microphone and system-audio permissions are required".to_string());
    }
    if require_auto_scheduled
        && controller.settings.meeting_automation_mode != AutomationMode::AutoScheduled
    {
        return Err("automatic scheduled recording is no longer enabled".to_string());
    }
    Ok(())
}

fn refresh_calendar_permission_ui(app: &tauri::AppHandle) -> Result<(), String> {
    let permission = calendar_permission_state();
    let changed = update_controller(app, |controller| {
        if controller.permission_snapshot.calendar == permission {
            false
        } else {
            controller.permission_snapshot.calendar = permission;
            true
        }
    })?;
    if changed {
        refresh_tray(app);
    }
    if permission != PermissionState::Authorized {
        let cleared_prompt = with_runtime(app, |runtime| {
            if changed {
                invalidate_calendar_context(runtime)
            } else {
                clear_calendar_context(runtime)
            }
        })?;
        if cleared_prompt {
            clear_prompt_ui(app);
        }
    }
    Ok(())
}

fn permission_state(status: CalendarAuthorizationStatus) -> PermissionState {
    if status == CalendarAuthorizationStatus::FullAccess {
        PermissionState::Authorized
    } else {
        PermissionState::Missing
    }
}

fn prompt_title(
    evidence: &MeetingEvidence,
    _scheduled_occurrence: Option<&CalendarOccurrence>,
) -> String {
    evidence
        .provider()
        .map(|provider| format!("{} call", provider_name(provider)))
        .unwrap_or_else(|| "Browser call".to_string())
}

fn recording_detail(
    evidence: &MeetingEvidence,
    scheduled_occurrence: Option<&CalendarOccurrence>,
    automatic: bool,
) -> String {
    let title = prompt_title(evidence, scheduled_occurrence);
    if automatic {
        format!("Auto-recording {title}")
    } else {
        format!("Recording {title}")
    }
}

fn provider_name(provider: MeetingProvider) -> &'static str {
    match provider {
        MeetingProvider::Zoom => "Zoom",
        MeetingProvider::Teams => "Teams",
        MeetingProvider::Meet => "Google Meet",
        MeetingProvider::Webex => "Webex",
    }
}

fn same_evidence_identity(left: &MeetingEvidence, right: &MeetingEvidence) -> bool {
    match (left, right) {
        (MeetingEvidence::Native(left), MeetingEvidence::Native(right)) => {
            left.provider == right.provider && left.application_id == right.application_id
        }
        (MeetingEvidence::Browser(left), MeetingEvidence::Browser(right)) => {
            left.browser_id == right.browser_id
        }
        _ => false,
    }
}

fn automatic_candidate_is_current(
    expected_evidence: &NativeMeetingEvidence,
    expected_occurrence: &CalendarOccurrence,
    fresh_evidence: &[MeetingEvidence],
    fresh_calendar: &[CalendarOccurrence],
    now_unix_ms: i64,
) -> bool {
    expected_evidence.strength.permits_automatic_start()
        && fresh_evidence.iter().any(|current| {
            matches!(
                current,
                MeetingEvidence::Native(current)
                    if current.provider == expected_evidence.provider
                        && current.application_id == expected_evidence.application_id
                        && current.strength.permits_automatic_start()
            )
        })
        && select_occurrence(
            fresh_calendar,
            expected_evidence.provider,
            now_unix_ms,
            SCHEDULE_EARLY_TOLERANCE_MS,
            SCHEDULE_LATE_TOLERANCE_MS,
        )
        .is_some_and(|current| current.occurrence_id_hash == expected_occurrence.occurrence_id_hash)
}

/// Removes all in-memory calendar data and any prompt whose context came from
/// a calendar occurrence. Returns whether the tray prompt must be cleared.
fn clear_calendar_context(runtime: &mut MeetingAutomationRuntime) -> bool {
    runtime.calendar_cache.clear();
    runtime.calendar_refreshed_at_unix_ms = None;
    let clear_prompt = runtime
        .pending_prompt
        .as_ref()
        .is_some_and(|prompt| prompt.scheduled_occurrence.is_some());
    if clear_prompt {
        runtime.pending_prompt = None;
    }
    clear_prompt
}

fn invalidate_calendar_context(runtime: &mut MeetingAutomationRuntime) -> bool {
    runtime.calendar_generation = runtime.calendar_generation.wrapping_add(1).max(1);
    clear_calendar_context(runtime)
}

fn notify(app: &tauri::AppHandle, title: &str, body: &str) {
    if let Err(error) = app
        .notification()
        .builder()
        .title(title)
        .body(body)
        .auto_cancel()
        .show()
    {
        tracing::debug!(%error, "meeting automation notification failed");
    }
}

fn with_runtime<T>(
    app: &tauri::AppHandle,
    update: impl FnOnce(&mut MeetingAutomationRuntime) -> T,
) -> Result<T, String> {
    let state = app.state::<AppState>();
    let mut runtime = state
        .meeting_automation
        .lock()
        .map_err(|_| "meeting automation lock poisoned".to_string())?;
    Ok(update(&mut runtime))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::meeting_detection::{
        BrowserMeetingEvidence, NativeActivityStrength, NativeMeetingEvidence,
    };

    fn occurrence(provider: MeetingProvider, now: i64) -> CalendarOccurrence {
        CalendarOccurrence {
            starts_at_unix_ms: now - 1_000,
            ends_at_unix_ms: now + 60_000,
            provider,
            occurrence_id_hash: format!("{provider:?}"),
        }
    }

    #[test]
    fn browser_activity_prompts_without_calendar_and_uses_unambiguous_enrichment() {
        let now = 100_000;
        let applications = [ActiveMeetingApplication::Browser {
            bundle_id: "com.google.Chrome".to_string(),
        }];
        assert_eq!(
            evidence_from_applications(&applications, &[], now),
            vec![MeetingEvidence::Browser(BrowserMeetingEvidence {
                provider: None,
                browser_id: "com.google.Chrome".to_string(),
            })]
        );
        assert_eq!(
            evidence_from_applications(
                &applications,
                &[occurrence(MeetingProvider::Meet, now)],
                now,
            ),
            vec![MeetingEvidence::Browser(BrowserMeetingEvidence {
                provider: Some(MeetingProvider::Meet),
                browser_id: "com.google.Chrome".to_string(),
            })]
        );
        assert_eq!(
            evidence_from_applications(
                &applications,
                &[
                    occurrence(MeetingProvider::Meet, now),
                    occurrence(MeetingProvider::Zoom, now),
                ],
                now,
            ),
            vec![MeetingEvidence::Browser(BrowserMeetingEvidence {
                provider: None,
                browser_id: "com.google.Chrome".to_string(),
            })]
        );
    }

    #[test]
    fn native_activity_does_not_require_calendar_data() {
        let applications = [ActiveMeetingApplication::Native(NativeMeetingEvidence {
            provider: MeetingProvider::Teams,
            application_id: "com.microsoft.teams2".to_string(),
            strength: NativeActivityStrength::DuplexAudio,
        })];
        assert_eq!(
            evidence_from_applications(&applications, &[], 100),
            vec![MeetingEvidence::Native(NativeMeetingEvidence {
                provider: MeetingProvider::Teams,
                application_id: "com.microsoft.teams2".to_string(),
                strength: NativeActivityStrength::DuplexAudio,
            })]
        );
    }

    #[test]
    fn automatic_candidate_requires_fresh_duplex_evidence_and_same_occurrence() {
        let now = 100_000;
        let native = NativeMeetingEvidence {
            provider: MeetingProvider::Zoom,
            application_id: "us.zoom.xos".to_string(),
            strength: NativeActivityStrength::DuplexAudio,
        };
        let scheduled = occurrence(MeetingProvider::Zoom, now);
        let evidence = vec![MeetingEvidence::Native(native.clone())];
        assert!(automatic_candidate_is_current(
            &native,
            &scheduled,
            &evidence,
            std::slice::from_ref(&scheduled),
            now,
        ));
        assert!(!automatic_candidate_is_current(
            &native,
            &scheduled,
            &[],
            std::slice::from_ref(&scheduled),
            now,
        ));

        let power_only = MeetingEvidence::Native(NativeMeetingEvidence {
            strength: NativeActivityStrength::PowerAssertionOnly,
            ..native.clone()
        });
        assert!(!automatic_candidate_is_current(
            &native,
            &scheduled,
            &[power_only],
            std::slice::from_ref(&scheduled),
            now,
        ));

        let mut replacement = scheduled.clone();
        replacement.occurrence_id_hash = "replacement".to_string();
        assert!(!automatic_candidate_is_current(
            &native,
            &scheduled,
            &evidence,
            &[replacement],
            now,
        ));
    }

    #[test]
    fn prompt_copy_is_provider_only() {
        let now = 100_000;
        let scheduled = occurrence(MeetingProvider::Webex, now);
        let evidence = MeetingEvidence::Browser(BrowserMeetingEvidence {
            provider: Some(MeetingProvider::Webex),
            browser_id: "com.apple.Safari".to_string(),
        });
        assert_eq!(prompt_title(&evidence, Some(&scheduled)), "Webex call");
    }
}
