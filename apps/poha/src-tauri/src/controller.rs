use std::path::PathBuf;
use std::time::{Duration, Instant};

use tauri::image::Image;
use tauri::menu::{Menu, MenuItem, MenuItemKind, PredefinedMenuItem, Submenu};
use tauri::tray::TrayIconBuilder;
use tauri::{AppHandle, Result};
use tauri_plugin_audio_priority::AudioPriorityPluginExt;

use crate::live_test_diagnostics::LiveTestMode;
use crate::meeting_detection::AutomationMode;
use crate::recorder_settings::{RecorderSettings, SpeakerLabelMode};
use crate::recording_end_reminder::RecordingEndReminderState;

pub const TRAY_ID: &str = "poha-tray";

pub const MENU_ID_STATUS: &str = "poha_status";
pub const MENU_ID_START: &str = "poha_start";
pub const MENU_ID_STOP: &str = "poha_stop";
pub const MENU_ID_KEEP_RECORDING: &str = "poha_keep_recording";
pub const MENU_ID_OPEN_RECORDINGS: &str = "poha_open_recordings";
pub const MENU_ID_OPEN_LAST_TRANSCRIPT: &str = "poha_open_last_transcript";
pub const MENU_ID_OPEN_MEETING_BROWSER: &str = "poha_open_meeting_browser";
pub const MENU_ID_ONBOARDING: &str = "poha_onboarding";
pub const MENU_ID_GUIDED_AUDIO_TEST: &str = "poha_guided_audio_test";
pub const MENU_ID_PERMISSION_MIC: &str = "poha_permission_mic";
pub const MENU_ID_PERMISSION_SYSTEM: &str = "poha_permission_system";
pub const MENU_ID_SPEAKER_FAST: &str = "poha_speaker_fast";
pub const MENU_ID_SPEAKER_ME_CALL: &str = "poha_speaker_me_call";
pub const MENU_ID_TOGGLE_END_REMINDERS: &str = "poha_toggle_end_reminders";
pub const MENU_ID_AUTOMATION_OFF: &str = "poha_automation_off";
pub const MENU_ID_AUTOMATION_ASK: &str = "poha_automation_ask";
pub const MENU_ID_AUTOMATION_AUTO_SCHEDULED: &str = "poha_automation_auto_scheduled";
pub const MENU_ID_CALENDAR_ACCESS: &str = "poha_calendar_access";
pub const MENU_ID_TOGGLE_CALENDAR: &str = "poha_toggle_calendar";
pub const MENU_ID_ACCEPT_DETECTED_MEETING: &str = "poha_accept_detected_meeting";
pub const MENU_ID_DISMISS_DETECTED_MEETING: &str = "poha_dismiss_detected_meeting";
pub const MENU_ID_QUIT: &str = "poha_quit";
pub const MENU_ID_REFRESH: &str = "poha_refresh";

pub const MIC_PREFIX: &str = "poha_mic::";

const TRAY_IDLE_ICON: &[u8] = include_bytes!("../icons/tray/poha-tray-idle.png");
const TRAY_EATING_ICONS: [&[u8]; 6] = [
    include_bytes!("../icons/tray/poha-tray-eating-0.png"),
    include_bytes!("../icons/tray/poha-tray-eating-1.png"),
    include_bytes!("../icons/tray/poha-tray-eating-2.png"),
    include_bytes!("../icons/tray/poha-tray-eating-3.png"),
    include_bytes!("../icons/tray/poha-tray-eating-4.png"),
    include_bytes!("../icons/tray/poha-tray-eating-5.png"),
];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RecorderPhase {
    Idle,
    Recording,
    Finalizing,
    Done,
    Error,
}

