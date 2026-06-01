use std::path::{Path, PathBuf};
use std::process::Command;

use super::{POHA_STATE_DIR_NAME, shell_quote, tail_string_with_limit};

const FAILURE_MESSAGE_TAIL_CHARS: usize = 1000;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum PohaCliInvocation {
    Direct {
        path: PathBuf,
    },
    Cargo {
        cargo_path: PathBuf,
        rustc_path: PathBuf,
        manifest_path: PathBuf,
        cargo_target_dir: PathBuf,
    },
}

impl PohaCliInvocation {
    pub(super) fn command_prefix(&self, recordings_dir: &Path) -> String {
        match self {
            Self::Direct { path } => {
                format!(
                    "{} --recordings-dir {}",
                    shell_quote(path),
                    shell_quote(recordings_dir)
                )
            }
            Self::Cargo {
                cargo_path,
                rustc_path,
                manifest_path,
                cargo_target_dir,
            } => {
                format!(
                    "env PATH={} CARGO_TARGET_DIR={} RUSTC={} {} run --quiet --locked --manifest-path {} --bin poha-cli -- --recordings-dir {}",
                    shell_quote(&PathBuf::from(tool_path_env())),
                    shell_quote(cargo_target_dir),
                    shell_quote(rustc_path),
                    shell_quote(cargo_path),
                    shell_quote(manifest_path),
                    shell_quote(recordings_dir)
                )
            }
        }
    }
}

pub(super) fn resolve_codex_cli() -> Result<PathBuf, String> {
    if let Ok(path) = std::env::var("CODEX_CLI") {
        let path = PathBuf::from(path);
        if path.is_file() {
            return Ok(path);
        }
    }

    let mut candidates = Vec::new();
    if let Some(home) = dirs::home_dir() {
        candidates.push(home.join(".local/bin/codex"));
        candidates.push(home.join(".cargo/bin/codex"));
    }
    candidates.push(PathBuf::from("/opt/homebrew/bin/codex"));
    candidates.push(PathBuf::from("/usr/local/bin/codex"));
    candidates.push(PathBuf::from("/usr/bin/codex"));

    for candidate in candidates {
        if candidate.is_file() {
            return Ok(candidate);
        }
    }

    match Command::new("which").arg("codex").output() {
        Ok(output) if output.status.success() => {
            let path = String::from_utf8_lossy(&output.stdout).trim().to_string();
            if !path.is_empty() {
                return Ok(PathBuf::from(path));
            }
        }
        _ => {}
    }

    Err("Codex CLI not found. Set CODEX_CLI or install codex on PATH.".to_string())
}

pub(super) fn resolve_poha_cli_invocation(
    cargo_target_dir: &Path,
) -> Result<PohaCliInvocation, String> {
    if let Some(path) = resolve_direct_poha_cli() {
        return Ok(PohaCliInvocation::Direct { path });
    }

    let cargo_path = resolve_cargo_cli()?;
    let rustc_path = resolve_rustc_cli()?;
    let manifest_path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("Cargo.toml");
    Ok(PohaCliInvocation::Cargo {
        cargo_path,
        rustc_path,
        manifest_path,
        cargo_target_dir: cargo_target_dir.to_path_buf(),
    })
}

fn resolve_direct_poha_cli() -> Option<PathBuf> {
    if let Ok(path) = std::env::var("POHA_CLI") {
        let path = PathBuf::from(path);
        if path.is_file() {
            return Some(path);
        }
    }

    let current_exe = std::env::current_exe().ok()?;
    let exe_dir = current_exe.parent()?;
    ["poha-cli", "poha_cli"]
        .iter()
        .map(|name| exe_dir.join(name))
        .find(|path| path.is_file())
}

fn resolve_cargo_cli() -> Result<PathBuf, String> {
    if let Ok(path) = std::env::var("CARGO") {
        let path = PathBuf::from(path);
        if path.is_file() {
            return Ok(path);
        }
    }

    let mut candidates = Vec::new();
    if let Some(home) = dirs::home_dir() {
        candidates.push(home.join(".cargo/bin/cargo"));
    }
    candidates.push(PathBuf::from("/opt/homebrew/bin/cargo"));
    candidates.push(PathBuf::from("/usr/local/bin/cargo"));

    for candidate in candidates {
        if candidate.is_file() {
            return Ok(candidate);
        }
    }

    match Command::new("which").arg("cargo").output() {
        Ok(output) if output.status.success() => {
            let path = String::from_utf8_lossy(&output.stdout).trim().to_string();
            if !path.is_empty() {
                return Ok(PathBuf::from(path));
            }
        }
        _ => {}
    }

    Err("Cargo CLI not found. Poha queued summaries need cargo to run poha-cli; install Rust or set CARGO.".to_string())
}

fn resolve_rustc_cli() -> Result<PathBuf, String> {
    if let Ok(path) = std::env::var("RUSTC") {
        let path = PathBuf::from(path);
        if path.is_file() {
            return Ok(path);
        }
    }

    let mut candidates = Vec::new();
    if let Some(home) = dirs::home_dir() {
        candidates.push(home.join(".cargo/bin/rustc"));
    }
    candidates.push(PathBuf::from("/opt/homebrew/bin/rustc"));
    candidates.push(PathBuf::from("/usr/local/bin/rustc"));

    for candidate in candidates {
        if candidate.is_file() {
            return Ok(candidate);
        }
    }

    match Command::new("which").arg("rustc").output() {
        Ok(output) if output.status.success() => {
            let path = String::from_utf8_lossy(&output.stdout).trim().to_string();
            if !path.is_empty() {
                return Ok(PathBuf::from(path));
            }
        }
        _ => {}
    }

    Err("Rust compiler not found. Poha queued summaries need rustc when falling back to cargo; install Rust or set RUSTC.".to_string())
}

