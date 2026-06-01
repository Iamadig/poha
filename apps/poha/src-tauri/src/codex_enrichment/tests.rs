use super::*;
use tempfile::tempdir;

fn write_session(recordings_dir: &Path, id: &str, started_at: &str) -> PathBuf {
    let session_dir = recordings_dir.join(id);
    std::fs::create_dir_all(&session_dir).expect("session dir");
    std::fs::write(
        session_dir.join("session.json"),
        format!(r#"{{"id":"{id}","status":"done","startedAt":"{started_at}"}}"#),
    )
    .expect("session json");
    session_dir
}

fn write_transcript(session_dir: &Path, text: &str) {
    std::fs::write(
        session_dir.join("transcript.json"),
        format!(
            r#"{{"engine":"mlx-whisper","model":"test","fallbackModel":"test","speakerLabelMode":"meAndCall","generatedAt":"2026-05-19T10:05:00Z","segments":[{{"speaker":"Speaker 1","source":"system","startMs":0,"endMs":1000,"text":"{text}"}}]}}"#
        ),
    )
    .expect("transcript json");
    std::fs::write(
        session_dir.join("transcript.md"),
        format!("# Transcript\n\n[00:00] Speaker 1: {text}\n"),
    )
    .expect("transcript");
}

#[test]
fn missing_summary_candidates_refreshes_stale_summary_state() {
    let dir = tempdir().expect("temp dir");
    let session_dir = write_session(dir.path(), "session-1", "2026-05-19T10:00:00Z");
    write_transcript(&session_dir, "We should follow up.");
    let summary_path = session_dir.join("summary.md");
    std::fs::write(&summary_path, "# Summary\n\nExisting.\n").expect("summary");
    meeting_store::rebuild_index(dir.path()).expect("initial index");
    std::fs::remove_file(summary_path).expect("remove summary");

    let candidates = missing_summary_candidates(dir.path(), 10).expect("candidates");

    assert_eq!(candidates.len(), 1);
    assert_eq!(candidates[0].id, "session-1");
}

#[test]
fn missing_summary_candidates_filters_before_limit() {
    let dir = tempdir().expect("temp dir");
    let complete_session = write_session(dir.path(), "complete", "2026-05-20T10:00:00Z");
    write_transcript(&complete_session, "This one is already summarized.");
    std::fs::write(
        complete_session.join("summary.md"),
        "# Summary\n\nAlready summarized.\n",
    )
    .expect("summary");
    let missing_session = write_session(dir.path(), "missing", "2026-05-19T10:00:00Z");
    write_transcript(&missing_session, "This one still needs a summary.");

    let candidates = missing_summary_candidates(dir.path(), 1).expect("candidates");

    assert_eq!(candidates.len(), 1);
    assert_eq!(candidates[0].id, "missing");
}

#[test]
fn codex_bulk_enrichment_status_reads_existing_queue_without_rebuild() {
    let dir = tempdir().expect("temp dir");
    let indexed_session = write_session(dir.path(), "session-1", "2026-05-19T10:00:00Z");
    write_transcript(&indexed_session, "We should follow up.");
    meeting_store::rebuild_index(dir.path()).expect("index");
    let unindexed_session = write_session(dir.path(), "session-2", "2026-05-20T10:00:00Z");
    write_transcript(
        &unindexed_session,
        "This should wait for an explicit reindex.",
    );

    let status = codex_bulk_enrichment_status(
        dir.path().to_path_buf(),
        CodexBulkEnrichmentRequest {
            operation: CodexBulkOperation::MissingSummaries,
            limit: None,
            refresh: None,
        },
    )
    .expect("status");

    assert_eq!(status.pending_count, 1);
}

#[test]
fn codex_bulk_enrichment_status_can_refresh_queue_before_user_action() {
    let dir = tempdir().expect("temp dir");
    let session = write_session(dir.path(), "session-1", "2026-05-19T10:00:00Z");
    write_transcript(&session, "We should follow up.");

    let status = codex_bulk_enrichment_status(
        dir.path().to_path_buf(),
        CodexBulkEnrichmentRequest {
            operation: CodexBulkOperation::MissingSummaries,
            limit: None,
            refresh: Some(true),
        },
    )
    .expect("status");

    assert_eq!(status.pending_count, 1);
    assert_eq!(status.sample_items[0].id, "session-1");
    assert_eq!(status.sample_items[0].reason, "missing summary");
}

#[test]
fn codex_exec_runs_from_recordings_workspace_not_repo_root() {
    let recordings = Path::new("/tmp/poha-recordings");
    let workspace = codex_workspace_dir(recordings);
    let args = codex_exec_args(&workspace, recordings)
        .into_iter()
        .map(|arg| arg.to_string_lossy().into_owned())
        .collect::<Vec<_>>();

    assert_eq!(args[0], "exec");
    assert_eq!(args[1], "-C");
    assert_eq!(args[2], "/tmp/poha-recordings/.poha/codex-workspace");
    assert!(args.iter().any(|arg| arg == "--skip-git-repo-check"));
    assert!(
        args.windows(2)
            .any(|pair| pair[0] == "--add-dir" && pair[1] == "/tmp/poha-recordings")
    );
    assert!(!args.iter().any(|arg| arg.ends_with("/projects/poha")));
}

#[test]
fn missing_summary_prompt_routes_cargo_target_into_recordings_dir() {
    let recordings = Path::new("/tmp/poha-recordings");
    let target = codex_cargo_target_dir(recordings);
    let invocation = preflight::PohaCliInvocation::Cargo {
        cargo_path: PathBuf::from("/opt/homebrew/bin/cargo"),
        rustc_path: PathBuf::from("/opt/homebrew/bin/rustc"),
        manifest_path: PathBuf::from("/repo/apps/poha/src-tauri/Cargo.toml"),
        cargo_target_dir: target,
    };
    let prompt = missing_summary_prompt(recordings, &invocation, &[]);

    assert!(prompt.contains("CARGO_TARGET_DIR='/tmp/poha-recordings/.poha/codex-target'"));
    assert!(prompt.contains("RUSTC='/opt/homebrew/bin/rustc'"));
    assert!(prompt.contains("--recordings-dir '/tmp/poha-recordings'"));
    assert!(prompt.contains("Codex is sandboxed to the recordings directory"));
}

#[test]
fn missing_summary_prompt_prefers_direct_cli_when_available() {
    let recordings = Path::new("/tmp/poha-recordings");
    let invocation = preflight::PohaCliInvocation::Direct {
        path: PathBuf::from("/Applications/Poha.app/Contents/MacOS/poha-cli"),
    };

    let prompt = missing_summary_prompt(recordings, &invocation, &[]);

    assert!(prompt.contains("'/Applications/Poha.app/Contents/MacOS/poha-cli'"));
    assert!(prompt.contains("--recordings-dir '/tmp/poha-recordings'"));
    assert!(!prompt.contains("cargo run"));
}

#[test]
fn codex_exec_help_requires_non_repo_flag() {
    assert!(preflight::validate_codex_exec_help("Usage\n  --skip-git-repo-check\n").is_ok());
    assert_eq!(
        preflight::validate_codex_exec_help("Usage\n").expect_err("missing flag"),
        "Codex CLI does not support --skip-git-repo-check."
    );
}

#[test]
fn bulk_run_guard_rejects_concurrent_runs() {
    let guard = BulkEnrichmentRunGuard::acquire().expect("first guard");

    let error = BulkEnrichmentRunGuard::acquire().expect_err("second guard should fail");

    assert!(error.contains("already running"));
    drop(guard);
    let guard = BulkEnrichmentRunGuard::acquire().expect("guard after drop");
    drop(guard);
}

#[test]
fn codex_failure_classifies_trusted_directory_as_non_resumable() {
    let detail = classify_codex_failure(
        "",
        "Not inside a trusted directory and --skip-git-repo-check was not specified.",
        Some(1),
    );

    assert_eq!(detail.kind, "trusted_directory");
    assert!(!detail.resumable);
    assert!(detail.message.contains("trusted directory"));
}

#[test]
fn codex_failure_classifies_auth_as_non_resumable() {
    let detail = classify_codex_failure(
        "",
        "Auth error: OAuth token refresh failed: invalid_grant",
        Some(1),
    );

    assert_eq!(detail.kind, "codex_auth");
    assert!(!detail.resumable);
    assert!(detail.message.contains("codex login"));
}

#[test]
fn shell_quote_escapes_single_quotes() {
    assert_eq!(
        shell_quote(Path::new("/tmp/poha user's files")),
        "'/tmp/poha user'\\''s files'"
    );
}

#[test]
fn tail_string_keeps_short_text() {
    assert_eq!(tail_string("short"), "short");
}