impl RecorderPhase {
    pub fn status_text(self) -> &'static str {
        match self {
            Self::Idle => "Idle",
            Self::Recording => "Recording",
            Self::Finalizing => "Finalizing",
            Self::Done => "Done",
            Self::Error => "Error",
        }
    }

    pub fn can_start(self) -> bool {
        matches!(self, Self::Idle | Self::Done | Self::Error)
    }

    pub fn can_stop(self) -> bool {
        matches!(self, Self::Recording)
    }

    pub fn has_active_capture(self) -> bool {
        matches!(self, Self::Recording | Self::Finalizing)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PermissionState {
    Authorized,
    Missing,
}

#[derive(Debug, Clone)]
pub struct PermissionSnapshot {
    pub microphone: PermissionState,
    pub system_audio: PermissionState,
    pub calendar: PermissionState,
}

impl Default for PermissionSnapshot {
    fn default() -> Self {
        Self {
            microphone: PermissionState::Missing,
            system_audio: PermissionState::Missing,
            calendar: PermissionState::Missing,
        }
    }
}

#[derive(Debug, Clone)]
pub struct AppController {
    pub phase: RecorderPhase,
    pub status_detail: String,
    pub quit_requested: bool,
    pub active_session_id: Option<String>,
    pub recording_started_at: Option<Instant>,
    pub active_capture_dir: Option<PathBuf>,
    pub background_transcription_count: usize,
    pub live_test_session_id: Option<String>,
    pub live_test_mode: Option<LiveTestMode>,
    pub last_transcript_path: Option<PathBuf>,
    pub permission_snapshot: PermissionSnapshot,
    pub recording_end_reminder: RecordingEndReminderState,
    pub pending_meeting_prompt_title: Option<String>,
    pub settings: RecorderSettings,
}

impl AppController {
    pub fn new(settings: RecorderSettings, last_transcript_path: Option<PathBuf>) -> Self {
        let status_detail = if settings.onboarding_completed {
            "Ready"
        } else {
            "Run Setup Guide"
        };
        Self {
            phase: RecorderPhase::Idle,
            status_detail: status_detail.to_string(),
            quit_requested: false,
            active_session_id: None,
            recording_started_at: None,
            active_capture_dir: None,
            background_transcription_count: 0,
            live_test_session_id: None,
            live_test_mode: None,
            last_transcript_path,
            permission_snapshot: PermissionSnapshot::default(),
            recording_end_reminder: RecordingEndReminderState::default(),
            pending_meeting_prompt_title: None,
            settings,
        }
    }

    pub fn can_start_recording(&self) -> bool {
        self.phase.can_start() && self.active_session_id.is_none()
    }

    pub fn can_stop_recording(&self) -> bool {
        self.phase.can_stop()
    }

    pub fn has_active_capture(&self) -> bool {
        self.phase.has_active_capture() || self.active_session_id.is_some()
    }

    pub fn has_active_work(&self) -> bool {
        self.has_active_capture() || self.background_transcription_count > 0
    }

    pub fn can_keep_recording(&self) -> bool {
        self.phase == RecorderPhase::Recording && self.recording_end_reminder.needs_response()
    }

    pub fn background_transcription_detail(&self) -> Option<String> {
        match self.background_transcription_count {
            0 => None,
            1 => Some("Transcribing 1 recording".to_string()),
            count => Some(format!("Transcribing {count} recordings")),
        }
    }
}

pub fn create_tray(app: &AppHandle, controller: &AppController) -> Result<()> {
    let menu = build_menu(app, controller)?;

    TrayIconBuilder::with_id(TRAY_ID)
        .icon(tray_icon(controller)?)
        .icon_as_template(true)
        .menu(&menu)
        .show_menu_on_left_click(true)
        .build(app)?;

    Ok(())
}

pub fn update_tray(app: &AppHandle, controller: &AppController) -> Result<()> {
    let Some(tray) = app.tray_by_id(TRAY_ID) else {
        return Ok(());
    };
    tray.set_menu(Some(build_menu(app, controller)?))?;
    tray.set_title(tray_title(controller))?;
    tray.set_icon(Some(tray_icon(controller)?))?;
    #[cfg(target_os = "macos")]
    tray.set_icon_as_template(true)?;
    Ok(())
}

pub fn tray_icon(controller: &AppController) -> Result<Image<'static>> {
    Image::from_bytes(tray_icon_bytes(controller))
}

