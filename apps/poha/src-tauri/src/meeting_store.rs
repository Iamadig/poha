use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

use chrono::Utc;
use rusqlite::{Connection, OpenFlags, params};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::recorder_settings;
use crate::transcription::{TranscriptDocument, TranscriptSegment};

const METADATA_FILE_NAME: &str = "meeting.json";
const INDEX_DIR_NAME: &str = ".poha";
const INDEX_FILE_NAME: &str = "meetings.sqlite";
const DEFAULT_LIST_LIMIT: usize = 100;
const MAX_LIST_LIMIT: usize = 500;
const INDIVIDUAL_NETWORKING_COMPANY: &str = "Individual Networking";
const REVIEW_NEEDED_FILTER: &str = "Review Needed";
const DEFAULT_ARCHIVE_ROOT: &str =
    "/Users/adi/Documents/Codex/2026-05-18/i-need-to-reorganize-my-meetings";

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MeetingMetadata {
    #[serde(default = "default_schema_version")]
    pub schema_version: u32,
    #[serde(default)]
    pub id: String,
    #[serde(default)]
    pub title: Option<String>,
    #[serde(default)]
    pub company: Option<String>,
    #[serde(default)]
    pub context: Option<String>,
    #[serde(default)]
    pub context_kind: Option<String>,
    #[serde(default)]
    pub people: Vec<String>,
    #[serde(default)]
    pub speaker_map: BTreeMap<String, String>,
    #[serde(default = "default_speaker_status")]
    pub speaker_status: String,
    #[serde(default)]
    pub source: Option<String>,
    #[serde(default)]
    pub original_id: Option<String>,
    #[serde(default)]
    pub notion_page_id: Option<String>,
    #[serde(default)]
    pub notion_url: Option<String>,
    #[serde(default)]
    pub imported_from: Option<String>,
    #[serde(default)]
    pub imported_at: Option<String>,
    #[serde(default)]
    pub updated_at: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MeetingListItem {
    pub id: String,
    pub title: String,
    pub status: Option<String>,
    pub started_at: Option<String>,
    pub ended_at: Option<String>,
    pub company: Option<String>,
    pub context: Option<String>,
    pub context_kind: Option<String>,
    pub source: Option<String>,
    pub people: Vec<String>,
    pub speakers: Vec<String>,
    pub has_summary: bool,
    pub has_transcript: bool,
    pub session_dir: String,
    pub summary_path: Option<String>,
    pub transcript_path: Option<String>,
    pub updated_at: Option<String>,
    pub snippet: Option<String>,
    pub enrichment: MeetingEnrichmentStatus,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MeetingEnrichmentStatus {
    pub status: String,
    pub needs_enrichment: bool,
    pub needs_summary: bool,
    pub summary_stale: bool,
    pub needs_title: bool,
    pub needs_speakers: bool,
    pub needs_context: bool,
    pub reasons: Vec<String>,
    pub summary_status: String,
    pub title_status: String,
    pub speaker_status: String,
    pub context_status: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MeetingDetail {
    pub item: MeetingListItem,
    pub metadata: MeetingMetadata,
    pub session_metadata: Value,
    pub summary_markdown: Option<String>,
    pub transcript_markdown: Option<String>,
    pub speaker_candidates: Vec<String>,
    pub paths: MeetingPaths,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MeetingPaths {
    pub session_dir: String,
    pub index_path: String,
    pub session_json: String,
    pub meeting_json: String,
    pub summary_markdown: String,
    pub transcript_markdown: Option<String>,
    pub transcript_json: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct IndexSummary {
    pub path: String,
    pub meetings_indexed: usize,
    pub contexts_indexed: usize,
    pub rebuilt_at: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MeetingContextItem {
    pub name: String,
    pub kind: String,
    pub company: Option<String>,
    pub meeting_count: usize,
    pub transcript_count: usize,
    pub latest_started_at: Option<String>,
    pub sample_titles: Vec<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateMeetingMetadataRequest {
    pub id: String,
    pub title: Option<String>,
    pub company: Option<String>,
    pub context: Option<String>,
    pub context_kind: Option<String>,
    pub people: Option<Vec<String>>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ApplySpeakerMapRequest {
    pub id: String,
    pub speaker_map: BTreeMap<String, String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ImportArchiveRequest {
    pub archive_root: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ImportArchiveSummary {
    pub imported: usize,
    pub created_sessions: usize,
    pub updated_metadata: usize,
    pub skipped: usize,
    pub source_root: String,
    pub index_path: String,
    pub imported_at: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExportMeetingsRequest {
    pub output_dir: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ExportMeetingsSummary {
    pub path: String,
    pub meetings_exported: usize,
    pub generated_at: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DeleteMeetingSummary {
    pub id: String,
    pub deleted_at: String,
    pub original_path: String,
    pub moved_to: String,
    pub index_path: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DeleteMeetingsSummary {
    pub deleted_count: usize,
    pub deleted: Vec<DeleteMeetingSummary>,
    pub index_path: String,
}

#[derive(Debug, Clone)]
struct MeetingIndexRow {
    id: String,
    title: String,
    status: Option<String>,
    started_at: Option<String>,
    ended_at: Option<String>,
    company: Option<String>,
    context: Option<String>,
    context_kind: Option<String>,
    source: Option<String>,
    people: Vec<String>,
    speakers: Vec<String>,
    speaker_map: BTreeMap<String, String>,
    has_summary: bool,
    has_transcript: bool,
    session_dir: PathBuf,
    summary_path: Option<PathBuf>,
    transcript_path: Option<PathBuf>,
    transcript_json_path: Option<PathBuf>,
    meeting_metadata_path: PathBuf,
    session_metadata_path: PathBuf,
    summary_stale: bool,
    updated_at: Option<String>,
    snippet: Option<String>,
    search_text: String,
    session_mtime_ms: i64,
    meeting_mtime_ms: i64,
}

#[derive(Debug, Clone)]
struct ContextIndexRow {
    name: String,
    kind: String,
    company: Option<String>,
    meeting_count: usize,
    transcript_count: usize,
    latest_started_at: Option<String>,
    sample_titles: Vec<String>,
    search_text: String,
}

#[derive(Debug, Clone)]
struct SummaryInputState {
    input_hash: String,
    summary_mtime_ms: i64,
}

pub fn list_meetings_for_app(
    app: &tauri::AppHandle,
    query: Option<String>,
    company: Option<String>,
    context: Option<String>,
    limit: Option<usize>,
) -> Result<Vec<MeetingListItem>, String> {
    let recordings_dir = recordings_dir_for_app(app)?;
    list_meetings(&recordings_dir, query, company, context, limit)
}

pub fn list_meeting_contexts_for_app(
    app: &tauri::AppHandle,
    query: Option<String>,
) -> Result<Vec<MeetingContextItem>, String> {
    let recordings_dir = recordings_dir_for_app(app)?;
    list_meeting_contexts(&recordings_dir, query)
}

pub fn get_meeting_for_app(app: &tauri::AppHandle, id: &str) -> Result<MeetingDetail, String> {
    let recordings_dir = recordings_dir_for_app(app)?;
    get_meeting(&recordings_dir, id)
}

pub fn update_meeting_metadata_for_app(
    app: &tauri::AppHandle,
    request: UpdateMeetingMetadataRequest,
) -> Result<MeetingDetail, String> {
    let recordings_dir = recordings_dir_for_app(app)?;
    update_meeting_metadata(&recordings_dir, request)
}

pub fn apply_speaker_map_for_app(
    app: &tauri::AppHandle,
    request: ApplySpeakerMapRequest,
) -> Result<MeetingDetail, String> {
    let recordings_dir = recordings_dir_for_app(app)?;
    apply_speaker_map(&recordings_dir, request)
}

pub fn delete_meeting_for_app(
    app: &tauri::AppHandle,
    id: &str,
) -> Result<DeleteMeetingSummary, String> {
    let recordings_dir = recordings_dir_for_app(app)?;
    delete_meeting(&recordings_dir, id)
}

pub fn delete_meetings_for_app(
    app: &tauri::AppHandle,
    ids: &[String],
) -> Result<DeleteMeetingsSummary, String> {
    let recordings_dir = recordings_dir_for_app(app)?;
    delete_meetings(&recordings_dir, ids)
}

pub fn rebuild_meeting_index_for_app(app: &tauri::AppHandle) -> Result<IndexSummary, String> {
    let recordings_dir = recordings_dir_for_app(app)?;
    rebuild_index(&recordings_dir)
}

pub fn copy_meeting_for_llm_for_app(app: &tauri::AppHandle, id: &str) -> Result<String, String> {
    let recordings_dir = recordings_dir_for_app(app)?;
    copy_meeting_for_llm(&recordings_dir, id)
}

pub fn copy_company_for_llm_for_app(
    app: &tauri::AppHandle,
    company: &str,
) -> Result<String, String> {
    let recordings_dir = recordings_dir_for_app(app)?;
    copy_context_for_llm(&recordings_dir, company)
}

pub fn copy_context_for_llm_for_app(
    app: &tauri::AppHandle,
    context: &str,
) -> Result<String, String> {
    let recordings_dir = recordings_dir_for_app(app)?;
    copy_context_for_llm(&recordings_dir, context)
}

pub fn import_archive_snapshot_for_app(
    app: &tauri::AppHandle,
    request: ImportArchiveRequest,
) -> Result<ImportArchiveSummary, String> {
    let recordings_dir = recordings_dir_for_app(app)?;
    import_archive_snapshot(&recordings_dir, request)
}

pub fn export_meetings_for_app(
    app: &tauri::AppHandle,
    request: ExportMeetingsRequest,
) -> Result<ExportMeetingsSummary, String> {
    let recordings_dir = recordings_dir_for_app(app)?;
    export_meetings(&recordings_dir, request)
}

fn recordings_dir_for_app(app: &tauri::AppHandle) -> Result<PathBuf, String> {
    let settings = recorder_settings::load(app)?;
    let recordings_dir = settings.recordings_dir_path();
    std::fs::create_dir_all(&recordings_dir).map_err(|e| {
        format!(
            "failed creating recordings dir {}: {e}",
            recordings_dir.display()
        )
    })?;
    Ok(recordings_dir)
}

pub fn list_meetings(
    recordings_dir: &Path,
    query: Option<String>,
    company: Option<String>,
    context: Option<String>,
    limit: Option<usize>,
) -> Result<Vec<MeetingListItem>, String> {
    ensure_index(recordings_dir)?;
    let conn = open_index(recordings_dir)?;
    let query = query.unwrap_or_default().trim().to_lowercase();
    let company = company.unwrap_or_default().trim().to_lowercase();
    let context = context.unwrap_or_default().trim().to_lowercase();
    let limit = limit.unwrap_or(DEFAULT_LIST_LIMIT).clamp(1, MAX_LIST_LIMIT) as i64;
    let review_filter = context == REVIEW_NEEDED_FILTER.to_lowercase();
    let query_limit = if review_filter {
        MAX_LIST_LIMIT as i64
    } else {
        limit
    };

    let mut stmt = conn
        .prepare(
            r#"
            SELECT id, title, status, started_at, ended_at, company, context, context_kind, source,
                   people_json, speakers_json, speaker_map_json, has_summary, has_transcript, summary_stale, session_dir,
                   summary_path, transcript_path, transcript_json_path, meeting_metadata_path, session_metadata_path,
                   updated_at, snippet
            FROM meetings
            WHERE (?1 = '' OR lower(search_text) LIKE '%' || ?1 || '%')
              AND (?2 = '' OR lower(coalesce(company, 'Individual Networking')) = ?2)
              AND (?3 = '' OR ?3 = 'review needed' OR lower(coalesce(company, 'Individual Networking')) = ?3)
            ORDER BY coalesce(started_at, '') DESC, id DESC
            LIMIT ?4
            "#,
        )
        .map_err_sql("prepare meeting list")?;

    let rows = stmt
        .query_map(params![query, company, context, query_limit], |row| {
            meeting_list_item_from_index_row(row)
        })
        .map_err_sql("query meeting list")?;

    let mut rows = rows
        .collect::<Result<Vec<_>, _>>()
        .map_err_sql("read meeting list rows")?;
    if review_filter {
        rows.retain(|row| row.enrichment.needs_enrichment);
    }
    rows.truncate(limit as usize);
    Ok(rows)
}

fn list_all_meetings(recordings_dir: &Path) -> Result<Vec<MeetingListItem>, String> {
    ensure_index(recordings_dir)?;
    let conn = open_index(recordings_dir)?;
    let mut stmt = conn
        .prepare(
            r#"
            SELECT id, title, status, started_at, ended_at, company, context, context_kind, source,
                   people_json, speakers_json, speaker_map_json, has_summary, has_transcript, summary_stale, session_dir,
                   summary_path, transcript_path, transcript_json_path, meeting_metadata_path, session_metadata_path,
                   updated_at, snippet
            FROM meetings
            ORDER BY coalesce(started_at, '') DESC, id DESC
            "#,
        )
        .map_err_sql("prepare full meeting list")?;

    let rows = stmt
        .query_map([], meeting_list_item_from_index_row)
        .map_err_sql("query full meeting list")?;
    rows.collect::<Result<Vec<_>, _>>()
        .map_err_sql("read full meeting list rows")
}

pub fn list_summary_queue_from_index(
    recordings_dir: &Path,
    limit: usize,
) -> Result<Vec<MeetingListItem>, String> {
    let Some(conn) = open_index_read_only(recordings_dir)? else {
        return Ok(Vec::new());
    };
    let limit = limit.clamp(1, MAX_LIST_LIMIT) as i64;
    let mut stmt = conn
        .prepare(
            r#"
            SELECT id, title, status, started_at, ended_at, company, context, context_kind, source,
                   people_json, speakers_json, speaker_map_json, has_summary, has_transcript, summary_stale, session_dir,
                   summary_path, transcript_path, transcript_json_path, meeting_metadata_path, session_metadata_path,
                   updated_at, snippet
            FROM meetings
            WHERE has_transcript = 1
              AND (has_summary = 0 OR summary_stale = 1)
            ORDER BY coalesce(started_at, '') DESC, id DESC
            LIMIT ?1
            "#,
        )
        .map_err_sql("prepare summary queue list")?;

    let rows = stmt
        .query_map(params![limit], |row| meeting_list_item_from_index_row(row))
        .map_err_sql("query summary queue list")?;

    rows.collect::<Result<Vec<_>, _>>()
        .map_err_sql("read summary queue rows")
}

fn meeting_list_item_from_index_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<MeetingListItem> {
    let title = row.get::<_, String>(1)?;
    let company = row
        .get::<_, Option<String>>(5)?
        .unwrap_or_else(|| INDIVIDUAL_NETWORKING_COMPANY.to_string());
    let context = Some(company.clone());
    let context_kind = Some("Company".to_string());
    let speakers = string_vec_from_json(&row.get::<_, String>(10)?);
    let speaker_map = string_map_from_json(&row.get::<_, String>(11)?);
    let has_summary = row.get::<_, i64>(12)? != 0;
    let has_transcript = row.get::<_, i64>(13)? != 0;
    let summary_stale = row.get::<_, i64>(14)? != 0;
    let session_dir = row.get::<_, String>(15)?;
    let summary_path = row.get::<_, Option<String>>(16)?;
    let transcript_path = row.get::<_, Option<String>>(17)?;
    Ok(MeetingListItem {
        id: row.get(0)?,
        title: title.clone(),
        status: row.get(2)?,
        started_at: row.get(3)?,
        ended_at: row.get(4)?,
        company: Some(company),
        context: context.clone(),
        context_kind: context_kind.clone(),
        source: row.get(8)?,
        people: string_vec_from_json(&row.get::<_, String>(9)?),
        speakers: speakers.clone(),
        has_summary,
        has_transcript,
        session_dir,
        summary_path,
        transcript_path,
        updated_at: row.get(21)?,
        snippet: row.get(22)?,
        enrichment: enrichment_status_from_fields(
            &title,
            has_summary,
            summary_stale,
            has_transcript,
            &speakers,
            &speaker_map,
            context.as_deref(),
            context_kind.as_deref(),
        ),
    })
}

pub fn list_meeting_contexts(
    recordings_dir: &Path,
    query: Option<String>,
) -> Result<Vec<MeetingContextItem>, String> {
    ensure_index(recordings_dir)?;
    let conn = open_index(recordings_dir)?;
    let query = query.unwrap_or_default().trim().to_lowercase();
    let mut stmt = conn
        .prepare(
            r#"
            SELECT name, kind, company, meeting_count, transcript_count, latest_started_at, sample_titles_json
            FROM contexts
            WHERE ?1 = '' OR lower(search_text) LIKE '%' || ?1 || '%'
            ORDER BY
              CASE kind WHEN 'Company' THEN 0 WHEN 'Bucket' THEN 1 WHEN 'Review' THEN 2 ELSE 3 END,
              meeting_count DESC,
              lower(name)
            "#,
        )
        .map_err_sql("prepare context list")?;
    let rows = stmt
        .query_map(params![query], |row| {
            Ok(MeetingContextItem {
                name: row.get(0)?,
                kind: row.get(1)?,
                company: row.get(2)?,
                meeting_count: row.get::<_, i64>(3)? as usize,
                transcript_count: row.get::<_, i64>(4)? as usize,
                latest_started_at: row.get(5)?,
                sample_titles: string_vec_from_json(&row.get::<_, String>(6)?),
            })
        })
        .map_err_sql("query context list")?;

    rows.collect::<Result<Vec<_>, _>>()
        .map_err_sql("read context list rows")
}

pub fn get_meeting(recordings_dir: &Path, id: &str) -> Result<MeetingDetail, String> {
    ensure_index(recordings_dir)?;
    let session_dir = session_dir(recordings_dir, id)?;
    build_meeting_detail(recordings_dir, &session_dir)
}

pub fn update_meeting_metadata(
    recordings_dir: &Path,
    request: UpdateMeetingMetadataRequest,
) -> Result<MeetingDetail, String> {
    let session_dir = session_dir(recordings_dir, &request.id)?;
    let mut metadata = read_meeting_metadata(&session_dir, &request.id)?;
    if let Some(title) = request.title {
        metadata.title = clean_optional(title);
    }
    if let Some(company) = request.company {
        metadata.company = Some(company_or_catch_all(clean_optional(company)));
    }
    if let Some(context) = request.context {
        metadata.context = clean_optional(context);
    }
    if let Some(context_kind) = request.context_kind {
        metadata.context_kind = clean_optional(context_kind);
    }
    if let Some(people) = request.people {
        metadata.people = normalize_people(people);
    }
    normalize_company_grouping(&mut metadata);
    metadata.updated_at = Some(Utc::now().to_rfc3339());
    write_meeting_metadata(&session_dir, &metadata)?;
    rebuild_index(recordings_dir)?;
    build_meeting_detail(recordings_dir, &session_dir)
}

pub fn apply_speaker_map(
    recordings_dir: &Path,
    request: ApplySpeakerMapRequest,
) -> Result<MeetingDetail, String> {
    let session_dir = session_dir(recordings_dir, &request.id)?;
    let mut metadata = read_meeting_metadata(&session_dir, &request.id)?;
    metadata.speaker_map = normalize_speaker_map(request.speaker_map);
    metadata.speaker_status = if metadata.speaker_map.is_empty() {
        "unmapped".to_string()
    } else {
        "mapped".to_string()
    };
    metadata.updated_at = Some(Utc::now().to_rfc3339());
    write_meeting_metadata(&session_dir, &metadata)?;
    rebuild_index(recordings_dir)?;
    build_meeting_detail(recordings_dir, &session_dir)
}

pub fn delete_meeting(recordings_dir: &Path, id: &str) -> Result<DeleteMeetingSummary, String> {
    let ids = vec![id.to_string()];
    delete_meetings(recordings_dir, &ids)?
        .deleted
        .into_iter()
        .next()
        .ok_or_else(|| "failed deleting meeting".to_string())
}

pub fn delete_meetings(
    recordings_dir: &Path,
    ids: &[String],
) -> Result<DeleteMeetingsSummary, String> {
    let mut seen = BTreeSet::new();
    let mut unique_ids = Vec::new();
    for id in ids {
        let id = id.trim();
        if !id.is_empty() && seen.insert(id.to_string()) {
            unique_ids.push(id.to_string());
        }
    }
    if unique_ids.is_empty() {
        return Err("select at least one recording to move out".to_string());
    }

    let mut sessions = Vec::new();
    for id in unique_ids {
        sessions.push((id.clone(), session_dir(recordings_dir, &id)?));
    }

    let deleted_at = Utc::now().to_rfc3339();
    let deleted_root = recordings_dir.join(INDEX_DIR_NAME).join("deleted");
    std::fs::create_dir_all(&deleted_root).map_err(|e| {
        format!(
            "failed creating deleted meetings dir {}: {e}",
            deleted_root.display()
        )
    })?;

    let mut deleted = Vec::new();
    for (id, session_dir) in sessions {
        let moved_to = unique_deleted_path(&deleted_root, &id);
        std::fs::rename(&session_dir, &moved_to).map_err(|e| {
            format!(
                "failed moving meeting {} to {}: {e}",
                session_dir.display(),
                moved_to.display()
            )
        })?;
        deleted.push(DeleteMeetingSummary {
            id,
            deleted_at: deleted_at.clone(),
            original_path: path_string(&session_dir),
            moved_to: path_string(&moved_to),
            index_path: path_string(&index_path(recordings_dir)),
        });
    }

    rebuild_index(recordings_dir)?;

    Ok(DeleteMeetingsSummary {
        deleted_count: deleted.len(),
        deleted,
        index_path: path_string(&index_path(recordings_dir)),
    })
}

pub fn copy_meeting_for_llm(recordings_dir: &Path, id: &str) -> Result<String, String> {
    let detail = get_meeting(recordings_dir, id)?;
    let item = &detail.item;
    let mut out = Vec::new();
    out.push(format!("# Poha Meeting: {}", item.title));
    out.push(String::new());
    out.push(format!("- ID: {}", item.id));
    if let Some(company) = &item.company {
        out.push(format!("- Company: {company}"));
    }
    let speakers = mapped_speaker_names(&detail.metadata.speaker_map);
    if !speakers.is_empty() {
        out.push(format!("- Speakers: {}", speakers.join(", ")));
    }
    if let Some(started_at) = &item.started_at {
        out.push(format!("- Started: {started_at}"));
    }
    out.push(String::new());

    out.push("## Summary".to_string());
    out.push(String::new());
    out.push(
        detail
            .summary_markdown
            .as_deref()
            .map(str::trim)
            .filter(|text| !text.is_empty())
            .unwrap_or("No summary.md yet.")
            .to_string(),
    );
    out.push(String::new());

    out.push("## Transcript".to_string());
    out.push(String::new());
    out.push(
        detail
            .transcript_markdown
            .as_deref()
            .map(str::trim)
            .filter(|text| !text.is_empty())
            .unwrap_or("No transcript yet.")
            .to_string(),
    );
    out.push(String::new());
    Ok(out.join("\n"))
}

pub fn copy_context_for_llm(recordings_dir: &Path, context: &str) -> Result<String, String> {
    let context = context.trim();
    if context.is_empty() {
        return Err("company is required".to_string());
    }

    let meetings = list_meetings(
        recordings_dir,
        None,
        None,
        Some(context.to_string()),
        Some(MAX_LIST_LIMIT),
    )?;
    if meetings.is_empty() {
        return Err(format!("no meetings found for company: {context}"));
    }

    let mut out = Vec::new();
    out.push(format!("# Poha Company: {context}"));
    out.push(String::new());
    for meeting in meetings {
        out.push(format!("## {}", meeting.title));
        out.push(String::new());
        out.push(format!("- ID: {}", meeting.id));
        let detail = build_meeting_detail(recordings_dir, &PathBuf::from(&meeting.session_dir))?;
        let speakers = mapped_speaker_names(&detail.metadata.speaker_map);
        if !speakers.is_empty() {
            out.push(format!("- Speakers: {}", speakers.join(", ")));
        }
        if let Some(started_at) = &meeting.started_at {
            out.push(format!("- Started: {started_at}"));
        }
        out.push(String::new());
        out.push("### Summary".to_string());
        out.push(String::new());
        out.push(
            detail
                .summary_markdown
                .as_deref()
                .map(str::trim)
                .filter(|text| !text.is_empty())
                .or(meeting.snippet.as_deref())
                .unwrap_or("No summary yet.")
                .to_string(),
        );
        out.push(String::new());
        out.push("### Transcript".to_string());
        out.push(String::new());
        out.push(
            detail
                .transcript_markdown
                .as_deref()
                .map(str::trim)
                .filter(|text| !text.is_empty())
                .unwrap_or("No transcript yet.")
                .to_string(),
        );
        out.push(String::new());
    }

    Ok(out.join("\n"))
}

pub fn import_archive_snapshot(
    recordings_dir: &Path,
    request: ImportArchiveRequest,
) -> Result<ImportArchiveSummary, String> {
    let archive_root = request
        .archive_root
        .and_then(clean_optional)
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(DEFAULT_ARCHIVE_ROOT));
    let imported_at = Utc::now().to_rfc3339();
    let archive = ArchiveSnapshot::load(&archive_root)?;
    let contexts = archive.contexts.clone();
    let mut summary = ImportArchiveSummary {
        imported: 0,
        created_sessions: 0,
        updated_metadata: 0,
        skipped: 0,
        source_root: path_string(&archive_root),
        index_path: path_string(&index_path(recordings_dir)),
        imported_at: imported_at.clone(),
    };

    for record in archive.records {
        match import_archive_record(recordings_dir, &record, &imported_at) {
            Ok(result) => {
                summary.imported += 1;
                if result.created_session {
                    summary.created_sessions += 1;
                }
                if result.updated_metadata {
                    summary.updated_metadata += 1;
                }
            }
            Err(error) => {
                summary.skipped += 1;
                tracing::warn!("skipping archive import {}: {error}", record.id);
            }
        }
    }

    summary.updated_metadata += backfill_missing_local_metadata(recordings_dir, &imported_at)?;
    summary.updated_metadata += backfill_context_metadata(recordings_dir, &contexts, &imported_at)?;
    rebuild_index(recordings_dir)?;
    Ok(summary)
}

pub fn export_meetings(
    recordings_dir: &Path,
    request: ExportMeetingsRequest,
) -> Result<ExportMeetingsSummary, String> {
    rebuild_index(recordings_dir)?;
    let generated_at = Utc::now().to_rfc3339();
    let output_dir = request
        .output_dir
        .and_then(clean_optional)
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            recordings_dir
                .join(INDEX_DIR_NAME)
                .join("exports")
                .join(format!(
                    "poha-export-{}",
                    Utc::now().format("%Y%m%d-%H%M%S")
                ))
        });
    std::fs::create_dir_all(&output_dir)
        .map_err(|e| format!("failed creating export dir {}: {e}", output_dir.display()))?;

    let meetings = list_all_meetings(recordings_dir)?;
    let contexts = list_meeting_contexts(recordings_dir, None)?;
    let mut folder_by_id = BTreeMap::new();
    let mut manifest_items = Vec::new();
    let mut index_lines = vec![
        "# Poha Meeting Export".to_string(),
        String::new(),
        format!("Generated: {generated_at}"),
        format!("Meetings: {}", meetings.len()),
        format!("Contexts: {}", contexts.len()),
        String::new(),
    ];

    for meeting in &meetings {
        let detail = build_meeting_detail(recordings_dir, &PathBuf::from(&meeting.session_dir))?;
        let folder_name = export_folder_name(meeting);
        let meeting_dir = output_dir.join(&folder_name);
        std::fs::create_dir_all(&meeting_dir).map_err(|e| {
            format!(
                "failed creating meeting export dir {}: {e}",
                meeting_dir.display()
            )
        })?;

        let transcript_path = meeting_dir.join("transcript.md");
        let metadata_path = meeting_dir.join("metadata.json");
        std::fs::write(
            &transcript_path,
            detail
                .transcript_markdown
                .as_deref()
                .unwrap_or("# Transcript\n\nNo transcript available.\n"),
        )
        .map_err(|e| format!("failed writing {}: {e}", transcript_path.display()))?;
        if let Some(summary) = detail.summary_markdown.as_deref() {
            let summary_path = meeting_dir.join("summary.md");
            std::fs::write(&summary_path, summary)
                .map_err(|e| format!("failed writing {}: {e}", summary_path.display()))?;
        }
        std::fs::write(
            &metadata_path,
            serde_json::to_string_pretty(&detail)
                .map_err(|e| format!("failed serializing export metadata: {e}"))?,
        )
        .map_err(|e| format!("failed writing {}: {e}", metadata_path.display()))?;

        folder_by_id.insert(meeting.id.clone(), folder_name.clone());
        manifest_items.push(serde_json::json!({
            "id": meeting.id,
            "title": meeting.title,
            "folder": folder_name,
            "transcript": "transcript.md",
            "metadata": "metadata.json",
        }));
    }

    for context in &contexts {
        index_lines.push(format!(
            "## {} ({}, {})",
            context.name, context.kind, context.meeting_count
        ));
        index_lines.push(String::new());
        for meeting in meetings.iter().filter(|meeting| {
            meeting
                .context
                .as_deref()
                .or(meeting.company.as_deref())
                .is_some_and(|name| name == context.name)
        }) {
            let folder_name = folder_by_id
                .get(&meeting.id)
                .map(String::as_str)
                .unwrap_or_default();
            index_lines.push(format!(
                "- [{}]({}/transcript.md){}",
                meeting.title,
                folder_name,
                meeting
                    .started_at
                    .as_ref()
                    .map(|date| format!(" - {date}"))
                    .unwrap_or_default()
            ));
        }
        index_lines.push(String::new());
    }

    std::fs::write(output_dir.join("index.md"), index_lines.join("\n"))
        .map_err(|e| format!("failed writing export index: {e}"))?;
    std::fs::write(
        output_dir.join("manifest.json"),
        serde_json::to_string_pretty(&serde_json::json!({
            "generatedAt": generated_at,
            "meetingsExported": meetings.len(),
            "meetings": manifest_items,
        }))
        .map_err(|e| format!("failed serializing export manifest: {e}"))?,
    )
    .map_err(|e| format!("failed writing export manifest: {e}"))?;
    #[cfg(not(test))]
    let _ = open::that(&output_dir);

    Ok(ExportMeetingsSummary {
        path: path_string(&output_dir),
        meetings_exported: meetings.len(),
        generated_at,
    })
}

#[derive(Debug, Clone)]
struct ArchiveRecord {
    source: String,
    id: String,
    title: String,
    status: String,
    started_at: Option<String>,
    ended_at: Option<String>,
    audio_path: Option<String>,
    original_dir: Option<String>,
    page_path: PathBuf,
    people: Vec<String>,
    context: Option<String>,
    context_kind: Option<String>,
    company: Option<String>,
    notion_page_id: Option<String>,
    notion_url: Option<String>,
    imported_from: String,
}

#[derive(Debug, Clone, Default)]
struct ArchivePageProps {
    title: Option<String>,
    people: Vec<String>,
    context: Option<String>,
    context_kind: Option<String>,
    company: Option<String>,
    notion_page_id: Option<String>,
    notion_url: Option<String>,
}

#[derive(Debug, Clone)]
struct ContextCatalogItem {
    id: String,
    name: String,
    kind: String,
}

#[derive(Debug, Clone)]
struct ContextAssignment {
    name: String,
    kind: String,
    company: Option<String>,
}

#[derive(Debug)]
struct ArchiveSnapshot {
    records: Vec<ArchiveRecord>,
    contexts: Vec<ContextCatalogItem>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "snake_case")]
struct NormalizedArchiveRecord {
    source: String,
    id: String,
    title: String,
    status: Option<String>,
    started_at: Option<String>,
    ended_at: Option<String>,
    created_at: Option<String>,
    audio_path: Option<String>,
    original_dir: Option<String>,
    normalized_page: String,
}

#[derive(Debug, Clone, Deserialize)]
struct NotionSourceRecord {
    id: String,
    url: Option<String>,
    title: String,
    date: Option<String>,
    select: Option<String>,
    page_file: String,
}

#[derive(Debug, Clone, Copy)]
struct ImportRecordResult {
    created_session: bool,
    updated_metadata: bool,
}

impl ArchiveSnapshot {
    fn load(root: &Path) -> Result<Self, String> {
        let contexts = load_context_catalog(root)?;
        let archive_props = load_archive_page_props(root, &contexts)?;
        let mut records = Vec::new();

        let normalized_path = root.join("normalized-meetings").join("index.json");
        if normalized_path.exists() {
            let normalized = read_json_file::<Vec<NormalizedArchiveRecord>>(&normalized_path)?;
            for record in normalized {
                let props = archive_props.get(&record.id).cloned().unwrap_or_default();
                let started_at = record.started_at.clone().or(record.created_at.clone());
                records.push(ArchiveRecord {
                    source: record.source.clone(),
                    id: record.id.clone(),
                    title: props.title.clone().unwrap_or(record.title),
                    status: record.status.unwrap_or_else(|| "imported".to_string()),
                    started_at,
                    ended_at: record.ended_at,
                    audio_path: clean_optional(record.audio_path.unwrap_or_default()),
                    original_dir: record.original_dir.and_then(clean_optional),
                    page_path: PathBuf::from(record.normalized_page),
                    people: props.people,
                    context: props.context,
                    context_kind: props.context_kind,
                    company: props.company,
                    notion_page_id: props.notion_page_id,
                    notion_url: props.notion_url,
                    imported_from: "normalized-meetings".to_string(),
                });
            }
        }

        let notion_source_path = root
            .join("notion-source-export")
            .join("meetings-adi")
            .join("index.json");
        if notion_source_path.exists() {
            let notion_records = read_json_file::<Vec<NotionSourceRecord>>(&notion_source_path)?;
            let imported_ids = records
                .iter()
                .map(|record| record.id.clone())
                .collect::<BTreeSet<_>>();
            for record in notion_records {
                if imported_ids.contains(&record.id) {
                    continue;
                }
                let props = archive_props.get(&record.id).cloned().unwrap_or_default();
                records.push(ArchiveRecord {
                    source: "notion".to_string(),
                    id: record.id.clone(),
                    title: props.title.clone().unwrap_or(record.title),
                    status: "imported".to_string(),
                    started_at: record.date.clone(),
                    ended_at: None,
                    audio_path: None,
                    original_dir: None,
                    page_path: PathBuf::from(record.page_file),
                    people: props.people,
                    context: props.context.or(record.select),
                    context_kind: props.context_kind,
                    company: props.company,
                    notion_page_id: props.notion_page_id.or(Some(record.id)),
                    notion_url: props.notion_url.or(record.url),
                    imported_from: "notion-source-export".to_string(),
                });
            }
        }

        if records.is_empty() {
            return Err(format!("no archive records found under {}", root.display()));
        }

        Ok(Self { records, contexts })
    }
}

fn load_archive_page_props(
    root: &Path,
    contexts: &[ContextCatalogItem],
) -> Result<BTreeMap<String, ArchivePageProps>, String> {
    let contexts = contexts
        .iter()
        .map(|context| (context.id.clone(), context.clone()))
        .collect::<BTreeMap<_, _>>();
    let mut props_by_original_id = BTreeMap::new();

    for archive_path in archive_snapshot_paths(root) {
        if !archive_path.exists() {
            continue;
        }
        let archive = read_json_value(&archive_path)?;
        for page in archive
            .get("results")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
        {
            let properties = page.get("properties").unwrap_or(&Value::Null);
            let original_id = notion_prop_text(properties, "Original ID");
            if original_id.is_empty() {
                continue;
            }
            let context_id = relation_id(properties, "Context");
            let context_record = context_id.as_deref().and_then(|id| contexts.get(id));
            let context = context_record.map(|record| record.name.clone());
            let context_kind = context_record.map(|record| record.kind.clone());
            let company = context_record
                .filter(|record| record.kind.eq_ignore_ascii_case("company"))
                .map(|record| record.name.clone());
            props_by_original_id.insert(
                original_id,
                ArchivePageProps {
                    title: clean_optional(notion_prop_text(properties, "Meeting Name")),
                    people: notion_multi_select(properties, "People"),
                    context,
                    context_kind,
                    company,
                    notion_page_id: page
                        .get("id")
                        .and_then(Value::as_str)
                        .map(ToString::to_string),
                    notion_url: page
                        .get("url")
                        .and_then(Value::as_str)
                        .map(ToString::to_string),
                },
            );
        }
    }

    Ok(props_by_original_id)
}

fn archive_snapshot_paths(root: &Path) -> Vec<PathBuf> {
    [
        "notion-archive-sandbox-query.json",
        "notion-archive-sandbox-after-backfill.json",
        "notion-archive-after-poha-sync.json",
        "notion-archive-after-poha-rerun.json",
        "archive-after-title-update.json",
    ]
    .into_iter()
    .map(|name| root.join(name))
    .collect()
}

fn load_context_catalog(root: &Path) -> Result<Vec<ContextCatalogItem>, String> {
    let mut by_name = BTreeMap::new();
    for path in [
        "notion-contexts-sandbox-query.json",
        "notion-contexts-sandbox-enriched-query.json",
        "notion-contexts-sandbox-after-backfill.json",
    ] {
        let path = root.join(path);
        if !path.exists() {
            continue;
        }
        for item in load_context_file(&path)? {
            by_name.insert(item.name.clone(), item);
        }
    }
    for (name, kind) in [
        ("Individual Networking", "Bucket"),
        ("Investor Calls", "Bucket"),
        ("Datapipe Internal", "Bucket"),
        ("Research", "Bucket"),
        ("Review Needed", "Review"),
    ] {
        by_name
            .entry(name.to_string())
            .or_insert(ContextCatalogItem {
                id: name.to_string(),
                name: name.to_string(),
                kind: kind.to_string(),
            });
    }
    Ok(by_name.into_values().collect())
}

fn load_context_file(path: &Path) -> Result<Vec<ContextCatalogItem>, String> {
    let contexts = read_json_value(path)?;
    let mut out = Vec::new();
    for page in contexts
        .get("results")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        let Some(id) = page.get("id").and_then(Value::as_str) else {
            continue;
        };
        let properties = page.get("properties").unwrap_or(&Value::Null);
        let context = notion_prop_text(properties, "Context");
        if context.is_empty() {
            continue;
        }
        let kind = notion_prop_text(properties, "Type");
        out.push(ContextCatalogItem {
            id: id.to_string(),
            name: context,
            kind: clean_optional(kind).unwrap_or_else(|| "Bucket".to_string()),
        });
    }
    Ok(out)
}

fn import_archive_record(
    recordings_dir: &Path,
    record: &ArchiveRecord,
    imported_at: &str,
) -> Result<ImportRecordResult, String> {
    let session_dir = recordings_dir.join(&record.id);
    let created_session = !session_dir.join("session.json").exists();
    std::fs::create_dir_all(&session_dir)
        .map_err(|e| format!("failed creating session dir {}: {e}", session_dir.display()))?;

    let page = std::fs::read_to_string(&record.page_path).map_err(|e| {
        format!(
            "failed reading archive page {}: {e}",
            record.page_path.display()
        )
    })?;
    let transcript = archive_page_transcript(&page);
    let summary = archive_page_summary(&page);
    let transcript_path = session_dir.join("transcript.md");
    if !transcript_path.exists() {
        std::fs::write(&transcript_path, transcript.as_bytes())
            .map_err(|e| format!("failed writing {}: {e}", transcript_path.display()))?;
    }
    let summary_path = session_dir.join("summary.md");
    if !summary_path.exists()
        && let Some(summary) = summary.as_deref()
    {
        std::fs::write(&summary_path, summary.as_bytes())
            .map_err(|e| format!("failed writing {}: {e}", summary_path.display()))?;
    }

    if created_session {
        let session = serde_json::json!({
            "id": record.id,
            "status": record.status,
            "startedAt": record.started_at,
            "endedAt": record.ended_at,
            "captureDir": record.original_dir.as_deref().unwrap_or_else(|| session_dir.to_str().unwrap_or_default()),
            "audioPath": record.audio_path,
            "transcriptPath": path_string(&transcript_path),
            "transcription": {
                "engine": "imported-archive",
                "status": "imported",
                "model": record.source,
                "fallbackModel": "",
                "transcriptJsonPath": "",
                "transcriptMarkdownPath": path_string(&transcript_path),
                "updatedAt": imported_at,
            },
            "error": null,
        });
        std::fs::write(
            session_dir.join("session.json"),
            serde_json::to_string_pretty(&session)
                .map_err(|e| format!("failed serializing imported session: {e}"))?,
        )
        .map_err(|e| format!("failed writing imported session: {e}"))?;
    }

    let mut metadata = read_meeting_metadata(&session_dir, &record.id)?;
    merge_archive_metadata(&mut metadata, record, imported_at);
    write_meeting_metadata(&session_dir, &metadata)?;

    Ok(ImportRecordResult {
        created_session,
        updated_metadata: true,
    })
}

fn backfill_missing_local_metadata(
    recordings_dir: &Path,
    imported_at: &str,
) -> Result<usize, String> {
    let mut updated = 0;
    for entry in std::fs::read_dir(recordings_dir).map_err(|e| {
        format!(
            "failed reading recordings dir {}: {e}",
            recordings_dir.display()
        )
    })? {
        let entry = entry.map_err(|e| format!("failed reading recordings entry: {e}"))?;
        let session_dir = entry.path();
        if !session_dir.is_dir()
            || !session_dir.join("session.json").exists()
            || session_dir.join(METADATA_FILE_NAME).exists()
        {
            continue;
        }

        let session = read_json_value(&session_dir.join("session.json"))?;
        let id = value_string(&session, "id")
            .or_else(|| session_dir.file_name_string())
            .unwrap_or_else(|| "meeting".to_string());
        let mut metadata = MeetingMetadata::for_id(&id);
        metadata.source = Some("poha".to_string());
        metadata.original_id = Some(id);
        metadata.imported_from = Some("local-recordings".to_string());
        metadata.imported_at = Some(imported_at.to_string());
        metadata.updated_at = Some(imported_at.to_string());
        write_meeting_metadata(&session_dir, &metadata)?;
        updated += 1;
    }
    Ok(updated)
}

fn backfill_context_metadata(
    recordings_dir: &Path,
    contexts: &[ContextCatalogItem],
    updated_at: &str,
) -> Result<usize, String> {
    let mut updated = 0;
    for entry in std::fs::read_dir(recordings_dir).map_err(|e| {
        format!(
            "failed reading recordings dir {}: {e}",
            recordings_dir.display()
        )
    })? {
        let entry = entry.map_err(|e| format!("failed reading recordings entry: {e}"))?;
        let session_dir = entry.path();
        if !session_dir.is_dir() || !session_dir.join("session.json").exists() {
            continue;
        }

        let session_metadata = read_json_value(&session_dir.join("session.json"))?;
        let id = value_string(&session_metadata, "id")
            .or_else(|| session_dir.file_name_string())
            .unwrap_or_else(|| "meeting".to_string());
        let mut metadata = read_meeting_metadata(&session_dir, &id)?;
        let summary_path = session_dir.join("summary.md");
        let summary = read_optional_string(&summary_path)?;
        let transcript_path = transcript_markdown_path(&session_metadata, &session_dir);
        let transcript = transcript_path
            .as_ref()
            .and_then(|path| read_optional_string(path).ok().flatten())
            .unwrap_or_default();
        let title = metadata
            .title
            .clone()
            .or_else(|| title_from_summary(summary.as_deref()))
            .unwrap_or_else(|| default_title(value_string(&session_metadata, "started_at"), &id));
        let haystack = format!(
            "{title}\n{}\n{}",
            summary.as_deref().unwrap_or_default(),
            transcript
        );
        let Some(assignment) =
            context_assignment_for_meeting(&metadata, &title, &haystack, contexts)
        else {
            continue;
        };

        let mut changed = false;
        if metadata.context.as_deref() != Some(assignment.name.as_str()) {
            metadata.context = Some(assignment.name.clone());
            changed = true;
        }
        if metadata.context_kind.as_deref() != Some(assignment.kind.as_str()) {
            metadata.context_kind = Some(assignment.kind.clone());
            changed = true;
        }
        if assignment.kind == "Company" && metadata.company != assignment.company {
            metadata.company = assignment.company.clone();
            changed = true;
        }
        if changed {
            metadata.updated_at = Some(updated_at.to_string());
            write_meeting_metadata(&session_dir, &metadata)?;
            updated += 1;
        }
    }
    Ok(updated)
}

fn context_assignment_for_meeting(
    metadata: &MeetingMetadata,
    title: &str,
    haystack: &str,
    contexts: &[ContextCatalogItem],
) -> Option<ContextAssignment> {
    let ignore_existing = should_reinfer_existing_context(metadata, title, contexts);
    if !ignore_existing {
        if let Some(context) = metadata.context.as_deref().and_then(clean_context_alias) {
            return Some(context);
        }
        if let Some(context) = metadata.context.as_deref() {
            if let Some(item) = find_context(contexts, context) {
                return Some(context_assignment(item));
            }
        }
        if let Some(company) = metadata.company.as_deref() {
            if let Some(item) = find_context(contexts, company) {
                return Some(context_assignment(item));
            }
            return Some(ContextAssignment {
                name: company.to_string(),
                kind: "Company".to_string(),
                company: Some(company.to_string()),
            });
        }
    }
    infer_context_from_text(title, haystack, contexts).or_else(|| {
        Some(ContextAssignment {
            name: "Individual Networking".to_string(),
            kind: "Bucket".to_string(),
            company: None,
        })
    })
}

fn should_reinfer_existing_context(
    metadata: &MeetingMetadata,
    title: &str,
    contexts: &[ContextCatalogItem],
) -> bool {
    if metadata.notion_page_id.is_some() {
        return false;
    }
    let Some(context) = metadata.context.as_deref() else {
        return false;
    };
    let Some(item) = find_context(contexts, context) else {
        return false;
    };
    if !item.kind.eq_ignore_ascii_case("company") || !is_low_signal_title(title) {
        return false;
    }
    let title = normalized_match_text(title);
    !context_aliases(&item.name)
        .iter()
        .any(|alias| text_contains_alias(&title, alias))
}

fn is_low_signal_title(title: &str) -> bool {
    let title = title.trim().to_lowercase();
    title.starts_with("poha recording ")
        || title.starts_with("meeting ")
        || title.starts_with("candidate background")
}

fn clean_context_alias(value: &str) -> Option<ContextAssignment> {
    match value.trim().to_lowercase().as_str() {
        "networking" | "personal" | "coaching" => Some(ContextAssignment {
            name: "Individual Networking".to_string(),
            kind: "Bucket".to_string(),
            company: None,
        }),
        "investor call" | "investor calls" => Some(ContextAssignment {
            name: "Investor Calls".to_string(),
            kind: "Bucket".to_string(),
            company: None,
        }),
        "review needed" => Some(ContextAssignment {
            name: "Review Needed".to_string(),
            kind: "Review".to_string(),
            company: None,
        }),
        _ => None,
    }
}

fn infer_context_from_text(
    title: &str,
    haystack: &str,
    contexts: &[ContextCatalogItem],
) -> Option<ContextAssignment> {
    let title = normalized_match_text(title);
    let haystack = normalized_match_text(haystack);
    let companies = contexts
        .iter()
        .filter(|context| context.kind.eq_ignore_ascii_case("company"));

    for context in companies.clone() {
        if context_aliases(&context.name)
            .iter()
            .any(|alias| text_contains_alias(&title, alias))
        {
            return Some(context_assignment(context));
        }
    }
    for context in companies {
        if !allow_body_context_match(&context.name) {
            continue;
        }
        if context_aliases(&context.name)
            .iter()
            .any(|alias| text_contains_alias(&haystack, alias))
        {
            return Some(context_assignment(context));
        }
    }

    if text_contains_alias(&title, "datapipe") || text_contains_alias(&haystack, "datapipe") {
        return Some(ContextAssignment {
            name: "Datapipe Internal".to_string(),
            kind: "Bucket".to_string(),
            company: None,
        });
    }
    if text_contains_alias(&title, "investor") {
        return Some(ContextAssignment {
            name: "Investor Calls".to_string(),
            kind: "Bucket".to_string(),
            company: None,
        });
    }
    if text_contains_alias(&title, "research") {
        return Some(ContextAssignment {
            name: "Research".to_string(),
            kind: "Bucket".to_string(),
            company: None,
        });
    }
    None
}

fn allow_body_context_match(name: &str) -> bool {
    !matches!(normalized_match_text(name).trim(), "circle")
}

fn find_context<'a>(
    contexts: &'a [ContextCatalogItem],
    name: &str,
) -> Option<&'a ContextCatalogItem> {
    contexts
        .iter()
        .find(|context| context.name.eq_ignore_ascii_case(name.trim()))
}

fn context_assignment(context: &ContextCatalogItem) -> ContextAssignment {
    ContextAssignment {
        name: context.name.clone(),
        kind: context.kind.clone(),
        company: context
            .kind
            .eq_ignore_ascii_case("company")
            .then_some(context.name.clone()),
    }
}

fn context_aliases(name: &str) -> Vec<String> {
    let base = normalized_match_text(name);
    let mut aliases = vec![base.clone()];
    let tokens = base.split_whitespace().collect::<Vec<_>>();
    if tokens.len() > 1 {
        let first = tokens[0];
        if first.len() >= 4 && !matches!(first, "scale" | "circle") {
            aliases.push(first.to_string());
        }
        aliases.push(tokens[..tokens.len().min(2)].join(" "));
    }
    aliases.sort();
    aliases.dedup();
    aliases
}

fn normalized_match_text(value: &str) -> String {
    let mut out = String::with_capacity(value.len() + 2);
    out.push(' ');
    let mut previous_space = false;
    for ch in value.chars().flat_map(char::to_lowercase) {
        if ch.is_ascii_alphanumeric() {
            out.push(ch);
            previous_space = false;
        } else if !previous_space {
            out.push(' ');
            previous_space = true;
        }
    }
    if !out.ends_with(' ') {
        out.push(' ');
    }
    out
}

fn text_contains_alias(text: &str, alias: &str) -> bool {
    !alias.trim().is_empty() && text.contains(&format!(" {} ", alias.trim()))
}

fn merge_archive_metadata(
    metadata: &mut MeetingMetadata,
    record: &ArchiveRecord,
    imported_at: &str,
) {
    if metadata
        .title
        .as_deref()
        .is_none_or(|title| is_placeholder_title(title))
    {
        metadata.title = clean_optional(record.title.clone());
    }
    if metadata.company.is_none() {
        metadata.company = record.company.clone();
    }
    if metadata.context.is_none() {
        metadata.context = record.context.clone();
    }
    if metadata.context_kind.is_none() {
        metadata.context_kind = record.context_kind.clone();
    }
    if metadata.people.is_empty() {
        metadata.people = record.people.clone();
    }
    metadata.source = Some(record.source.clone());
    metadata.original_id = Some(record.id.clone());
    metadata.notion_page_id = record.notion_page_id.clone();
    metadata.notion_url = record.notion_url.clone();
    metadata.imported_from = Some(record.imported_from.clone());
    metadata.imported_at = Some(imported_at.to_string());
    metadata.updated_at = Some(imported_at.to_string());
}

fn archive_page_summary(page: &str) -> Option<String> {
    extract_tag(page, "summary")
        .or_else(|| extract_heading_section(page, "Notes"))
        .or_else(|| extract_heading_section(page, "Summary"))
        .map(|text| format!("# Summary\n\n{}\n", cleanup_archive_text(&text)))
        .filter(|text| word_count(text) > 2)
}

fn archive_page_transcript(page: &str) -> String {
    let transcript = extract_tag(page, "transcript")
        .or_else(|| extract_heading_section(page, "Transcript"))
        .unwrap_or_else(|| strip_frontmatter(page).to_string());
    let transcript = cleanup_archive_text(&transcript);
    if transcript.trim_start().starts_with("# Transcript") {
        format!("{}\n", transcript.trim())
    } else {
        format!("# Transcript\n\n{}\n", transcript.trim())
    }
}

fn extract_tag(page: &str, tag: &str) -> Option<String> {
    let open = format!("<{tag}>");
    let close = format!("</{tag}>");
    let start = page.find(&open)? + open.len();
    let end = page[start..].find(&close)? + start;
    clean_optional(page[start..end].to_string())
}

fn extract_heading_section(page: &str, heading: &str) -> Option<String> {
    let body = strip_frontmatter(page);
    let mut in_section = false;
    let mut lines = Vec::new();
    for line in body.lines() {
        let trimmed = line.trim();
        let is_heading = trimmed.starts_with('#');
        let heading_text = trimmed.trim_start_matches('#').trim();
        if is_heading && heading_text.eq_ignore_ascii_case(heading) {
            in_section = true;
            continue;
        }
        if in_section && is_heading {
            break;
        }
        if in_section {
            lines.push(line);
        }
    }
    clean_optional(lines.join("\n"))
}

fn strip_frontmatter(page: &str) -> &str {
    if !page.starts_with("---\n") {
        return page;
    }
    let Some(end) = page[4..].find("\n---") else {
        return page;
    };
    &page[end + 8..]
}

fn cleanup_archive_text(text: &str) -> String {
    text.lines()
        .map(str::trim_end)
        .filter(|line| {
            let trimmed = line.trim();
            !(trimmed.starts_with('<') && trimmed.ends_with('>'))
        })
        .collect::<Vec<_>>()
        .join("\n")
        .replace("<empty-block/>", "")
        .replace("<meeting-notes>", "")
        .replace("</meeting-notes>", "")
        .trim()
        .to_string()
}

fn notion_prop_text(properties: &Value, name: &str) -> String {
    let Some(prop) = properties.get(name) else {
        return String::new();
    };
    match prop.get("type").and_then(Value::as_str) {
        Some("title") => prop
            .get("title")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(|item| item.get("plain_text").and_then(Value::as_str))
            .collect::<String>(),
        Some("rich_text") => prop
            .get("rich_text")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(|item| item.get("plain_text").and_then(Value::as_str))
            .collect::<String>(),
        Some("select") => prop
            .get("select")
            .and_then(|select| select.get("name"))
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string(),
        Some("date") => prop
            .get("date")
            .and_then(|date| date.get("start"))
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string(),
        _ => String::new(),
    }
}

fn relation_id(properties: &Value, name: &str) -> Option<String> {
    properties
        .get(name)
        .and_then(|prop| prop.get("relation"))
        .and_then(Value::as_array)
        .and_then(|relations| relations.first())
        .and_then(|relation| relation.get("id"))
        .and_then(Value::as_str)
        .map(ToString::to_string)
}

fn notion_multi_select(properties: &Value, name: &str) -> Vec<String> {
    properties
        .get(name)
        .and_then(|prop| prop.get("multi_select"))
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|item| item.get("name").and_then(Value::as_str))
        .map(ToString::to_string)
        .collect()
}

fn read_json_file<T: for<'de> Deserialize<'de>>(path: &Path) -> Result<T, String> {
    let body = std::fs::read_to_string(path)
        .map_err(|e| format!("failed reading json {}: {e}", path.display()))?;
    serde_json::from_str(&body).map_err(|e| format!("failed parsing json {}: {e}", path.display()))
}

fn export_folder_name(meeting: &MeetingListItem) -> String {
    let date = meeting
        .started_at
        .as_deref()
        .and_then(|value| value.split('T').next())
        .unwrap_or("undated");
    let title = slug(&meeting.title);
    format!("{date}-{title}-{}", &meeting.id[..meeting.id.len().min(8)])
}

fn unique_deleted_path(deleted_root: &Path, id: &str) -> PathBuf {
    let base = format!("{}-{}", Utc::now().format("%Y%m%d-%H%M%S"), slug(id));
    let mut candidate = deleted_root.join(&base);
    let mut suffix = 1;
    while candidate.exists() {
        candidate = deleted_root.join(format!("{base}-{suffix}"));
        suffix += 1;
    }
    candidate
}

fn slug(value: &str) -> String {
    let mut out = String::new();
    let mut previous_dash = false;
    for ch in value.chars().flat_map(char::to_lowercase) {
        if ch.is_ascii_alphanumeric() {
            out.push(ch);
            previous_dash = false;
        } else if !previous_dash {
            out.push('-');
            previous_dash = true;
        }
        if out.len() >= 72 {
            break;
        }
    }
    let out = out.trim_matches('-').to_string();
    if out.is_empty() {
        "meeting".to_string()
    } else {
        out
    }
}

fn word_count(text: &str) -> usize {
    text.split_whitespace()
        .filter(|word| !word.is_empty())
        .count()
}

fn is_placeholder_title(title: &str) -> bool {
    let title = title.trim().to_lowercase();
    title.is_empty() || title.starts_with("meeting ") || title.starts_with("poha recording ")
}

fn context_index_rows(rows: &[MeetingIndexRow]) -> Vec<ContextIndexRow> {
    let mut groups: BTreeMap<String, ContextIndexRow> = BTreeMap::new();
    for row in rows {
        let company = row
            .company
            .clone()
            .unwrap_or_else(|| INDIVIDUAL_NETWORKING_COMPANY.to_string());
        add_context_index_row(
            &mut groups,
            company.clone(),
            "Company".to_string(),
            Some(company),
            row,
        );
        if enrichment_status_for(row).needs_enrichment {
            add_context_index_row(
                &mut groups,
                REVIEW_NEEDED_FILTER.to_string(),
                "Review".to_string(),
                None,
                row,
            );
        }
    }

    groups.into_values().collect()
}

fn add_context_index_row(
    groups: &mut BTreeMap<String, ContextIndexRow>,
    name: String,
    kind: String,
    company: Option<String>,
    row: &MeetingIndexRow,
) {
    let entry = groups
        .entry(name.clone())
        .or_insert_with(|| ContextIndexRow {
            name,
            kind,
            company,
            meeting_count: 0,
            transcript_count: 0,
            latest_started_at: None,
            sample_titles: Vec::new(),
            search_text: String::new(),
        });
    entry.meeting_count += 1;
    entry.transcript_count += usize::from(row.has_transcript);
    if row.started_at.as_ref().is_some_and(|started| {
        entry
            .latest_started_at
            .as_ref()
            .is_none_or(|latest| started > latest)
    }) {
        entry.latest_started_at = row.started_at.clone();
    }
    if entry.sample_titles.len() < 4 {
        entry.sample_titles.push(row.title.clone());
    }
    entry.search_text.push_str(&row.search_text);
    entry.search_text.push('\n');
}

pub fn rebuild_index(recordings_dir: &Path) -> Result<IndexSummary, String> {
    let conn = open_index(recordings_dir)?;
    let mut rows = index_rows(recordings_dir)?;
    refresh_summary_input_states(&conn, &mut rows)?;
    let context_rows = context_index_rows(&rows);
    let tx = conn
        .unchecked_transaction()
        .map_err_sql("begin meeting index rebuild")?;
    tx.execute("DELETE FROM meetings", [])
        .map_err_sql("clear meeting index")?;
    tx.execute("DELETE FROM contexts", [])
        .map_err_sql("clear context index")?;

    for row in &rows {
        tx.execute(
            r#"
            INSERT INTO meetings (
                id, title, status, started_at, ended_at, company, context, context_kind, source, people_json, speakers_json, speaker_map_json,
                has_summary, has_transcript, summary_stale, session_dir, summary_path, transcript_path,
                transcript_json_path, meeting_metadata_path, session_metadata_path,
                updated_at, snippet, search_text, session_mtime_ms, meeting_mtime_ms
            )
            VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18, ?19, ?20, ?21, ?22, ?23, ?24, ?25, ?26)
            "#,
            params![
                row.id,
                row.title,
                row.status,
                row.started_at,
                row.ended_at,
                row.company,
                row.context,
                row.context_kind,
                row.source,
                json_string(&row.people)?,
                json_string(&row.speakers)?,
                json_string(&row.speaker_map)?,
                row.has_summary as i64,
                row.has_transcript as i64,
                row.summary_stale as i64,
                path_string(&row.session_dir),
                row.summary_path.as_ref().map(|path| path_string(path)),
                row.transcript_path.as_ref().map(|path| path_string(path)),
                row.transcript_json_path.as_ref().map(|path| path_string(path)),
                path_string(&row.meeting_metadata_path),
                path_string(&row.session_metadata_path),
                row.updated_at,
                row.snippet,
                row.search_text,
                row.session_mtime_ms,
                row.meeting_mtime_ms,
            ],
        )
        .map_err_sql("insert meeting index row")?;
    }

    for row in &context_rows {
        tx.execute(
            r#"
            INSERT INTO contexts (
                name, kind, company, meeting_count, transcript_count, latest_started_at, sample_titles_json, search_text
            )
            VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
            "#,
            params![
                row.name,
                row.kind,
                row.company,
                row.meeting_count as i64,
                row.transcript_count as i64,
                row.latest_started_at,
                json_string(&row.sample_titles)?,
                row.search_text,
            ],
        )
        .map_err_sql("insert context index row")?;
    }

    tx.commit().map_err_sql("commit meeting index rebuild")?;
    Ok(IndexSummary {
        path: path_string(&index_path(recordings_dir)),
        meetings_indexed: rows.len(),
        contexts_indexed: context_rows.len(),
        rebuilt_at: Utc::now().to_rfc3339(),
    })
}

fn ensure_index(recordings_dir: &Path) -> Result<(), String> {
    if !index_path(recordings_dir).exists() {
        rebuild_index(recordings_dir).map(|_| ())
    } else {
        let conn = open_index(recordings_dir)?;
        if recording_session_count(recordings_dir)? > indexed_meeting_count(&conn)? {
            rebuild_index(recordings_dir).map(|_| ())
        } else {
            Ok(())
        }
    }
}

fn open_index(recordings_dir: &Path) -> Result<Connection, String> {
    let path = index_path(recordings_dir);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| format!("failed creating index dir {}: {e}", parent.display()))?;
    }

    match open_index_once(&path) {
        Ok(conn) => Ok(conn),
        Err(first_error) => {
            if path.exists() {
                let backup = path.with_extension(format!(
                    "sqlite.corrupt-{}",
                    Utc::now().format("%Y%m%d%H%M%S")
                ));
                std::fs::rename(&path, &backup).map_err(|e| {
                    format!(
                        "failed moving corrupt meeting index {} -> {} after {first_error}: {e}",
                        path.display(),
                        backup.display()
                    )
                })?;
            }
            open_index_once(&path).map_err(|second_error| {
                format!(
                    "failed opening meeting index {} after rebuild attempt: {second_error}",
                    path.display()
                )
            })
        }
    }
}

fn open_index_read_only(recordings_dir: &Path) -> Result<Option<Connection>, String> {
    let path = index_path(recordings_dir);
    if !path.exists() {
        return Ok(None);
    }
    Connection::open_with_flags(&path, OpenFlags::SQLITE_OPEN_READ_ONLY)
        .map(Some)
        .map_err(|e| {
            format!(
                "failed opening meeting index {} read-only: {e}",
                path.display()
            )
        })
}

fn open_index_once(path: &Path) -> rusqlite::Result<Connection> {
    let conn = Connection::open(path)?;
    init_schema(&conn)?;
    Ok(conn)
}

fn init_schema(conn: &Connection) -> rusqlite::Result<()> {
    let user_version: i64 = conn.query_row("PRAGMA user_version", [], |row| row.get(0))?;
    if user_version > 0 && user_version < 3 {
        conn.execute_batch("DROP TABLE IF EXISTS meetings; DROP TABLE IF EXISTS contexts;")?;
    }
    conn.execute_batch(
        r#"
        PRAGMA journal_mode = WAL;
        CREATE TABLE IF NOT EXISTS meetings (
            id TEXT PRIMARY KEY,
            title TEXT NOT NULL,
            status TEXT,
            started_at TEXT,
            ended_at TEXT,
            company TEXT,
            context TEXT,
            context_kind TEXT,
            source TEXT,
            people_json TEXT NOT NULL,
            speakers_json TEXT NOT NULL,
            speaker_map_json TEXT NOT NULL,
            has_summary INTEGER NOT NULL,
            has_transcript INTEGER NOT NULL,
            summary_stale INTEGER NOT NULL DEFAULT 0,
            session_dir TEXT NOT NULL,
            summary_path TEXT,
            transcript_path TEXT,
            transcript_json_path TEXT,
            meeting_metadata_path TEXT NOT NULL,
            session_metadata_path TEXT NOT NULL,
            updated_at TEXT,
            snippet TEXT,
            search_text TEXT NOT NULL,
            session_mtime_ms INTEGER NOT NULL,
            meeting_mtime_ms INTEGER NOT NULL
        );
        CREATE INDEX IF NOT EXISTS idx_meetings_started_at ON meetings(started_at);
        CREATE INDEX IF NOT EXISTS idx_meetings_company ON meetings(company);
        CREATE INDEX IF NOT EXISTS idx_meetings_context ON meetings(context);
        CREATE TABLE IF NOT EXISTS contexts (
            name TEXT PRIMARY KEY,
            kind TEXT NOT NULL,
            company TEXT,
            meeting_count INTEGER NOT NULL,
            transcript_count INTEGER NOT NULL,
            latest_started_at TEXT,
            sample_titles_json TEXT NOT NULL,
            search_text TEXT NOT NULL
        );
        CREATE INDEX IF NOT EXISTS idx_contexts_kind ON contexts(kind);
        CREATE INDEX IF NOT EXISTS idx_contexts_latest_started_at ON contexts(latest_started_at);
        CREATE TABLE IF NOT EXISTS summary_inputs (
            id TEXT PRIMARY KEY,
            input_hash TEXT NOT NULL,
            summary_mtime_ms INTEGER NOT NULL,
            updated_at TEXT NOT NULL
        );
        PRAGMA user_version = 5;
        "#,
    )?;
    if !meeting_table_has_speaker_map(conn)? {
        conn.execute_batch(
            "ALTER TABLE meetings ADD COLUMN speaker_map_json TEXT NOT NULL DEFAULT '{}';",
        )?;
    }
    if !meeting_table_has_column(conn, "summary_stale")? {
        conn.execute_batch(
            "ALTER TABLE meetings ADD COLUMN summary_stale INTEGER NOT NULL DEFAULT 0;",
        )?;
    }
    conn.execute_batch("PRAGMA user_version = 5;")
}

fn meeting_table_has_speaker_map(conn: &Connection) -> rusqlite::Result<bool> {
    meeting_table_has_column(conn, "speaker_map_json")
}

fn meeting_table_has_column(conn: &Connection, name: &str) -> rusqlite::Result<bool> {
    let mut stmt = conn.prepare("PRAGMA table_info(meetings)")?;
    let columns = stmt.query_map([], |row| row.get::<_, String>(1))?;
    for column in columns {
        if column? == name {
            return Ok(true);
        }
    }
    Ok(false)
}

fn recording_session_count(recordings_dir: &Path) -> Result<usize, String> {
    let mut count = 0;
    for entry in std::fs::read_dir(recordings_dir).map_err(|e| {
        format!(
            "failed reading recordings dir {}: {e}",
            recordings_dir.display()
        )
    })? {
        let entry = entry.map_err(|e| format!("failed reading recordings entry: {e}"))?;
        let session_dir = entry.path();
        if session_dir.is_dir() && session_dir.join("session.json").exists() {
            count += 1;
        }
    }
    Ok(count)
}

fn indexed_meeting_count(conn: &Connection) -> Result<usize, String> {
    conn.query_row("SELECT COUNT(*) FROM meetings", [], |row| {
        row.get::<_, i64>(0)
    })
    .map(|count| count as usize)
    .map_err_sql("count indexed meetings")
}

fn index_rows(recordings_dir: &Path) -> Result<Vec<MeetingIndexRow>, String> {
    let mut rows = Vec::new();
    for entry in std::fs::read_dir(recordings_dir).map_err(|e| {
        format!(
            "failed reading recordings dir {}: {e}",
            recordings_dir.display()
        )
    })? {
        let entry = entry.map_err(|e| format!("failed reading recordings entry: {e}"))?;
        let session_dir = entry.path();
        if !session_dir.is_dir() || !session_dir.join("session.json").exists() {
            continue;
        }
        match build_index_row(&session_dir) {
            Ok(row) => rows.push(row),
            Err(error) => tracing::warn!("skipping meeting {}: {error}", session_dir.display()),
        }
    }
    Ok(rows)
}

fn build_index_row(session_dir: &Path) -> Result<MeetingIndexRow, String> {
    let session_metadata_path = session_dir.join("session.json");
    let session_metadata = read_json_value(&session_metadata_path)?;
    let id = value_string(&session_metadata, "id").unwrap_or_else(|| {
        session_dir
            .file_name_string()
            .unwrap_or_else(|| "meeting".to_string())
    });
    let meeting_metadata = read_meeting_metadata(session_dir, &id)?;
    let summary_path = session_dir.join("summary.md");
    let meeting_metadata_path = session_dir.join(METADATA_FILE_NAME);
    let transcript_path = transcript_markdown_path(&session_metadata, session_dir);
    let transcript_json_path =
        transcript_json_path(&session_metadata, session_dir, &transcript_path);
    let summary_text = read_optional_string(&summary_path)?;
    let (speakers, transcript_text) =
        read_transcript_index_text(&transcript_json_path, &transcript_path)?;
    let has_transcript = transcript_path.as_ref().is_some_and(|path| path.exists())
        && has_usable_transcript(transcript_text.as_deref());
    let title = meeting_metadata
        .title
        .clone()
        .or_else(|| title_from_summary(summary_text.as_deref()))
        .unwrap_or_else(|| default_title(value_string(&session_metadata, "started_at"), &id));
    let company = Some(company_or_catch_all(meeting_metadata.company.clone()));
    let context = company.clone();
    let context_kind = Some("Company".to_string());
    let source = meeting_metadata.source.clone();
    let people = meeting_metadata.people.clone();
    let all_speakers = speaker_candidates_from_labels(speakers, &meeting_metadata.speaker_map);
    let snippet = summary_text
        .as_deref()
        .or(transcript_text
            .as_deref()
            .filter(|text| has_usable_transcript(Some(text))))
        .and_then(make_snippet);
    let search_text = [
        id.as_str(),
        title.as_str(),
        company.as_deref().unwrap_or_default(),
        context.as_deref().unwrap_or_default(),
        context_kind.as_deref().unwrap_or_default(),
        source.as_deref().unwrap_or_default(),
        &people.join(" "),
        &all_speakers.join(" "),
        summary_text.as_deref().unwrap_or_default(),
        transcript_text.as_deref().unwrap_or_default(),
    ]
    .join("\n");
    let has_summary = summary_path.exists();

    Ok(MeetingIndexRow {
        id,
        title,
        status: value_string(&session_metadata, "status"),
        started_at: value_string(&session_metadata, "started_at"),
        ended_at: value_string(&session_metadata, "ended_at"),
        company,
        context,
        context_kind,
        source,
        people,
        speaker_map: meeting_metadata.speaker_map.clone(),
        speakers: all_speakers,
        has_summary,
        has_transcript,
        session_dir: session_dir.to_path_buf(),
        summary_path: summary_path.exists().then_some(summary_path),
        transcript_path,
        transcript_json_path,
        meeting_metadata_path: meeting_metadata_path.clone(),
        session_metadata_path: session_metadata_path.clone(),
        summary_stale: false,
        updated_at: meeting_metadata.updated_at,
        snippet,
        search_text,
        session_mtime_ms: mtime_ms(&session_metadata_path),
        meeting_mtime_ms: mtime_ms(&meeting_metadata_path),
    })
}

fn build_meeting_detail(
    recordings_dir: &Path,
    session_dir: &Path,
) -> Result<MeetingDetail, String> {
    let mut row = build_index_row(session_dir)?;
    let conn = open_index(recordings_dir)?;
    apply_summary_input_state(&conn, &mut row, false)?;
    let metadata = read_meeting_metadata(session_dir, &row.id)?;
    let session_metadata = read_json_value(&row.session_metadata_path)?;
    let transcript_markdown = mapped_transcript_markdown(&row, &metadata)?;
    let summary_markdown = row
        .summary_path
        .as_ref()
        .map(|path| read_optional_string(path))
        .transpose()?
        .flatten();
    let speaker_candidates =
        speaker_candidates_from_labels(row.speakers.clone(), &metadata.speaker_map);

    Ok(MeetingDetail {
        item: row.to_list_item(),
        metadata,
        session_metadata,
        summary_markdown,
        transcript_markdown,
        speaker_candidates,
        paths: MeetingPaths {
            session_dir: path_string(session_dir),
            index_path: path_string(&index_path(recordings_dir)),
            session_json: path_string(&row.session_metadata_path),
            meeting_json: path_string(&row.meeting_metadata_path),
            summary_markdown: path_string(&session_dir.join("summary.md")),
            transcript_markdown: row.transcript_path.as_ref().map(|path| path_string(path)),
            transcript_json: row
                .transcript_json_path
                .as_ref()
                .map(|path| path_string(path)),
        },
    })
}

fn mapped_transcript_markdown(
    row: &MeetingIndexRow,
    metadata: &MeetingMetadata,
) -> Result<Option<String>, String> {
    if let Some(path) = &row.transcript_json_path
        && path.exists()
    {
        let body = std::fs::read_to_string(path)
            .map_err(|e| format!("failed reading transcript json {}: {e}", path.display()))?;
        match serde_json::from_str::<TranscriptDocument>(&body) {
            Ok(mut document) => {
                apply_speaker_map_to_segments(&mut document.segments, &metadata.speaker_map);
                return Ok(Some(crate::transcription::render_markdown(
                    &document.segments,
                )));
            }
            Err(error) => {
                tracing::warn!(
                    "falling back to transcript markdown for {} after transcript json parse failed: {error}",
                    path.display()
                );
            }
        }
    }

    row.transcript_path
        .as_ref()
        .map(|path| read_optional_string(path))
        .transpose()
        .map(Option::flatten)
}

fn apply_speaker_map_to_segments(
    segments: &mut [TranscriptSegment],
    speaker_map: &BTreeMap<String, String>,
) {
    for segment in segments {
        let mapped = mapped_speaker_for_segment(segment, speaker_map);
        if let Some(speaker) = mapped {
            segment.speaker = speaker;
        }
    }
}

fn mapped_speaker_for_segment(
    segment: &TranscriptSegment,
    speaker_map: &BTreeMap<String, String>,
) -> Option<String> {
    segment
        .speaker_id
        .as_ref()
        .and_then(|speaker_id| speaker_map.get(speaker_id))
        .or_else(|| speaker_map.get(&segment.speaker))
        .or_else(|| speaker_label_key(&segment.speaker).and_then(|key| speaker_map.get(&key)))
        .cloned()
}

fn read_meeting_metadata(session_dir: &Path, id: &str) -> Result<MeetingMetadata, String> {
    let path = session_dir.join(METADATA_FILE_NAME);
    let mut metadata = if path.exists() {
        let body = std::fs::read_to_string(&path)
            .map_err(|e| format!("failed reading meeting metadata {}: {e}", path.display()))?;
        serde_json::from_str::<MeetingMetadata>(&body)
            .map_err(|e| format!("failed parsing meeting metadata {}: {e}", path.display()))?
    } else {
        MeetingMetadata::for_id(id)
    };

    if metadata.id.trim().is_empty() {
        metadata.id = id.to_string();
    }
    metadata.title = metadata.title.and_then(clean_optional);
    metadata.company = metadata.company.and_then(clean_optional);
    metadata.context = metadata.context.and_then(clean_optional);
    metadata.context_kind = metadata.context_kind.and_then(clean_optional);
    metadata.people = normalize_people(metadata.people);
    metadata.speaker_map = normalize_speaker_map(metadata.speaker_map);
    metadata.source = metadata.source.and_then(clean_optional);
    metadata.original_id = metadata.original_id.and_then(clean_optional);
    metadata.notion_page_id = metadata.notion_page_id.and_then(clean_optional);
    metadata.notion_url = metadata.notion_url.and_then(clean_optional);
    metadata.imported_from = metadata.imported_from.and_then(clean_optional);
    metadata.imported_at = metadata.imported_at.and_then(clean_optional);
    if metadata.speaker_status.trim().is_empty() {
        metadata.speaker_status = default_speaker_status();
    }
    Ok(metadata)
}

fn write_meeting_metadata(session_dir: &Path, metadata: &MeetingMetadata) -> Result<(), String> {
    let path = session_dir.join(METADATA_FILE_NAME);
    let data = serde_json::to_string_pretty(metadata)
        .map_err(|e| format!("failed serializing meeting metadata: {e}"))?;
    std::fs::write(&path, data)
        .map_err(|e| format!("failed writing meeting metadata {}: {e}", path.display()))
}

fn session_dir(recordings_dir: &Path, id: &str) -> Result<PathBuf, String> {
    let dir = recordings_dir.join(id);
    if dir.is_dir() {
        Ok(dir)
    } else {
        Err(format!("meeting not found: {id}"))
    }
}

fn read_transcript_index_text(
    transcript_json_path: &Option<PathBuf>,
    transcript_markdown_path: &Option<PathBuf>,
) -> Result<(Vec<String>, Option<String>), String> {
    if let Some(path) = transcript_json_path
        && path.exists()
    {
        let body = std::fs::read_to_string(path)
            .map_err(|e| format!("failed reading transcript json {}: {e}", path.display()))?;
        if let Ok(document) = serde_json::from_str::<TranscriptDocument>(&body) {
            let mut speakers = Vec::new();
            let mut text = Vec::new();
            for segment in document.segments {
                if let Some(speaker_id) = segment.speaker_id {
                    speakers.push(speaker_id);
                }
                speakers.push(segment.speaker);
                text.push(segment.text);
            }
            if !text.is_empty() {
                return Ok((normalize_people(speakers), Some(text.join("\n"))));
            }
        }
    }

    let markdown = transcript_markdown_path
        .as_ref()
        .map(|path| read_optional_string(path))
        .transpose()?
        .flatten();
    Ok((Vec::new(), markdown))
}

fn transcript_markdown_path(session_metadata: &Value, session_dir: &Path) -> Option<PathBuf> {
    value_string(session_metadata, "transcript_path")
        .or_else(|| transcription_string(session_metadata, "transcript_markdown_path"))
        .map(PathBuf::from)
        .filter(|path| path.exists())
        .or_else(|| {
            let path = session_dir.join("transcript.md");
            path.exists().then_some(path)
        })
}

fn transcript_json_path(
    session_metadata: &Value,
    session_dir: &Path,
    transcript_markdown_path: &Option<PathBuf>,
) -> Option<PathBuf> {
    transcription_string(session_metadata, "transcript_json_path")
        .map(PathBuf::from)
        .filter(|path| path.exists())
        .or_else(|| {
            transcript_markdown_path
                .as_ref()
                .map(|path| path.with_extension("json"))
                .filter(|path| path.exists())
        })
        .or_else(|| {
            let path = session_dir.join("transcript.json");
            path.exists().then_some(path)
        })
}

fn transcription_string(session_metadata: &Value, key: &str) -> Option<String> {
    session_metadata
        .get("transcription")
        .and_then(|value| value_string(value, key))
}

fn value_string(value: &Value, key: &str) -> Option<String> {
    value_string_exact(value, key)
        .or_else(|| metadata_key_alias(key).and_then(|alias| value_string_exact(value, &alias)))
}

fn value_string_exact(value: &Value, key: &str) -> Option<String> {
    value
        .get(key)
        .and_then(Value::as_str)
        .map(ToString::to_string)
}

fn metadata_key_alias(key: &str) -> Option<String> {
    if key.contains('_') {
        let mut alias = String::with_capacity(key.len());
        let mut uppercase_next = false;
        for ch in key.chars() {
            if ch == '_' {
                uppercase_next = true;
            } else if uppercase_next {
                alias.extend(ch.to_uppercase());
                uppercase_next = false;
            } else {
                alias.push(ch);
            }
        }
        return (alias != key).then_some(alias);
    }

    let mut alias = String::with_capacity(key.len() + 4);
    for ch in key.chars() {
        if ch.is_ascii_uppercase() {
            alias.push('_');
            alias.push(ch.to_ascii_lowercase());
        } else {
            alias.push(ch);
        }
    }
    (alias != key).then_some(alias)
}

fn read_json_value(path: &Path) -> Result<Value, String> {
    let body = std::fs::read_to_string(path)
        .map_err(|e| format!("failed reading json {}: {e}", path.display()))?;
    serde_json::from_str(&body).map_err(|e| format!("failed parsing json {}: {e}", path.display()))
}

fn read_optional_string(path: &Path) -> Result<Option<String>, String> {
    if !path.exists() {
        return Ok(None);
    }
    std::fs::read_to_string(path)
        .map(Some)
        .map_err(|e| format!("failed reading {}: {e}", path.display()))
}

fn title_from_summary(summary: Option<&str>) -> Option<String> {
    summary?
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty() && !line.starts_with('#'))
        .and_then(|line| clean_optional(line.trim_start_matches("- ").to_string()))
        .map(|line| line.chars().take(80).collect())
}

fn default_title(started_at: Option<String>, id: &str) -> String {
    let date = started_at
        .as_deref()
        .and_then(|value| value.split('T').next())
        .filter(|value| !value.is_empty());
    match date {
        Some(date) => format!("Meeting {date}"),
        None => format!("Meeting {}", &id[..id.len().min(8)]),
    }
}

fn make_snippet(text: &str) -> Option<String> {
    let cleaned = text
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty() && !line.starts_with('#'))
        .collect::<Vec<_>>()
        .join(" ");
    if cleaned.is_empty() {
        return None;
    }
    Some(cleaned.chars().take(220).collect())
}

fn has_usable_transcript(text: Option<&str>) -> bool {
    let cleaned = text
        .unwrap_or_default()
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty() && !line.starts_with('#'))
        .collect::<Vec<_>>()
        .join(" ");
    let normalized = cleaned.to_ascii_lowercase();

    !normalized.is_empty()
        && normalized != "no transcript produced."
        && normalized != "no transcript available."
        && normalized != "no transcript yet."
}

