mod audio_check_window;
mod codex_enrichment;
mod commands;
mod controller;
mod live_test_diagnostics;
mod manifest;
pub mod meeting_store;
mod recorder_settings;
mod recording_end_reminder;
mod recording_end_reminder_runtime;
pub mod storage_lifecycle;
mod transcription;

use std::panic::{AssertUnwindSafe, catch_unwind};
use std::path::{Path, PathBuf};
use std::sync::Mutex;

#[cfg(target_os = "macos")]
use block2::RcBlock;
use controller::{AppController, PermissionState, RecorderPhase};
#[cfg(target_os = "macos")]
use objc2_av_foundation::{AVAuthorizationStatus, AVCaptureDevice, AVMediaTypeAudio};
use recorder_settings::{RecorderSettings, RecordingMode, SpeakerLabelMode};
use serde::Serialize;
use tauri::{Listener, Manager, RunEvent, WindowEvent};
use tauri_plugin_audio_priority::AudioPriorityPluginExt;
use tauri_plugin_settings::SettingsPluginExt;
use tauri_plugin_transcription::{CaptureLifecycleEvent, CaptureParams, ListenerPluginExt};

const CAPTURE_LIFECYCLE_EVENT_NAME: &str = "plugin:transcription:capture-lifecycle-event";
const BATCH_CAPTURE_BASE_URL: &str = "http://localhost:50060/v1";
const BATCH_CAPTURE_MODEL: &str = "cactus-parakeet-tdt-0.6b-v3-int8";
#[derive(Default)]
struct AppState {
    controller: Mutex<Option<AppController>>,
    live_transcription: Mutex<Option<transcription::LiveTranscriptionHandle>>,
    audio_check_state: Mutex<Option<audio_check_window::AudioCheckState>>,
}

#[derive(Clone)]
struct StoppedSessionContext {
    is_live_test: bool,
    live_test_mode: Option<live_test_diagnostics::LiveTestMode>,
    output_dir: PathBuf,
    capture_dir: PathBuf,
    settings: RecorderSettings,
}

#[derive(Debug, Clone)]
pub struct RecoverSessionRequest {
    pub recordings_dir: PathBuf,
    pub session_id: String,
    pub settings_path: Option<PathBuf>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RecoverSessionResult {
    pub id: String,
    pub status: String,
    pub audio_path: Option<String>,
    pub transcript_markdown_path: Option<String>,
    pub transcript_json_path: Option<String>,
    pub capture_dir: String,
    pub output_dir: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum StopRecordingReason {
    Manual,
    AutoQuiet,
}

#[tokio::main]
pub async fn main() {
    tauri::async_runtime::set(tokio::runtime::Handle::current());
    let context = tauri::generate_context!();

    let audio: std::sync::Arc<dyn poha_audio_actual::AudioProvider> =
        std::sync::Arc::new(poha_audio_actual::ActualAudio);

    let app = tauri::Builder::default()
        .manage(audio)
        .manage(AppState::default())
        .plugin(tauri_plugin_single_instance::init(|_, _, _| {}))
        .plugin(tauri_plugin_tracing::init())
        .plugin(tauri_plugin_audio_priority::init())
        .plugin(tauri_plugin_clipboard_manager::init())
        .plugin(tauri_plugin_notification::init())
        .plugin(tauri_plugin_settings::init())
        .plugin(tauri_plugin_tray::init())
        .plugin(tauri_plugin_transcription::init())
        .invoke_handler(tauri::generate_handler![
            commands::start_recording,
            commands::stop_recording,
            commands::keep_recording,
            commands::open_recordings_folder,
            commands::open_last_transcript,
            commands::open_meeting_browser,
            commands::open_path,
            commands::get_audio_check_state,
            commands::run_live_test,
            commands::run_guided_audio_test,
            commands::set_recordings_folder,
            commands::set_microphone_device,
            commands::set_speaker_label_mode,
            commands::set_meeting_end_reminders_enabled,
            commands::list_meetings,
            commands::list_meeting_contexts,
            commands::get_meeting,
            commands::update_meeting_metadata,
            commands::apply_speaker_map,
            commands::delete_meeting,
            commands::delete_meetings,
            commands::rebuild_meeting_index,
            commands::copy_meeting_for_llm,
            commands::copy_company_for_llm,
            commands::copy_context_for_llm,
            commands::copy_text_to_clipboard,
            commands::run_codex_bulk_enrichment,
            commands::get_codex_bulk_enrichment_status,
            commands::cancel_codex_bulk_enrichment,
            commands::import_archive_snapshot,
            commands::export_meetings
        ])
        .setup(|app| {
            let app_handle = app.handle().clone();
            init_controller(&app_handle)?;
            install_menu_handler(&app_handle);
            install_capture_lifecycle_listener(&app_handle);
            recording_end_reminder_runtime::install(&app_handle);
            install_recording_title_ticker(&app_handle);
            install_permission_status_ticker(&app_handle);
            if let Ok(controller) = get_controller(&app_handle) {
                schedule_storage_maintenance(controller.settings.recordings_dir_path(), "startup");
            }

            if let Some(window) = app_handle.get_webview_window("main") {
                let _ = window.hide();
            }

            Ok(())
        })
        .build(context)
        .expect("failed to build Poha app");

    app.run(|app_handle, event| {
        if let RunEvent::WindowEvent {
            label,
            event: WindowEvent::CloseRequested { api, .. },
            ..
        } = event
            && label == "main"
        {
            api.prevent_close();
            if let Some(window) = app_handle.get_webview_window(&label) {
                let _ = window.hide();
            }
        }
    });
}

fn init_controller(app: &tauri::AppHandle) -> Result<(), Box<dyn std::error::Error>> {
    let settings = match recorder_settings::load(app) {
        Ok(settings) => settings,
        Err(error) => {
            tracing::error!("failed_to_load_settings: {error}");
            RecorderSettings::default_with_recordings_dir(PathBuf::from("./recordings"))
        }
    };
    std::fs::create_dir_all(settings.recordings_dir_path()).map_err(|e| {
        format!(
            "failed creating recordings dir {}: {e}",
            settings.recordings_dir_path().display()
        )
    })?;
    let last_transcript =
        recorder_settings::latest_transcript_path(&settings.recordings_dir_path());
    let controller = AppController::new(settings, last_transcript);
    set_controller(app, controller)?;
    refresh_tray(app);
    Ok(())
}

fn install_menu_handler(app: &tauri::AppHandle) {
    app.on_menu_event(|app, event| {
        let menu_id = event.id().0.to_string();
        if catch_unwind(AssertUnwindSafe(|| handle_menu_event(app, &menu_id))).is_err() {
            tracing::error!(menu_id, "menu_event_panicked");
        }
    });
}

fn install_capture_lifecycle_listener(app: &tauri::AppHandle) {
    let app_for_events = app.clone();
    app.listen(CAPTURE_LIFECYCLE_EVENT_NAME, move |event| {
        let payload = event.payload();
        let parsed = serde_json::from_str::<CaptureLifecycleEvent>(payload);
        match parsed {
            Ok(lifecycle) => handle_capture_lifecycle_event(app_for_events.clone(), lifecycle),
            Err(error) => tracing::error!("failed parsing capture lifecycle event: {error}"),
        }
    });
}

fn install_permission_status_ticker(app: &tauri::AppHandle) {
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        refresh_permission_statuses(app.clone(), false).await;
        let mut tick = tokio::time::interval(std::time::Duration::from_secs(5));
        loop {
            tick.tick().await;
            refresh_permission_statuses(app.clone(), false).await;
        }
    });
}

fn install_recording_title_ticker(app: &tauri::AppHandle) {
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        let mut tick = tokio::time::interval(std::time::Duration::from_secs(1));
        loop {
            tick.tick().await;
            let controller = match get_controller(&app) {
                Ok(controller) => controller,
                Err(_) => continue,
            };
            if controller.phase != RecorderPhase::Recording {
                continue;
            }
            let Some(tray) = app.tray_by_id(controller::TRAY_ID) else {
                continue;
            };
            if let Err(error) = tray.set_title(controller::tray_title(&controller)) {
                tracing::error!("failed updating recording title ticker: {error}");
            }
        }
    });
}

fn handle_menu_event(app: &tauri::AppHandle, menu_id: &str) {
    if let Some(device_id) = controller::parse_mic_menu_id(menu_id) {
        if let Err(error) = set_microphone_device(app.clone(), device_id) {
            tracing::error!("failed to set microphone from menu: {error}");
        }
        return;
    }

    match menu_id {
        controller::MENU_ID_START => {
            let app = app.clone();
            tauri::async_runtime::spawn(async move {
                if let Err(error) = start_recording(app).await {
                    tracing::error!("start_recording_failed: {error}");
                }
            });
        }
        controller::MENU_ID_STOP => {
            let app = app.clone();
            tauri::async_runtime::spawn(async move {
                if let Err(error) = stop_recording(app).await {
                    tracing::error!("stop_recording_failed: {error}");
                }
            });
        }
        controller::MENU_ID_KEEP_RECORDING => {
            if let Err(error) = keep_recording(app.clone()) {
                tracing::error!("keep_recording_failed: {error}");
            }
        }
        controller::MENU_ID_OPEN_RECORDINGS => {
            if let Err(error) = open_recordings_folder(app.clone()) {
                tracing::error!("open_recordings_folder_failed: {error}");
            }
        }
        controller::MENU_ID_OPEN_LAST_TRANSCRIPT => {
            if let Err(error) = open_last_transcript(app.clone()) {
                tracing::error!("open_last_transcript_failed: {error}");
            }
        }
        controller::MENU_ID_OPEN_MEETING_BROWSER => {
            if let Err(error) = open_meeting_browser(app.clone()) {
                tracing::error!("open_meeting_browser_failed: {error}");
            }
        }
        controller::MENU_ID_ONBOARDING => {
            let app = app.clone();
            tauri::async_runtime::spawn(async move {
                if let Err(error) = run_live_test(app).await {
                    tracing::error!("run_live_test_failed: {error}");
                }
            });
        }
        controller::MENU_ID_GUIDED_AUDIO_TEST => {
            let app = app.clone();
            tauri::async_runtime::spawn(async move {
                if let Err(error) = run_guided_audio_test(app).await {
                    tracing::error!("run_guided_audio_test_failed: {error}");
                }
            });
        }
        controller::MENU_ID_PERMISSION_MIC => {
            open_permission(app.clone(), PermissionTarget::Microphone);
        }
        controller::MENU_ID_PERMISSION_SYSTEM => {
            open_permission(app.clone(), PermissionTarget::SystemAudio);
        }
        controller::MENU_ID_REFRESH => {
            let app = app.clone();
            tauri::async_runtime::spawn(async move {
                refresh_permission_statuses(app, true).await;
            });
        }
        controller::MENU_ID_SPEAKER_ME_CALL => {
            if let Err(error) = set_speaker_label_mode(app.clone(), SpeakerLabelMode::MeAndCall) {
                tracing::error!("set_speaker_label_mode_failed: {error}");
            }
        }
        controller::MENU_ID_SPEAKER_FAST => {
            if let Err(error) = set_speaker_label_mode(app.clone(), SpeakerLabelMode::FastMixed) {
                tracing::error!("set_speaker_label_mode_failed: {error}");
            }
        }
        controller::MENU_ID_RECORD_ONLY => {
            if let Err(error) = set_recording_mode(app.clone(), RecordingMode::RecordOnly) {
                tracing::error!("set_recording_mode_failed: {error}");
            }
        }
        controller::MENU_ID_RECORD_AND_TRANSCRIBE => {
            if let Err(error) = set_recording_mode(app.clone(), RecordingMode::RecordAndTranscribe)
            {
                tracing::error!("set_recording_mode_failed: {error}");
            }
        }
        controller::MENU_ID_TOGGLE_END_REMINDERS => {
            let enabled = get_controller(app)
                .map(|controller| !controller.settings.meeting_end_reminders_enabled)
                .unwrap_or(true);
            if let Err(error) = set_meeting_end_reminders_enabled(app.clone(), enabled) {
                tracing::error!("set_meeting_end_reminders_enabled_failed: {error}");
            }
        }
        controller::MENU_ID_QUIT => {
            let app = app.clone();
            tauri::async_runtime::spawn(async move {
                request_quit(app).await;
            });
        }
        _ => {}
    }
}

