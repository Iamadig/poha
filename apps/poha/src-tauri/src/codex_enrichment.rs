use std::ffi::OsString;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::Mutex;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Instant;

use serde::{Deserialize, Serialize};

use crate::{meeting_store, recorder_settings};

mod failure;
mod preflight;
#[cfg(test)]
mod tests;

use failure::classify_codex_failure;
use preflight::{preflight_codex_enrichment, resolve_codex_cli, resolve_poha_cli_invocation};

const DEFAULT_BULK_RUN_LIMIT: usize = 1;
const DEFAULT_BULK_STATUS_LIMIT: usize = 500;
const MAX_BULK_LIMIT: usize = 500;
const OUTPUT_TAIL_CHARS: usize = 4000;
const POHA_STATE_DIR_NAME: &str = ".poha";
const CODEX_WORKSPACE_DIR_NAME: &str = "codex-workspace";
const CODEX_TARGET_DIR_NAME: &str = "codex-target";

static ACTIVE_CODEX_PID: OnceLock<Mutex<Option<u32>>> = OnceLock::new();
static CANCEL_REQUESTED: AtomicBool = AtomicBool::new(false);
static BULK_ENRICHMENT_RUNNING: AtomicBool = AtomicBool::new(false);

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CodexBulkEnrichmentRequest {
    pub operation: CodexBulkOperation,
    pub limit: Option<usize>,
    pub refresh: Option<bool>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum CodexBulkOperation {
    MissingSummaries,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CodexBulkEnrichmentSummary {
    pub operation: String,
    pub success: bool,
    pub resumable: bool,
    pub exit_code: Option<i32>,
    pub candidate_count: usize,
    pub completed_count: usize,
    pub remaining_count: usize,
    pub recordings_dir: String,
    pub codex_path: String,
    pub stdout_tail: String,
    pub stderr_tail: String,
    pub failure_kind: Option<String>,
    pub failure_message: Option<String>,
    pub duration_ms: u128,
    pub cancelled: bool,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CodexBulkEnrichmentQueueStatus {
    pub operation: String,
    pub pending_count: usize,
    pub recordings_dir: String,
    pub sample_items: Vec<CodexBulkEnrichmentQueueItem>,
    pub active_pid: Option<u32>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CodexBulkEnrichmentQueueItem {
    pub id: String,
    pub title: String,
    pub reason: String,
}

pub async fn run_codex_bulk_enrichment_for_app(
    app: tauri::AppHandle,
    request: CodexBulkEnrichmentRequest,
) -> Result<CodexBulkEnrichmentSummary, String> {
    let settings = recorder_settings::load(&app)?;
    let recordings_dir = settings.recordings_dir_path();
    std::fs::create_dir_all(&recordings_dir).map_err(|e| {
        format!(
            "failed creating recordings dir {}: {e}",
            recordings_dir.display()
        )
    })?;

    tauri::async_runtime::spawn_blocking(move || run_codex_bulk_enrichment(recordings_dir, request))
        .await
        .map_err(|e| format!("codex enrichment worker failed: {e}"))?
}

pub async fn get_codex_bulk_enrichment_status_for_app(
    app: tauri::AppHandle,
    request: CodexBulkEnrichmentRequest,
) -> Result<CodexBulkEnrichmentQueueStatus, String> {
    let settings = recorder_settings::load(&app)?;
    let recordings_dir = settings.recordings_dir_path();
    std::fs::create_dir_all(&recordings_dir).map_err(|e| {
        format!(
            "failed creating recordings dir {}: {e}",
            recordings_dir.display()
        )
    })?;

    tauri::async_runtime::spawn_blocking(move || {
        codex_bulk_enrichment_status(recordings_dir, request)
    })
    .await
    .map_err(|e| format!("codex enrichment status worker failed: {e}"))?
}

pub async fn cancel_codex_bulk_enrichment_for_app() -> Result<bool, String> {
    tauri::async_runtime::spawn_blocking(cancel_codex_bulk_enrichment)
        .await
        .map_err(|e| format!("codex enrichment cancel worker failed: {e}"))?
}

fn run_codex_bulk_enrichment(
    recordings_dir: PathBuf,
    request: CodexBulkEnrichmentRequest,
) -> Result<CodexBulkEnrichmentSummary, String> {
    let _guard = BulkEnrichmentRunGuard::acquire()?;
    match request.operation {
        CodexBulkOperation::MissingSummaries => {
            run_missing_summary_enrichment(recordings_dir, request.limit)
        }
    }
}

fn run_missing_summary_enrichment(
    recordings_dir: PathBuf,
    limit: Option<usize>,
) -> Result<CodexBulkEnrichmentSummary, String> {
    let started = Instant::now();
    let limit = limit
        .unwrap_or(DEFAULT_BULK_RUN_LIMIT)
        .clamp(1, MAX_BULK_LIMIT);
    let candidates = missing_summary_candidates(&recordings_dir, limit)?;

    if candidates.is_empty() {
        return Ok(CodexBulkEnrichmentSummary {
            operation: operation_name(CodexBulkOperation::MissingSummaries).to_string(),
            success: true,
            resumable: false,
            exit_code: Some(0),
            candidate_count: 0,
            completed_count: 0,
            remaining_count: 0,
            recordings_dir: path_string(&recordings_dir),
            codex_path: String::new(),
            stdout_tail: String::new(),
            stderr_tail: String::new(),
            failure_kind: None,
            failure_message: None,
            duration_ms: started.elapsed().as_millis(),
            cancelled: false,
        });
    }

    let codex_path = resolve_codex_cli()?;
    let workspace_dir = codex_workspace_dir(&recordings_dir);
    let cargo_target_dir = codex_cargo_target_dir(&recordings_dir);
    std::fs::create_dir_all(&workspace_dir).map_err(|e| {
        format!(
            "failed creating Codex workspace {}: {e}",
            workspace_dir.display()
        )
    })?;
    std::fs::create_dir_all(&cargo_target_dir).map_err(|e| {
        format!(
            "failed creating Codex target dir {}: {e}",
            cargo_target_dir.display()
        )
    })?;
    let cli_invocation = resolve_poha_cli_invocation(&cargo_target_dir)?;
    preflight_codex_enrichment(
        &codex_path,
        &cli_invocation,
        &workspace_dir,
        &recordings_dir,
    )?;
    let prompt = missing_summary_prompt(&recordings_dir, &cli_invocation, &candidates);
    let output = run_codex_exec(&codex_path, &workspace_dir, &recordings_dir, &prompt)?;

    let _ = meeting_store::rebuild_index(&recordings_dir);
    let remaining_count = candidates
        .iter()
        .filter(|meeting| {
            meeting_store::get_meeting(&recordings_dir, &meeting.id)
                .map(|detail| detail.item.enrichment.needs_summary)
                .unwrap_or(true)
        })
        .count();
    let codex_success = output.output.status.success();
    let failure = if output.cancelled || codex_success {
        None
    } else {
        Some(classify_codex_failure(
            &String::from_utf8_lossy(&output.output.stdout),
            &String::from_utf8_lossy(&output.output.stderr),
            output.output.status.code(),
        ))
    };
    let resumable = remaining_count > 0
        && failure
            .as_ref()
            .map(|detail| detail.resumable)
            .unwrap_or(true);

    Ok(CodexBulkEnrichmentSummary {
        operation: operation_name(CodexBulkOperation::MissingSummaries).to_string(),
        success: !output.cancelled && codex_success && remaining_count == 0,
        resumable,
        exit_code: output.output.status.code(),
        candidate_count: candidates.len(),
        completed_count: candidates.len().saturating_sub(remaining_count),
        remaining_count,
        recordings_dir: path_string(&recordings_dir),
        codex_path: path_string(&codex_path),
        stdout_tail: tail_string(&String::from_utf8_lossy(&output.output.stdout)),
        stderr_tail: tail_string(&String::from_utf8_lossy(&output.output.stderr)),
        failure_kind: failure.as_ref().map(|detail| detail.kind.to_string()),
        failure_message: failure.map(|detail| detail.message),
        duration_ms: started.elapsed().as_millis(),
        cancelled: output.cancelled,
    })
}

fn missing_summary_candidates(
    recordings_dir: &Path,
    limit: usize,
) -> Result<Vec<meeting_store::MeetingListItem>, String> {
    meeting_store::rebuild_index(recordings_dir)?;
    meeting_store::list_summary_queue_from_index(recordings_dir, limit)
}

fn codex_bulk_enrichment_status(
    recordings_dir: PathBuf,
    request: CodexBulkEnrichmentRequest,
) -> Result<CodexBulkEnrichmentQueueStatus, String> {
    match request.operation {
        CodexBulkOperation::MissingSummaries => {
            let limit = request
                .limit
                .unwrap_or(DEFAULT_BULK_STATUS_LIMIT)
                .clamp(1, MAX_BULK_LIMIT);
            if request.refresh.unwrap_or(false) {
                meeting_store::rebuild_index(&recordings_dir)?;
            }
            let candidates = meeting_store::list_summary_queue_from_index(&recordings_dir, limit)?;
            let sample_items = candidates
                .iter()
                .take(5)
                .map(|meeting| CodexBulkEnrichmentQueueItem {
                    id: meeting.id.clone(),
                    title: meeting.title.clone(),
                    reason: if meeting.enrichment.summary_stale {
                        "stale summary".to_string()
                    } else {
                        "missing summary".to_string()
                    },
                })
                .collect::<Vec<_>>();
            Ok(CodexBulkEnrichmentQueueStatus {
                operation: operation_name(CodexBulkOperation::MissingSummaries).to_string(),
                pending_count: candidates.len(),
                recordings_dir: path_string(&recordings_dir),
                sample_items,
                active_pid: active_codex_pid(),
            })
        }
    }
}

struct CodexExecOutput {
    output: std::process::Output,
    cancelled: bool,
}

#[derive(Debug)]
struct BulkEnrichmentRunGuard;

impl BulkEnrichmentRunGuard {
    fn acquire() -> Result<Self, String> {
        BULK_ENRICHMENT_RUNNING
            .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
            .map(|_| Self)
            .map_err(|_| {
                "Codex bulk enrichment is already running. Wait for the current job or cancel it."
                    .to_string()
            })
    }
}

impl Drop for BulkEnrichmentRunGuard {
    fn drop(&mut self) {
        BULK_ENRICHMENT_RUNNING.store(false, Ordering::SeqCst);
    }
}

fn run_codex_exec(
    codex_path: &Path,
    workspace_dir: &Path,
    recordings_dir: &Path,
    prompt: &str,
) -> Result<CodexExecOutput, String> {
    CANCEL_REQUESTED.store(false, Ordering::SeqCst);
    let mut command = Command::new(codex_path);
    command.args(codex_exec_args(workspace_dir, recordings_dir));
    let mut child = command
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("failed starting Codex at {}: {e}", codex_path.display()))?;
    set_active_codex_pid(Some(child.id()));

    let mut stdin = child.stdin.take().ok_or_else(|| {
        set_active_codex_pid(None);
        "failed opening Codex stdin".to_string()
    })?;
    if let Err(error) = stdin.write_all(prompt.as_bytes()) {
        let _ = child.kill();
        set_active_codex_pid(None);
        return Err(format!("failed writing Codex prompt: {error}"));
    }
    drop(stdin);

    let output = child
        .wait_with_output()
        .map_err(|e| format!("failed waiting for Codex: {e}"));
    set_active_codex_pid(None);
    output.map(|output| CodexExecOutput {
        output,
        cancelled: CANCEL_REQUESTED.swap(false, Ordering::SeqCst),
    })
}

fn codex_exec_args(workspace_dir: &Path, recordings_dir: &Path) -> Vec<OsString> {
    vec![
        OsString::from("exec"),
        OsString::from("-C"),
        workspace_dir.as_os_str().to_owned(),
        OsString::from("--skip-git-repo-check"),
        OsString::from("--add-dir"),
        recordings_dir.as_os_str().to_owned(),
        OsString::from("--sandbox"),
        OsString::from("workspace-write"),
        OsString::from("-"),
    ]
}

fn cancel_codex_bulk_enrichment() -> Result<bool, String> {
    let Some(pid) = active_codex_pid() else {
        return Ok(false);
    };
    CANCEL_REQUESTED.store(true, Ordering::SeqCst);
    let status = Command::new("kill")
        .arg("-TERM")
        .arg(pid.to_string())
        .status()
        .map_err(|e| format!("failed sending cancel signal to Codex pid {pid}: {e}"))?;
    Ok(status.success())
}

fn active_codex_pid() -> Option<u32> {
    let lock = ACTIVE_CODEX_PID.get_or_init(|| Mutex::new(None));
    lock.lock().ok().and_then(|guard| *guard)
}

fn set_active_codex_pid(pid: Option<u32>) {
    let lock = ACTIVE_CODEX_PID.get_or_init(|| Mutex::new(None));
    if let Ok(mut guard) = lock.lock() {
        *guard = pid;
    }
}

fn missing_summary_prompt(
    recordings_dir: &Path,
    cli_invocation: &preflight::PohaCliInvocation,
    candidates: &[meeting_store::MeetingListItem],
) -> String {
    let cli_prefix = cli_invocation.command_prefix(recordings_dir);
    let ids = candidates
        .iter()
        .map(|meeting| format!("- `{}`: {}", meeting.id, meeting.title))
        .collect::<Vec<_>>()
        .join("\n");

    format!(
        r#"Run a Poha bulk enrichment job for queued meeting summaries.

Scope:
{ids}

Use this CLI prefix for every Poha read/write:
`{cli_prefix}`

For each scoped meeting:
1. Read it with:
   `{cli_prefix} meetings get <meeting-id> --include metadata,session,transcript,speakers,paths`
2. If it has no transcript, skip it.
3. If `data.item.enrichment.needsSummary` is false, skip it.
4. Create a faithful Markdown summary from current transcript, metadata, and speaker names.
5. Replace any existing stale summary; do not skip solely because summary.md already exists.
6. Write only through:
   `{cli_prefix} sessions update-notes <meeting-id> --stdin`

Summary format:
`# Summary`
One concise paragraph.

`## Key Points`
Bullets grounded in the transcript.

`## Follow-ups`
Bullets for explicit action items, or `- None captured.`

Rules:
- Codex is sandboxed to the recordings directory; never attempt to write source files.
- Do not edit audio, transcript files, session JSON, or meeting JSON directly.
- Do not invent facts missing from the transcript.
- Keep each summary compact enough to scan.
- Treat this as a resumable queue: finish one meeting, write it, then move to the next.
- After each successful `sessions update-notes`, run `{cli_prefix} meetings reindex` before continuing so interrupted jobs preserve visible progress.
- After all writes, run `{cli_prefix} meetings reindex`.
- Finish with a short count of written and skipped meetings.
"#
    )
}

fn codex_workspace_dir(recordings_dir: &Path) -> PathBuf {
    recordings_dir
        .join(POHA_STATE_DIR_NAME)
        .join(CODEX_WORKSPACE_DIR_NAME)
}

fn codex_cargo_target_dir(recordings_dir: &Path) -> PathBuf {
    recordings_dir
        .join(POHA_STATE_DIR_NAME)
        .join(CODEX_TARGET_DIR_NAME)
}

fn operation_name(operation: CodexBulkOperation) -> &'static str {
    match operation {
        CodexBulkOperation::MissingSummaries => "missingSummaries",
    }
}

fn tail_string(text: &str) -> String {
    tail_string_with_limit(text, OUTPUT_TAIL_CHARS)
}

fn tail_string_with_limit(text: &str, limit: usize) -> String {
    let chars = text.chars().collect::<Vec<_>>();
    if chars.len() <= limit {
        return text.to_string();
    }
    chars[chars.len() - limit..].iter().collect()
}

fn shell_quote(path: &Path) -> String {
    let value = path.to_string_lossy();
    format!("'{}'", value.replace('\'', "'\\''"))
}

fn path_string(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}