fn normalize_people(values: Vec<String>) -> Vec<String> {
    let mut seen = BTreeSet::new();
    let mut normalized = Vec::new();
    for value in values {
        let value = value.trim();
        if value.is_empty() {
            continue;
        }
        let key = value.to_lowercase();
        if seen.insert(key) {
            normalized.push(value.to_string());
        }
    }
    normalized
}

fn mapped_speaker_names(speaker_map: &BTreeMap<String, String>) -> Vec<String> {
    normalize_people(speaker_map.values().cloned().collect())
}

fn speaker_candidates_from_labels(
    labels: Vec<String>,
    speaker_map: &BTreeMap<String, String>,
) -> Vec<String> {
    let mapped_values = speaker_map
        .values()
        .map(|value| value.trim().to_lowercase())
        .collect::<BTreeSet<_>>();
    let mut seen = BTreeSet::new();
    let mut candidates = Vec::new();

    for label in labels.into_iter().chain(speaker_map.keys().cloned()) {
        if label.trim().to_lowercase().starts_with("spk_") {
            continue;
        }
        let Some(key) = speaker_label_key(&label) else {
            continue;
        };
        if mapped_values.contains(&key.to_lowercase()) {
            continue;
        }
        if seen.insert(speaker_sort_key(&key)) {
            candidates.push(key);
        }
    }

    candidates.sort_by_key(|candidate| speaker_sort_key(candidate));
    candidates
}