pub fn parse_mic_menu_id(id: &str) -> Option<String> {
    id.strip_prefix(MIC_PREFIX).map(ToString::to_string)
}

fn build_menu(app: &AppHandle, controller: &AppController) -> Result<Menu<tauri::Wry>> {
    let status = status_line(controller);
    let quit_label = quit_label(controller);
    let quit_enabled = quit_enabled(controller);
    Menu::with_items(
        app,
        &[
            &MenuItem::with_id(app, MENU_ID_STATUS, status, false, None::<&str>)?,
            &PredefinedMenuItem::separator(app)?,
            &MenuItem::with_id(
                app,
                MENU_ID_START,
                "Start Recording",
                controller.can_start_recording(),
                None::<&str>,
            )?,
            &MenuItem::with_id(
                app,
                MENU_ID_STOP,
                "Stop Recording",
                controller.can_stop_recording(),
                None::<&str>,
            )?,
            &MenuItem::with_id(
                app,
                MENU_ID_KEEP_RECORDING,
                keep_recording_label(controller),
                controller.can_keep_recording(),
                None::<&str>,
            )?,
            &build_microphone_menu(app, controller)?,
            &build_speaker_labels_menu(app, controller)?,
            &MenuItem::with_id(
                app,
                MENU_ID_TOGGLE_END_REMINDERS,
                meeting_end_reminders_label(controller),
                true,
                None::<&str>,
            )?,
            &build_meeting_automation_menu(app, controller)?,
            &build_permissions_menu(app, controller)?,
            &build_troubleshooting_menu(app, controller)?,
            &PredefinedMenuItem::separator(app)?,
            &MenuItem::with_id(
                app,
                MENU_ID_OPEN_MEETING_BROWSER,
                "Open Recording Browser",
                true,
                None::<&str>,
            )?,
            &MenuItem::with_id(
                app,
                MENU_ID_OPEN_RECORDINGS,
                "Open Recordings Folder",
                true,
                None::<&str>,
            )?,
            &MenuItem::with_id(
                app,
                MENU_ID_OPEN_LAST_TRANSCRIPT,
                "Open Last Transcript",
                controller.last_transcript_path.is_some(),
                None::<&str>,
            )?,
            &MenuItem::with_id(app, MENU_ID_REFRESH, "Refresh", true, None::<&str>)?,
            &PredefinedMenuItem::separator(app)?,
            &MenuItem::with_id(app, MENU_ID_QUIT, quit_label, quit_enabled, None::<&str>)?,
        ],
    )
}

fn build_permissions_menu(
    app: &AppHandle,
    controller: &AppController,
) -> Result<MenuItemKind<tauri::Wry>> {
    let menu = Submenu::with_items(
        app,
        "Permissions",
        true,
        &[
            &MenuItem::with_id(
                app,
                MENU_ID_PERMISSION_MIC,
                permission_label("Microphone", controller.permission_snapshot.microphone),
                true,
                None::<&str>,
            )?,
            &MenuItem::with_id(
                app,
                MENU_ID_PERMISSION_SYSTEM,
                permission_label("System Audio", controller.permission_snapshot.system_audio),
                true,
                None::<&str>,
            )?,
            &MenuItem::with_id(
                app,
                MENU_ID_CALENDAR_ACCESS,
                permission_label("Calendar", controller.permission_snapshot.calendar),
                true,
                None::<&str>,
            )?,
        ],
    )?;
    Ok(MenuItemKind::Submenu(menu))
}

