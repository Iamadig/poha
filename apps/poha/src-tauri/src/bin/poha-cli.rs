use clap::{Args, Parser, Subcommand, ValueEnum};
use serde::Serialize;
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::io::{self, Read};
use std::path::{Path, PathBuf};

#[path = "poha_cli/output.rs"]
mod output;
#[path = "poha_cli/storage.rs"]
mod storage;

use output::{emit_failure, emit_success};
use poha_lib::meeting_store::{
    ApplySpeakerMapRequest, ExportMeetingsRequest, ImportArchiveRequest,
    UpdateMeetingMetadataRequest, apply_speaker_map, copy_context_for_llm, copy_meeting_for_llm,
    export_meetings, get_meeting, import_archive_snapshot, list_meetings, rebuild_index,
    update_meeting_metadata,
};
use poha_lib::{RecoverSessionRequest, recover_crashed_session};

const SCHEMA_VERSION: u32 = 1;

#[derive(Parser)]
#[command(name = "poha-cli", version, about = "Agent CLI for Poha recordings")]
struct Cli {
    #[arg(long, env = "POHA_RECORDINGS_DIR", global = true)]
    recordings_dir: Option<PathBuf>,
    #[arg(
        long,
        global = true,
        help = "Emit JSON output. This is the default and only output mode."
    )]
    json: bool,
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    Spec,
    Capabilities,
    Info,
    Sessions {
        #[command(subcommand)]
        command: SessionCommand,
    },
    Meetings {
        #[command(subcommand)]
        command: MeetingCommand,
    },
    Storage {
        #[command(subcommand)]
        command: StorageCommand,
    },
    Diagnostics {
        #[command(subcommand)]
        command: DiagnosticsCommand,
    },
}

#[derive(Subcommand)]
enum SessionCommand {
    List(ListArgs),
    Get(GetArgs),
    Status(SessionIdArgs),
    Recover(RecoverArgs),
    UpdateNotes(NoteArgs),
    AppendNotes(NoteArgs),
}

#[derive(Subcommand)]
enum MeetingCommand {
    List(MeetingListArgs),
    Get(MeetingGetArgs),
    Context(MeetingContextArgs),
    Update(MeetingUpdateArgs),
    Speakers(MeetingSpeakersArgs),
    Reindex,
    Copy(MeetingCopyArgs),
    ImportArchive(ImportArchiveArgs),
    Export(ExportArgs),
}

#[derive(Subcommand)]
enum StorageCommand {
    Report(StorageReportArgs),
    #[command(name = "audio-quality")]
    AudioQuality(StorageAudioQualityArgs),
    Maintain(StorageMaintainArgs),
}

#[derive(Subcommand)]
enum DiagnosticsCommand {
    Audio(DiagnosticsAudioArgs),
}

#[derive(Args)]
struct ListArgs {
    #[arg(long, default_value_t = 50)]
    limit: usize,
}

#[derive(Args)]
struct StorageReportArgs {
    #[arg(long, default_value_t = 20)]
    limit: usize,
}

#[derive(Args)]
struct StorageAudioQualityArgs {
    #[arg(long, default_value_t = 200)]
    limit: usize,
    #[arg(long, help = "Include sessions whose mixed MP3 audio quality passes.")]
    all: bool,
    #[arg(
        long,
        help = "Decode full audio files instead of the default opening window."
    )]
    full: bool,
}

#[derive(Args)]
struct StorageMaintainArgs {
    #[arg(
        long,
        help = "Preview default storage maintenance without moving files."
    )]
    dry_run: bool,
}

#[derive(Args)]
struct DiagnosticsAudioArgs {
    #[arg(long, help = "Run the guided mic + system audio check.")]
    guided: bool,
}

#[derive(Args)]
struct MeetingListArgs {
    #[arg(long)]
    query: Option<String>,
    #[arg(long)]
    company: Option<String>,
    #[arg(long)]
    context: Option<String>,
    #[arg(long)]
    since: Option<String>,
    #[arg(long, help = "Only return meetings that need Codex enrichment.")]
    needs_enrichment: bool,
    #[arg(long, default_value_t = 50)]
    limit: usize,
}

#[derive(Args)]
struct MeetingGetArgs {
    id: String,
    #[arg(
        long,
        help = "Comma-separated include list: all,metadata,session,summary,transcript,speakers,paths"
    )]
    include: Option<String>,
}

#[derive(Args)]
struct MeetingContextArgs {
    context: String,
    #[arg(long, value_enum, default_value = "json")]
    format: MeetingContextFormat,
    #[arg(long, default_value_t = 200)]
    limit: usize,
}

#[derive(Args)]
struct MeetingUpdateArgs {
    id: String,
    #[arg(long)]
    title: Option<String>,
    #[arg(long)]
    company: Option<String>,
    #[arg(long)]
    context: Option<String>,
    #[arg(long = "context-kind")]
    context_kind: Option<String>,
    #[arg(long, value_delimiter = ',')]
    people: Vec<String>,
    #[arg(long)]
    clear_people: bool,
}

#[derive(Args)]
struct MeetingSpeakersArgs {
    id: String,
    #[arg(
        long = "set",
        help = "Speaker mapping as label=name. Repeat for multiple mappings."
    )]
    sets: Vec<String>,
}

#[derive(Args)]
struct MeetingCopyArgs {
    target: String,
    #[arg(long, value_enum, default_value = "auto")]
    kind: CopyTargetKind,
    #[arg(long, value_enum, default_value = "llm")]
    format: MeetingCopyFormat,
}

#[derive(Debug, Clone, ValueEnum)]
enum MeetingContextFormat {
    Json,
    Llm,
}

#[derive(Debug, Clone, ValueEnum)]
enum CopyTargetKind {
    Auto,
    Meeting,
    Context,
}

#[derive(Debug, Clone, ValueEnum)]
enum MeetingCopyFormat {
    Llm,
}