fn enrichment_status_for(row: &MeetingIndexRow) -> MeetingEnrichmentStatus {
    enrichment_status_from_fields(
        &row.title,
        row.has_summary,
        row.summary_stale,
        row.has_transcript,
        &row.speakers,
        &row.speaker_map,
        row.context.as_deref(),
        row.context_kind.as_deref(),
    )
}

fn enrichment_status_from_fields(
    title: &str,
    has_summary: bool,
    summary_stale: bool,
    has_transcript: bool,
    speakers: &[String],
    speaker_map: &BTreeMap<String, String>,
    _context: Option<&str>,
    _context_kind: Option<&str>,
) -> MeetingEnrichmentStatus {
    let needs_summary = has_transcript && (!has_summary || summary_stale);
    let needs_title = has_transcript && is_low_signal_title(title);
    let needs_speakers = has_transcript
        && speakers
            .iter()
            .any(|speaker| is_unmapped_numbered_speaker(speaker, speaker_map));
    let needs_context = false;

    let mut reasons = Vec::new();
    if needs_summary {
        if has_summary && summary_stale {
            reasons.push("stale_summary".to_string());
        } else {
            reasons.push("missing_summary".to_string());
        }
    }
    if needs_title {
        reasons.push("generic_title".to_string());
    }
    if needs_speakers {
        reasons.push("unmapped_speakers".to_string());
    }
    if needs_context {
        reasons.push("missing_context".to_string());
    }

    let needs_enrichment = !reasons.is_empty();
    MeetingEnrichmentStatus {
        status: if needs_enrichment {
            "needs_enrichment".to_string()
        } else {
            "ready".to_string()
        },
        needs_enrichment,
        needs_summary,
        summary_stale,
        needs_title,
        needs_speakers,
        needs_context,
        reasons,
        summary_status: if needs_summary {
            if has_summary && summary_stale {
                "stale".to_string()
            } else {
                "missing".to_string()
            }
        } else if has_summary {
            "ready".to_string()
        } else {
            "unavailable".to_string()
        },
        title_status: if needs_title {
            "generic".to_string()
        } else {
            "ready".to_string()
        },
        speaker_status: if needs_speakers {
            "needs_review".to_string()
        } else {
            "ready".to_string()
        },
        context_status: if needs_context {
            "needs_review".to_string()
        } else {
            "ready".to_string()
        },
    }
}

