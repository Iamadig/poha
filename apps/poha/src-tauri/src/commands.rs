use tauri_plugin_clipboard_manager::ClipboardExt;

#[tauri::command]
pub async fn start_recording(app: tauri::AppHandle) -> Result<String, String> {
    crate::start_recording(app).await
}

#[tauri::command]
pub async fn stop_recording(app: tauri::AppHandle) -> Result<(), String> {
    crate::stop_recording(app).await
}

#[tauri::command]
pub async fn keep_recording(app: tauri::AppHandle) -> Result<(), String> {
    crate::keep_recording(app)
}

#[tauri::command]
pub async fn open_recordings_folder(app: tauri::AppHandle) -> Result<(), String> {
    crate::open_recordings_folder(app)
}

#[tauri::command]
pub async fn open_last_transcript(app: tauri::AppHandle) -> Result<(), String> {
    crate::open_last_transcript(app)
}

#[tauri::command]
pub async fn open_meeting_browser(app: tauri::AppHandle) -> Result<(), String> {
    crate::open_meeting_browser(app)
}

#[tauri::command]
pub async fn open_path(app: tauri::AppHandle, path: String) -> Result<(), String> {
    crate::open_path(app, path)
}

#[tauri::command]
pub async fn get_audio_check_state(
    app: tauri::AppHandle,
) -> Result<Option<crate::audio_check_window::AudioCheckState>, String> {
    crate::audio_check_window::current_state(&app)
}

#[tauri::command]
pub async fn run_live_test(app: tauri::AppHandle) -> Result<String, String> {
    crate::run_live_test(app).await
}

#[tauri::command]
pub async fn run_guided_audio_test(app: tauri::AppHandle) -> Result<String, String> {
    crate::run_guided_audio_test(app).await
}

#[tauri::command]
pub async fn set_recordings_folder(app: tauri::AppHandle, path: String) -> Result<(), String> {
    crate::set_recordings_folder(app, path)
}

#[tauri::command]
pub async fn set_microphone_device(app: tauri::AppHandle, device_id: String) -> Result<(), String> {
    crate::set_microphone_device(app, device_id)
}

#[tauri::command]
pub async fn set_speaker_label_mode(
    app: tauri::AppHandle,
    mode: crate::recorder_settings::SpeakerLabelMode,
) -> Result<(), String> {
    crate::set_speaker_label_mode(app, mode)
}

#[tauri::command]
pub async fn set_meeting_end_reminders_enabled(
    app: tauri::AppHandle,
    enabled: bool,
) -> Result<(), String> {
    crate::set_meeting_end_reminders_enabled(app, enabled)
}

#[tauri::command]
pub async fn list_meetings(
    app: tauri::AppHandle,
    query: Option<String>,
    company: Option<String>,
    context: Option<String>,
    limit: Option<usize>,
) -> Result<Vec<crate::meeting_store::MeetingListItem>, String> {
    crate::meeting_store::list_meetings_for_app(&app, query, company, context, limit)
}

#[tauri::command]
pub async fn list_meeting_contexts(
    app: tauri::AppHandle,
    query: Option<String>,
) -> Result<Vec<crate::meeting_store::MeetingContextItem>, String> {
    crate::meeting_store::list_meeting_contexts_for_app(&app, query)
}

#[tauri::command]
pub async fn get_meeting(
    app: tauri::AppHandle,
    id: String,
) -> Result<crate::meeting_store::MeetingDetail, String> {
    crate::meeting_store::get_meeting_for_app(&app, &id)
}

#[tauri::command]
pub async fn update_meeting_metadata(
    app: tauri::AppHandle,
    request: crate::meeting_store::UpdateMeetingMetadataRequest,
) -> Result<crate::meeting_store::MeetingDetail, String> {
    crate::meeting_store::update_meeting_metadata_for_app(&app, request)
}

#[tauri::command]
pub async fn apply_speaker_map(
    app: tauri::AppHandle,
    request: crate::meeting_store::ApplySpeakerMapRequest,
) -> Result<crate::meeting_store::MeetingDetail, String> {
    crate::meeting_store::apply_speaker_map_for_app(&app, request)
}

#[tauri::command]
pub async fn delete_meeting(
    app: tauri::AppHandle,
    id: String,
) -> Result<crate::meeting_store::DeleteMeetingSummary, String> {
    crate::delete_meeting(app, id)
}

#[tauri::command]
pub async fn delete_meetings(
    app: tauri::AppHandle,
    ids: Vec<String>,
) -> Result<crate::meeting_store::DeleteMeetingsSummary, String> {
    crate::delete_meetings(app, ids)
}

#[tauri::command]
pub async fn rebuild_meeting_index(
    app: tauri::AppHandle,
) -> Result<crate::meeting_store::IndexSummary, String> {
    crate::meeting_store::rebuild_meeting_index_for_app(&app)
}

#[tauri::command]
pub async fn copy_meeting_for_llm(app: tauri::AppHandle, id: String) -> Result<String, String> {
    crate::meeting_store::copy_meeting_for_llm_for_app(&app, &id)
}

#[tauri::command]
pub async fn copy_company_for_llm(
    app: tauri::AppHandle,
    company: String,
) -> Result<String, String> {
    crate::meeting_store::copy_company_for_llm_for_app(&app, &company)
}

#[tauri::command]
pub async fn copy_context_for_llm(
    app: tauri::AppHandle,
    context: String,
) -> Result<String, String> {
    crate::meeting_store::copy_context_for_llm_for_app(&app, &context)
}

#[tauri::command]
pub async fn copy_text_to_clipboard(app: tauri::AppHandle, text: String) -> Result<(), String> {
    app.clipboard()
        .write_text(text)
        .map_err(|e| format!("failed writing clipboard: {e}"))
}

#[tauri::command]
pub async fn run_codex_bulk_enrichment(
    app: tauri::AppHandle,
    request: crate::codex_enrichment::CodexBulkEnrichmentRequest,
) -> Result<crate::codex_enrichment::CodexBulkEnrichmentSummary, String> {
    crate::codex_enrichment::run_codex_bulk_enrichment_for_app(app, request).await
}

#[tauri::command]
pub async fn get_codex_bulk_enrichment_status(
    app: tauri::AppHandle,
    request: crate::codex_enrichment::CodexBulkEnrichmentRequest,
) -> Result<crate::codex_enrichment::CodexBulkEnrichmentQueueStatus, String> {
    crate::codex_enrichment::get_codex_bulk_enrichment_status_for_app(app, request).await
}

#[tauri::command]
pub async fn cancel_codex_bulk_enrichment() -> Result<bool, String> {
    crate::codex_enrichment::cancel_codex_bulk_enrichment_for_app().await
}

#[tauri::command]
pub async fn import_archive_snapshot(
    app: tauri::AppHandle,
    request: crate::meeting_store::ImportArchiveRequest,
) -> Result<crate::meeting_store::ImportArchiveSummary, String> {
    crate::meeting_store::import_archive_snapshot_for_app(&app, request)
}

#[tauri::command]
pub async fn export_meetings(
    app: tauri::AppHandle,
    request: crate::meeting_store::ExportMeetingsRequest,
) -> Result<crate::meeting_store::ExportMeetingsSummary, String> {
    crate::meeting_store::export_meetings_for_app(&app, request)
}