fn build_meeting_automation_menu(
    app: &AppHandle,
    controller: &AppController,
) -> Result<MenuItemKind<tauri::Wry>> {
    let current = controller.settings.meeting_automation_mode;
    let menu = Submenu::new(app, "Meeting Detection", true)?;
    if let Some(title) = controller.pending_meeting_prompt_title.as_deref() {
        menu.append(&MenuItem::with_id(
            app,
            MENU_ID_ACCEPT_DETECTED_MEETING,
            format!("Start: {title}"),
            controller.can_start_recording(),
            None::<&str>,
        )?)?;
        menu.append(&MenuItem::with_id(
            app,
            MENU_ID_DISMISS_DETECTED_MEETING,
            "Not Now",
            true,
            None::<&str>,
        )?)?;
        menu.append(&PredefinedMenuItem::separator(app)?)?;
    }
    menu.append(&MenuItem::with_id(
        app,
        MENU_ID_AUTOMATION_OFF,
        automation_mode_label("Off", current, AutomationMode::Off),
        true,
        None::<&str>,
    )?)?;
    menu.append(&MenuItem::with_id(
        app,
        MENU_ID_AUTOMATION_ASK,
        automation_mode_label("Ask Before Recording", current, AutomationMode::Ask),
        true,
        None::<&str>,
    )?)?;
    menu.append(&MenuItem::with_id(
        app,
        MENU_ID_AUTOMATION_AUTO_SCHEDULED,
        automation_mode_label(
            "Auto-start Scheduled Native Calls",
            current,
            AutomationMode::AutoScheduled,
        ),
        true,
        None::<&str>,
    )?)?;
    menu.append(&PredefinedMenuItem::separator(app)?)?;
    menu.append(&MenuItem::with_id(
        app,
        MENU_ID_TOGGLE_CALENDAR,
        calendar_matching_label(controller.settings.calendar_integration_enabled),
        true,
        None::<&str>,
    )?)?;
    Ok(MenuItemKind::Submenu(menu))
}

fn automation_mode_label(name: &str, current: AutomationMode, mode: AutomationMode) -> String {
    if current == mode {
        format!("✓ {name}")
    } else {
        name.to_string()
    }
}

fn calendar_matching_label(enabled: bool) -> &'static str {
    if enabled {
        "✓ Use Calendar Matching"
    } else {
        "Use Calendar Matching…"
    }
}

fn permission_label(name: &str, state: PermissionState) -> String {
    match state {
        PermissionState::Authorized => format!("✓ {name}"),
        PermissionState::Missing => format!("✗ {name} (Needed)"),
    }
}

fn onboarding_label(controller: &AppController) -> &'static str {
    if controller.settings.onboarding_completed {
        "Run Quick Live Test"
    } else {
        "Setup Guide / Live Test"
    }
}

fn build_troubleshooting_menu(
    app: &AppHandle,
    controller: &AppController,
) -> Result<MenuItemKind<tauri::Wry>> {
    let enabled = controller.can_start_recording();
    let menu = Submenu::with_items(
        app,
        "Troubleshooting",
        true,
        &[
            &MenuItem::with_id(
                app,
                MENU_ID_ONBOARDING,
                onboarding_label(controller),
                enabled,
                None::<&str>,
            )?,
            &MenuItem::with_id(
                app,
                MENU_ID_GUIDED_AUDIO_TEST,
                "Run Guided Audio Check",
                enabled,
                None::<&str>,
            )?,
        ],
    )?;
    Ok(MenuItemKind::Submenu(menu))
}

fn build_speaker_labels_menu(
    app: &AppHandle,
    controller: &AppController,
) -> Result<MenuItemKind<tauri::Wry>> {
    let enabled = !controller.has_active_capture();
    let menu = Submenu::with_items(
        app,
        "Speaker Labels",
        enabled,
        &[
            &MenuItem::with_id(
                app,
                MENU_ID_SPEAKER_ME_CALL,
                speaker_label(
                    "Me + Call",
                    controller.settings.speaker_label_mode,
                    SpeakerLabelMode::MeAndCall,
                ),
                enabled,
                None::<&str>,
            )?,
            &MenuItem::with_id(
                app,
                MENU_ID_SPEAKER_FAST,
                speaker_label(
                    "Fast",
                    controller.settings.speaker_label_mode,
                    SpeakerLabelMode::FastMixed,
                ),
                enabled,
                None::<&str>,
            )?,
        ],
    )?;
    Ok(MenuItemKind::Submenu(menu))
}