fn is_unmapped_numbered_speaker(label: &str, speaker_map: &BTreeMap<String, String>) -> bool {
    let Some(key) = speaker_label_key(label) else {
        return false;
    };
    let label = key.to_lowercase();
    let is_numbered = speaker_number(&label).is_some()
        || label.starts_with("speaker ")
        || label.starts_with("speaker-")
        || label.starts_with("speaker_");
    is_numbered && !speaker_map.contains_key(&key)
}

fn speaker_label_key(label: &str) -> Option<String> {
    let label = label.trim();
    if label.is_empty() {
        return None;
    }
    let lower = label.to_lowercase();
    if matches!(lower.as_str(), "me" | "adi" | "myself") {
        return Some("Me".to_string());
    }
    if matches!(lower.as_str(), "call" | "remote" | "system") {
        return Some("Call".to_string());
    }
    if let Some(number) = speaker_number(&lower) {
        return Some(format!("Speaker {number}"));
    }
    Some(label.to_string())
}

fn speaker_number(value: &str) -> Option<u32> {
    if value.chars().all(|ch| ch.is_ascii_digit()) {
        return value.parse().ok();
    }
    value
        .strip_prefix("speaker ")
        .or_else(|| value.strip_prefix("speaker-"))
        .or_else(|| value.strip_prefix("speaker_"))
        .and_then(|number| number.trim().parse().ok())
}

