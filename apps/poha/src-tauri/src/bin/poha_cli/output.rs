use chrono::Utc;
use serde::Serialize;
use std::io;

use super::{CliError, Context, SCHEMA_VERSION, path_string};

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct SuccessEnvelope<T: Serialize> {
    ok: bool,
    command: String,
    data: T,
    meta: Meta,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct FailureEnvelope {
    ok: bool,
    command: String,
    error: CliErrorBody,
    meta: Meta,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct Meta {
    schema_version: u32,
    generated_at: String,
    recordings_dir: String,
    warnings: Vec<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct CliErrorBody {
    code: String,
    message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    idempotency_key: Option<String>,
}

pub(super) fn emit_success<T: Serialize>(
    command: &str,
    data: T,
    ctx: &Context,
    warnings: Vec<String>,
) -> Result<(), CliError> {
    let body = SuccessEnvelope {
        ok: true,
        command: command.to_string(),
        data,
        meta: meta(ctx, warnings),
    };
    print_json(&body)
}

pub(super) fn emit_failure(command: &str, error: CliError, ctx: &Context) {
    let body = FailureEnvelope {
        ok: false,
        command: command.to_string(),
        error: CliErrorBody {
            code: error.code.to_string(),
            message: error.message,
            idempotency_key: error.idempotency_key,
        },
        meta: meta(ctx, vec![]),
    };
    let _ = print_json(&body);
}

fn meta(ctx: &Context, warnings: Vec<String>) -> Meta {
    Meta {
        schema_version: SCHEMA_VERSION,
        generated_at: Utc::now().to_rfc3339(),
        recordings_dir: path_string(&ctx.recordings_dir),
        warnings,
    }
}

fn print_json<T: Serialize>(body: &T) -> Result<(), CliError> {
    let stdout = io::stdout();
    serde_json::to_writer_pretty(stdout.lock(), body).map_err(|error| {
        CliError::new(
            "jsonWriteFailed",
            format!("failed to write json output: {error}"),
        )
    })?;
    println!();
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn failure_body_serializes_retry_key() {
        let value = serde_json::to_value(CliErrorBody {
            code: "controlUnavailable".to_string(),
            message: "connection closed".to_string(),
            idempotency_key: Some("generated-retry-key".to_string()),
        })
        .expect("serialize failure body");

        assert_eq!(
            value.get("idempotencyKey").and_then(|value| value.as_str()),
            Some("generated-retry-key")
        );
    }
}