fn speaker_label(name: &str, current: SpeakerLabelMode, mode: SpeakerLabelMode) -> String {
    if current == mode {
        format!("✓ {name}")
    } else {
        name.to_string()
    }
}

fn meeting_end_reminders_label(controller: &AppController) -> &'static str {
    if controller.settings.meeting_end_reminders_enabled {
        "✓ Meeting End Reminders"
    } else {
        "Meeting End Reminders"
    }
}

fn keep_recording_label(controller: &AppController) -> &'static str {
    if controller.can_keep_recording() {
        "Keep Recording (pause reminders)"
    } else {
        "Keep Recording"
    }
}

pub fn tray_title(controller: &AppController) -> Option<String> {
    match controller.phase {
        RecorderPhase::Recording => Some(format_elapsed(recording_elapsed(controller))),
        RecorderPhase::Finalizing => Some("...".to_string()),
        RecorderPhase::Done | RecorderPhase::Error | RecorderPhase::Idle => None,
    }
}

fn recording_elapsed(controller: &AppController) -> Duration {
    controller
        .recording_started_at
        .map(|started| started.elapsed())
        .unwrap_or_default()
}

fn tray_icon_bytes(controller: &AppController) -> &'static [u8] {
    match controller.phase {
        RecorderPhase::Recording => {
            let elapsed = recording_elapsed(controller);
            let idx = ((elapsed.as_millis() / 180) as usize) % TRAY_EATING_ICONS.len();
            TRAY_EATING_ICONS[idx]
        }
        RecorderPhase::Finalizing => TRAY_EATING_ICONS[TRAY_EATING_ICONS.len() - 1],
        RecorderPhase::Done | RecorderPhase::Error | RecorderPhase::Idle => TRAY_IDLE_ICON,
    }
}

fn format_elapsed(elapsed: Duration) -> String {
    let total_seconds = elapsed.as_secs();
    let minutes = total_seconds / 60;
    let seconds = total_seconds % 60;
    format!("{minutes:02}:{seconds:02}")
}

fn status_line(controller: &AppController) -> String {
    let detail = if controller.has_active_capture() {
        controller.status_detail.clone()
    } else {
        controller
            .background_transcription_detail()
            .unwrap_or_else(|| controller.status_detail.clone())
    };
    format!("Status: {} ({})", controller.phase.status_text(), detail,)
}

fn quit_label(controller: &AppController) -> &'static str {
    if controller.quit_requested {
        "Force Quit Now"
    } else if controller.has_active_work() {
        "Quit After Finish"
    } else {
        "Quit"
    }
}

fn quit_enabled(_controller: &AppController) -> bool {
    true
}