fn speaker_sort_key(label: &str) -> (u8, u32, String) {
    match label {
        "Me" => (0, 0, label.to_string()),
        "Call" => (2, 0, label.to_string()),
        _ => speaker_number(&label.to_lowercase())
            .map(|number| (1, number, label.to_string()))
            .unwrap_or_else(|| (3, 0, label.to_string())),
    }
}

fn normalize_speaker_map(map: BTreeMap<String, String>) -> BTreeMap<String, String> {
    map.into_iter()
        .filter_map(|(key, value)| {
            let key = speaker_label_key(&key)?;
            let value = value.trim();
            if value.is_empty() {
                None
            } else {
                Some((key, value.to_string()))
            }
        })
        .collect()
}

fn clean_optional(value: String) -> Option<String> {
    let value = value.trim();
    if value.is_empty() {
        None
    } else {
        Some(value.to_string())
    }
}

fn company_or_catch_all(company: Option<String>) -> String {
    company.unwrap_or_else(|| INDIVIDUAL_NETWORKING_COMPANY.to_string())
}

fn normalize_company_grouping(metadata: &mut MeetingMetadata) {
    let company = company_or_catch_all(metadata.company.clone());
    metadata.company = Some(company.clone());
    metadata.context = Some(company);
    metadata.context_kind = Some("Company".to_string());
}