#[derive(Args)]
struct SessionIdArgs {
    id: String,
}

#[derive(Args)]
struct GetArgs {
    id: String,
    #[arg(long)]
    partial: bool,
}

#[derive(Args)]
struct RecoverArgs {
    id: String,
    #[arg(long)]
    settings_path: Option<PathBuf>,
}

#[derive(Args)]
struct NoteArgs {
    id: String,
    #[arg(long)]
    file: Option<PathBuf>,
    #[arg(long)]
    stdin: bool,
}

#[derive(Args)]
struct ImportArchiveArgs {
    #[arg(long)]
    archive_root: Option<PathBuf>,
}

#[derive(Args)]
struct ExportArgs {
    #[arg(long)]
    output_dir: Option<PathBuf>,
}

#[derive(Debug)]
struct CliError {
    code: &'static str,
    message: String,
}

#[derive(Clone)]
struct Context {
    recordings_dir: PathBuf,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct SessionListItem {
    id: String,
    status: Option<String>,
    started_at: Option<String>,
    ended_at: Option<String>,
    has_summary: bool,
    has_transcript: bool,
    summary_path: Option<String>,
    transcript_path: Option<String>,
    metadata_path: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct SessionDetail {
    id: String,
    status: Option<String>,
    started_at: Option<String>,
    ended_at: Option<String>,
    summary_markdown: Option<String>,
    transcript_markdown: Option<String>,
    metadata: Value,
    paths: SessionPaths,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct SessionStatusDetail {
    id: String,
    status: Option<String>,
    transcription_status: Option<String>,
    started_at: Option<String>,
    ended_at: Option<String>,
    has_partial_transcript: bool,
    has_final_transcript: bool,
    chunks: Option<ChunkStatusSummary>,
    paths: SessionStatusPaths,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct ChunkStatusSummary {
    total: usize,
    queued: usize,
    transcribing: usize,
    done: usize,
    failed: usize,
    updated_at: Option<String>,
    path: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct SessionStatusPaths {
    session_dir: String,
    metadata_json: String,
    partial_transcript_markdown: String,
    final_transcript_markdown: Option<String>,
    live_transcript_json: String,
    chunk_manifest_json: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct SessionPaths {
    session_dir: String,
    summary_markdown: String,
    transcript_markdown: Option<String>,
    transcript_json: Option<String>,
    metadata_json: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct NoteWriteResult {
    id: String,
    summary_path: String,
    bytes_written: usize,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct DiagnosticsAudioResult {
    status: String,
    mode: String,
    menu_path: Vec<String>,
    report_glob: String,
    expected_report_files: Vec<String>,
}

fn main() {
    let cli = Cli::parse();
    let command_name = command_name(&cli.command);
    let ctx = Context::new(cli.recordings_dir);
    let _json_output = cli.json;
    let result = match cli.command {
        Command::Spec => emit_success(&command_name, spec(), &ctx, vec![]),
        Command::Capabilities => emit_success(&command_name, capabilities(), &ctx, vec![]),
        Command::Info => emit_success(&command_name, info(&ctx), &ctx, vec![]),
        Command::Sessions { command } => run_session_command(&command_name, command, &ctx),
        Command::Meetings { command } => run_meeting_command(&command_name, command, &ctx),
        Command::Storage { command } => run_storage_command(&command_name, command, &ctx),
        Command::Diagnostics { command } => run_diagnostics_command(&command_name, command, &ctx),
    };

    if let Err(error) = result {
        emit_failure(&command_name, error, &ctx);
        std::process::exit(1);
    }
}

fn command_name(command: &Command) -> String {
    match command {
        Command::Spec => "spec".to_string(),
        Command::Capabilities => "capabilities".to_string(),
        Command::Info => "info".to_string(),
        Command::Sessions { command } => match command {
            SessionCommand::List(_) => "sessions.list",
            SessionCommand::Get(_) => "sessions.get",
            SessionCommand::Status(_) => "sessions.status",
            SessionCommand::Recover(_) => "sessions.recover",
            SessionCommand::UpdateNotes(_) => "sessions.updateNotes",
            SessionCommand::AppendNotes(_) => "sessions.appendNotes",
        }
        .to_string(),
        Command::Meetings { command } => match command {
            MeetingCommand::List(_) => "meetings.list",
            MeetingCommand::Get(_) => "meetings.get",
            MeetingCommand::Context(_) => "meetings.context",
            MeetingCommand::Update(_) => "meetings.update",
            MeetingCommand::Speakers(_) => "meetings.speakers",
            MeetingCommand::Reindex => "meetings.reindex",
            MeetingCommand::Copy(_) => "meetings.copy",
            MeetingCommand::ImportArchive(_) => "meetings.importArchive",
            MeetingCommand::Export(_) => "meetings.export",
        }
        .to_string(),
        Command::Storage { command } => match command {
            StorageCommand::Report(_) => "storage.report",
            StorageCommand::AudioQuality(_) => "storage.audioQuality",
            StorageCommand::Maintain(_) => "storage.maintain",
        }
        .to_string(),
        Command::Diagnostics { command } => match command {
            DiagnosticsCommand::Audio(_) => "diagnostics.audio",
        }
        .to_string(),
    }
}

impl Context {
    fn new(recordings_dir: Option<PathBuf>) -> Self {
        Self {
            recordings_dir: recordings_dir.unwrap_or_else(default_recordings_dir),
        }
    }
}

fn default_recordings_dir() -> PathBuf {
    dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("Library")
        .join("Application Support")
        .join("Poha")
        .join("recordings")
}

fn run_session_command(
    command_name: &str,
    command: SessionCommand,
    ctx: &Context,
) -> Result<(), CliError> {
    match command {
        SessionCommand::List(args) => {
            let data = list_sessions(ctx, args.limit)?;
            emit_success(command_name, data, ctx, vec![])
        }
        SessionCommand::Get(args) => {
            let (data, warnings) = get_session(ctx, &args.id, args.partial)?;
            emit_success(command_name, data, ctx, warnings)
        }
        SessionCommand::Status(args) => {
            let data = get_session_status(ctx, &args.id)?;
            emit_success(command_name, data, ctx, vec![])
        }
        SessionCommand::Recover(args) => {
            let data = recover_crashed_session(RecoverSessionRequest {
                recordings_dir: ctx.recordings_dir.clone(),
                session_id: args.id,
                settings_path: args.settings_path,
            })
            .map_err(|message| CliError::new("sessionRecoverFailed", message))?;
            emit_success(command_name, data, ctx, vec![])
        }
        SessionCommand::UpdateNotes(args) => {
            let data = write_notes(ctx, &args, WriteMode::Replace)?;
            emit_success(command_name, data, ctx, vec![])
        }
        SessionCommand::AppendNotes(args) => {
            let data = write_notes(ctx, &args, WriteMode::Append)?;
            emit_success(command_name, data, ctx, vec![])
        }
    }
}

fn run_meeting_command(
    command_name: &str,
    command: MeetingCommand,
    ctx: &Context,
) -> Result<(), CliError> {
    match command {
        MeetingCommand::List(args) => {
            let data = list_meetings_cli(ctx, &args)?;
            emit_success(command_name, data, ctx, vec![])
        }
        MeetingCommand::Get(args) => {
            let data = get_meeting_cli(ctx, &args)?;
            emit_success(command_name, data, ctx, vec![])
        }
        MeetingCommand::Context(args) => {
            let data = meeting_context_cli(ctx, &args)?;
            emit_success(command_name, data, ctx, vec![])
        }
        MeetingCommand::Update(args) => {
            let data = update_meeting_cli(ctx, args)?;
            emit_success(command_name, data, ctx, vec![])
        }
        MeetingCommand::Speakers(args) => {
            let data = update_speakers_cli(ctx, args)?;
            emit_success(command_name, data, ctx, vec![])
        }
        MeetingCommand::Reindex => {
            let data = rebuild_index(&ctx.recordings_dir)
                .map_err(|message| CliError::new("meetingReindexFailed", message))?;
            emit_success(command_name, data, ctx, vec![])
        }
        MeetingCommand::Copy(args) => {
            let data = copy_meeting_context_cli(ctx, &args)?;
            emit_success(command_name, data, ctx, vec![])
        }
        MeetingCommand::ImportArchive(args) => {
            let data = import_archive_snapshot(
                &ctx.recordings_dir,
                ImportArchiveRequest {
                    archive_root: args.archive_root.map(|path| path_string(&path)),
                },
            )
            .map_err(|message| CliError::new("archiveImportFailed", message))?;
            emit_success(command_name, data, ctx, vec![])
        }
        MeetingCommand::Export(args) => {
            let data = export_meetings(
                &ctx.recordings_dir,
                ExportMeetingsRequest {
                    output_dir: args.output_dir.map(|path| path_string(&path)),
                },
            )
            .map_err(|message| CliError::new("meetingExportFailed", message))?;
            emit_success(command_name, data, ctx, vec![])
        }
    }
}

fn run_storage_command(
    command_name: &str,
    command: StorageCommand,
    ctx: &Context,
) -> Result<(), CliError> {
    match command {
        StorageCommand::Report(args) => {
            let data = storage::report(ctx, args.limit)?;
            emit_success(command_name, data, ctx, vec![])
        }
        StorageCommand::AudioQuality(args) => {
            let data = storage::audio_quality(ctx, args.limit, args.all, args.full)?;
            emit_success(command_name, data, ctx, vec![])
        }
        StorageCommand::Maintain(args) => {
            if !args.dry_run {
                return Err(CliError::new(
                    "storageMaintainRequiresDryRun",
                    "storage maintain currently supports --dry-run only".to_string(),
                ));
            }
            let data = storage::maintain_dry_run(ctx)?;
            emit_success(command_name, data, ctx, vec![])
        }
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct MeetingContextResult {
    context: String,
    format: String,
    meetings: Vec<poha_lib::meeting_store::MeetingListItem>,
    text: Option<String>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct MeetingWriteResult {
    meeting: Value,
    writes: Vec<String>,
    reindexed: bool,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct MeetingCopyResult {
    target: String,
    target_kind: String,
    format: String,
    text: String,
}

fn list_meetings_cli(
    ctx: &Context,
    args: &MeetingListArgs,
) -> Result<Vec<poha_lib::meeting_store::MeetingListItem>, CliError> {
    let fetch_limit = if args.needs_enrichment {
        500
    } else {
        args.limit
    };
    let mut meetings = list_meetings(
        &ctx.recordings_dir,
        args.query.clone(),
        args.company.clone(),
        args.context.clone(),
        Some(fetch_limit),
    )
    .map_err(|message| CliError::new("meetingListFailed", message))?;

    if let Some(since) = args.since.as_deref() {
        meetings.retain(|meeting| {
            meeting
                .started_at
                .as_deref()
                .is_some_and(|started_at| started_at >= since)
        });
    }
    if args.needs_enrichment {
        meetings.retain(|meeting| meeting.enrichment.needs_enrichment);
        meetings.truncate(args.limit);
    }

    Ok(meetings)
}

fn get_meeting_cli(ctx: &Context, args: &MeetingGetArgs) -> Result<Value, CliError> {
    let detail = get_meeting(&ctx.recordings_dir, &args.id)
        .map_err(|message| CliError::new("meetingGetFailed", message))?;
    let mut value = serde_json::to_value(detail).map_err(|error| {
        CliError::new(
            "jsonSerializeFailed",
            format!("failed to serialize meeting detail: {error}"),
        )
    })?;
    apply_meeting_include_filter(&mut value, args.include.as_deref())?;
    Ok(value)
}

fn meeting_context_cli(
    ctx: &Context,
    args: &MeetingContextArgs,
) -> Result<MeetingContextResult, CliError> {
    let meetings = list_meetings(
        &ctx.recordings_dir,
        None,
        None,
        Some(args.context.clone()),
        Some(args.limit),
    )
    .map_err(|message| CliError::new("meetingContextListFailed", message))?;
    let text = match args.format {
        MeetingContextFormat::Json => None,
        MeetingContextFormat::Llm => Some(
            copy_context_for_llm(&ctx.recordings_dir, &args.context)
                .map_err(|message| CliError::new("meetingContextCopyFailed", message))?,
        ),
    };

    Ok(MeetingContextResult {
        context: args.context.clone(),
        format: meeting_context_format_name(&args.format).to_string(),
        meetings,
        text,
    })
}

fn update_meeting_cli(
    ctx: &Context,
    args: MeetingUpdateArgs,
) -> Result<MeetingWriteResult, CliError> {
    let people = if args.clear_people {
        Some(Vec::new())
    } else if args.people.is_empty() {
        None
    } else {
        Some(args.people)
    };
    let detail = update_meeting_metadata(
        &ctx.recordings_dir,
        UpdateMeetingMetadataRequest {
            id: args.id,
            title: args.title,
            company: args.company,
            context: args.context,
            context_kind: args.context_kind,
            people,
        },
    )
    .map_err(|message| CliError::new("meetingUpdateFailed", message))?;
    meeting_write_result(detail)
}

fn update_speakers_cli(
    ctx: &Context,
    args: MeetingSpeakersArgs,
) -> Result<MeetingWriteResult, CliError> {
    let speaker_map = parse_speaker_sets(&args.sets)?;
    let detail = apply_speaker_map(
        &ctx.recordings_dir,
        ApplySpeakerMapRequest {
            id: args.id,
            speaker_map,
        },
    )
    .map_err(|message| CliError::new("meetingSpeakersFailed", message))?;
    meeting_write_result(detail)
}

fn copy_meeting_context_cli(
    ctx: &Context,
    args: &MeetingCopyArgs,
) -> Result<MeetingCopyResult, CliError> {
    let (target_kind, text) = match args.kind {
        CopyTargetKind::Meeting => (
            "meeting",
            copy_meeting_for_llm(&ctx.recordings_dir, &args.target)
                .map_err(|message| CliError::new("meetingCopyFailed", message))?,
        ),
        CopyTargetKind::Context => (
            "context",
            copy_context_for_llm(&ctx.recordings_dir, &args.target)
                .map_err(|message| CliError::new("meetingContextCopyFailed", message))?,
        ),
        CopyTargetKind::Auto => match copy_meeting_for_llm(&ctx.recordings_dir, &args.target) {
            Ok(text) => ("meeting", text),
            Err(_) => (
                "context",
                copy_context_for_llm(&ctx.recordings_dir, &args.target)
                    .map_err(|message| CliError::new("meetingCopyFailed", message))?,
            ),
        },
    };

    Ok(MeetingCopyResult {
        target: args.target.clone(),
        target_kind: target_kind.to_string(),
        format: meeting_copy_format_name(&args.format).to_string(),
        text,
    })
}

fn meeting_write_result(
    detail: poha_lib::meeting_store::MeetingDetail,
) -> Result<MeetingWriteResult, CliError> {
    let writes = vec![
        detail.paths.meeting_json.clone(),
        detail.paths.index_path.clone(),
    ];
    let meeting = serde_json::to_value(detail).map_err(|error| {
        CliError::new(
            "jsonSerializeFailed",
            format!("failed to serialize meeting write result: {error}"),
        )
    })?;
    Ok(MeetingWriteResult {
        meeting,
        writes,
        reindexed: true,
    })
}

fn apply_meeting_include_filter(value: &mut Value, include: Option<&str>) -> Result<(), CliError> {
    let Some(include) = include else {
        return Ok(());
    };
    let include = parse_include_set(include)?;
    if include.contains("all") {
        return Ok(());
    }
    let Some(object) = value.as_object_mut() else {
        return Ok(());
    };

    if !include.contains("metadata") {
        object.remove("metadata");
    }
    if !include.contains("session") && !include.contains("sessionmetadata") {
        object.remove("sessionMetadata");
    }
    if !include.contains("summary") {
        object.remove("summaryMarkdown");
    }
    if !include.contains("transcript") {
        object.remove("transcriptMarkdown");
    }
    if !include.contains("speakers") {
        object.remove("speakerCandidates");
    }
    if !include.contains("paths") {
        object.remove("paths");
    }
    Ok(())
}

fn parse_include_set(include: &str) -> Result<BTreeSet<String>, CliError> {
    let set = include
        .split(',')
        .map(|item| item.trim().to_lowercase())
        .filter(|item| !item.is_empty())
        .collect::<BTreeSet<_>>();
    let allowed = [
        "all",
        "metadata",
        "session",
        "sessionmetadata",
        "summary",
        "transcript",
        "speakers",
        "paths",
    ]
    .into_iter()
    .collect::<BTreeSet<_>>();
    if let Some(unknown) = set.iter().find(|item| !allowed.contains(item.as_str())) {
        return Err(CliError::new(
            "invalidInclude",
            format!("unknown include value: {unknown}"),
        ));
    }
    Ok(set)
}

fn parse_speaker_sets(values: &[String]) -> Result<BTreeMap<String, String>, CliError> {
    if values.is_empty() {
        return Err(CliError::new(
            "invalidSpeakerMap",
            "pass at least one --set label=name value".to_string(),
        ));
    }

    let mut map = BTreeMap::new();
    for value in values {
        let Some((key, name)) = value.split_once('=') else {
            return Err(CliError::new(
                "invalidSpeakerMap",
                format!("speaker mapping must use label=name: {value}"),
            ));
        };
        let key = key.trim();
        let name = name.trim();
        if key.is_empty() || name.is_empty() {
            return Err(CliError::new(
                "invalidSpeakerMap",
                format!("speaker mapping must include label and name: {value}"),
            ));
        }
        map.insert(key.to_string(), name.to_string());
    }
    Ok(map)
}

fn meeting_context_format_name(format: &MeetingContextFormat) -> &'static str {
    match format {
        MeetingContextFormat::Json => "json",
        MeetingContextFormat::Llm => "llm",
    }
}

fn meeting_copy_format_name(format: &MeetingCopyFormat) -> &'static str {
    match format {
        MeetingCopyFormat::Llm => "llm",
    }
}

fn run_diagnostics_command(
    command_name: &str,
    command: DiagnosticsCommand,
    ctx: &Context,
) -> Result<(), CliError> {
    match command {
        DiagnosticsCommand::Audio(args) => {
            let data = trigger_audio_diagnostics(ctx, args.guided)?;
            emit_success(command_name, data, ctx, vec![])
        }
    }
}

fn trigger_audio_diagnostics(
    ctx: &Context,
    guided: bool,
) -> Result<DiagnosticsAudioResult, CliError> {
    let menu_item = if guided {
        "Run Guided Audio Check"
    } else {
        "Run Quick Live Test"
    };
    trigger_tray_menu_item("Troubleshooting", menu_item)?;
    Ok(DiagnosticsAudioResult {
        status: "triggered".to_string(),
        mode: if guided { "guided" } else { "quick" }.to_string(),
        menu_path: vec![
            "Poha".to_string(),
            "Troubleshooting".to_string(),
            menu_item.to_string(),
        ],
        report_glob: path_string(&ctx.recordings_dir.join("*/live-test-report.json")),
        expected_report_files: vec![
            "live-test-report.json".to_string(),
            "live-test-report.md".to_string(),
        ],
    })
}

fn trigger_tray_menu_item(parent: &str, item: &str) -> Result<(), CliError> {
    #[cfg(target_os = "macos")]
    {
        let script = format!(
            r#"tell application "System Events"
  if not (exists process "Poha") then error "Poha is not running"
  tell process "Poha"
    click menu bar item 1 of menu bar 2
    delay 0.2
    set diagnosticsMenu to menu 1 of menu item "{parent}" of menu 1 of menu bar item 1 of menu bar 2
    click menu item "{item}" of diagnosticsMenu
  end tell
end tell"#
        );
        let output = std::process::Command::new("/usr/bin/osascript")
            .arg("-e")
            .arg(script)
            .output()
            .map_err(|error| {
                CliError::new(
                    "diagnosticsAudioTriggerFailed",
                    format!("failed launching osascript: {error}"),
                )
            })?;
        if output.status.success() {
            return Ok(());
        }
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
        return Err(CliError::new(
            "diagnosticsAudioTriggerFailed",
            if stderr.is_empty() {
                "osascript failed to trigger the Poha tray menu".to_string()
            } else {
                stderr
            },
        ));
    }

    #[cfg(not(target_os = "macos"))]
    {
        let _ = parent;
        let _ = item;
        Err(CliError::new(
            "diagnosticsAudioUnsupportedPlatform",
            "audio diagnostics can only be triggered from the macOS menu bar app".to_string(),
        ))
    }
}

fn list_sessions(ctx: &Context, limit: usize) -> Result<Vec<SessionListItem>, CliError> {
    let mut items = Vec::new();
    for entry in read_recordings_dir(ctx)? {
        let entry = entry.map_err_io("read recordings entry")?;
        if !entry
            .file_type()
            .map_err_io("read recordings entry type")?
            .is_dir()
        {
            continue;
        }
        let session_dir = entry.path();
        let metadata_path = session_dir.join("session.json");
        if !metadata_path.exists() {
            continue;
        }
        let metadata = read_json(&metadata_path)?;
        let id = metadata
            .get("id")
            .and_then(Value::as_str)
            .map(ToString::to_string)
            .unwrap_or_else(|| entry.file_name().to_string_lossy().into_owned());
        let summary_path = session_dir.join("summary.md");
        let transcript_path = transcript_markdown_path(&metadata, &session_dir);
        items.push(SessionListItem {
            id,
            status: metadata_string(&metadata, "status"),
            started_at: metadata_string(&metadata, "started_at"),
            ended_at: metadata_string(&metadata, "ended_at"),
            has_summary: summary_path.exists(),
            has_transcript: transcript_path.as_ref().is_some_and(|path| path.exists()),
            summary_path: summary_path.exists().then(|| path_string(&summary_path)),
            transcript_path: transcript_path
                .filter(|path| path.exists())
                .map(|path| path_string(&path)),
            metadata_path: path_string(&metadata_path),
        });
    }
    items.sort_by(|left, right| {
        right
            .started_at
            .cmp(&left.started_at)
            .then_with(|| right.id.cmp(&left.id))
    });
    items.truncate(limit);
    Ok(items)
}

fn get_session(
    ctx: &Context,
    id: &str,
    prefer_partial: bool,
) -> Result<(SessionDetail, Vec<String>), CliError> {
    let session_dir = session_dir(ctx, id)?;
    let metadata_path = session_dir.join("session.json");
    let metadata = read_json(&metadata_path)?;
    let summary_path = session_dir.join("summary.md");
    let final_transcript_md_path = transcript_markdown_path(&metadata, &session_dir);
    let partial_transcript_md_path = session_dir.join("transcript.partial.md");
    let transcript_md_path = if prefer_partial && partial_transcript_md_path.exists() {
        Some(partial_transcript_md_path)
    } else {
        final_transcript_md_path.clone()
    };
    let transcript_json_path = metadata
        .get("transcript_path")
        .and_then(Value::as_str)
        .map(ToString::to_string)
        .or_else(|| metadata_string(&metadata, "transcript_path"))
        .map(PathBuf::from)
        .map(|path| path.with_extension("json"))
        .filter(|path| path.exists())
        .or_else(|| {
            let path = session_dir.join("transcript.json");
            path.exists().then_some(path)
        });

    let mut warnings = Vec::new();
    if !summary_path.exists() {
        warnings.push("summary.md is missing; notes write commands will create it".to_string());
    }
    if transcript_md_path
        .as_ref()
        .is_none_or(|path| !path.exists())
    {
        warnings.push(if prefer_partial {
            "transcript.partial.md is missing".to_string()
        } else {
            "transcript.md is missing".to_string()
        });
    }

    Ok((
        SessionDetail {
            id: metadata_string(&metadata, "id").unwrap_or_else(|| id.to_string()),
            status: metadata_string(&metadata, "status"),
            started_at: metadata_string(&metadata, "started_at"),
            ended_at: metadata_string(&metadata, "ended_at"),
            summary_markdown: read_optional_string(&summary_path)?,
            transcript_markdown: transcript_md_path
                .as_ref()
                .map(|path| read_optional_string(path))
                .transpose()?
                .flatten(),
            metadata,
            paths: SessionPaths {
                session_dir: path_string(&session_dir),
                summary_markdown: path_string(&summary_path),
                transcript_markdown: transcript_md_path.map(|path| path_string(&path)),
                transcript_json: transcript_json_path.map(|path| path_string(&path)),
                metadata_json: path_string(&metadata_path),
            },
        },
        warnings,
    ))
}

fn get_session_status(ctx: &Context, id: &str) -> Result<SessionStatusDetail, CliError> {
    let session_dir = session_dir(ctx, id)?;
    let metadata_path = session_dir.join("session.json");
    let metadata = read_json(&metadata_path)?;
    let partial_path = session_dir.join("transcript.partial.md");
    let final_path = transcript_markdown_path(&metadata, &session_dir);
    let live_path = session_dir.join("transcript.live.json");
    let chunks_path = session_dir.join("transcript.chunks.json");

    Ok(SessionStatusDetail {
        id: metadata_string(&metadata, "id").unwrap_or_else(|| id.to_string()),
        status: metadata_string(&metadata, "status"),
        transcription_status: metadata
            .get("transcription")
            .and_then(|value| value.get("status"))
            .and_then(Value::as_str)
            .map(ToString::to_string),
        started_at: metadata_string(&metadata, "started_at"),
        ended_at: metadata_string(&metadata, "ended_at"),
        has_partial_transcript: partial_path.exists(),
        has_final_transcript: final_path.as_ref().is_some_and(|path| path.exists()),
        chunks: chunk_status_summary(&live_path).or_else(|| chunk_status_summary(&chunks_path)),
        paths: SessionStatusPaths {
            session_dir: path_string(&session_dir),
            metadata_json: path_string(&metadata_path),
            partial_transcript_markdown: path_string(&partial_path),
            final_transcript_markdown: final_path.map(|path| path_string(&path)),
            live_transcript_json: path_string(&live_path),
            chunk_manifest_json: path_string(&chunks_path),
        },
    })
}

enum WriteMode {
    Replace,
    Append,
}

fn write_notes(
    ctx: &Context,
    args: &NoteArgs,
    mode: WriteMode,
) -> Result<NoteWriteResult, CliError> {
    let content = read_note_input(args)?;
    let session_dir = session_dir(ctx, &args.id)?;
    let metadata_path = session_dir.join("session.json");
    if !metadata_path.exists() {
        return Err(CliError::new(
            "sessionMetadataMissing",
            format!("session metadata not found for {}", args.id),
        ));
    }
    let summary_path = session_dir.join("summary.md");
    let final_content = match mode {
        WriteMode::Replace => content,
        WriteMode::Append => {
            let existing = read_optional_string(&summary_path)?.unwrap_or_default();
            if existing.trim().is_empty() {
                content
            } else {
                format!("{existing}\n\n---\n\n{content}")
            }
        }
    };
    fs::write(&summary_path, final_content.as_bytes()).map_err_io("write summary.md")?;
    Ok(NoteWriteResult {
        id: args.id.clone(),
        summary_path: path_string(&summary_path),
        bytes_written: final_content.len(),
    })
}

fn read_note_input(args: &NoteArgs) -> Result<String, CliError> {
    match (&args.file, args.stdin) {
        (Some(_), true) | (None, false) => Err(CliError::new(
            "invalidInput",
            "pass exactly one of --file or --stdin".to_string(),
        )),
        (Some(path), false) => fs::read_to_string(path).map_err_io("read notes file"),
        (None, true) => {
            let mut content = String::new();
            io::stdin()
                .read_to_string(&mut content)
                .map_err_io("read stdin")?;
            Ok(content)
        }
    }
}

fn session_dir(ctx: &Context, id: &str) -> Result<PathBuf, CliError> {
    let dir = ctx.recordings_dir.join(id);
    if dir.is_dir() {
        Ok(dir)
    } else {
        Err(CliError::new(
            "sessionNotFound",
            format!("session not found: {id}"),
        ))
    }
}

fn read_recordings_dir(ctx: &Context) -> Result<fs::ReadDir, CliError> {
    fs::read_dir(&ctx.recordings_dir).map_err(|error| {
        CliError::new(
            "recordingsUnavailable",
            format!(
                "failed to read {}: {error}",
                path_string(&ctx.recordings_dir)
            ),
        )
    })
}

fn read_json(path: &Path) -> Result<Value, CliError> {
    let content = fs::read_to_string(path).map_err_io("read json")?;
    serde_json::from_str(&content).map_err(|error| {
        CliError::new(
            "invalidJson",
            format!("failed to parse {}: {error}", path_string(path)),
        )
    })
}

fn read_optional_string(path: &Path) -> Result<Option<String>, CliError> {
    if path.exists() {
        fs::read_to_string(path)
            .map(Some)
            .map_err_io("read markdown")
    } else {
        Ok(None)
    }
}

fn chunk_status_summary(path: &Path) -> Option<ChunkStatusSummary> {
    let value = read_json(path).ok()?;
    let chunks = value.get("chunks")?.as_array()?;
    let mut summary = ChunkStatusSummary {
        total: chunks.len(),
        queued: 0,
        transcribing: 0,
        done: 0,
        failed: 0,
        updated_at: metadata_string(&value, "updated_at")
            .or_else(|| metadata_string(&value, "generated_at")),
        path: path_string(path),
    };

    for chunk in chunks {
        match chunk
            .get("status")
            .and_then(Value::as_str)
            .unwrap_or("done")
        {
            "queued" => summary.queued += 1,
            "transcribing" => summary.transcribing += 1,
            "failed" => summary.failed += 1,
            _ => summary.done += 1,
        }
    }

    Some(summary)
}

fn transcript_markdown_path(metadata: &Value, session_dir: &Path) -> Option<PathBuf> {
    metadata_string(metadata, "transcript_path")
        .map(PathBuf::from)
        .filter(|path| path.exists())
        .or_else(|| {
            let path = session_dir.join("transcript.md");
            path.exists().then_some(path)
        })
}

fn metadata_string(metadata: &Value, key: &str) -> Option<String> {
    metadata_string_exact(metadata, key).or_else(|| {
        metadata_key_alias(key).and_then(|alias| metadata_string_exact(metadata, &alias))
    })
}

fn metadata_string_exact(metadata: &Value, key: &str) -> Option<String> {
    metadata
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

fn spec() -> Value {
    json!({
        "schemaVersion": SCHEMA_VERSION,
        "commands": {
            "spec": { "writes": [] },
            "capabilities": { "writes": [] },
            "info": { "writes": [] },
            "sessions.list": { "writes": [] },
            "sessions.get": { "writes": [], "options": ["--partial"] },
            "sessions.status": { "writes": [] },
            "sessions.updateNotes": { "writes": ["summary.md"] },
            "sessions.appendNotes": { "writes": ["summary.md"] },
            "meetings.list": { "writes": [], "filters": ["--query", "--company", "--context", "--since", "--needs-enrichment", "--limit"] },
            "meetings.get": { "writes": [], "options": ["--include"] },
            "meetings.context": { "writes": [], "options": ["--format json", "--format llm", "--limit"] },
            "meetings.copy": { "writes": [], "options": ["--kind auto", "--kind meeting", "--kind context", "--format llm"] },
            "meetings.update": { "writes": ["meeting.json", ".poha/meetings.sqlite"], "fields": ["--title", "--company", "--context", "--context-kind"] },
            "meetings.speakers": { "writes": ["meeting.json", ".poha/meetings.sqlite"], "fields": ["--set label=name"] },
            "meetings.reindex": { "writes": [".poha/meetings.sqlite"] },
            "meetings.importArchive": { "writes": ["session.json", "meeting.json", "summary.md", "transcript.md", ".poha/meetings.sqlite"] },
            "meetings.export": { "writes": [".poha/exports"] },
            "storage.report": { "writes": [], "options": ["--limit"] },
            "storage.audioQuality": { "writes": [], "options": ["--limit", "--all", "--full"] },
            "storage.maintain": { "writes": [], "options": ["--dry-run"] },
            "diagnostics.audio": { "writes": ["live-test-report.json", "live-test-report.md"], "options": ["--guided"] }
        },
        "safety": {
            "contract": "Agents may organize and annotate Poha meetings, but must not rewrite source evidence.",
            "readOnlyFiles": [
                "session.json",
                "transcript.md",
                "transcript.partial.md",
                "transcript.json",
                "transcript.live.json",
                "transcript.chunks.json"
            ],
            "writableFiles": ["summary.md", "meeting.json", ".poha/meetings.sqlite", ".poha/exports"],
            "agentSafeWrites": ["summary.md", "meeting.json", ".poha/meetings.sqlite", ".poha/exports"],
            "forbiddenWrites": ["audio files", "transcript.md", "transcript.json", "transcript.partial.md", "transcript.live.json", "transcript.chunks.json", "session.json"]
        }
    })
}

fn capabilities() -> Value {
    json!({
        "schemaVersion": SCHEMA_VERSION,
        "app": "Poha",
        "cli": "poha-cli",
        "output": {
            "format": "json",
            "envelope": ["ok", "command", "data", "meta"]
        },
        "readCommands": [
            "info",
            "sessions.list",
            "sessions.get",
            "sessions.status",
            "meetings.list",
            "meetings.get",
            "meetings.context",
            "meetings.copy",
            "storage.report",
            "storage.audioQuality",
            "storage.maintain"
        ],
        "writeCommands": {
            "sessions.updateNotes": ["summary.md"],
            "sessions.appendNotes": ["summary.md"],
            "meetings.update": ["meeting.json", ".poha/meetings.sqlite"],
            "meetings.speakers": ["meeting.json", ".poha/meetings.sqlite"],
            "meetings.reindex": [".poha/meetings.sqlite"],
            "meetings.importArchive": ["session.json", "meeting.json", "summary.md", "transcript.md", ".poha/meetings.sqlite"],
            "meetings.export": [".poha/exports"],
            "diagnostics.audio": ["live-test-report.json", "live-test-report.md"]
        },
        "safety": {
            "principle": "Codex can annotate, organize, and export. It must not rewrite source evidence.",
            "preferredWritePath": "meeting.json",
            "derivedIndex": ".poha/meetings.sqlite",
            "readOnlyEvidence": [
                "audio files",
                "session.json",
                "transcript.md",
                "transcript.partial.md",
                "transcript.json",
                "transcript.live.json",
                "transcript.chunks.json"
            ],
            "safeAgentWrites": [
                "summary.md",
                "meeting.json",
                ".poha/meetings.sqlite",
                ".poha/exports"
            ]
        }
    })
}

fn info(ctx: &Context) -> Value {
    json!({
        "app": "Poha",
        "cli": "poha-cli",
        "recordingsDir": path_string(&ctx.recordings_dir),
        "recordingsDirExists": ctx.recordings_dir.is_dir()
    })
}

fn path_string(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}

impl CliError {
    fn new(code: &'static str, message: String) -> Self {
        Self { code, message }
    }
}

trait IoResultExt<T> {
    fn map_err_io(self, action: &str) -> Result<T, CliError>;
}

impl<T> IoResultExt<T> for io::Result<T> {
    fn map_err_io(self, action: &str) -> Result<T, CliError> {
        self.map_err(|error| CliError::new("ioError", format!("{action}: {error}")))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn metadata_string_accepts_camel_case_session_keys() {
        let metadata = json!({
            "startedAt": "2026-05-11T21:31:17Z",
            "endedAt": "2026-05-11T22:25:30Z",
            "transcriptPath": "/tmp/transcript.md"
        });

        assert_eq!(
            metadata_string(&metadata, "started_at").as_deref(),
            Some("2026-05-11T21:31:17Z")
        );
        assert_eq!(
            metadata_string(&metadata, "ended_at").as_deref(),
            Some("2026-05-11T22:25:30Z")
        );
        assert_eq!(
            metadata_string(&metadata, "transcript_path").as_deref(),
            Some("/tmp/transcript.md")
        );
    }

    #[test]
    fn chunk_status_summary_counts_live_chunk_states() {
        let dir = tempfile::tempdir().expect("temp dir");
        let path = dir.path().join("transcript.live.json");
        fs::write(
            &path,
            r#"{
              "updatedAt": "2026-05-11T22:00:00Z",
              "chunks": [
                {"status": "queued"},
                {"status": "transcribing"},
                {"status": "done"},
                {"status": "failed"}
              ]
            }"#,
        )
        .expect("write live json");

        let summary = chunk_status_summary(&path).expect("chunk summary");

        assert_eq!(summary.total, 4);
        assert_eq!(summary.queued, 1);
        assert_eq!(summary.transcribing, 1);
        assert_eq!(summary.done, 1);
        assert_eq!(summary.failed, 1);
        assert_eq!(summary.updated_at.as_deref(), Some("2026-05-11T22:00:00Z"));
    }

    #[test]
    fn spec_exposes_partial_get_and_status_commands() {
        let value = spec();
        let commands = value.get("commands").expect("commands");

        assert!(commands.get("sessions.status").is_some());
        assert!(commands.get("meetings.list").is_some());
        assert!(commands.get("meetings.get").is_some());
        assert!(commands.get("meetings.context").is_some());
        assert!(commands.get("meetings.update").is_some());
        assert!(commands.get("meetings.reindex").is_some());
        assert!(commands.get("storage.audioQuality").is_some());
        assert!(commands.get("diagnostics.audio").is_some());
        assert!(
            commands
                .get("meetings.list")
                .and_then(|value| value.get("filters"))
                .and_then(Value::as_array)
                .is_some_and(|filters| filters.iter().any(|value| value == "--needs-enrichment"))
        );
        assert!(
            commands
                .get("storage.audioQuality")
                .and_then(|value| value.get("options"))
                .and_then(Value::as_array)
                .is_some_and(|options| options.iter().any(|value| value == "--all"))
        );
        assert!(
            commands
                .get("diagnostics.audio")
                .and_then(|value| value.get("options"))
                .and_then(Value::as_array)
                .is_some_and(|options| options.iter().any(|value| value == "--guided"))
        );
        assert_eq!(
            commands
                .get("sessions.get")
                .and_then(|value| value.get("options"))
                .and_then(Value::as_array)
                .and_then(|options| options.first())
                .and_then(Value::as_str),
            Some("--partial")
        );
    }

    #[test]
    fn capabilities_publish_safe_write_contract() {
        let value = capabilities();
        let safety = value.get("safety").expect("safety");
        let read_only = safety
            .get("readOnlyEvidence")
            .and_then(Value::as_array)
            .expect("read only evidence");
        let safe_writes = safety
            .get("safeAgentWrites")
            .and_then(Value::as_array)
            .expect("safe writes");

        assert!(read_only.iter().any(|value| value == "transcript.md"));
        assert!(read_only.iter().any(|value| value == "session.json"));
        assert!(safe_writes.iter().any(|value| value == "summary.md"));
        assert!(safe_writes.iter().any(|value| value == "meeting.json"));
        assert!(
            value
                .get("writeCommands")
                .and_then(|value| value.get("meetings.update"))
                .is_some()
        );
    }

    #[test]
    fn meeting_include_filter_keeps_requested_fields() {
        let mut value = json!({
            "item": {"id": "meeting-1"},
            "metadata": {},
            "sessionMetadata": {},
            "summaryMarkdown": "summary",
            "transcriptMarkdown": "transcript",
            "speakerCandidates": ["Me"],
            "paths": {}
        });

        apply_meeting_include_filter(&mut value, Some("metadata,transcript")).expect("filter");

        assert!(value.get("item").is_some());
        assert!(value.get("metadata").is_some());
        assert!(value.get("transcriptMarkdown").is_some());
        assert!(value.get("summaryMarkdown").is_none());
        assert!(value.get("sessionMetadata").is_none());
        assert!(value.get("speakerCandidates").is_none());
        assert!(value.get("paths").is_none());
    }

    #[test]
    fn speaker_sets_parse_label_name_pairs() {
        let map = parse_speaker_sets(&["Me=Adi".to_string(), "Speaker 1=Ravi".to_string()])
            .expect("speaker map");

        assert_eq!(map.get("Me").map(String::as_str), Some("Adi"));
        assert_eq!(map.get("Speaker 1").map(String::as_str), Some("Ravi"));
    }
}
