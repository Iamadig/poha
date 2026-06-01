use super::tail_string_with_limit;

const FAILURE_MESSAGE_TAIL_CHARS: usize = 1000;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct CodexFailureDetail {
    pub(super) kind: &'static str,
    pub(super) message: String,
    pub(super) resumable: bool,
}

pub(super) fn classify_codex_failure(
    stdout: &str,
    stderr: &str,
    exit_code: Option<i32>,
) -> CodexFailureDetail {
    let combined = format!("{stderr}\n{stdout}");
    let lower = combined.to_lowercase();

    if lower.contains("not inside a trusted directory") && lower.contains("--skip-git-repo-check") {
        return CodexFailureDetail {
            kind: "trusted_directory",
            message: "Codex refused Poha's generated workspace: Not inside a trusted directory and --skip-git-repo-check was not specified. This Poha build should pass that flag; check the Codex launch arguments.".to_string(),
            resumable: false,
        };
    }

    if lower.contains("invalid_grant")
        || lower.contains("token refresh failed")
        || lower.contains("oauth token refresh failed")
        || lower.contains("auth error")
    {
        return CodexFailureDetail {
            kind: "codex_auth",
            message: "Codex authentication failed. Run `codex login` in Terminal, then retry queued summaries.".to_string(),
            resumable: false,
        };
    }

    if lower.contains("cargo")
        && (lower.contains("error:")
            || lower.contains("failed")
            || lower.contains("could not compile")
            || lower.contains("no such file"))
    {
        return CodexFailureDetail {
            kind: "poha_cli",
            message: format!(
                "Codex hit a Poha CLI/Cargo error: {}",
                command_text_failure_line(&combined)
                    .unwrap_or_else(|| command_output_text_tail(&combined))
            ),
            resumable: false,
        };
    }

    if lower.contains("rate limit")
        || lower.contains("timed out")
        || lower.contains("temporarily unavailable")
    {
        return CodexFailureDetail {
            kind: "transient",
            message: format!(
                "Codex hit a transient error: {}",
                command_text_failure_line(&combined)
                    .unwrap_or_else(|| command_output_text_tail(&combined))
            ),
            resumable: true,
        };
    }

    CodexFailureDetail {
        kind: "codex_failed",
        message: format!(
            "Codex exited {}: {}",
            exit_code
                .map(|code| code.to_string())
                .unwrap_or_else(|| "without a code".to_string()),
            command_text_failure_line(&combined)
                .unwrap_or_else(|| command_output_text_tail(&combined))
        ),
        resumable: true,
    }
}

fn command_output_text_tail(text: &str) -> String {
    let tail = tail_string_with_limit(text.trim(), FAILURE_MESSAGE_TAIL_CHARS);
    if tail.is_empty() {
        "no output captured".to_string()
    } else {
        tail
    }
}

fn command_text_failure_line(text: &str) -> Option<String> {
    let lines = text
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .collect::<Vec<_>>();
    lines
        .iter()
        .find(|line| line.to_lowercase().starts_with("error:"))
        .or_else(|| lines.last())
        .map(|line| tail_string_with_limit(line, FAILURE_MESSAGE_TAIL_CHARS))
}