pub(super) fn preflight_codex_enrichment(
    codex_path: &Path,
    cli_invocation: &PohaCliInvocation,
    workspace_dir: &Path,
    recordings_dir: &Path,
) -> Result<(), String> {
    if let PohaCliInvocation::Cargo { manifest_path, .. } = cli_invocation
        && !manifest_path.is_file()
    {
        return Err(format!(
            "Poha CLI manifest not found at {}. Rebuild Poha from a checkout that still exists.",
            manifest_path.display()
        ));
    }
    validate_codex_exec_supports_non_repo(codex_path)?;
    ensure_recordings_writable(recordings_dir)?;
    ensure_workspace_is_directory(workspace_dir)?;
    run_poha_cli_reindex_preflight(cli_invocation, recordings_dir)
}

fn validate_codex_exec_supports_non_repo(codex_path: &Path) -> Result<(), String> {
    let output = Command::new(codex_path)
        .args(["exec", "--help"])
        .output()
        .map_err(|e| format!("failed checking Codex CLI at {}: {e}", codex_path.display()))?;
    let help = format!(
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    if !output.status.success() {
        return Err(format!(
            "Codex CLI preflight failed at {}: {}",
            codex_path.display(),
            tail_string_with_limit(&help, FAILURE_MESSAGE_TAIL_CHARS)
        ));
    }
    validate_codex_exec_help(&help).map_err(|message| {
        format!(
            "{message} Found Codex at {}. Update Codex CLI, then retry queued summaries.",
            codex_path.display()
        )
    })
}

pub(super) fn validate_codex_exec_help(help: &str) -> Result<(), String> {
    if help.contains("--skip-git-repo-check") {
        Ok(())
    } else {
        Err("Codex CLI does not support --skip-git-repo-check.".to_string())
    }
}

fn ensure_recordings_writable(recordings_dir: &Path) -> Result<(), String> {
    let state_dir = recordings_dir.join(POHA_STATE_DIR_NAME);
    std::fs::create_dir_all(&state_dir).map_err(|e| {
        format!(
            "failed creating Poha state dir {}: {e}",
            state_dir.display()
        )
    })?;
    let probe_path = state_dir.join(format!("codex-write-check-{}.tmp", std::process::id()));
    std::fs::write(&probe_path, b"ok").map_err(|e| {
        format!(
            "recordings dir is not writable; failed writing {}: {e}",
            probe_path.display()
        )
    })?;
    std::fs::remove_file(&probe_path).map_err(|e| {
        format!(
            "recordings dir write check could not clean up {}: {e}",
            probe_path.display()
        )
    })?;
    Ok(())
}

fn ensure_workspace_is_directory(workspace_dir: &Path) -> Result<(), String> {
    if workspace_dir.is_dir() {
        Ok(())
    } else {
        Err(format!(
            "Codex workspace is not a directory: {}",
            workspace_dir.display()
        ))
    }
}

fn run_poha_cli_reindex_preflight(
    cli_invocation: &PohaCliInvocation,
    recordings_dir: &Path,
) -> Result<(), String> {
    let command = format!(
        "{} meetings reindex",
        cli_invocation.command_prefix(recordings_dir)
    );
    let output = match cli_invocation {
        PohaCliInvocation::Direct { path } => Command::new(path)
            .arg("--recordings-dir")
            .arg(recordings_dir)
            .args(["meetings", "reindex"])
            .output(),
        PohaCliInvocation::Cargo {
            cargo_path,
            rustc_path,
            manifest_path,
            cargo_target_dir,
        } => Command::new(cargo_path)
            .env("PATH", tool_path_env())
            .env("CARGO_TARGET_DIR", cargo_target_dir)
            .env("RUSTC", rustc_path)
            .args(["run", "--quiet", "--locked", "--manifest-path"])
            .arg(manifest_path)
            .args(["--bin", "poha-cli", "--", "--recordings-dir"])
            .arg(recordings_dir)
            .args(["meetings", "reindex"])
            .output(),
    }
    .map_err(|e| format!("Poha CLI preflight failed starting `{command}`: {e}"))?;
    if output.status.success() {
        return Ok(());
    }
    Err(format!(
        "Poha CLI preflight failed while running `{command}`: {}",
        command_output_tail(&output)
    ))
}

fn command_output_tail(output: &std::process::Output) -> String {
    let text = format!(
        "{}\n{}",
        String::from_utf8_lossy(&output.stderr),
        String::from_utf8_lossy(&output.stdout)
    );
    let tail = tail_string_with_limit(text.trim(), FAILURE_MESSAGE_TAIL_CHARS);
    if tail.is_empty() {
        "no output captured".to_string()
    } else {
        tail
    }
}

fn tool_path_env() -> String {
    let mut paths = std::env::var("PATH")
        .unwrap_or_default()
        .split(':')
        .filter(|path| !path.is_empty())
        .map(str::to_string)
        .collect::<Vec<_>>();
    for path in [
        "/opt/homebrew/bin",
        "/usr/local/bin",
        "/usr/bin",
        "/bin",
        "/usr/sbin",
        "/sbin",
    ] {
        if !paths.iter().any(|existing| existing == path) {
            paths.push(path.to_string());
        }
    }
    if let Some(home) = dirs::home_dir() {
        let cargo_bin = home.join(".cargo/bin").to_string_lossy().into_owned();
        if !paths.iter().any(|existing| existing == &cargo_bin) {
            paths.push(cargo_bin);
        }
    }
    paths.join(":")
}