fn string_vec_from_json(value: &str) -> Vec<String> {
    serde_json::from_str::<Vec<String>>(value).unwrap_or_default()
}

fn string_map_from_json(value: &str) -> BTreeMap<String, String> {
    serde_json::from_str::<BTreeMap<String, String>>(value).unwrap_or_default()
}

fn json_string<T: Serialize>(value: &T) -> Result<String, String> {
    serde_json::to_string(value).map_err(|e| format!("failed serializing json: {e}"))
}

fn index_path(recordings_dir: &Path) -> PathBuf {
    recordings_dir.join(INDEX_DIR_NAME).join(INDEX_FILE_NAME)
}

fn path_string(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}

fn refresh_summary_input_states(
    conn: &Connection,
    rows: &mut [MeetingIndexRow],
) -> Result<(), String> {
    for row in rows {
        apply_summary_input_state(conn, row, true)?;
    }
    Ok(())
}

fn apply_summary_input_state(
    conn: &Connection,
    row: &mut MeetingIndexRow,
    update_baseline: bool,
) -> Result<(), String> {
    row.summary_stale = false;
    if !row.has_summary {
        return Ok(());
    }
    let Some(summary_path) = row.summary_path.as_ref() else {
        return Ok(());
    };
    let summary_mtime_ms = mtime_ms(summary_path);
    if summary_mtime_ms == 0 {
        return Ok(());
    }

    let input_hash = summary_input_hash(row)?;
    let state = summary_input_state(conn, &row.id)?;
    let stale = state.as_ref().is_some_and(|state| {
        state.input_hash != input_hash && summary_mtime_ms <= state.summary_mtime_ms
    });
    row.summary_stale = stale;

    if update_baseline && !stale {
        upsert_summary_input_state(conn, &row.id, &input_hash, summary_mtime_ms)?;
    }
    Ok(())
}

fn summary_input_state(conn: &Connection, id: &str) -> Result<Option<SummaryInputState>, String> {
    match conn.query_row(
        "SELECT input_hash, summary_mtime_ms FROM summary_inputs WHERE id = ?1",
        params![id],
        |row| {
            Ok(SummaryInputState {
                input_hash: row.get(0)?,
                summary_mtime_ms: row.get(1)?,
            })
        },
    ) {
        Ok(state) => Ok(Some(state)),
        Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
        Err(error) => Err(format!("read summary input state: {error}")),
    }
}

fn upsert_summary_input_state(
    conn: &Connection,
    id: &str,
    input_hash: &str,
    summary_mtime_ms: i64,
) -> Result<(), String> {
    conn.execute(
        r#"
        INSERT INTO summary_inputs (id, input_hash, summary_mtime_ms, updated_at)
        VALUES (?1, ?2, ?3, ?4)
        ON CONFLICT(id) DO UPDATE SET
            input_hash = excluded.input_hash,
            summary_mtime_ms = excluded.summary_mtime_ms,
            updated_at = excluded.updated_at
        "#,
        params![id, input_hash, summary_mtime_ms, Utc::now().to_rfc3339()],
    )
    .map(|_| ())
    .map_err_sql("upsert summary input state")
}

fn summary_input_hash(row: &MeetingIndexRow) -> Result<String, String> {
    let mut hash = FNV_OFFSET_BASIS;
    hash_path_content(&mut hash, "session", Some(&row.session_metadata_path))?;
    hash_path_content(&mut hash, "meeting", Some(&row.meeting_metadata_path))?;
    hash_path_content(&mut hash, "transcript_md", row.transcript_path.as_deref())?;
    hash_path_content(
        &mut hash,
        "transcript_json",
        row.transcript_json_path.as_deref(),
    )?;
    Ok(format!("{hash:016x}"))
}

const FNV_OFFSET_BASIS: u64 = 0xcbf29ce484222325;
const FNV_PRIME: u64 = 0x100000001b3;

fn hash_path_content(hash: &mut u64, label: &str, path: Option<&Path>) -> Result<(), String> {
    hash_bytes(hash, label.as_bytes());
    hash_bytes(hash, b"\0");
    let Some(path) = path else {
        hash_bytes(hash, b"<missing>");
        hash_bytes(hash, b"\0");
        return Ok(());
    };
    if !path.exists() {
        hash_bytes(hash, b"<missing>");
        hash_bytes(hash, b"\0");
        return Ok(());
    }
    let body = std::fs::read(path)
        .map_err(|e| format!("failed reading {} for summary hash: {e}", path.display()))?;
    hash_bytes(hash, &body);
    hash_bytes(hash, b"\0");
    Ok(())
}

fn hash_bytes(hash: &mut u64, bytes: &[u8]) {
    for byte in bytes {
        *hash ^= *byte as u64;
        *hash = hash.wrapping_mul(FNV_PRIME);
    }
}

fn mtime_ms(path: &Path) -> i64 {
    let Ok(metadata) = path.metadata() else {
        return 0;
    };
    let Ok(modified) = metadata.modified() else {
        return 0;
    };
    modified
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis() as i64)
        .unwrap_or_default()
}

fn default_schema_version() -> u32 {
    1
}

fn default_speaker_status() -> String {
    "unmapped".to_string()
}

impl MeetingMetadata {
    fn for_id(id: &str) -> Self {
        Self {
            schema_version: default_schema_version(),
            id: id.to_string(),
            title: None,
            company: None,
            context: None,
            context_kind: None,
            people: Vec::new(),
            speaker_map: BTreeMap::new(),
            speaker_status: default_speaker_status(),
            source: None,
            original_id: None,
            notion_page_id: None,
            notion_url: None,
            imported_from: None,
            imported_at: None,
            updated_at: None,
        }
    }
}

impl MeetingIndexRow {
    fn to_list_item(&self) -> MeetingListItem {
        MeetingListItem {
            id: self.id.clone(),
            title: self.title.clone(),
            status: self.status.clone(),
            started_at: self.started_at.clone(),
            ended_at: self.ended_at.clone(),
            company: self.company.clone(),
            context: self.context.clone(),
            context_kind: self.context_kind.clone(),
            source: self.source.clone(),
            people: self.people.clone(),
            speakers: self.speakers.clone(),
            has_summary: self.has_summary,
            has_transcript: self.has_transcript,
            session_dir: path_string(&self.session_dir),
            summary_path: self.summary_path.as_ref().map(|path| path_string(path)),
            transcript_path: self.transcript_path.as_ref().map(|path| path_string(path)),
            updated_at: self.updated_at.clone(),
            snippet: self.snippet.clone(),
            enrichment: enrichment_status_for(self),
        }
    }
}