fn build_microphone_menu(
    app: &AppHandle,
    controller: &AppController,
) -> Result<MenuItemKind<tauri::Wry>> {
    let menu = Submenu::new(app, "Microphone Source", true)?;

    match app.audio_priority().list_input_devices() {
        Ok(devices) if devices.is_empty() => {
            menu.append(&MenuItem::with_id(
                app,
                "poha_mic_none",
                "No microphones found",
                false,
                None::<&str>,
            )?)?;
        }
        Ok(devices) => {
            for device in devices {
                let selected = controller
                    .settings
                    .mic_device_id
                    .as_deref()
                    .map(|id| id == device.id.to_string())
                    .unwrap_or(device.is_default);
                let text = if selected {
                    format!("✓ {}", device.name)
                } else {
                    device.name
                };
                menu.append(&MenuItem::with_id(
                    app,
                    format!("{MIC_PREFIX}{}", device.id),
                    text,
                    true,
                    None::<&str>,
                )?)?;
            }
        }
        Err(e) => {
            menu.append(&MenuItem::with_id(
                app,
                "poha_mic_error",
                format!("Microphones unavailable: {e}"),
                false,
                None::<&str>,
            )?)?;
        }
    }

    Ok(MenuItemKind::Submenu(menu))
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;
    use crate::recorder_settings::RecorderSettings;

    #[test]
    fn phase_start_stop_guards_match_expected_states() {
        assert!(RecorderPhase::Idle.can_start());
        assert!(!RecorderPhase::Idle.can_stop());

        assert!(!RecorderPhase::Recording.can_start());
        assert!(RecorderPhase::Recording.can_stop());

        assert!(!RecorderPhase::Finalizing.can_start());
        assert!(!RecorderPhase::Finalizing.can_stop());

        assert!(RecorderPhase::Done.can_start());
        assert!(!RecorderPhase::Done.can_stop());

        assert!(RecorderPhase::Error.can_start());
        assert!(!RecorderPhase::Error.can_stop());
    }

    #[test]
    fn parse_mic_menu_id_extracts_device_id() {
        let id = format!("{MIC_PREFIX}builtin-mic");
        assert_eq!(parse_mic_menu_id(&id), Some("builtin-mic".to_string()));
        assert_eq!(parse_mic_menu_id("other-menu-item"), None);
    }

    #[test]
    fn new_controller_defaults_to_idle_ready() {
        let settings =
            RecorderSettings::default_with_recordings_dir(PathBuf::from("/tmp/poha-tests"));
        let controller = AppController::new(settings, None);
        assert_eq!(controller.phase, RecorderPhase::Idle);
        assert_eq!(controller.status_detail, "Run Setup Guide");
        assert!(!controller.quit_requested);
        assert!(controller.active_session_id.is_none());
        assert!(controller.recording_started_at.is_none());
        assert!(controller.active_capture_dir.is_none());
        assert_eq!(controller.background_transcription_count, 0);
        assert!(controller.live_test_session_id.is_none());
        assert!(controller.live_test_mode.is_none());
        assert!(controller.last_transcript_path.is_none());
        assert!(!controller.can_keep_recording());
        assert!(!controller.recording_end_reminder.needs_response());
    }

    #[test]
    fn recording_title_shows_timer_only() {
        let mut controller = AppController::new(
            RecorderSettings::default_with_recordings_dir(PathBuf::from("/tmp/poha-tests")),
            None,
        );
        controller.phase = RecorderPhase::Recording;
        controller.recording_started_at = Some(Instant::now());
        let rendered = tray_title(&controller).unwrap_or_default();
        assert_eq!(rendered, "00:00");
    }

    #[test]
    fn recording_icon_animates_separately_from_timer() {
        let mut controller = AppController::new(
            RecorderSettings::default_with_recordings_dir(PathBuf::from("/tmp/poha-tests")),
            None,
        );
        controller.phase = RecorderPhase::Recording;
        controller.recording_started_at = Some(Instant::now());
        let first = tray_icon_bytes(&controller);
        controller.recording_started_at = Some(Instant::now() - Duration::from_millis(180));
        let second = tray_icon_bytes(&controller);
        assert_ne!(first, second);
    }

    #[test]
    fn permission_labels_show_needed_and_authorized_states() {
        assert_eq!(
            permission_label("Microphone", PermissionState::Authorized),
            "✓ Microphone"
        );
        assert_eq!(
            permission_label("System Audio", PermissionState::Missing),
            "✗ System Audio (Needed)"
        );
    }

    #[test]
    fn onboarding_label_switches_after_completion() {
        let mut controller = AppController::new(
            RecorderSettings::default_with_recordings_dir(PathBuf::from("/tmp/poha-tests")),
            None,
        );
        assert_eq!(onboarding_label(&controller), "Setup Guide / Live Test");
        controller.settings.onboarding_completed = true;
        assert_eq!(onboarding_label(&controller), "Run Quick Live Test");
    }

    #[test]
    fn speaker_label_marks_selected_mode() {
        assert_eq!(
            speaker_label(
                "Me + Call",
                SpeakerLabelMode::MeAndCall,
                SpeakerLabelMode::MeAndCall
            ),
            "✓ Me + Call"
        );
        assert_eq!(
            speaker_label(
                "Fast",
                SpeakerLabelMode::MeAndCall,
                SpeakerLabelMode::FastMixed
            ),
            "Fast"
        );
    }

    #[test]
    fn meeting_end_reminder_label_marks_enabled_setting() {
        let mut controller = AppController::new(
            RecorderSettings::default_with_recordings_dir(PathBuf::from("/tmp/poha-tests")),
            None,
        );
        assert_eq!(
            meeting_end_reminders_label(&controller),
            "✓ Meeting End Reminders"
        );

        controller.settings.meeting_end_reminders_enabled = false;
        assert_eq!(
            meeting_end_reminders_label(&controller),
            "Meeting End Reminders"
        );
    }

    #[test]
    fn keep_recording_enabled_only_for_active_reminder() {
        let mut controller = AppController::new(
            RecorderSettings::default_with_recordings_dir(PathBuf::from("/tmp/poha-tests")),
            None,
        );
        controller.recording_end_reminder.mark_response_requested();
        assert!(!controller.can_keep_recording());
        assert_eq!(keep_recording_label(&controller), "Keep Recording");

        controller.phase = RecorderPhase::Recording;
        assert!(controller.can_keep_recording());
        assert_eq!(
            keep_recording_label(&controller),
            "Keep Recording (pause reminders)"
        );
    }

    #[test]
    fn quit_labels_follow_state() {
        let settings =
            RecorderSettings::default_with_recordings_dir(PathBuf::from("/tmp/poha-tests"));
        let mut controller = AppController::new(settings, None);
        assert_eq!(quit_label(&controller), "Quit");
        assert!(quit_enabled(&controller));

        controller.phase = RecorderPhase::Recording;
        assert_eq!(quit_label(&controller), "Quit After Finish");
        assert!(quit_enabled(&controller));

        controller.quit_requested = true;
        assert_eq!(quit_label(&controller), "Force Quit Now");
        assert!(quit_enabled(&controller));
    }

    #[test]
    fn background_transcription_keeps_start_enabled_but_quit_waiting() {
        let settings =
            RecorderSettings::default_with_recordings_dir(PathBuf::from("/tmp/poha-tests"));
        let mut controller = AppController::new(settings, None);
        controller.background_transcription_count = 1;
        controller.status_detail = "Transcript ready".to_string();

        assert!(controller.can_start_recording());
        assert!(!controller.can_stop_recording());
        assert!(!controller.has_active_capture());
        assert!(controller.has_active_work());
        assert_eq!(quit_label(&controller), "Quit After Finish");
        assert_eq!(
            status_line(&controller),
            "Status: Idle (Transcribing 1 recording)"
        );
        assert!(tray_title(&controller).is_none());
    }

    #[test]
    fn active_capture_status_wins_over_background_transcription() {
        let settings =
            RecorderSettings::default_with_recordings_dir(PathBuf::from("/tmp/poha-tests"));
        let mut controller = AppController::new(settings, None);
        controller.phase = RecorderPhase::Recording;
        controller.active_session_id = Some("new-session".to_string());
        controller.background_transcription_count = 2;
        controller.status_detail = "Capture active".to_string();

        assert!(!controller.can_start_recording());
        assert!(controller.can_stop_recording());
        assert_eq!(
            status_line(&controller),
            "Status: Recording (Capture active)"
        );
    }
}
