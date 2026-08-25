use crate::controller::RecorderPhase;
use crate::recorder_settings;
use crate::recording_end_reminder::{
    AudioLevels, RecordingEndReminderAction, RecordingEndReminderConfig, duration_label,
    reminder_status_detail,
};
use crate::{
    AppController, StopRecordingReason, refresh_tray, stop_recording_inner, update_controller,
};
use tauri::Listener;
use tauri_plugin_notification::NotificationExt;
use tauri_plugin_transcription::CaptureDataEvent;

const CAPTURE_DATA_EVENT_NAME: &str = "plugin:transcription:capture-data-event";

pub fn install(app: &tauri::AppHandle) {
    install_capture_data_listener(app);
    install_ticker(app);
}

fn install_capture_data_listener(app: &tauri::AppHandle) {
    let app_for_events = app.clone();
    app.listen(CAPTURE_DATA_EVENT_NAME, move |event| {
        let payload = event.payload();
        let parsed = serde_json::from_str::<CaptureDataEvent>(payload);
        match parsed {
            Ok(data) => handle_capture_data_event(app_for_events.clone(), data),
            Err(error) => tracing::error!("failed parsing capture data event: {error}"),
        }
    });
}

fn install_ticker(app: &tauri::AppHandle) {
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        let config = RecordingEndReminderConfig::from_env();
        let mut tick = tokio::time::interval(config.tick_interval);
        loop {
            tick.tick().await;
            evaluate(app.clone(), &config);
        }
    });
}

fn handle_capture_data_event(app: tauri::AppHandle, event: CaptureDataEvent) {
    match event {
        CaptureDataEvent::AudioAmplitude {
            session_id,
            mic,
            speaker,
        } => {
            let config = RecordingEndReminderConfig::from_env();
            let actions =
                update_from_audio(&app, &session_id, AudioLevels { mic, speaker }, &config);
            handle_actions(app, actions);
        }
        CaptureDataEvent::TranscriptDelta { .. }
        | CaptureDataEvent::TranscriptSegmentDelta { .. } => {}
        CaptureDataEvent::MicMuted { .. } => {}
    }
}

fn update_from_audio(
    app: &tauri::AppHandle,
    session_id: &str,
    levels: AudioLevels,
    config: &RecordingEndReminderConfig,
) -> Vec<RecordingEndReminderAction> {
    let updated = update_controller(app, |controller| {
        if !applies(controller, session_id) {
            return (Vec::new(), false);
        }

        let now = std::time::Instant::now();
        let actions = controller.recording_end_reminder.observe_audio(
            now,
            controller.recording_started_at,
            levels,
            config,
        );
        update_status_detail(controller, now, config);
        // Replacing the macOS tray menu while it is open makes the menu collapse.
        // Action handlers refresh the tray when buttons actually change state.
        (actions, false)
    });

    let (actions, should_refresh) = match updated {
        Ok(updated) => updated,
        Err(error) => {
            tracing::warn!("recording_end_reminder: controller unavailable: {error}");
            return Vec::new();
        }
    };
    if should_refresh {
        refresh_tray(app);
    }
    actions
}

fn evaluate(app: tauri::AppHandle, config: &RecordingEndReminderConfig) {
    let updated = update_controller(&app, |controller| {
        let Some(session_id) = controller.active_session_id.clone() else {
            return (Vec::new(), false);
        };
        if !applies(controller, &session_id) {
            return (Vec::new(), false);
        }

        let now = std::time::Instant::now();
        let actions = controller.recording_end_reminder.evaluate(
            now,
            controller.recording_started_at,
            config,
        );
        update_status_detail(controller, now, config);
        // Replacing the macOS tray menu while it is open makes the menu collapse.
        // Action handlers refresh the tray when buttons actually change state.
        (actions, false)
    });

    let (actions, should_refresh) = match updated {
        Ok(updated) => updated,
        Err(error) => {
            tracing::warn!("recording_end_reminder: controller unavailable: {error}");
            return;
        }
    };
    if should_refresh {
        refresh_tray(&app);
    }
    handle_actions(app, actions);
}

fn applies(controller: &AppController, session_id: &str) -> bool {
    controller.phase == RecorderPhase::Recording
        && controller.settings.meeting_end_reminders_enabled
        && controller.active_session_id.as_deref() == Some(session_id)
        && controller.live_test_session_id.as_deref() != Some(session_id)
}