async fn request_quit(app: tauri::AppHandle) {
    let controller = match get_controller(&app) {
        Ok(controller) => controller,
        Err(error) => {
            tracing::error!("quit_requested: controller unavailable: {error}");
            app.exit(0);
            return;
        }
    };
    let phase = controller.phase;

    if controller.quit_requested {
        tracing::warn!("quit_requested: forcing immediate exit");
        app.exit(0);
        return;
    }

    if !controller.has_active_work() {
        app.exit(0);
        return;
    }

    let queued_status = match phase {
        RecorderPhase::Recording => "Stopping capture before quit",
        RecorderPhase::Finalizing => "Finalizing before quit",
        RecorderPhase::Idle | RecorderPhase::Done | RecorderPhase::Error
            if controller.background_transcription_count > 0 =>
        {
            "Transcribing before quit"
        }
        RecorderPhase::Idle | RecorderPhase::Done | RecorderPhase::Error => "Quitting",
    };

    if let Err(error) = queue_quit_request(&app, queued_status) {
        tracing::error!("quit_requested: failed to queue quit: {error}");
        return;
    }

    if matches!(phase, RecorderPhase::Recording)
        && let Err(error) = stop_recording(app.clone()).await
    {
        tracing::error!("quit_requested: failed stopping capture before quit: {error}");
    }
}

