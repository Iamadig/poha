use chrono::Utc;
use poha_lib::storage_lifecycle::{
    StorageAudioQualityReport, StorageMaintenancePlan, StorageReport,
};

use super::{CliError, Context};

pub(super) fn report(ctx: &Context, limit: usize) -> Result<StorageReport, CliError> {
    poha_lib::storage_lifecycle::report(&ctx.recordings_dir, limit)
        .map_err(|error| CliError::new(error.code(), error.message().to_string()))
}

pub(super) fn audio_quality(
    ctx: &Context,
    limit: usize,
    include_ok: bool,
    full_scan: bool,
) -> Result<StorageAudioQualityReport, CliError> {
    let analysis_window_seconds = if full_scan {
        None
    } else {
        Some(poha_lib::storage_lifecycle::DEFAULT_AUDIO_QUALITY_ANALYSIS_WINDOW_SECONDS)
    };
    poha_lib::storage_lifecycle::audio_quality_report_with_options(
        &ctx.recordings_dir,
        limit,
        include_ok,
        analysis_window_seconds,
    )
    .map_err(|error| CliError::new(error.code(), error.message().to_string()))
}

pub(super) fn maintain_dry_run(ctx: &Context) -> Result<StorageMaintenancePlan, CliError> {
    poha_lib::storage_lifecycle::maintenance_plan(&ctx.recordings_dir, Utc::now())
        .map_err(|error| CliError::new(error.code(), error.message().to_string()))
}

#[cfg(test)]
#[path = "storage_tests.rs"]
mod tests;