trait PathExt {
    fn file_name_string(&self) -> Option<String>;
}

impl PathExt for Path {
    fn file_name_string(&self) -> Option<String> {
        self.file_name()
            .map(|value| value.to_string_lossy().into_owned())
    }
}

trait SqlResultExt<T> {
    fn map_err_sql(self, action: &str) -> Result<T, String>;
}

impl<T> SqlResultExt<T> for rusqlite::Result<T> {
    fn map_err_sql(self, action: &str) -> Result<T, String> {
        self.map_err(|e| format!("{action}: {e}"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn rebuild_index_lists_existing_recordings() {
        let dir = tempdir().expect("temp dir");
        write_session(
            dir.path(),
            "session-1",
            r#"{"id":"session-1","status":"done","startedAt":"2026-05-19T10:00:00Z"}"#,
        );
        std::fs::write(
            dir.path().join("session-1").join("summary.md"),
            "Follow up with Ravi.",
        )
        .expect("summary");

        let summary = rebuild_index(dir.path()).expect("rebuild");
        assert_eq!(summary.meetings_indexed, 1);

        let meetings = list_meetings(dir.path(), Some("Ravi".to_string()), None, None, None)
            .expect("list meetings");
        assert_eq!(meetings.len(), 1);
        assert_eq!(meetings[0].id, "session-1");
        assert_eq!(meetings[0].title, "Follow up with Ravi.");
        assert!(meetings[0].has_summary);
    }

    #[test]
    fn list_meetings_rebuilds_empty_existing_index() {
        let dir = tempdir().expect("temp dir");
        write_session(
            dir.path(),
            "session-1",
            r#"{"id":"session-1","status":"done","startedAt":"2026-05-19T10:00:00Z"}"#,
        );
        std::fs::write(
            dir.path().join("session-1").join("summary.md"),
            "Follow up with Ravi.",
        )
        .expect("summary");
        open_index(dir.path()).expect("open empty index");

        let meetings = list_meetings(dir.path(), Some("Ravi".to_string()), None, None, None)
            .expect("list meetings");
        assert_eq!(meetings.len(), 1);
        assert_eq!(meetings[0].id, "session-1");
    }

    #[test]
    fn delete_meeting_moves_session_out_of_archive_and_reindexes() {
        let dir = tempdir().expect("temp dir");
        let session_dir = write_session(
            dir.path(),
            "session-1",
            r#"{"id":"session-1","status":"done","startedAt":"2026-05-19T10:00:00Z"}"#,
        );
        std::fs::write(session_dir.join("summary.md"), "Follow up with Ravi.").expect("summary");
        rebuild_index(dir.path()).expect("rebuild");

        let summary = delete_meeting(dir.path(), "session-1").expect("delete");

        assert_eq!(summary.id, "session-1");
        assert!(!session_dir.exists());
        let moved_to = PathBuf::from(summary.moved_to);
        assert!(moved_to.join("session.json").exists());
        let meetings = list_meetings(dir.path(), None, None, None, None).expect("list meetings");
        assert!(meetings.is_empty());
    }

    #[test]
    fn delete_meetings_moves_multiple_sessions_and_reindexes_once() {
        let dir = tempdir().expect("temp dir");
        let first = write_session(
            dir.path(),
            "session-1",
            r#"{"id":"session-1","status":"done","startedAt":"2026-05-19T10:00:00Z"}"#,
        );
        let second = write_session(
            dir.path(),
            "session-2",
            r#"{"id":"session-2","status":"done","startedAt":"2026-05-20T10:00:00Z"}"#,
        );
        let third = write_session(
            dir.path(),
            "session-3",
            r#"{"id":"session-3","status":"done","startedAt":"2026-05-21T10:00:00Z"}"#,
        );
        rebuild_index(dir.path()).expect("rebuild");

        let summary = delete_meetings(
            dir.path(),
            &[
                "session-1".to_string(),
                "session-2".to_string(),
                "session-1".to_string(),
            ],
        )
        .expect("delete meetings");

        assert_eq!(summary.deleted_count, 2);
        assert_eq!(
            summary
                .deleted
                .iter()
                .map(|item| item.id.as_str())
                .collect::<Vec<_>>(),
            vec!["session-1", "session-2"]
        );
        assert!(!first.exists());
        assert!(!second.exists());
        assert!(third.exists());
        for item in summary.deleted {
            assert!(PathBuf::from(item.moved_to).join("session.json").exists());
        }

        let meetings = list_meetings(dir.path(), None, None, None, None).expect("list meetings");
        assert_eq!(meetings.len(), 1);
        assert_eq!(meetings[0].id, "session-3");
    }

    #[test]
    fn enrichment_status_flags_codex_queue_items() {
        let dir = tempdir().expect("temp dir");
        let session_dir = write_session(
            dir.path(),
            "session-1",
            r#"{"id":"session-1","status":"done","startedAt":"2026-05-19T10:00:00Z"}"#,
        );
        write_transcript_json(&session_dir, "Speaker 1", "We should follow up with Ravi.");
        std::fs::write(
            session_dir.join("transcript.md"),
            "# Transcript\n\n[00:00] Speaker 1: We should follow up with Ravi.\n",
        )
        .expect("transcript");

        let meetings = list_meetings(dir.path(), None, None, None, None).expect("list meetings");
        assert_eq!(meetings.len(), 1);
        let enrichment = &meetings[0].enrichment;

        assert!(enrichment.needs_enrichment);
        assert!(enrichment.needs_summary);
        assert!(enrichment.needs_title);
        assert!(enrichment.needs_speakers);
        assert!(!enrichment.needs_context);
        assert_eq!(
            meetings[0].company.as_deref(),
            Some(INDIVIDUAL_NETWORKING_COMPANY)
        );
        assert_eq!(enrichment.status, "needs_enrichment");
        assert!(
            enrichment
                .reasons
                .iter()
                .any(|reason| reason == "missing_summary")
        );
        assert!(
            enrichment
                .reasons
                .iter()
                .any(|reason| reason == "generic_title")
        );
        assert!(
            enrichment
                .reasons
                .iter()
                .any(|reason| reason == "unmapped_speakers")
        );
        assert!(
            !enrichment
                .reasons
                .iter()
                .any(|reason| reason == "missing_context")
        );
    }

    #[test]
    fn empty_transcript_placeholder_is_not_queued_for_summary() {
        let dir = tempdir().expect("temp dir");
        let session_dir = write_session(
            dir.path(),
            "session-1",
            r#"{"id":"session-1","status":"done","startedAt":"2026-05-19T10:00:00Z"}"#,
        );
        let document = serde_json::json!({
            "engine": "mlx-whisper",
            "model": "test-model",
            "fallbackModel": "test-model",
            "speakerLabelMode": "meAndCall",
            "generatedAt": "2026-05-19T10:05:00Z",
            "segments": []
        });
        std::fs::write(
            session_dir.join("transcript.json"),
            serde_json::to_string_pretty(&document).expect("transcript json"),
        )
        .expect("write transcript json");
        std::fs::write(
            session_dir.join("transcript.md"),
            "# Transcript\n\nNo transcript produced.\n",
        )
        .expect("write transcript markdown");

        rebuild_index(dir.path()).expect("rebuild");
        let meetings = list_meetings(dir.path(), None, None, None, None).expect("list meetings");
        let queue = list_summary_queue_from_index(dir.path(), 100).expect("summary queue");

        assert_eq!(meetings.len(), 1);
        assert!(!meetings[0].has_transcript);
        assert!(!meetings[0].enrichment.needs_summary);
        assert_eq!(meetings[0].enrichment.summary_status, "unavailable");
        assert!(queue.is_empty());
    }

    #[test]
    fn enrichment_status_ready_after_codex_writes_safe_artifacts() {
        let dir = tempdir().expect("temp dir");
        let session_dir = write_session(
            dir.path(),
            "session-1",
            r#"{"id":"session-1","status":"done","startedAt":"2026-05-19T10:00:00Z"}"#,
        );
        write_transcript_json(
            &session_dir,
            "Speaker 1",
            "Ravi discussed the Nuance hiring plan.",
        );
        update_meeting_metadata(
            dir.path(),
            UpdateMeetingMetadataRequest {
                id: "session-1".to_string(),
                title: Some("Nuance hiring plan with Ravi".to_string()),
                company: Some("Nuance Labs".to_string()),
                context: Some("Nuance Labs".to_string()),
                context_kind: Some("Company".to_string()),
                people: Some(vec!["Ravi".to_string()]),
            },
        )
        .expect("metadata");
        apply_speaker_map(
            dir.path(),
            ApplySpeakerMapRequest {
                id: "session-1".to_string(),
                speaker_map: BTreeMap::from([("Speaker 1".to_string(), "Ravi".to_string())]),
            },
        )
        .expect("speakers");
        std::fs::write(
            session_dir.join("summary.md"),
            "# Summary\n\nNuance hiring plan.\n",
        )
        .expect("summary");
        rebuild_index(dir.path()).expect("rebuild");

        let meetings = list_meetings(dir.path(), None, None, None, None).expect("list meetings");
        assert_eq!(meetings.len(), 1);
        let enrichment = &meetings[0].enrichment;

        assert!(!enrichment.needs_enrichment);
        assert_eq!(enrichment.status, "ready");
        assert!(enrichment.reasons.is_empty());
        assert_eq!(enrichment.summary_status, "ready");
        assert_eq!(enrichment.title_status, "ready");
        assert_eq!(enrichment.speaker_status, "ready");
        assert_eq!(enrichment.context_status, "ready");
    }

    #[test]
    fn metadata_edit_requeues_existing_summary() {
        let dir = tempdir().expect("temp dir");
        let session_dir = write_session(
            dir.path(),
            "session-1",
            r#"{"id":"session-1","status":"done","startedAt":"2026-05-19T10:00:00Z"}"#,
        );
        write_transcript_json(
            &session_dir,
            "Speaker 1",
            "Ravi discussed the Nuance hiring plan.",
        );
        update_meeting_metadata(
            dir.path(),
            UpdateMeetingMetadataRequest {
                id: "session-1".to_string(),
                title: Some("Nuance hiring plan with Ravi".to_string()),
                company: Some("Nuance Labs".to_string()),
                context: Some("Nuance Labs".to_string()),
                context_kind: Some("Company".to_string()),
                people: Some(vec!["Ravi".to_string()]),
            },
        )
        .expect("metadata");
        apply_speaker_map(
            dir.path(),
            ApplySpeakerMapRequest {
                id: "session-1".to_string(),
                speaker_map: BTreeMap::from([("Speaker 1".to_string(), "Ravi".to_string())]),
            },
        )
        .expect("speakers");
        std::fs::write(
            session_dir.join("summary.md"),
            "# Summary\n\nNuance hiring plan.\n",
        )
        .expect("summary");
        rebuild_index(dir.path()).expect("rebuild");

        std::thread::sleep(std::time::Duration::from_millis(25));
        update_meeting_metadata(
            dir.path(),
            UpdateMeetingMetadataRequest {
                id: "session-1".to_string(),
                title: Some("Updated Nuance hiring plan with Ravi".to_string()),
                company: Some("Nuance Labs".to_string()),
                context: Some("Nuance Labs".to_string()),
                context_kind: Some("Company".to_string()),
                people: Some(vec!["Ravi".to_string()]),
            },
        )
        .expect("metadata update");

        let meetings = list_meetings(dir.path(), None, None, None, None).expect("list meetings");
        let enrichment = &meetings[0].enrichment;

        assert!(meetings[0].has_summary);
        assert!(enrichment.needs_summary);
        assert!(enrichment.summary_stale);
        assert_eq!(enrichment.summary_status, "stale");
        assert!(
            enrichment
                .reasons
                .iter()
                .any(|reason| reason == "stale_summary")
        );
    }

    #[test]
    fn speaker_edit_requeues_existing_summary() {
        let dir = tempdir().expect("temp dir");
        let session_dir = write_session(
            dir.path(),
            "session-1",
            r#"{"id":"session-1","status":"done","startedAt":"2026-05-19T10:00:00Z"}"#,
        );
        write_transcript_json(
            &session_dir,
            "Speaker 1",
            "Ravi discussed the Nuance hiring plan.",
        );
        update_meeting_metadata(
            dir.path(),
            UpdateMeetingMetadataRequest {
                id: "session-1".to_string(),
                title: Some("Nuance hiring plan with Ravi".to_string()),
                company: Some("Nuance Labs".to_string()),
                context: Some("Nuance Labs".to_string()),
                context_kind: Some("Company".to_string()),
                people: Some(vec!["Ravi".to_string()]),
            },
        )
        .expect("metadata");
        apply_speaker_map(
            dir.path(),
            ApplySpeakerMapRequest {
                id: "session-1".to_string(),
                speaker_map: BTreeMap::from([("Speaker 1".to_string(), "Ravi".to_string())]),
            },
        )
        .expect("speakers");
        std::fs::write(
            session_dir.join("summary.md"),
            "# Summary\n\nNuance hiring plan.\n",
        )
        .expect("summary");
        rebuild_index(dir.path()).expect("rebuild");

        std::thread::sleep(std::time::Duration::from_millis(25));
        apply_speaker_map(
            dir.path(),
            ApplySpeakerMapRequest {
                id: "session-1".to_string(),
                speaker_map: BTreeMap::from([("Speaker 1".to_string(), "Ravi Patel".to_string())]),
            },
        )
        .expect("speaker update");

        let meetings = list_meetings(dir.path(), None, None, None, None).expect("list meetings");
        let enrichment = &meetings[0].enrichment;

        assert!(meetings[0].has_summary);
        assert!(enrichment.needs_summary);
        assert!(enrichment.summary_stale);
        assert_eq!(enrichment.summary_status, "stale");
    }

    #[test]
    fn initial_summary_state_seeds_existing_metadata_without_requeue() {
        let dir = tempdir().expect("temp dir");
        let session_dir = write_session(
            dir.path(),
            "session-1",
            r#"{"id":"session-1","status":"done","startedAt":"2026-05-19T10:00:00Z"}"#,
        );
        write_transcript_json(
            &session_dir,
            "Ravi",
            "Ravi discussed the Nuance hiring plan.",
        );
        std::fs::write(
            session_dir.join("summary.md"),
            "# Summary\n\nNuance hiring plan.\n",
        )
        .expect("summary");

        std::thread::sleep(std::time::Duration::from_millis(25));
        let mut metadata = MeetingMetadata::for_id("session-1");
        metadata.title = Some("Nuance hiring plan with Ravi".to_string());
        metadata.company = Some("Nuance Labs".to_string());
        metadata.context = Some("Nuance Labs".to_string());
        metadata.context_kind = Some("Company".to_string());
        metadata.people = vec!["Ravi".to_string()];
        metadata.updated_at = Some(Utc::now().to_rfc3339());
        write_meeting_metadata(&session_dir, &metadata).expect("metadata");

        rebuild_index(dir.path()).expect("rebuild");
        let meetings = list_meetings(dir.path(), None, None, None, None).expect("list meetings");
        let enrichment = &meetings[0].enrichment;

        assert!(meetings[0].has_summary);
        assert!(!enrichment.needs_summary);
        assert!(!enrichment.summary_stale);
        assert_eq!(enrichment.summary_status, "ready");
    }

    #[test]
    fn metadata_edits_are_sidecar_only_and_indexed() {
        let dir = tempdir().expect("temp dir");
        write_session(
            dir.path(),
            "session-1",
            r#"{"id":"session-1","status":"done","startedAt":"2026-05-19T10:00:00Z"}"#,
        );

        let detail = update_meeting_metadata(
            dir.path(),
            UpdateMeetingMetadataRequest {
                id: "session-1".to_string(),
                title: Some("Hiring sync".to_string()),
                company: Some("Mostly Harmless".to_string()),
                context: Some("Mostly Harmless".to_string()),
                context_kind: Some("Company".to_string()),
                people: Some(vec!["Adi".to_string(), "Ravi".to_string()]),
            },
        )
        .expect("update metadata");

        assert_eq!(detail.item.title, "Hiring sync");
        assert_eq!(detail.item.company.as_deref(), Some("Mostly Harmless"));
        assert!(dir.path().join("session-1").join("meeting.json").exists());

        let meetings = list_meetings(
            dir.path(),
            Some("mostly".to_string()),
            Some("Mostly Harmless".to_string()),
            None,
            None,
        )
        .expect("list");
        assert_eq!(meetings.len(), 1);
        assert_eq!(meetings[0].people, vec!["Adi", "Ravi"]);
    }

    #[test]
    fn speaker_map_renders_detail_without_rewriting_transcript() {
        let dir = tempdir().expect("temp dir");
        let session_dir = write_session(
            dir.path(),
            "session-1",
            r#"{"id":"session-1","status":"done","transcriptPath":"TRANSCRIPT_PATH"}"#,
        );
        let transcript_path = session_dir.join("transcript.md");
        let session_json = session_dir.join("session.json");
        let session_body = std::fs::read_to_string(&session_json)
            .expect("read session")
            .replace("TRANSCRIPT_PATH", &path_string(&transcript_path));
        std::fs::write(&session_json, session_body).expect("write session");
        let transcript = TranscriptDocument {
            engine: "test".to_string(),
            model: "test".to_string(),
            fallback_model: "test".to_string(),
            speaker_label_mode: crate::recorder_settings::SpeakerLabelMode::MeAndCall,
            generated_at: "2026-05-19T10:00:00Z".to_string(),
            audio_inputs: Vec::new(),
            segments: vec![
                TranscriptSegment {
                    speaker: "Me".to_string(),
                    speaker_id: None,
                    source: "mic".to_string(),
                    start_ms: 0,
                    end_ms: 500,
                    text: "hi".to_string(),
                },
                TranscriptSegment {
                    speaker: "Speaker 1".to_string(),
                    speaker_id: Some("spk_1".to_string()),
                    source: "system".to_string(),
                    start_ms: 500,
                    end_ms: 1000,
                    text: "hello".to_string(),
                },
            ],
        };
        std::fs::write(
            session_dir.join("transcript.json"),
            serde_json::to_string_pretty(&transcript).expect("transcript json"),
        )
        .expect("write transcript json");
        std::fs::write(&transcript_path, "[00:00] Speaker 1: hello\n").expect("write transcript");

        let mut speaker_map = BTreeMap::new();
        speaker_map.insert("Me".to_string(), "Adi".to_string());
        speaker_map.insert("spk_1".to_string(), "Ravi".to_string());
        let detail = apply_speaker_map(
            dir.path(),
            ApplySpeakerMapRequest {
                id: "session-1".to_string(),
                speaker_map,
            },
        )
        .expect("apply map");

        assert!(
            detail
                .transcript_markdown
                .as_deref()
                .unwrap_or_default()
                .contains("Ravi: hello")
        );
        assert!(
            detail
                .transcript_markdown
                .as_deref()
                .unwrap_or_default()
                .contains("Adi: hi")
        );
        assert_eq!(detail.speaker_candidates, vec!["Me", "Speaker 1"]);
        let raw_transcript = std::fs::read_to_string(&transcript_path).expect("raw transcript");
        assert!(raw_transcript.contains("Speaker 1"));
    }

    #[test]
    fn speaker_candidates_hide_mapped_names_and_numeric_duplicates() {
        let mut speaker_map = BTreeMap::new();
        speaker_map.insert("Me".to_string(), "Adi".to_string());
        speaker_map.insert("Speaker 1".to_string(), "Jihad".to_string());

        let candidates = speaker_candidates_from_labels(
            vec![
                "Me".to_string(),
                "1".to_string(),
                "Speaker 1".to_string(),
                "Adi".to_string(),
                "Jihad".to_string(),
            ],
            &speaker_map,
        );

        assert_eq!(candidates, vec!["Me", "Speaker 1"]);
    }

    #[test]
    fn import_archive_snapshot_creates_local_session_metadata_and_transcript() {
        let recordings = tempdir().expect("recordings");
        let archive = tempdir().expect("archive");
        let page_dir = archive
            .path()
            .join("normalized-meetings")
            .join("pages")
            .join("poha");
        std::fs::create_dir_all(&page_dir).expect("page dir");
        let page_path = page_dir.join("2026-01-01-hiring.md");
        std::fs::write(
            &page_path,
            r#"---
source: "poha"
---

# Hiring sync

## Notes

Bring Ravi into the loop.

## Transcript

**Speaker 1:** We should follow up.
"#,
        )
        .expect("page");
        std::fs::write(
            archive
                .path()
                .join("normalized-meetings")
                .join("index.json"),
            serde_json::to_string_pretty(&serde_json::json!([
                {
                    "source": "poha",
                    "id": "archive-1",
                    "title": "Hiring sync",
                    "status": "recorded",
                    "started_at": "2026-01-01T10:00:00Z",
                    "ended_at": "2026-01-01T10:30:00Z",
                    "normalized_page": path_string(&page_path)
                }
            ]))
            .expect("index json"),
        )
        .expect("index");
        std::fs::write(
            archive.path().join("notion-contexts-sandbox-after-backfill.json"),
            serde_json::to_string_pretty(&serde_json::json!({
                "results": [
                    {
                        "id": "context-1",
                        "properties": {
                            "Context": { "type": "title", "title": [{"plain_text": "Mostly Harmless"}] },
                            "Type": { "type": "select", "select": {"name": "Company"} }
                        }
                    }
                ]
            }))
            .expect("contexts json"),
        )
        .expect("contexts");
        std::fs::write(
            archive.path().join("notion-archive-sandbox-after-backfill.json"),
            serde_json::to_string_pretty(&serde_json::json!({
                "results": [
                    {
                        "id": "notion-page-1",
                        "url": "https://notion.test/hiring",
                        "properties": {
                            "Original ID": { "type": "rich_text", "rich_text": [{"plain_text": "archive-1"}] },
                            "Meeting Name": { "type": "title", "title": [{"plain_text": "Archive Hiring Sync"}] },
                            "People": { "type": "multi_select", "multi_select": [{"name": "Ravi"}] },
                            "Context": { "type": "relation", "relation": [{"id": "context-1"}] }
                        }
                    }
                ]
            }))
            .expect("archive json"),
        )
        .expect("archive");

        let summary = import_archive_snapshot(
            recordings.path(),
            ImportArchiveRequest {
                archive_root: Some(path_string(archive.path())),
            },
        )
        .expect("import archive");

        assert_eq!(summary.imported, 1);
        assert_eq!(summary.created_sessions, 1);
        let detail = get_meeting(recordings.path(), "archive-1").expect("detail");
        assert_eq!(detail.item.title, "Archive Hiring Sync");
        assert_eq!(detail.item.company.as_deref(), Some("Mostly Harmless"));
        assert_eq!(detail.item.people, vec!["Ravi"]);
        assert!(
            detail
                .transcript_markdown
                .as_deref()
                .unwrap_or_default()
                .contains("Speaker 1")
        );
    }

    #[test]
    fn import_archive_backfills_company_context_from_catalog() {
        let recordings = tempdir().expect("recordings");
        let archive = tempdir().expect("archive");
        let page_dir = archive
            .path()
            .join("normalized-meetings")
            .join("pages")
            .join("poha");
        std::fs::create_dir_all(&page_dir).expect("page dir");
        let page_path = page_dir.join("2026-01-01-replit.md");
        std::fs::write(
            &page_path,
            r#"# Replit Product Demo

## Transcript

**Speaker 1:** We should compare the Replit agent workflow.
"#,
        )
        .expect("page");
        std::fs::write(
            archive
                .path()
                .join("normalized-meetings")
                .join("index.json"),
            serde_json::to_string_pretty(&serde_json::json!([
                {
                    "source": "poha",
                    "id": "replit-1",
                    "title": "Replit Product Demo and Internal Collaboration",
                    "status": "recorded",
                    "started_at": "2026-01-01T10:00:00Z",
                    "normalized_page": path_string(&page_path)
                }
            ]))
            .expect("index json"),
        )
        .expect("index");
        std::fs::write(
            archive
                .path()
                .join("notion-contexts-sandbox-after-backfill.json"),
            serde_json::to_string_pretty(&serde_json::json!({
                "results": [
                    {
                        "id": "context-replit",
                        "properties": {
                            "Context": { "type": "title", "title": [{"plain_text": "Replit"}] },
                            "Type": { "type": "select", "select": {"name": "Company"} }
                        }
                    }
                ]
            }))
            .expect("contexts json"),
        )
        .expect("contexts");

        import_archive_snapshot(
            recordings.path(),
            ImportArchiveRequest {
                archive_root: Some(path_string(archive.path())),
            },
        )
        .expect("import archive");

        let detail = get_meeting(recordings.path(), "replit-1").expect("detail");
        assert_eq!(detail.item.company.as_deref(), Some("Replit"));
        assert_eq!(detail.item.context.as_deref(), Some("Replit"));
        assert_eq!(detail.item.context_kind.as_deref(), Some("Company"));
        let contexts = list_meeting_contexts(recordings.path(), None).expect("contexts");
        assert_eq!(contexts[0].name, "Replit");
        assert_eq!(contexts[0].meeting_count, 1);
    }

    #[test]
    fn export_meetings_writes_transcripts_and_metadata_folder() {
        let recordings = tempdir().expect("recordings");
        write_session(
            recordings.path(),
            "session-1",
            r#"{"id":"session-1","status":"done","startedAt":"2026-05-19T10:00:00Z"}"#,
        );
        std::fs::write(
            recordings.path().join("session-1").join("transcript.md"),
            "# Transcript\n\nhello\n",
        )
        .expect("transcript");
        update_meeting_metadata(
            recordings.path(),
            UpdateMeetingMetadataRequest {
                id: "session-1".to_string(),
                title: Some("Minimal export test".to_string()),
                company: None,
                context: None,
                context_kind: None,
                people: Some(vec!["Adi".to_string()]),
            },
        )
        .expect("metadata");
        let export_root = tempdir().expect("export");
        let out = export_root.path().join("poha-export");

        let summary = export_meetings(
            recordings.path(),
            ExportMeetingsRequest {
                output_dir: Some(path_string(&out)),
            },
        )
        .expect("export");

        assert_eq!(summary.meetings_exported, 1);
        assert!(out.join("index.md").exists());
        assert!(out.join("manifest.json").exists());
        let exported_transcripts = std::fs::read_dir(&out)
            .expect("read export")
            .flatten()
            .filter(|entry| entry.path().join("transcript.md").exists())
            .count();
        assert_eq!(exported_transcripts, 1);
    }

    #[test]
    fn export_meetings_is_not_capped_by_list_limit() {
        let recordings = tempdir().expect("recordings");
        for index in 0..=MAX_LIST_LIMIT {
            write_session(
                recordings.path(),
                &format!("session-{index:04}"),
                &format!(
                    r#"{{"id":"session-{index:04}","status":"done","startedAt":"2026-05-19T10:{:02}:00Z"}}"#,
                    index % 60
                ),
            );
        }
        let export_root = tempdir().expect("export");
        let out = export_root.path().join("poha-export");

        let summary = export_meetings(
            recordings.path(),
            ExportMeetingsRequest {
                output_dir: Some(path_string(&out)),
            },
        )
        .expect("export");

        assert_eq!(summary.meetings_exported, MAX_LIST_LIMIT + 1);
        let manifest = std::fs::read_to_string(out.join("manifest.json")).expect("manifest");
        assert!(manifest.contains(&format!(r#""meetingsExported": {}"#, MAX_LIST_LIMIT + 1)));
    }

    #[test]
    fn copy_context_includes_all_meeting_transcripts() {
        let recordings = tempdir().expect("recordings");
        let first = write_session(
            recordings.path(),
            "session-1",
            r#"{"id":"session-1","status":"done","startedAt":"2026-05-19T10:00:00Z"}"#,
        );
        let second = write_session(
            recordings.path(),
            "session-2",
            r#"{"id":"session-2","status":"done","startedAt":"2026-05-20T10:00:00Z"}"#,
        );
        std::fs::write(first.join("transcript.md"), "# Transcript\n\nfirst call\n")
            .expect("first transcript");
        std::fs::write(
            second.join("transcript.md"),
            "# Transcript\n\nsecond call\n",
        )
        .expect("second transcript");

        for (id, title) in [
            ("session-1", "First Nuance conversation"),
            ("session-2", "Second Nuance conversation"),
        ] {
            update_meeting_metadata(
                recordings.path(),
                UpdateMeetingMetadataRequest {
                    id: id.to_string(),
                    title: Some(title.to_string()),
                    company: Some("Nuance Labs".to_string()),
                    context: Some("Nuance Labs".to_string()),
                    context_kind: Some("Company".to_string()),
                    people: None,
                },
            )
            .expect("metadata");
        }

        let copied = copy_context_for_llm(recordings.path(), "Nuance Labs").expect("copy context");
        assert!(copied.contains("First Nuance conversation"));
        assert!(copied.contains("Second Nuance conversation"));
        assert!(copied.contains("first call"));
        assert!(copied.contains("second call"));
    }

    #[test]
    fn metadata_save_moves_meeting_between_contexts() {
        let recordings = tempdir().expect("recordings");
        write_session(
            recordings.path(),
            "session-1",
            r#"{"id":"session-1","status":"done","startedAt":"2026-05-19T10:00:00Z"}"#,
        );

        update_meeting_metadata(
            recordings.path(),
            UpdateMeetingMetadataRequest {
                id: "session-1".to_string(),
                title: Some("Original title".to_string()),
                company: Some("Nuance Labs".to_string()),
                context: Some("Nuance Labs".to_string()),
                context_kind: Some("Company".to_string()),
                people: None,
            },
        )
        .expect("initial metadata");

        update_meeting_metadata(
            recordings.path(),
            UpdateMeetingMetadataRequest {
                id: "session-1".to_string(),
                title: Some("Moved title".to_string()),
                company: Some("Replit".to_string()),
                context: Some("Replit".to_string()),
                context_kind: Some("Company".to_string()),
                people: Some(vec!["Adi".to_string()]),
            },
        )
        .expect("moved metadata");

        let old_context = list_meetings(
            recordings.path(),
            None,
            None,
            Some("Nuance Labs".to_string()),
            None,
        )
        .expect("old context");
        let new_context = list_meetings(
            recordings.path(),
            None,
            None,
            Some("Replit".to_string()),
            None,
        )
        .expect("new context");
        let contexts = list_meeting_contexts(recordings.path(), None).expect("contexts");

        assert!(old_context.is_empty());
        assert_eq!(new_context.len(), 1);
        assert_eq!(new_context[0].title, "Moved title");
        assert_eq!(new_context[0].people, vec!["Adi"]);
        assert!(contexts.iter().any(|context| context.name == "Replit"));
    }

    fn write_session(recordings_dir: &Path, id: &str, session_json: &str) -> PathBuf {
        let session_dir = recordings_dir.join(id);
        std::fs::create_dir_all(&session_dir).expect("session dir");
        std::fs::write(session_dir.join("session.json"), session_json).expect("session json");
        session_dir
    }

    fn write_transcript_json(session_dir: &Path, speaker: &str, text: &str) {
        let document = serde_json::json!({
            "engine": "mlx-whisper",
            "model": "test-model",
            "fallbackModel": "test-model",
            "speakerLabelMode": "meAndCall",
            "generatedAt": "2026-05-19T10:05:00Z",
            "segments": [{
                "speaker": speaker,
                "source": "system",
                "startMs": 0,
                "endMs": 1000,
                "text": text
            }]
        });
        std::fs::write(
            session_dir.join("transcript.json"),
            serde_json::to_string_pretty(&document).expect("transcript json"),
        )
        .expect("write transcript json");
        std::fs::write(
            session_dir.join("transcript.md"),
            format!("# Transcript\n\n[00:00] {speaker}: {text}\n"),
        )
        .expect("write transcript markdown");
    }
}