fn queue_quit_request(app: &tauri::AppHandle, detail: &str) -> Result<(), String> {
    update_controller(app, |controller| {
        if !controller.quit_requested {
            controller.quit_requested = true;
            controller.status_detail = detail.to_string();
        }
    })?;
    refresh_tray(app);
    Ok(())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PermissionTarget {
    Microphone,
    SystemAudio,
}

fn open_permission(app: tauri::AppHandle, permission: PermissionTarget) {
    tauri::async_runtime::spawn(async move {
        if let Err(error) = request_permission(&app, permission) {
            tracing::error!("failed requesting permission {:?}: {error}", permission);
        }

        if let Err(error) = open_permission_settings(permission) {
            tracing::error!("failed opening permissions settings: {error}");
        }

        refresh_permission_statuses(
            app.clone(),
            matches!(permission, PermissionTarget::SystemAudio),
        )
        .await;
    });
}

fn request_permission(app: &tauri::AppHandle, permission: PermissionTarget) -> Result<(), String> {
    #[cfg(target_os = "macos")]
    {
        match permission {
            PermissionTarget::Microphone => {
                request_microphone_permission();
            }
            PermissionTarget::SystemAudio => {
                // Best effort: probing speaker capture triggers Audio Capture prompt when needed.
                let _ = probe_system_audio_state(app);
            }
        }
    }

    #[cfg(not(target_os = "macos"))]
    {
        let _ = app;
        let _ = permission;
    }

    Ok(())
}

#[cfg(target_os = "macos")]
fn request_microphone_permission() {
    let (tx, rx) = std::sync::mpsc::channel::<bool>();
    let completion = RcBlock::new(move |granted: objc2::runtime::Bool| {
        let _ = tx.send(granted.as_bool());
    });

    unsafe {
        let media_type = AVMediaTypeAudio.unwrap();
        AVCaptureDevice::requestAccessForMediaType_completionHandler(media_type, &completion);
    }

    let _ = rx.recv_timeout(std::time::Duration::from_secs(60));
}

fn open_permission_settings(permission: PermissionTarget) -> Result<(), String> {
    #[cfg(target_os = "macos")]
    {
        let anchor = match permission {
            PermissionTarget::Microphone => "Privacy_Microphone",
            PermissionTarget::SystemAudio => "Privacy_AudioCapture",
        };

        let urls = [
            format!(
                "x-apple.systempreferences:com.apple.settings.PrivacySecurity.extension?{anchor}"
            ),
            format!("x-apple.systempreferences:com.apple.preference.security?{anchor}"),
            "x-apple.systempreferences:com.apple.settings.PrivacySecurity.extension".to_string(),
            "x-apple.systempreferences:com.apple.preference.security".to_string(),
        ];

        let mut last_error = None;
        for url in urls {
            match std::process::Command::new("open").arg(&url).status() {
                Ok(status) if status.success() => return Ok(()),
                Ok(status) => {
                    last_error = Some(format!(
                        "open returned non-zero status ({status}) for {url}"
                    ))
                }
                Err(error) => {
                    last_error = Some(format!("failed launching open for {url}: {error}"))
                }
            }
        }

        return Err(last_error.unwrap_or_else(|| "failed opening System Settings".to_string()));
    }

    #[cfg(not(target_os = "macos"))]
    {
        let _ = permission;
        Ok(())
    }
}

fn handle_capture_lifecycle_event(app: tauri::AppHandle, event: CaptureLifecycleEvent) {
    match event {
        CaptureLifecycleEvent::Started { session_id, .. } => {
            let _ = update_controller(&app, |controller| {
                if controller.active_session_id.as_deref() == Some(session_id.as_str()) {
                    controller.phase = RecorderPhase::Recording;
                    controller.status_detail = if controller.live_test_session_id.as_deref()
                        == Some(session_id.as_str())
                    {
                        "Live test: recording".to_string()
                    } else {
                        "Capture active".to_string()
                    };
                    if controller.recording_started_at.is_none() {
                        controller.recording_started_at = Some(std::time::Instant::now());
                    }
                    controller
                        .recording_end_reminder
                        .reset_for_recording(std::time::Instant::now());
                }
            });
            refresh_tray(&app);
        }
        CaptureLifecycleEvent::Finalizing { session_id } => {
            let _ = update_controller(&app, |controller| {
                if controller.active_session_id.as_deref() == Some(session_id.as_str()) {
                    controller.phase = RecorderPhase::Finalizing;
                    controller.status_detail = if controller.live_test_session_id.as_deref()
                        == Some(session_id.as_str())
                    {
                        "Live test: finalizing".to_string()
                    } else {
                        "Finalizing capture".to_string()
                    };
                }
            });
            refresh_tray(&app);
        }
        CaptureLifecycleEvent::Stopped {
            session_id,
            audio_path,
            error,
            ..
        } => {
            let transition = update_controller(&app, |controller| {
                if controller.active_session_id.as_deref() != Some(session_id.as_str()) {
                    return Ok(None);
                }

                let Some(output_dir) = controller.active_output_dir.clone() else {
                    controller.clear_active_session();
                    controller.phase = RecorderPhase::Error;
                    controller.status_detail = "Recording context was lost".to_string();
                    return Err("active recording output directory is missing".to_string());
                };
                let Some(capture_dir) = controller.active_capture_dir.clone() else {
                    controller.clear_active_session();
                    controller.phase = RecorderPhase::Error;
                    controller.status_detail = "Recording context was lost".to_string();
                    return Err("active recording capture directory is missing".to_string());
                };
                let Some(settings) = controller.active_settings_snapshot.clone() else {
                    controller.clear_active_session();
                    controller.phase = RecorderPhase::Error;
                    controller.status_detail = "Recording context was lost".to_string();
                    return Err("active recording settings snapshot is missing".to_string());
                };

                let is_live_test =
                    controller.live_test_session_id.as_deref() == Some(session_id.as_str());
                let live_test_mode = is_live_test.then_some(controller.live_test_mode).flatten();
                let should_transcribe =
                    is_live_test || settings.recording_mode == RecordingMode::RecordAndTranscribe;
                let context = StoppedSessionContext {
                    is_live_test,
                    live_test_mode,
                    output_dir,
                    capture_dir,
                    settings,
                };

                controller.background_transcription_count =
                    controller.background_transcription_count.saturating_add(1);
                if !should_transcribe {
                    controller.phase = RecorderPhase::Finalizing;
                    controller.status_detail = "Saving recording".to_string();
                } else {
                    controller.clear_active_session();
                    controller.phase = RecorderPhase::Idle;
                    controller.status_detail = if is_live_test {
                        "Live test: transcribing".to_string()
                    } else {
                        "Transcribing in background".to_string()
                    };
                }
                Ok(Some(context))
            });

            let context = match transition {
                Ok(Ok(Some(context))) => Some(context),
                Ok(Ok(None)) => {
                    tracing::warn!(
                        session_id,
                        "ignoring stopped event for unknown or already-finalized session"
                    );
                    None
                }
                Ok(Err(error)) | Err(error) => {
                    tracing::error!(session_id, "cannot finalize stopped session: {error}");
                    None
                }
            };

            let live_handle = if context.is_some() {
                match take_live_transcription_handle(&app) {
                    Ok(handle) => handle,
                    Err(error) => {
                        tracing::warn!("live_transcription_take_failed: {error}");
                        None
                    }
                }
            } else {
                None
            };

            if let Some(context) = context.as_ref() {
                if context.live_test_mode == Some(live_test_diagnostics::LiveTestMode::Guided) {
                    emit_audio_check_state(
                        &app,
                        audio_check_window::state(
                            "transcribing",
                            "Analyzing",
                            "Transcribing the phrases and checking audio levels.",
                            0.94,
                            Some(session_id.clone()),
                            Some(context.output_dir.to_string_lossy().into_owned()),
                        ),
                    );
                }
            }
            refresh_tray(&app);

            if let Some(context) = context {
                let app_for_task = app.clone();
                tauri::async_runtime::spawn(async move {
                    process_stopped_session(
                        app_for_task,
                        session_id,
                        audio_path,
                        error,
                        context,
                        live_handle,
                    )
                    .await;
                });
            }
        }
    }
}

async fn process_stopped_session(
    app: tauri::AppHandle,
    session_id: String,
    audio_path: Option<String>,
    capture_error: Option<String>,
    context: StoppedSessionContext,
    live_handle: Option<transcription::LiveTranscriptionHandle>,
) {
    let StoppedSessionContext {
        is_live_test,
        live_test_mode,
        output_dir,
        capture_dir,
        settings,
    } = context;
    let capture_error_for_report = capture_error.clone();

    let mut manifest = manifest::read_manifest(&output_dir).unwrap_or_else(|_| {
        manifest::SessionManifest::recording_with_policy(
            session_id.clone(),
            capture_dir.to_string_lossy().into_owned(),
            settings.recording_mode,
            settings.preserve_stems,
        )
    });
    let should_transcribe =
        is_live_test || manifest.recording_mode == RecordingMode::RecordAndTranscribe;
    manifest.set_status(if should_transcribe {
        "transcribing"
    } else {
        "finalizing"
    });
    let _ = manifest::write_manifest(&output_dir, &manifest);

    if let Some(handle) = live_handle
        && let Err(error) = handle.stop().await
    {
        tracing::warn!("live_transcription_stop_failed: {error}");
    }

    if !should_transcribe {
        finalize_record_only_session(
            app,
            session_id,
            output_dir,
            capture_dir,
            capture_error,
            manifest,
        )
        .await;
        return;
    }

    if let Some(err) = capture_error {
        manifest.mark_error(err);
        let _ = manifest::write_manifest(&output_dir, &manifest);
    }

    let input = transcription::ProcessInput {
        session_id: session_id.clone(),
        capture_dir: capture_dir.clone(),
        output_dir: output_dir.clone(),
        audio_path_from_event: audio_path.map(PathBuf::from),
        settings: settings.clone(),
    };

    let result =
        tauri::async_runtime::spawn_blocking(move || transcription::process_session(input)).await;
    let mut completion_last_transcript_path = None;
    let mut completion_onboarding = false;

    let (completion_phase, completion_detail) = match result {
        Ok(Ok(artifacts)) => {
            let audio_path = artifacts
                .audio_output_path
                .unwrap_or_else(|| output_dir.join("audio.mp3"));
            manifest.set_audio_artifacts(artifacts.audio_artifacts.clone());
            manifest.mark_done(
                audio_path.to_string_lossy().into_owned(),
                artifacts
                    .transcript_markdown_path
                    .to_string_lossy()
                    .into_owned(),
                artifacts
                    .transcript_json_path
                    .to_string_lossy()
                    .into_owned(),
                settings.mlx_model.clone(),
                settings.mlx_fallback_model.clone(),
            );
            let _ = manifest::write_manifest(&output_dir, &manifest);
            reindex_after_transcription(&settings);

            let mut live_test_report = None;
            let live_test_failure = if is_live_test {
                match write_live_test_report(
                    &session_id,
                    &output_dir,
                    &capture_dir,
                    &manifest,
                    capture_error_for_report.clone(),
                    None,
                ) {
                    Ok(report) if report.passed() => {
                        completion_last_transcript_path = Some(report.report_markdown_path());
                        live_test_report = Some(report);
                        None
                    }
                    Ok(report) => {
                        completion_last_transcript_path = Some(report.report_markdown_path());
                        let failure = report.failure_summary();
                        live_test_report = Some(report);
                        Some(failure)
                    }
                    Err(error) => Some(error),
                }
            } else {
                None
            };
            let should_run_storage_maintenance = !is_live_test || live_test_failure.is_none();

            if let Some(failure) = &live_test_failure {
                manifest.mark_error(format!("live test diagnostics failed: {failure}"));
                let _ = manifest::write_manifest(&output_dir, &manifest);
            }
            if should_run_storage_maintenance {
                schedule_session_storage_maintenance(
                    settings.recordings_dir_path(),
                    output_dir.clone(),
                    "session_finalized",
                );
            }

            if is_live_test && live_test_failure.is_none() {
                completion_onboarding = true;
            }
            if !is_live_test {
                completion_last_transcript_path = Some(artifacts.transcript_markdown_path.clone());
            }
            if live_test_mode == Some(live_test_diagnostics::LiveTestMode::Guided) {
                let title = if live_test_failure.is_some() {
                    "Audio check failed"
                } else {
                    "Audio check passed"
                };
                if let Err(error) = set_audio_check_report_state(
                    &app,
                    live_test_report.as_ref(),
                    if live_test_failure.is_some() {
                        "failed"
                    } else {
                        "passed"
                    },
                    title,
                    Vec::new(),
                ) {
                    tracing::warn!("audio_check_complete_state_failed: {error}");
                }
            }
            (
                if live_test_failure.is_some() {
                    RecorderPhase::Error
                } else {
                    RecorderPhase::Done
                },
                if let Some(failure) = live_test_failure {
                    format!("Live test failed: {failure}")
                } else if is_live_test {
                    "Live test passed".to_string()
                } else {
                    "Transcript ready".to_string()
                },
            )
        }
        Ok(Err(error)) => {
            manifest.mark_error(error.clone());
            let _ = manifest::write_manifest(&output_dir, &manifest);
            let mut live_test_report = None;
            if is_live_test {
                match write_live_test_report(
                    &session_id,
                    &output_dir,
                    &capture_dir,
                    &manifest,
                    capture_error_for_report.clone(),
                    Some(error.clone()),
                ) {
                    Ok(report) => {
                        completion_last_transcript_path = Some(report.report_markdown_path());
                        live_test_report = Some(report);
                    }
                    Err(error) => tracing::warn!("live_test_report_failed: {error}"),
                }
            }
            if live_test_mode == Some(live_test_diagnostics::LiveTestMode::Guided) {
                if let Err(error) = set_audio_check_report_state(
                    &app,
                    live_test_report.as_ref(),
                    "failed",
                    "Audio check failed",
                    vec!["transcript".to_string()],
                ) {
                    tracing::warn!("audio_check_error_state_failed: {error}");
                }
            }
            reindex_after_transcription(&settings);
            let detail = if is_live_test {
                "Live test failed".to_string()
            } else {
                transcription_error_detail(&error)
            };
            tracing::error!("transcription_failed: {error}");
            (RecorderPhase::Error, detail)
        }
        Err(join_error) => {
            let message = format!("transcription task join failed: {join_error}");
            manifest.mark_error(message.clone());
            let _ = manifest::write_manifest(&output_dir, &manifest);
            let mut live_test_report = None;
            if is_live_test {
                match write_live_test_report(
                    &session_id,
                    &output_dir,
                    &capture_dir,
                    &manifest,
                    capture_error_for_report,
                    Some(message.clone()),
                ) {
                    Ok(report) => {
                        completion_last_transcript_path = Some(report.report_markdown_path());
                        live_test_report = Some(report);
                    }
                    Err(error) => tracing::warn!("live_test_report_failed: {error}"),
                }
            }
            if live_test_mode == Some(live_test_diagnostics::LiveTestMode::Guided) {
                if let Err(error) = set_audio_check_report_state(
                    &app,
                    live_test_report.as_ref(),
                    "failed",
                    "Audio check crashed",
                    vec!["transcription".to_string()],
                ) {
                    tracing::warn!("audio_check_crash_state_failed: {error}");
                }
            }
            reindex_after_transcription(&settings);
            tracing::error!("{message}");
            (
                RecorderPhase::Error,
                if is_live_test {
                    "Live test crashed".to_string()
                } else {
                    "Transcription task crashed".to_string()
                },
            )
        }
    };

    let should_exit = match complete_background_session(
        &app,
        &session_id,
        completion_phase,
        completion_detail,
        completion_last_transcript_path,
        completion_onboarding,
    ) {
        Ok(should_exit) => should_exit,
        Err(error) => {
            tracing::error!("missing controller after transcription: {error}");
            return;
        }
    };

    if should_exit {
        app.exit(0);
        return;
    }

    refresh_tray(&app);
}

async fn finalize_record_only_session(
    app: tauri::AppHandle,
    session_id: String,
    output_dir: PathBuf,
    capture_dir: PathBuf,
    capture_error: Option<String>,
    mut manifest: manifest::SessionManifest,
) {
    let archive_output_dir = output_dir.clone();
    let archive_capture_dir = capture_dir.clone();
    let archive_result = tauri::async_runtime::spawn_blocking(move || {
        transcription::archive_audio_artifacts(&archive_capture_dir, &archive_output_dir).map(
            |artifacts| {
                let validation = transcription::validate_recording_stems(&artifacts)
                    .map_err(|error| error.to_string());
                (artifacts, validation)
            },
        )
    })
    .await;

    let (phase, detail) = match archive_result {
        Ok(Ok((artifacts, validation))) => {
            manifest.set_audio_artifacts(artifacts.clone());

            if let Some(error) = capture_error {
                manifest.mark_error(format!(
                    "capture failed; available audio was preserved: {error}"
                ));
                (
                    RecorderPhase::Error,
                    "Capture failed; available audio was preserved".to_string(),
                )
            } else if let Err(error) = validation {
                manifest.mark_error(format!("recording integrity check failed: {error}"));
                (
                    RecorderPhase::Error,
                    format!("Recording integrity check failed: {error}"),
                )
            } else {
                manifest.mark_recorded(artifacts);
                (
                    RecorderPhase::Done,
                    "Recording saved (microphone + system audio)".to_string(),
                )
            }
        }
        Ok(Err(error)) => {
            manifest.mark_error(error.clone());
            tracing::error!("recording_archive_failed: {error}");
            (RecorderPhase::Error, "Failed to save recording".to_string())
        }
        Err(join_error) => {
            let error = format!("recording archive task join failed: {join_error}");
            manifest.mark_error(error.clone());
            tracing::error!("{error}");
            (
                RecorderPhase::Error,
                "Recording save task crashed".to_string(),
            )
        }
    };

    let (phase, detail, manifest_committed) = match manifest::write_manifest(&output_dir, &manifest)
    {
        Ok(()) => (phase, detail, true),
        Err(error) => {
            tracing::error!("failed committing record-only manifest: {error}");
            (
                RecorderPhase::Error,
                "Recording saved, but metadata commit failed".to_string(),
                false,
            )
        }
    };

    if phase == RecorderPhase::Done && manifest_committed {
        let cleanup_capture_dir = capture_dir.clone();
        let cleanup_output_dir = output_dir.clone();
        match tauri::async_runtime::spawn_blocking(move || {
            transcription::remove_verified_capture_stem_duplicates(
                &cleanup_capture_dir,
                &cleanup_output_dir,
            )
        })
        .await
        {
            Ok(Ok(removed)) => {
                tracing::info!(
                    removed,
                    session_id,
                    "cleaned verified capture stem duplicates"
                )
            }
            Ok(Err(error)) => tracing::warn!(
                session_id,
                "verified capture stem cleanup was skipped: {error}"
            ),
            Err(error) => tracing::warn!(
                session_id,
                "verified capture stem cleanup task failed: {error}"
            ),
        }
    }

    let should_exit = match update_controller(&app, |controller| {
        finish_background_transcription(controller, &session_id, phase, detail);
        controller.quit_requested && !controller.has_active_work()
    }) {
        Ok(should_exit) => should_exit,
        Err(error) => {
            tracing::error!("missing controller after recording archive: {error}");
            return;
        }
    };
    if should_exit {
        app.exit(0);
    } else {
        refresh_tray(&app);
    }
}

fn write_live_test_report(
    session_id: &str,
    output_dir: &Path,
    capture_dir: &Path,
    manifest: &manifest::SessionManifest,
    capture_error: Option<String>,
    transcription_error: Option<String>,
) -> Result<live_test_diagnostics::LiveTestReport, String> {
    live_test_diagnostics::build_and_write_report(live_test_diagnostics::LiveTestReportInput {
        session_id: session_id.to_string(),
        output_dir: output_dir.to_path_buf(),
        capture_dir: capture_dir.to_path_buf(),
        manifest_status: manifest.status.clone(),
        microphone_permission: true,
        system_audio_permission: true,
        capture_error,
        transcription_error,
    })
}

fn emit_audio_check_state(app: &tauri::AppHandle, state: audio_check_window::AudioCheckState) {
    if let Err(error) = audio_check_window::set_state(app, state) {
        tracing::warn!("audio_check_state_failed: {error}");
    }
}

fn set_audio_check_report_state(
    app: &tauri::AppHandle,
    report: Option<&live_test_diagnostics::LiveTestReport>,
    status: &str,
    title: &str,
    failures: Vec<String>,
) -> Result<(), String> {
    let Some(report) = report else {
        return audio_check_window::set_state(
            app,
            audio_check_window::AudioCheckState {
                phase: "complete".to_string(),
                title: title.to_string(),
                detail: "Report was unavailable.".to_string(),
                phrase: None,
                seconds_remaining: None,
                progress: 1.0,
                session_id: None,
                output_dir: None,
                report_json_path: None,
                report_markdown_path: None,
                status: Some(status.to_string()),
                failures,
            },
        );
    };

    let failures = if failures.is_empty() {
        report
            .checks
            .iter()
            .filter(|check| check.status == "failed")
            .map(|check| check.name.clone())
            .collect()
    } else {
        failures
    };

    audio_check_window::set_state(
        app,
        audio_check_window::AudioCheckState {
            phase: "complete".to_string(),
            title: title.to_string(),
            detail: report.summary.clone(),
            phrase: None,
            seconds_remaining: None,
            progress: 1.0,
            session_id: Some(report.session_id.clone()),
            output_dir: Some(report.paths.output_dir.clone()),
            report_json_path: Some(report.paths.report_json_path.clone()),
            report_markdown_path: Some(report.paths.report_markdown_path.clone()),
            status: Some(status.to_string()),
            failures,
        },
    )
}

fn reindex_after_transcription(settings: &RecorderSettings) {
    if let Err(error) = crate::meeting_store::rebuild_index(&settings.recordings_dir_path()) {
        tracing::warn!("post_transcription_reindex_failed: {error}");
    }
}

fn schedule_storage_maintenance(recordings_dir: PathBuf, reason: &'static str) {
    tauri::async_runtime::spawn_blocking(move || {
        run_storage_maintenance_now(&recordings_dir, reason);
    });
}

fn schedule_session_storage_maintenance(
    recordings_dir: PathBuf,
    session_dir: PathBuf,
    reason: &'static str,
) {
    tauri::async_runtime::spawn_blocking(move || {
        run_session_storage_maintenance_now(&recordings_dir, &session_dir, reason);
    });
}

fn run_storage_maintenance_now(recordings_dir: &Path, reason: &str) {
    match crate::storage_lifecycle::maintain(recordings_dir) {
        Ok(result) => {
            tracing::info!(
                reason,
                moved_to_trash = result.moved_to_trash_count(),
                reclaimed_bytes = result.reclaimed_bytes(),
                errors = result.error_count(),
                "storage maintenance completed"
            );
        }
        Err(error) => {
            tracing::warn!(
                reason,
                code = error.code(),
                error = error.message(),
                "storage maintenance failed"
            );
        }
    }
}

fn run_session_storage_maintenance_now(recordings_dir: &Path, session_dir: &Path, reason: &str) {
    match crate::storage_lifecycle::maintain_session(recordings_dir, session_dir) {
        Ok(result) => {
            tracing::info!(
                reason,
                session_dir = %session_dir.display(),
                moved_to_trash = result.moved_to_trash_count(),
                reclaimed_bytes = result.reclaimed_bytes(),
                errors = result.error_count(),
                "session storage maintenance completed"
            );
            if result.error_count() > 0 {
                run_storage_maintenance_now(recordings_dir, "session_finalized_retry_full");
            }
        }
        Err(error) => {
            tracing::warn!(
                reason,
                session_dir = %session_dir.display(),
                code = error.code(),
                error = error.message(),
                "session storage maintenance failed"
            );
            run_storage_maintenance_now(recordings_dir, "session_finalized_fallback_full");
        }
    }
}

pub fn recover_crashed_session(
    request: RecoverSessionRequest,
) -> Result<RecoverSessionResult, String> {
    let output_dir = request.recordings_dir.join(&request.session_id);
    let mut manifest = manifest::read_manifest(&output_dir)?;
    let capture_dir = PathBuf::from(&manifest.capture_dir);
    if !capture_dir.is_dir() {
        return Err(format!("capture dir not found: {}", capture_dir.display()));
    }

    let settings_path = request
        .settings_path
        .unwrap_or_else(recorder_settings::default_settings_file);

    if manifest.recording_mode == RecordingMode::RecordOnly {
        manifest.set_status("finalizing");
        manifest::write_manifest(&output_dir, &manifest)?;
        let artifacts = match transcription::archive_audio_artifacts(&capture_dir, &output_dir) {
            Ok(artifacts) => artifacts,
            Err(error) => {
                manifest.mark_error(error.clone());
                manifest::write_manifest(&output_dir, &manifest)?;
                return Err(error);
            }
        };
        if let Err(error) = transcription::validate_recording_stems(&artifacts) {
            let error = format!("recording integrity check failed: {error}");
            manifest.set_audio_artifacts(artifacts);
            manifest.mark_error(error.clone());
            manifest::write_manifest(&output_dir, &manifest)?;
            return Err(error);
        }
        manifest.mark_recorded(artifacts);
        manifest::write_manifest(&output_dir, &manifest)?;
        if let Err(error) =
            transcription::remove_verified_capture_stem_duplicates(&capture_dir, &output_dir)
        {
            tracing::warn!(
                session_id = request.session_id,
                "recovered capture stem cleanup was skipped: {error}"
            );
        }
        return Ok(RecoverSessionResult {
            id: manifest.id,
            status: manifest.status,
            audio_path: manifest.audio_path,
            transcript_markdown_path: None,
            transcript_json_path: None,
            capture_dir: capture_dir.to_string_lossy().into_owned(),
            output_dir: output_dir.to_string_lossy().into_owned(),
        });
    }

    let settings =
        recorder_settings::load_from_file(&settings_path, request.recordings_dir.clone())?;

    manifest.set_status("transcribing");
    manifest::write_manifest(&output_dir, &manifest)?;

    let input = transcription::ProcessInput {
        session_id: request.session_id.clone(),
        capture_dir: capture_dir.clone(),
        output_dir: output_dir.clone(),
        audio_path_from_event: first_existing_audio(&capture_dir),
        settings: settings.clone(),
    };

    match transcription::process_session(input) {
        Ok(artifacts) => {
            let audio_path = artifacts
                .audio_output_path
                .unwrap_or_else(|| output_dir.join("audio.mp3"));
            manifest.set_audio_artifacts(artifacts.audio_artifacts.clone());
            manifest.mark_done(
                audio_path.to_string_lossy().into_owned(),
                artifacts
                    .transcript_markdown_path
                    .to_string_lossy()
                    .into_owned(),
                artifacts
                    .transcript_json_path
                    .to_string_lossy()
                    .into_owned(),
                settings.mlx_model.clone(),
                settings.mlx_fallback_model.clone(),
            );
            manifest::write_manifest(&output_dir, &manifest)?;
            reindex_after_transcription(&settings);
            run_session_storage_maintenance_now(
                &settings.recordings_dir_path(),
                &output_dir,
                "session_recovered",
            );
        }
        Err(error) => {
            manifest.mark_error(error.clone());
            manifest::write_manifest(&output_dir, &manifest)?;
            reindex_after_transcription(&settings);
            return Err(error);
        }
    }

    Ok(RecoverSessionResult {
        id: manifest.id,
        status: manifest.status,
        audio_path: manifest.audio_path,
        transcript_markdown_path: manifest.transcript_path,
        transcript_json_path: manifest
            .transcription
            .map(|transcription| transcription.transcript_json_path),
        capture_dir: capture_dir.to_string_lossy().into_owned(),
        output_dir: output_dir.to_string_lossy().into_owned(),
    })
}

fn first_existing_audio(dir: &std::path::Path) -> Option<PathBuf> {
    ["audio.mp3", "audio.wav", "audio.ogg", "audio.m4a"]
        .iter()
        .map(|file_name| dir.join(file_name))
        .find(|path| path.exists())
}

fn finish_background_transcription(
    controller: &mut AppController,
    session_id: &str,
    completed_phase: RecorderPhase,
    completed_detail: String,
) {
    controller.background_transcription_count =
        controller.background_transcription_count.saturating_sub(1);

    if controller.live_test_session_id.as_deref() == Some(session_id) {
        controller.live_test_session_id = None;
        controller.live_test_mode = None;
    }

    if controller.active_session_id.as_deref() == Some(session_id)
        && controller.phase == RecorderPhase::Finalizing
    {
        controller.clear_active_session();
        controller.phase = RecorderPhase::Idle;
    }

    if controller.has_active_capture() {
        return;
    }

    controller.clear_active_session();
    if controller.background_transcription_count > 0 {
        controller.phase = RecorderPhase::Idle;
        controller.status_detail = controller
            .background_transcription_detail()
            .unwrap_or_else(|| "Transcribing in background".to_string());
    } else {
        controller.phase = completed_phase;
        controller.status_detail = completed_detail;
    }
}

fn complete_background_session(
    app: &tauri::AppHandle,
    session_id: &str,
    completed_phase: RecorderPhase,
    completed_detail: String,
    last_transcript_path: Option<PathBuf>,
    onboarding_completed: bool,
) -> Result<bool, String> {
    let (should_exit, settings_to_save) = update_controller(app, |controller| {
        if let Some(path) = last_transcript_path {
            controller.last_transcript_path = Some(path);
        }
        if onboarding_completed {
            controller.settings.onboarding_completed = true;
        }
        finish_background_transcription(controller, session_id, completed_phase, completed_detail);
        (
            controller.quit_requested && !controller.has_active_work(),
            onboarding_completed.then(|| controller.settings.clone()),
        )
    })?;

    if let Some(settings) = settings_to_save
        && let Err(error) = recorder_settings::save(app, &settings)
    {
        tracing::warn!("failed saving onboarding completion: {error}");
    }
    Ok(should_exit)
}

pub async fn start_recording(app: tauri::AppHandle) -> Result<String, String> {
    start_recording_inner(app, false, "Capture active", false).await
}

async fn start_recording_inner(
    app: tauri::AppHandle,
    onboarding: bool,
    status_detail: &str,
    force_transcription: bool,
) -> Result<String, String> {
    let session_id = uuid::Uuid::new_v4().to_string();
    let vault_base = app
        .settings()
        .vault_base()
        .map_err(|e| format!("settings.vault_base failed: {e}"))?;
    let capture_dir = vault_base
        .join("sessions")
        .join(&session_id)
        .into_std_path_buf();
    let reservation = update_controller(&app, |controller| {
        if !controller.can_start_recording() {
            return Err(format!(
                "cannot start while status is {}",
                controller.phase.status_text()
            ));
        }
        let mut settings = controller.settings.clone();
        if force_transcription {
            settings.recording_mode = RecordingMode::RecordAndTranscribe;
        }
        let output_dir = settings.recordings_dir_path().join(&session_id);
        let mic_device_id = settings.mic_device_id.clone();

        controller.phase = RecorderPhase::Recording;
        controller.status_detail = status_detail.to_string();
        controller.quit_requested = false;
        controller.active_session_id = Some(session_id.clone());
        controller.recording_started_at = Some(std::time::Instant::now());
        controller.active_capture_dir = Some(capture_dir.clone());
        controller.active_output_dir = Some(output_dir.clone());
        controller.active_settings_snapshot = Some(settings.clone());
        controller
            .recording_end_reminder
            .reset_for_recording(std::time::Instant::now());

        Ok((settings, output_dir, mic_device_id))
    })??;
    let (settings, output_dir, mic_device_id) = reservation;
    refresh_tray(&app);

    let mic_device_name = selected_microphone_name(&app, mic_device_id.as_deref());
    let prepare_result = (|| -> Result<(), String> {
        if let Some(mic_device_id) = mic_device_id {
            app.audio_priority()
                .set_default_input_device(&mic_device_id)
                .map_err(|e| format!("failed setting microphone {mic_device_id}: {e}"))?;
        }

        manifest::write_manifest(
            &output_dir,
            &manifest::SessionManifest::recording_with_policy(
                session_id.clone(),
                capture_dir.to_string_lossy().into_owned(),
                settings.recording_mode,
                settings.preserve_stems,
            ),
        )
    })();
    if let Err(error) = prepare_result {
        rollback_recording_start(&app, &session_id, "Failed to prepare recording")?;
        return Err(error);
    }

    let params = CaptureParams {
        session_id: session_id.clone(),
        languages: vec![
            "en-US"
                .parse::<poha_language::Language>()
                .unwrap_or_default(),
        ],
        mic_device: mic_device_name,
        onboarding,
        model: BATCH_CAPTURE_MODEL.to_string(),
        base_url: BATCH_CAPTURE_BASE_URL.to_string(),
        api_key: String::new(),
        keywords: vec![],
        participant_human_ids: vec![],
        self_human_id: None,
    };

    if let Err(error) = app.listener().start_capture(params).await {
        rollback_recording_start(&app, &session_id, "Failed to start capture")?;
        if let Ok(mut manifest) = manifest::read_manifest(&output_dir) {
            manifest.mark_error(error.to_string());
            let _ = manifest::write_manifest(&output_dir, &manifest);
        }
        return Err(error.to_string());
    }

    if settings.recording_mode == RecordingMode::RecordAndTranscribe {
        set_live_transcription_handle(
            &app,
            transcription::spawn_live_transcription(transcription::ProcessInput {
                session_id: session_id.clone(),
                capture_dir,
                output_dir,
                audio_path_from_event: None,
                settings,
            }),
        )?;
    }

    Ok(session_id)
}

fn rollback_recording_start(
    app: &tauri::AppHandle,
    session_id: &str,
    detail: &str,
) -> Result<(), String> {
    let rolled_back = update_controller(app, |controller| {
        if controller.active_session_id.as_deref() != Some(session_id) {
            return false;
        }
        controller.clear_active_session();
        controller.phase = RecorderPhase::Error;
        controller.status_detail = detail.to_string();
        true
    })?;
    if rolled_back {
        refresh_tray(app);
    }
    Ok(())
}

fn selected_microphone_name(app: &tauri::AppHandle, mic_device_id: Option<&str>) -> Option<String> {
    if let Some(mic_device_id) = mic_device_id {
        return match app.audio_priority().list_input_devices() {
            Ok(devices) => devices
                .into_iter()
                .find(|device| device.id.to_string() == mic_device_id)
                .map(|device| device.name),
            Err(error) => {
                tracing::warn!("failed listing microphones for capture device name: {error}");
                None
            }
        };
    }

    match app.audio_priority().get_default_input_device() {
        Ok(Some(device)) => Some(device.name),
        Ok(None) => None,
        Err(error) => {
            tracing::warn!("failed resolving default microphone name: {error}");
            None
        }
    }
}

pub async fn run_live_test(app: tauri::AppHandle) -> Result<String, String> {
    run_live_test_with_mode(app, live_test_diagnostics::LiveTestMode::Automatic).await
}

pub async fn run_guided_audio_test(app: tauri::AppHandle) -> Result<String, String> {
    run_live_test_with_mode(app, live_test_diagnostics::LiveTestMode::Guided).await
}

async fn run_live_test_with_mode(
    app: tauri::AppHandle,
    mode: live_test_diagnostics::LiveTestMode,
) -> Result<String, String> {
    let guided = mode == live_test_diagnostics::LiveTestMode::Guided;
    if guided {
        audio_check_window::show(&app)?;
        audio_check_window::set_state(
            &app,
            audio_check_window::state(
                "starting",
                "Starting audio check",
                "Checking permissions and preparing capture.",
                0.02,
                None,
                None,
            ),
        )?;
    }

    refresh_permission_statuses(app.clone(), true).await;
    let controller = get_controller(&app)?;
    if controller.permission_snapshot.microphone != PermissionState::Authorized {
        let report = write_live_test_permission_report(
            &app,
            &controller,
            false,
            controller.permission_snapshot.system_audio == PermissionState::Authorized,
        )?;
        if guided {
            set_audio_check_report_state(
                &app,
                Some(&report),
                "failed",
                "Microphone permission missing",
                vec!["permissions".to_string()],
            )?;
        }
        open_permission(app.clone(), PermissionTarget::Microphone);
        return Err("microphone permission missing".to_string());
    }
    if controller.permission_snapshot.system_audio != PermissionState::Authorized {
        let report = write_live_test_permission_report(&app, &controller, true, false)?;
        if guided {
            set_audio_check_report_state(
                &app,
                Some(&report),
                "failed",
                "System audio permission missing",
                vec!["permissions".to_string()],
            )?;
        }
        open_permission(app.clone(), PermissionTarget::SystemAudio);
        return Err("system audio permission missing".to_string());
    }

    let session_id = start_recording_inner(app.clone(), false, "Live test: starting", true).await?;
    let (capture_dir, output_dir) = update_controller(&app, |controller| {
        if controller.active_session_id.as_deref() != Some(session_id.as_str()) {
            return Err("live test session is no longer active".to_string());
        }
        let capture_dir = controller
            .active_capture_dir
            .clone()
            .ok_or_else(|| "live test capture dir missing".to_string())?;
        let output_dir = controller
            .active_output_dir
            .clone()
            .ok_or_else(|| "live test output dir missing".to_string())?;
        controller.live_test_session_id = Some(session_id.clone());
        controller.live_test_mode = Some(mode);
        controller.status_detail = match mode {
            live_test_diagnostics::LiveTestMode::Automatic => {
                "Live test: playing diagnostic audio".to_string()
            }
            live_test_diagnostics::LiveTestMode::Guided => {
                format!(
                    "Audio check: say '{}'",
                    live_test_diagnostics::GUIDED_MIC_PHRASE
                )
            }
        };
        Ok((capture_dir, output_dir))
    })??;
    refresh_tray(&app);

    if guided {
        audio_check_window::set_state(
            &app,
            audio_check_window::state(
                "ready",
                "Get ready",
                "The microphone phase starts next.",
                0.08,
                Some(session_id.clone()),
                Some(output_dir.to_string_lossy().into_owned()),
            ),
        )?;
    }

    let session_id_for_task = session_id.clone();
    tauri::async_runtime::spawn(async move {
        drive_live_test(app, session_id_for_task, output_dir, capture_dir, mode).await;
    });

    Ok(session_id)
}

fn write_live_test_permission_report(
    app: &tauri::AppHandle,
    controller: &AppController,
    microphone_permission: bool,
    system_audio_permission: bool,
) -> Result<live_test_diagnostics::LiveTestReport, String> {
    let report = live_test_diagnostics::write_permission_failure_report(
        &controller.settings.recordings_dir_path(),
        microphone_permission,
        system_audio_permission,
    )?;
    update_controller(app, |controller| {
        controller.last_transcript_path = Some(report.report_markdown_path());
        controller.status_detail = format!("Live test failed: {}", report.failure_summary());
    })?;
    refresh_tray(app);
    Ok(report)
}

async fn drive_live_test(
    app: tauri::AppHandle,
    session_id: String,
    output_dir: PathBuf,
    capture_dir: PathBuf,
    mode: live_test_diagnostics::LiveTestMode,
) {
    match mode {
        live_test_diagnostics::LiveTestMode::Automatic => {
            drive_automatic_live_test(app, session_id, output_dir, capture_dir).await
        }
        live_test_diagnostics::LiveTestMode::Guided => {
            drive_guided_audio_test(app, session_id, output_dir, capture_dir).await
        }
    }
}

async fn drive_automatic_live_test(
    app: tauri::AppHandle,
    session_id: String,
    output_dir: PathBuf,
    capture_dir: PathBuf,
) {
    tokio::time::sleep(std::time::Duration::from_secs(1)).await;
    let playback_result = live_test_diagnostics::play_diagnostic_audio();
    if let Err(error) = set_status_detail(&app, "Live test: waiting for audio frames") {
        tracing::warn!("live_test_status_failed: {error}");
    }

    let warmup = live_test_diagnostics::wait_for_capture_audio(
        capture_dir.clone(),
        live_test_diagnostics::WARMUP_TIMEOUT,
    )
    .await;
    if warmup.ready {
        let _ = set_status_detail(&app, "Live test: recording diagnostics");
        tokio::time::sleep(live_test_diagnostics::RECORD_AFTER_READY).await;
    } else {
        let _ = set_status_detail(&app, "Live test: no audio frames");
    }

    let note = live_test_diagnostics::LiveTestRuntimeNote {
        mode: live_test_diagnostics::LiveTestMode::Automatic,
        generated_at: chrono::Utc::now().to_rfc3339(),
        warmup,
        diagnostic_phrase: live_test_diagnostics::DIAGNOSTIC_PHRASE.to_string(),
        playback_started: playback_result.is_ok(),
        playback_error: playback_result.err(),
        user_mic_phrase: None,
        mic_prompt_started_at: None,
        mic_prompt_ended_at: None,
        mic_prompt_shown: None,
        mic_prompt_error: None,
        system_playback_started_at: None,
        system_playback_ended_at: None,
    };
    if let Err(error) = live_test_diagnostics::write_runtime_note(&output_dir, &note) {
        tracing::warn!("live_test_runtime_note_failed: {error}");
    }

    let should_stop = get_controller(&app)
        .map(|controller| {
            controller.can_stop_recording()
                && controller.active_session_id.as_deref() == Some(session_id.as_str())
        })
        .unwrap_or(false);
    if should_stop && let Err(error) = stop_recording(app).await {
        tracing::error!("live_test_stop_failed: {error}");
    }
}

async fn drive_guided_audio_test(
    app: tauri::AppHandle,
    session_id: String,
    output_dir: PathBuf,
    capture_dir: PathBuf,
) {
    audio_check_countdown(
        &app,
        &session_id,
        &output_dir,
        "countdown",
        "Get ready",
        "The microphone check starts next.",
        None,
        3,
        0.08,
        0.18,
    )
    .await;

    let mic_prompt_started_at = chrono::Utc::now();
    let prompt_result =
        live_test_diagnostics::show_guided_mic_prompt(live_test_diagnostics::GUIDED_MIC_PHRASE);
    if let Err(error) = set_status_detail(
        &app,
        &format!(
            "Audio check: say '{}'",
            live_test_diagnostics::GUIDED_MIC_PHRASE
        ),
    ) {
        tracing::warn!("guided_audio_test_status_failed: {error}");
    }
    audio_check_countdown(
        &app,
        &session_id,
        &output_dir,
        "mic",
        "Speak now",
        "Say the phrase clearly toward the selected microphone.",
        Some(live_test_diagnostics::GUIDED_MIC_PHRASE),
        live_test_diagnostics::GUIDED_MIC_RECORD_SECONDS.as_secs(),
        0.18,
        0.62,
    )
    .await;
    let mic_prompt_ended_at = chrono::Utc::now();

    if let Err(error) = set_status_detail(&app, "Audio check: playing system phrase") {
        tracing::warn!("guided_audio_test_status_failed: {error}");
    }
    emit_audio_check_state(
        &app,
        audio_check_window::state(
            "systemReady",
            "System audio next",
            "Poha will play the system phrase now.",
            0.64,
            Some(session_id.clone()),
            Some(output_dir.to_string_lossy().into_owned()),
        ),
    );
    tokio::time::sleep(std::time::Duration::from_millis(500)).await;
    let system_playback_started_at = chrono::Utc::now();
    let playback_result = live_test_diagnostics::play_diagnostic_audio_phrase(
        live_test_diagnostics::GUIDED_SYSTEM_PHRASE,
    );
    audio_check_countdown(
        &app,
        &session_id,
        &output_dir,
        "system",
        "Playing system audio",
        "Keep the output route the same while Poha captures system audio.",
        Some(live_test_diagnostics::GUIDED_SYSTEM_PHRASE),
        live_test_diagnostics::GUIDED_SYSTEM_RECORD_SECONDS.as_secs(),
        0.66,
        0.84,
    )
    .await;
    let system_playback_ended_at = chrono::Utc::now();

    if let Err(error) = set_status_detail(&app, "Audio check: finalizing") {
        tracing::warn!("guided_audio_test_status_failed: {error}");
    }
    emit_audio_check_state(
        &app,
        audio_check_window::state(
            "finalizing",
            "Finalizing",
            "Stopping capture and writing the diagnostic report.",
            0.9,
            Some(session_id.clone()),
            Some(output_dir.to_string_lossy().into_owned()),
        ),
    );
    let warmup = live_test_diagnostics::wait_for_capture_audio(
        capture_dir.clone(),
        std::time::Duration::from_millis(250),
    )
    .await;

    let note = live_test_diagnostics::LiveTestRuntimeNote {
        mode: live_test_diagnostics::LiveTestMode::Guided,
        generated_at: chrono::Utc::now().to_rfc3339(),
        warmup,
        diagnostic_phrase: live_test_diagnostics::GUIDED_SYSTEM_PHRASE.to_string(),
        playback_started: playback_result.is_ok(),
        playback_error: playback_result.err(),
        user_mic_phrase: Some(live_test_diagnostics::GUIDED_MIC_PHRASE.to_string()),
        mic_prompt_started_at: Some(mic_prompt_started_at.to_rfc3339()),
        mic_prompt_ended_at: Some(mic_prompt_ended_at.to_rfc3339()),
        mic_prompt_shown: Some(prompt_result.is_ok()),
        mic_prompt_error: prompt_result.err(),
        system_playback_started_at: Some(system_playback_started_at.to_rfc3339()),
        system_playback_ended_at: Some(system_playback_ended_at.to_rfc3339()),
    };
    if let Err(error) = live_test_diagnostics::write_runtime_note(&output_dir, &note) {
        tracing::warn!("guided_audio_test_runtime_note_failed: {error}");
    }

    let should_stop = get_controller(&app)
        .map(|controller| {
            controller.can_stop_recording()
                && controller.active_session_id.as_deref() == Some(session_id.as_str())
        })
        .unwrap_or(false);
    if should_stop && let Err(error) = stop_recording(app).await {
        tracing::error!("guided_audio_test_stop_failed: {error}");
    }
}

async fn audio_check_countdown(
    app: &tauri::AppHandle,
    session_id: &str,
    output_dir: &Path,
    phase: &str,
    title: &str,
    detail: &str,
    phrase: Option<&str>,
    seconds: u64,
    progress_start: f32,
    progress_end: f32,
) {
    let seconds = seconds.max(1);
    for index in 0..seconds {
        let remaining = seconds - index;
        let progress =
            progress_start + (progress_end - progress_start) * (index as f32 / seconds as f32);
        let mut state = audio_check_window::state(
            phase,
            title,
            detail,
            progress,
            Some(session_id.to_string()),
            Some(output_dir.to_string_lossy().into_owned()),
        );
        state.phrase = phrase.map(ToString::to_string);
        state.seconds_remaining = Some(remaining);
        emit_audio_check_state(app, state);
        tokio::time::sleep(std::time::Duration::from_secs(1)).await;
    }
}

pub async fn stop_recording(app: tauri::AppHandle) -> Result<(), String> {
    stop_recording_inner(app, StopRecordingReason::Manual).await
}

pub(crate) async fn stop_recording_inner(
    app: tauri::AppHandle,
    reason: StopRecordingReason,
) -> Result<(), String> {
    update_controller(&app, |controller| {
        if !controller.can_stop_recording() {
            return Err(format!(
                "cannot stop while status is {}",
                controller.phase.status_text()
            ));
        }

        controller.phase = RecorderPhase::Finalizing;
        controller.status_detail = if controller.live_test_session_id.as_deref()
            == controller.active_session_id.as_deref()
        {
            "Live test: stopping".to_string()
        } else if reason == StopRecordingReason::AutoQuiet {
            "Auto-stopping after quiet period".to_string()
        } else {
            "Stopping capture".to_string()
        };
        Ok(())
    })??;
    refresh_tray(&app);

    app.listener().stop_capture().await;
    Ok(())
}

pub fn open_recordings_folder(app: tauri::AppHandle) -> Result<(), String> {
    let controller = get_controller(&app)?;
    let recordings_dir = controller.settings.recordings_dir_path();
    std::fs::create_dir_all(&recordings_dir).map_err(|e| {
        format!(
            "failed creating recordings folder {}: {e}",
            recordings_dir.display()
        )
    })?;
    open::that(&recordings_dir).map_err(|e| {
        format!(
            "failed opening recordings folder {}: {e}",
            recordings_dir.display()
        )
    })
}

pub fn open_last_transcript(app: tauri::AppHandle) -> Result<(), String> {
    let controller = get_controller(&app)?;
    let Some(path) = controller.last_transcript_path else {
        return Err("no transcript available yet".to_string());
    };
    open::that(&path).map_err(|e| format!("failed opening transcript {}: {e}", path.display()))
}

pub fn open_path(app: tauri::AppHandle, path: String) -> Result<(), String> {
    let controller = get_controller(&app)?;
    let recordings_dir = canonical_existing_path(&controller.settings.recordings_dir_path())?;
    let target = canonical_existing_path(&PathBuf::from(path))?;
    if !target.starts_with(&recordings_dir) {
        return Err(format!(
            "refusing to open path outside recordings folder: {}",
            target.display()
        ));
    }
    open::that(&target).map_err(|e| format!("failed opening path {}: {e}", target.display()))
}

pub fn open_meeting_browser(app: tauri::AppHandle) -> Result<(), String> {
    crate::meeting_store::rebuild_meeting_index_for_app(&app)?;
    let window = app
        .get_webview_window("main")
        .ok_or_else(|| "recording browser window not found".to_string())?;
    window
        .show()
        .map_err(|e| format!("failed showing recording browser: {e}"))?;
    window
        .set_focus()
        .map_err(|e| format!("failed focusing recording browser: {e}"))?;
    Ok(())
}

pub fn delete_meeting(
    app: tauri::AppHandle,
    id: String,
) -> Result<crate::meeting_store::DeleteMeetingSummary, String> {
    let controller = get_controller(&app)?;
    if controller.has_active_work() {
        return Err("cannot delete recordings while recording or transcribing".to_string());
    }
    crate::meeting_store::delete_meeting_for_app(&app, &id)
}

pub fn delete_meetings(
    app: tauri::AppHandle,
    ids: Vec<String>,
) -> Result<crate::meeting_store::DeleteMeetingsSummary, String> {
    let controller = get_controller(&app)?;
    if controller.has_active_work() {
        return Err("cannot delete recordings while recording or transcribing".to_string());
    }
    crate::meeting_store::delete_meetings_for_app(&app, &ids)
}

pub fn set_recordings_folder(app: tauri::AppHandle, path: String) -> Result<(), String> {
    let target = PathBuf::from(path);
    std::fs::create_dir_all(&target)
        .map_err(|e| format!("failed creating recordings dir {}: {e}", target.display()))?;

    let settings = update_controller(&app, |controller| {
        if controller.has_active_capture() {
            return Err("cannot change recordings folder during an active recording".to_string());
        }
        controller.settings.recordings_dir = target.to_string_lossy().into_owned();
        controller.last_transcript_path =
            recorder_settings::latest_transcript_path(&controller.settings.recordings_dir_path());
        Ok(controller.settings.clone())
    })??;
    recorder_settings::save(&app, &settings)?;
    refresh_tray(&app);
    Ok(())
}

pub fn set_microphone_device(app: tauri::AppHandle, device_id: String) -> Result<(), String> {
    let previous_device = update_controller(&app, |controller| {
        if controller.has_active_capture() {
            return Err("cannot change microphone during an active recording".to_string());
        }
        let previous = controller.settings.mic_device_id.replace(device_id.clone());
        Ok(previous)
    })??;
    if let Err(error) = app.audio_priority().set_default_input_device(&device_id) {
        let _ = update_controller(&app, |controller| {
            if controller.settings.mic_device_id.as_deref() == Some(device_id.as_str()) {
                controller.settings.mic_device_id = previous_device;
            }
        });
        return Err(format!(
            "failed setting microphone device {device_id}: {error}"
        ));
    }
    let settings = update_controller(&app, |controller| controller.settings.clone())?;
    recorder_settings::save(&app, &settings)?;
    refresh_tray(&app);
    Ok(())
}

pub fn set_speaker_label_mode(app: tauri::AppHandle, mode: SpeakerLabelMode) -> Result<(), String> {
    let settings = update_controller(&app, |controller| {
        if controller.has_active_capture() {
            return Err(format!(
                "cannot change speaker labels while status is {}",
                controller.phase.status_text()
            ));
        }

        controller.settings.speaker_label_mode = mode;
        controller.status_detail = format!("Speaker labels: {}", mode.menu_label());
        Ok(controller.settings.clone())
    })??;
    recorder_settings::save(&app, &settings)?;
    refresh_tray(&app);
    Ok(())
}

pub fn set_recording_mode(app: tauri::AppHandle, mode: RecordingMode) -> Result<(), String> {
    let settings = update_controller(&app, |controller| {
        if !controller.can_start_recording() {
            return Err("recording mode cannot change during an active capture".to_string());
        }
        controller.settings.recording_mode = mode;
        controller.status_detail = match mode {
            RecordingMode::RecordOnly => "Future recordings will keep audio only",
            RecordingMode::RecordAndTranscribe => "Future recordings will transcribe locally",
        }
        .to_string();
        Ok(controller.settings.clone())
    })??;
    recorder_settings::save(&app, &settings)?;
    refresh_tray(&app);
    Ok(())
}

pub fn keep_recording(app: tauri::AppHandle) -> Result<(), String> {
    recording_end_reminder_runtime::keep_recording(app)
}

pub fn set_meeting_end_reminders_enabled(
    app: tauri::AppHandle,
    enabled: bool,
) -> Result<(), String> {
    recording_end_reminder_runtime::set_enabled(app, enabled)
}

pub(crate) fn get_controller(app: &tauri::AppHandle) -> Result<AppController, String> {
    let state = app.state::<AppState>();
    let guard = state
        .controller
        .lock()
        .map_err(|_| "controller lock poisoned".to_string())?;
    guard
        .clone()
        .ok_or_else(|| "controller not initialized".to_string())
}

pub(crate) fn set_controller(
    app: &tauri::AppHandle,
    controller: AppController,
) -> Result<(), String> {
    let state = app.state::<AppState>();
    let mut guard = state
        .controller
        .lock()
        .map_err(|_| "controller lock poisoned".to_string())?;
    *guard = Some(controller);
    Ok(())
}

pub(crate) fn update_controller<T>(
    app: &tauri::AppHandle,
    update: impl FnOnce(&mut AppController) -> T,
) -> Result<T, String> {
    let state = app.state::<AppState>();
    let mut guard = state
        .controller
        .lock()
        .map_err(|_| "controller lock poisoned".to_string())?;
    let controller = guard
        .as_mut()
        .ok_or_else(|| "controller not initialized".to_string())?;
    Ok(update(controller))
}

fn set_live_transcription_handle(
    app: &tauri::AppHandle,
    handle: transcription::LiveTranscriptionHandle,
) -> Result<(), String> {
    let state = app.state::<AppState>();
    let mut guard = state
        .live_transcription
        .lock()
        .map_err(|_| "live transcription lock poisoned".to_string())?;
    *guard = Some(handle);
    Ok(())
}

fn take_live_transcription_handle(
    app: &tauri::AppHandle,
) -> Result<Option<transcription::LiveTranscriptionHandle>, String> {
    let state = app.state::<AppState>();
    let mut guard = state
        .live_transcription
        .lock()
        .map_err(|_| "live transcription lock poisoned".to_string())?;
    Ok(guard.take())
}

fn set_status_detail(app: &tauri::AppHandle, detail: &str) -> Result<(), String> {
    update_controller(app, |controller| {
        controller.status_detail = detail.to_string();
    })?;
    refresh_tray(app);
    Ok(())
}

pub(crate) fn refresh_tray(app: &tauri::AppHandle) {
    let controller = match get_controller(app) {
        Ok(controller) => controller,
        Err(error) => {
            tracing::error!("refresh_tray: failed to load controller: {error}");
            return;
        }
    };
    if app.tray_by_id(controller::TRAY_ID).is_none() {
        if let Err(error) = controller::create_tray(app, &controller) {
            tracing::error!("failed creating tray: {error}");
        }
    } else if let Err(error) = controller::update_tray(app, &controller) {
        tracing::error!("failed updating tray: {error}");
    }
}

async fn refresh_permission_statuses(app: tauri::AppHandle, probe_system_audio: bool) {
    let initial_controller = match get_controller(&app) {
        Ok(controller) => controller,
        Err(error) => {
            tracing::error!("permissions refresh: controller unavailable: {error}");
            return;
        }
    };

    let microphone = check_microphone_permission_state(&app).await;
    let (system_audio, system_audio_hint) = check_system_audio_permission_state(
        &app,
        probe_system_audio,
        initial_controller.settings.system_audio_authorized_hint,
    )
    .await;

    if probe_system_audio {
        tracing::info!(
            microphone = ?microphone,
            system_audio = ?system_audio,
            system_audio_hint = initial_controller.settings.system_audio_authorized_hint,
            "permissions_probe_snapshot"
        );
    }

    let update = update_controller(&app, |controller| {
        let mut changed = false;
        let mut settings_to_save = None;

        if let Some(next_hint) = system_audio_hint
            && next_hint != controller.settings.system_audio_authorized_hint
        {
            controller.settings.system_audio_authorized_hint = next_hint;
            changed = true;
            settings_to_save = Some(controller.settings.clone());
        }

        let permission_changed = controller.permission_snapshot.microphone != microphone
            || controller.permission_snapshot.system_audio != system_audio;
        if permission_changed {
            controller.permission_snapshot.microphone = microphone;
            controller.permission_snapshot.system_audio = system_audio;
            changed = true;
        }
        (changed, permission_changed, settings_to_save)
    });
    let (changed, permission_changed, settings_to_save) = match update {
        Ok(update) => update,
        Err(error) => {
            tracing::error!("permissions refresh: latest controller unavailable: {error}");
            return;
        }
    };

    if let Some(settings) = settings_to_save
        && let Err(error) = recorder_settings::save(&app, &settings)
    {
        tracing::warn!("permissions refresh: failed persisting system audio hint: {error}");
    }
    if permission_changed {
        tracing::info!(
            microphone = ?microphone,
            system_audio = ?system_audio,
            "permissions_snapshot_updated"
        );
    }
    if !changed {
        return;
    }
    refresh_tray(&app);
}

async fn check_microphone_permission_state(app: &tauri::AppHandle) -> PermissionState {
    #[cfg(target_os = "macos")]
    {
        let status = unsafe {
            let media_type = AVMediaTypeAudio.unwrap();
            AVCaptureDevice::authorizationStatusForMediaType(media_type)
        };
        let av_state = match status {
            AVAuthorizationStatus::Authorized => PermissionState::Authorized,
            AVAuthorizationStatus::NotDetermined | AVAuthorizationStatus::Denied => {
                PermissionState::Missing
            }
            _ => PermissionState::Missing,
        };
        let probe_state = if av_state == PermissionState::Authorized {
            PermissionState::Authorized
        } else {
            probe_microphone_state(app)
        };
        let resolved = resolve_microphone_permission_state(av_state, probe_state);
        if av_state == PermissionState::Missing && resolved == PermissionState::Authorized {
            tracing::warn!(
                "microphone permission probe succeeded after AVFoundation reported missing"
            );
        }
        return resolved;
    }

    #[cfg(not(target_os = "macos"))]
    {
        probe_microphone_state(app)
    }
}

fn resolve_microphone_permission_state(
    av_state: PermissionState,
    probe_state: PermissionState,
) -> PermissionState {
    if av_state == PermissionState::Authorized || probe_state == PermissionState::Authorized {
        PermissionState::Authorized
    } else {
        PermissionState::Missing
    }
}

fn probe_microphone_state(app: &tauri::AppHandle) -> PermissionState {
    match app.try_state::<std::sync::Arc<dyn poha_audio_actual::AudioProvider>>() {
        Some(audio) => match audio.probe_mic(None) {
            Ok(()) => PermissionState::Authorized,
            Err(error) => {
                tracing::warn!(error = %error, "microphone probe failed");
                PermissionState::Missing
            }
        },
        None => {
            tracing::warn!("microphone live probe skipped: audio provider unavailable");
            PermissionState::Missing
        }
    }
}

async fn check_system_audio_permission_state(
    app: &tauri::AppHandle,
    probe_system_audio: bool,
    authorized_hint: bool,
) -> (PermissionState, Option<bool>) {
    let checked_state = check_system_audio_permission_direct();

    if checked_state == PermissionState::Authorized {
        return (PermissionState::Authorized, Some(true));
    }

    if authorized_hint {
        tracing::debug!("system audio using authorized hint");
        return (PermissionState::Authorized, None);
    }

    if probe_system_audio {
        let probed = probe_system_audio_state(app);
        return if probed == PermissionState::Authorized {
            (PermissionState::Authorized, Some(true))
        } else {
            (PermissionState::Missing, Some(false))
        };
    }

    (PermissionState::Missing, None)
}

fn check_system_audio_permission_direct() -> PermissionState {
    #[cfg(target_os = "macos")]
    {
        let status = poha_tcc::audio_capture_permission_status();
        tracing::debug!(raw_status = status, "system_audio_tcc_status");
        return if status == poha_tcc::GRANTED {
            PermissionState::Authorized
        } else {
            PermissionState::Missing
        };
    }

    #[cfg(not(target_os = "macos"))]
    {
        PermissionState::Missing
    }
}

fn probe_system_audio_state(app: &tauri::AppHandle) -> PermissionState {
    match app.try_state::<std::sync::Arc<dyn poha_audio_actual::AudioProvider>>() {
        Some(audio) => match audio.probe_speaker() {
            Ok(()) => {
                tracing::info!("system audio probe succeeded after missing permission check");
                PermissionState::Authorized
            }
            Err(error) => {
                tracing::warn!(error = %error, "system audio probe failed");
                PermissionState::Missing
            }
        },
        None => {
            tracing::warn!("system audio live probe skipped: audio provider unavailable");
            PermissionState::Missing
        }
    }
}

fn transcription_error_detail(error: &str) -> String {
    let lower = error.to_lowercase();
    if lower.contains("failed to spawn uvx")
        || (lower.contains("no such file or directory") && lower.contains("uvx"))
    {
        "uvx missing (install uv + mlx-whisper)".to_string()
    } else {
        "Transcription failed".to_string()
    }
}

fn canonical_existing_path(path: &PathBuf) -> Result<PathBuf, String> {
    path.canonicalize()
        .map_err(|e| format!("failed resolving path {}: {e}", path.display()))
}

#[cfg(test)]
mod tests {
    use std::path::{Path, PathBuf};
    use std::time::Instant;

    use super::{
        canonical_existing_path, finish_background_transcription,
        resolve_microphone_permission_state, transcription_error_detail,
    };
    use crate::controller::{AppController, PermissionState, RecorderPhase};
    use crate::recorder_settings::RecorderSettings;
    use tempfile::tempdir;

    #[test]
    fn uvx_missing_error_is_actionable() {
        let detail = transcription_error_detail("failed to spawn uvx: No such file or directory");
        assert_eq!(detail, "uvx missing (install uv + mlx-whisper)");
    }

    #[test]
    fn generic_error_stays_generic() {
        let detail = transcription_error_detail("unexpected mlx output");
        assert_eq!(detail, "Transcription failed");
    }

    #[test]
    fn microphone_permission_uses_live_probe_when_av_state_is_missing() {
        assert_eq!(
            resolve_microphone_permission_state(
                PermissionState::Missing,
                PermissionState::Authorized
            ),
            PermissionState::Authorized
        );
    }

    #[test]
    fn microphone_permission_stays_missing_when_av_and_probe_fail() {
        assert_eq!(
            resolve_microphone_permission_state(PermissionState::Missing, PermissionState::Missing),
            PermissionState::Missing
        );
    }

    #[test]
    fn canonical_open_target_allows_recordings_children_only() {
        let recordings = tempdir().expect("recordings");
        let outside = tempdir().expect("outside");
        let export_index = recordings.path().join(".poha/exports/run/index.md");
        std::fs::create_dir_all(export_index.parent().expect("parent")).expect("export dir");
        std::fs::write(&export_index, "# Export\n").expect("export index");
        let outside_file = outside.path().join("index.md");
        std::fs::write(&outside_file, "# Other\n").expect("outside index");

        let recordings = canonical_existing_path(&recordings.path().to_path_buf())
            .expect("canonical recordings");
        let inside = canonical_existing_path(&export_index).expect("canonical inside");
        let outside = canonical_existing_path(&outside_file).expect("canonical outside");

        assert!(inside.starts_with(&recordings));
        assert!(!outside.starts_with(&recordings));
    }

    #[test]
    fn background_completion_does_not_clobber_active_recording() {
        let mut controller = AppController::new(
            RecorderSettings::default_with_recordings_dir(PathBuf::from("/tmp/poha-tests")),
            None,
        );
        controller.phase = RecorderPhase::Recording;
        controller.active_session_id = Some("new-recording".to_string());
        controller.recording_started_at = Some(Instant::now());
        controller.active_capture_dir = Some(PathBuf::from("/tmp/new-recording"));
        controller.active_output_dir = Some(PathBuf::from("/tmp/output/new-recording"));
        controller.active_settings_snapshot = Some(controller.settings.clone());
        controller.background_transcription_count = 1;
        controller.status_detail = "Capture active".to_string();

        finish_background_transcription(
            &mut controller,
            "old-recording",
            RecorderPhase::Done,
            "Transcript ready".to_string(),
        );

        assert_eq!(controller.phase, RecorderPhase::Recording);
        assert_eq!(
            controller.active_session_id.as_deref(),
            Some("new-recording")
        );
        assert_eq!(controller.status_detail, "Capture active");
        assert_eq!(controller.background_transcription_count, 0);
        assert_eq!(
            controller.active_output_dir.as_deref(),
            Some(Path::new("/tmp/output/new-recording"))
        );
        assert!(controller.active_settings_snapshot.is_some());
    }

    #[test]
    fn record_only_completion_clears_its_finalizing_context() {
        let mut controller = AppController::new(
            RecorderSettings::default_with_recordings_dir(PathBuf::from("/tmp/poha-tests")),
            None,
        );
        controller.phase = RecorderPhase::Finalizing;
        controller.active_session_id = Some("saved-recording".to_string());
        controller.recording_started_at = Some(Instant::now());
        controller.active_capture_dir = Some(PathBuf::from("/tmp/capture/saved-recording"));
        controller.active_output_dir = Some(PathBuf::from("/tmp/output/saved-recording"));
        controller.active_settings_snapshot = Some(controller.settings.clone());
        controller.background_transcription_count = 1;

        finish_background_transcription(
            &mut controller,
            "saved-recording",
            RecorderPhase::Done,
            "Recording saved".to_string(),
        );

        assert_eq!(controller.phase, RecorderPhase::Done);
        assert_eq!(controller.status_detail, "Recording saved");
        assert_eq!(controller.background_transcription_count, 0);
        assert!(controller.active_session_id.is_none());
        assert!(controller.active_capture_dir.is_none());
        assert!(controller.active_output_dir.is_none());
        assert!(controller.active_settings_snapshot.is_none());
    }

    #[test]
    fn final_background_completion_surfaces_result_when_idle() {
        let mut controller = AppController::new(
            RecorderSettings::default_with_recordings_dir(PathBuf::from("/tmp/poha-tests")),
            None,
        );
        controller.background_transcription_count = 1;

        finish_background_transcription(
            &mut controller,
            "old-recording",
            RecorderPhase::Done,
            "Transcript ready".to_string(),
        );

        assert_eq!(controller.phase, RecorderPhase::Done);
        assert_eq!(controller.status_detail, "Transcript ready");
        assert_eq!(controller.background_transcription_count, 0);
        assert!(controller.can_start_recording());
    }
}
