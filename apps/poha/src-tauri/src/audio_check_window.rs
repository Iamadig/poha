use serde::{Deserialize, Serialize};
use tauri::{Emitter, Manager, WebviewUrl, WebviewWindowBuilder};

use crate::AppState;

pub const WINDOW_LABEL: &str = "audio-check";
pub const STATE_EVENT: &str = "audio-check-state";

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AudioCheckState {
    pub phase: String,
    pub title: String,
    pub detail: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub phrase: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub seconds_remaining: Option<u64>,
    pub progress: f32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub output_dir: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub report_json_path: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub report_markdown_path: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub status: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub failures: Vec<String>,
}

pub fn show(app: &tauri::AppHandle) -> Result<(), String> {
    if let Some(window) = app.get_webview_window(WINDOW_LABEL) {
        window
            .show()
            .map_err(|error| format!("failed showing audio check window: {error}"))?;
        window
            .set_focus()
            .map_err(|error| format!("failed focusing audio check window: {error}"))?;
        return Ok(());
    }

    let window = WebviewWindowBuilder::new(
        app,
        WINDOW_LABEL,
        WebviewUrl::App("audio-check.html".into()),
    )
    .title("Poha Audio Check")
    .inner_size(460.0, 420.0)
    .min_inner_size(420.0, 360.0)
    .resizable(false)
    .always_on_top(true)
    .skip_taskbar(true)
    .build()
    .map_err(|error| format!("failed creating audio check window: {error}"))?;
    window
        .set_focus()
        .map_err(|error| format!("failed focusing audio check window: {error}"))?;
    Ok(())
}

pub fn current_state(app: &tauri::AppHandle) -> Result<Option<AudioCheckState>, String> {
    let state = app.state::<AppState>();
    state
        .audio_check_state
        .lock()
        .map(|guard| guard.clone())
        .map_err(|_| "audio check state lock poisoned".to_string())
}

pub fn set_state(app: &tauri::AppHandle, next: AudioCheckState) -> Result<(), String> {
    {
        let state = app.state::<AppState>();
        let mut guard = state
            .audio_check_state
            .lock()
            .map_err(|_| "audio check state lock poisoned".to_string())?;
        *guard = Some(next.clone());
    }
    let _ = app.emit_to(WINDOW_LABEL, STATE_EVENT, next);
    Ok(())
}

pub fn state(
    phase: &str,
    title: &str,
    detail: &str,
    progress: f32,
    session_id: Option<String>,
    output_dir: Option<String>,
) -> AudioCheckState {
    AudioCheckState {
        phase: phase.to_string(),
        title: title.to_string(),
        detail: detail.to_string(),
        phrase: None,
        seconds_remaining: None,
        progress,
        session_id,
        output_dir,
        report_json_path: None,
        report_markdown_path: None,
        status: None,
        failures: Vec::new(),
    }
}