fn update_status_detail(
    controller: &mut AppController,
    now: std::time::Instant,
    config: &RecordingEndReminderConfig,
) {
    if let Some(durations) = controller.recording_end_reminder.silence_elapsed(
        now,
        controller.recording_started_at,
        config,
    ) && let Some(detail) = reminder_status_detail(durations, config)
    {
        controller.status_detail = detail;
    }
}

fn handle_actions(app: tauri::AppHandle, actions: Vec<RecordingEndReminderAction>) {
    for action in actions {
        match action {
            RecordingEndReminderAction::Notify {
                level,
                system_quiet_for,
                mic_quiet_for,
                auto_stop_in,
            } => {
                send_notification(&app, level, system_quiet_for, mic_quiet_for, auto_stop_in);
                mark_response_requested(&app);
            }
            RecordingEndReminderAction::AutoStop {
                system_quiet_for,
                mic_quiet_for,
            } => {
                let app = app.clone();
                tauri::async_runtime::spawn(async move {
                    tracing::info!(
                        system_quiet_for_secs = system_quiet_for.as_secs(),
                        mic_quiet_for_secs = mic_quiet_for.as_secs(),
                        "recording_end_reminder_auto_stop"
                    );
                    if let Err(error) =
                        stop_recording_inner(app, StopRecordingReason::AutoQuiet).await
                    {
                        tracing::error!("recording_end_auto_stop_failed: {error}");
                    }
                });
            }
        }
    }
}

fn send_notification(
    app: &tauri::AppHandle,
    level: u8,
    system_quiet_for: std::time::Duration,
    mic_quiet_for: std::time::Duration,
    auto_stop_in: std::time::Duration,
) {
    let title = if level >= 4 {
        "Poha will stop recording soon"
    } else {
        "No call audio detected"
    };
    let body = if level >= 4 {
        format!(
            "No system audio for {}. Mic quiet for {}. Auto-stop in {} unless you choose Keep Recording.",
            duration_label(system_quiet_for),
            duration_label(mic_quiet_for),
            duration_label(auto_stop_in)
        )
    } else {
        format!(
            "No system audio for {}. Mic quiet for {}. Tray: Stop or Keep Recording. Auto-stop in {}.",
            duration_label(system_quiet_for),
            duration_label(mic_quiet_for),
            duration_label(auto_stop_in)
        )
    };

    match app
        .notification()
        .builder()
        .title(title)
        .body(body)
        .auto_cancel()
        .show()
    {
        Ok(()) => {}
        Err(error) => {
            tracing::warn!("recording_end_notification_failed: {error}");
        }
    }
}

fn mark_response_requested(app: &tauri::AppHandle) {
    let updated = update_controller(app, |controller| {
        if controller.phase != RecorderPhase::Recording {
            return false;
        }
        controller.recording_end_reminder.mark_response_requested();
        true
    });

    let should_refresh = match updated {
        Ok(should_refresh) => should_refresh,
        Err(error) => {
            tracing::warn!("recording_end_reminder: controller unavailable: {error}");
            return;
        }
    };
    if should_refresh {
        refresh_tray(app);
    }
}

pub fn keep_recording(app: tauri::AppHandle) -> Result<(), String> {
    let config = RecordingEndReminderConfig::from_env();
    update_controller(&app, |controller| {
        if !controller.can_keep_recording() {
            return Err("no meeting end reminder is waiting for a response".to_string());
        }

        controller
            .recording_end_reminder
            .snooze(std::time::Instant::now(), &config);
        controller.status_detail = "Keeping recording; reminders paused".to_string();
        Ok(())
    })??;
    refresh_tray(&app);
    Ok(())
}

pub fn set_enabled(app: tauri::AppHandle, enabled: bool) -> Result<(), String> {
    let settings = update_controller(&app, |controller| {
        controller.settings.meeting_end_reminders_enabled = enabled;
        controller.recording_end_reminder.clear();
        controller.status_detail = if enabled {
            "Meeting end reminders enabled".to_string()
        } else {
            "Meeting end reminders disabled".to_string()
        };
        controller.settings.clone()
    })?;
    recorder_settings::save(&app, &settings)?;
    refresh_tray(&app);
    Ok(())
}
