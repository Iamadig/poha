use std::time::{Duration, Instant};

const DEFAULT_MIN_RECORDING_SECS: u64 = 2 * 60;
const DEFAULT_SYSTEM_PROMPT_SECS: [u64; 4] = [30, 90, 150, 3 * 60];
const DEFAULT_SYSTEM_AUTO_STOP_SECS: u64 = 3 * 60;
const DEFAULT_MIC_AUTO_STOP_SECS: u64 = 60;
const DEFAULT_SNOOZE_SECS: u64 = 15 * 60;
const DEFAULT_QUIET_LEVEL: u16 = 20;
const DEFAULT_TICK_MS: u64 = 5_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AudioLevels {
    pub mic: u16,
    pub speaker: u16,
}

impl AudioLevels {
    fn mic_active(self, quiet_level: u16) -> bool {
        self.mic > quiet_level
    }

    fn speaker_active(self, quiet_level: u16) -> bool {
        self.speaker > quiet_level
    }
}

#[derive(Debug, Clone)]
pub struct RecordingEndReminderConfig {
    pub min_recording_duration: Duration,
    pub prompt_after: [Duration; 4],
    pub system_auto_stop_after: Duration,
    pub mic_auto_stop_after: Duration,
    pub snooze_duration: Duration,
    pub quiet_level: u16,
    pub tick_interval: Duration,
}

impl Default for RecordingEndReminderConfig {
    fn default() -> Self {
        Self {
            min_recording_duration: Duration::from_secs(DEFAULT_MIN_RECORDING_SECS),
            prompt_after: DEFAULT_SYSTEM_PROMPT_SECS.map(Duration::from_secs),
            system_auto_stop_after: Duration::from_secs(DEFAULT_SYSTEM_AUTO_STOP_SECS),
            mic_auto_stop_after: Duration::from_secs(DEFAULT_MIC_AUTO_STOP_SECS),
            snooze_duration: Duration::from_secs(DEFAULT_SNOOZE_SECS),
            quiet_level: DEFAULT_QUIET_LEVEL,
            tick_interval: Duration::from_millis(DEFAULT_TICK_MS),
        }
    }
}

impl RecordingEndReminderConfig {
    pub fn from_env() -> Self {
        let mut config = Self::default();
        if let Some(seconds) = env_u64("POHA_MEETING_END_MIN_RECORDING_SECS") {
            config.min_recording_duration = Duration::from_secs(seconds);
        }
        if let Some(seconds) = env_u64("POHA_MEETING_END_AUTO_STOP_SECS") {
            config.system_auto_stop_after = Duration::from_secs(seconds);
        }
        if let Some(seconds) = env_u64("POHA_MEETING_END_SYSTEM_AUTO_STOP_SECS") {
            config.system_auto_stop_after = Duration::from_secs(seconds);
        }
        if let Some(seconds) = env_u64("POHA_MEETING_END_MIC_AUTO_STOP_SECS") {
            config.mic_auto_stop_after = Duration::from_secs(seconds);
        }
        if let Some(seconds) = env_u64("POHA_MEETING_END_SNOOZE_SECS") {
            config.snooze_duration = Duration::from_secs(seconds);
        }
        if let Some(level) = env_u64("POHA_MEETING_END_QUIET_LEVEL") {
            config.quiet_level = level.min(u64::from(u16::MAX)) as u16;
        }
        if let Some(ms) = env_u64("POHA_MEETING_END_TICK_MS") {
            config.tick_interval = Duration::from_millis(ms.max(100));
        }
        if let Ok(value) = std::env::var("POHA_MEETING_END_PROMPT_SECS")
            && let Some(prompt_after) = parse_prompt_thresholds(&value)
        {
            config.prompt_after = prompt_after;
        }
        if let Ok(value) = std::env::var("POHA_MEETING_END_SYSTEM_PROMPT_SECS")
            && let Some(prompt_after) = parse_prompt_thresholds(&value)
        {
            config.prompt_after = prompt_after;
        }
        config
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SilenceDurations {
    pub system_quiet_for: Duration,
    pub mic_quiet_for: Duration,
    pub auto_stop_in: Duration,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RecordingEndReminderAction {
    Notify {
        level: u8,
        system_quiet_for: Duration,
        mic_quiet_for: Duration,
        auto_stop_in: Duration,
    },
    AutoStop {
        system_quiet_for: Duration,
        mic_quiet_for: Duration,
    },
}

#[derive(Debug, Clone)]
pub struct RecordingEndReminderState {
    system_quiet_since: Option<Instant>,
    mic_quiet_since: Option<Instant>,
    last_prompt_level: u8,
    response_requested: bool,
    auto_stop_requested: bool,
    snoozed_until: Option<Instant>,
}

impl Default for RecordingEndReminderState {
    fn default() -> Self {
        Self {
            system_quiet_since: None,
            mic_quiet_since: None,
            last_prompt_level: 0,
            response_requested: false,
            auto_stop_requested: false,
            snoozed_until: None,
        }
    }
}

impl RecordingEndReminderState {
    pub fn reset_for_recording(&mut self, now: Instant) {
        *self = Self {
            system_quiet_since: Some(now),
            mic_quiet_since: Some(now),
            ..Self::default()
        };
    }

    pub fn clear(&mut self) {
        *self = Self::default();
    }

    pub fn observe_audio(
        &mut self,
        now: Instant,
        recording_started_at: Option<Instant>,
        levels: AudioLevels,
        config: &RecordingEndReminderConfig,
    ) -> Vec<RecordingEndReminderAction> {
        let speaker_active = levels.speaker_active(config.quiet_level);
        let mic_active = levels.mic_active(config.quiet_level);

        if speaker_active {
            self.observe_system_activity(now);
        } else if self.system_quiet_since.is_none() {
            self.system_quiet_since = Some(now);
        }

        if mic_active {
            self.observe_mic_activity(now);
        } else if self.mic_quiet_since.is_none() {
            self.mic_quiet_since = Some(now);
        }

        if speaker_active {
            return Vec::new();
        }

        self.evaluate(now, recording_started_at, config)
    }

    fn observe_system_activity(&mut self, now: Instant) {
        self.system_quiet_since = Some(now);
        self.last_prompt_level = 0;
        self.response_requested = false;
        self.auto_stop_requested = false;
        self.snoozed_until = None;
    }

    fn observe_mic_activity(&mut self, now: Instant) {
        self.mic_quiet_since = Some(now);
        self.auto_stop_requested = false;
    }

    pub fn snooze(&mut self, now: Instant, config: &RecordingEndReminderConfig) {
        self.system_quiet_since = Some(now);
        self.mic_quiet_since = Some(now);
        self.last_prompt_level = 0;
        self.response_requested = false;
        self.auto_stop_requested = false;
        self.snoozed_until = Some(now + config.snooze_duration);
    }

    pub fn mark_response_requested(&mut self) {
        self.response_requested = true;
    }

    pub fn evaluate(
        &mut self,
        now: Instant,
        recording_started_at: Option<Instant>,
        config: &RecordingEndReminderConfig,
    ) -> Vec<RecordingEndReminderAction> {
        if self.auto_stop_requested {
            return Vec::new();
        }

        if let Some(snoozed_until) = self.snoozed_until {
            if now < snoozed_until {
                return Vec::new();
            }
            self.snoozed_until = None;
            self.system_quiet_since = Some(now);
            self.mic_quiet_since = Some(now);
            self.last_prompt_level = 0;
            self.response_requested = false;
            return Vec::new();
        }

        let Some(recording_started_at) = recording_started_at else {
            return Vec::new();
        };
        let reminder_window_start = recording_started_at + config.min_recording_duration;
        if now < reminder_window_start {
            return Vec::new();
        }

        let Some(durations) = self.silence_elapsed(now, Some(recording_started_at), config) else {
            return Vec::new();
        };

        for (index, threshold) in config.prompt_after.iter().copied().enumerate() {
            let level = (index + 1) as u8;
            if durations.system_quiet_for >= threshold && self.last_prompt_level < level {
                self.last_prompt_level = level;
                return vec![RecordingEndReminderAction::Notify {
                    level,
                    system_quiet_for: durations.system_quiet_for,
                    mic_quiet_for: durations.mic_quiet_for,
                    auto_stop_in: durations.auto_stop_in,
                }];
            }
        }

        if durations.system_quiet_for >= config.system_auto_stop_after
            && durations.mic_quiet_for >= config.mic_auto_stop_after
            && self.response_requested
        {
            self.auto_stop_requested = true;
            return vec![RecordingEndReminderAction::AutoStop {
                system_quiet_for: durations.system_quiet_for,
                mic_quiet_for: durations.mic_quiet_for,
            }];
        }

        Vec::new()
    }

    pub fn needs_response(&self) -> bool {
        self.response_requested && !self.auto_stop_requested
    }

    pub fn silence_elapsed(
        &self,
        now: Instant,
        recording_started_at: Option<Instant>,
        config: &RecordingEndReminderConfig,
    ) -> Option<SilenceDurations> {
        let recording_started_at = recording_started_at?;
        let reminder_window_start = recording_started_at + config.min_recording_duration;
        if now < reminder_window_start || self.snoozed_until.is_some_and(|until| now < until) {
            return None;
        }
        let system_quiet_since = self.system_quiet_since?.max(reminder_window_start);
        let mic_quiet_since = self.mic_quiet_since?.max(reminder_window_start);
        let system_quiet_for = now.saturating_duration_since(system_quiet_since);
        let mic_quiet_for = now.saturating_duration_since(mic_quiet_since);
        Some(SilenceDurations {
            system_quiet_for,
            mic_quiet_for,
            auto_stop_in: config
                .system_auto_stop_after
                .saturating_sub(system_quiet_for)
                .max(config.mic_auto_stop_after.saturating_sub(mic_quiet_for)),
        })
    }
}

pub fn reminder_status_detail(
    durations: SilenceDurations,
    config: &RecordingEndReminderConfig,
) -> Option<String> {
    if durations.system_quiet_for < config.prompt_after[0] {
        return None;
    }
    Some(format!(
        "System quiet {}; mic quiet {}; auto-stop in {}; Keep Recording available",
        duration_label(durations.system_quiet_for),
        duration_label(durations.mic_quiet_for),
        duration_label(durations.auto_stop_in)
    ))
}

pub fn duration_label(duration: Duration) -> String {
    let seconds = duration.as_secs();
    let minutes = seconds / 60;
    let seconds = seconds % 60;
    if minutes == 0 {
        format!("{seconds}s")
    } else if seconds == 0 {
        format!("{minutes}m")
    } else {
        format!("{minutes}m {seconds}s")
    }
}

fn env_u64(name: &str) -> Option<u64> {
    std::env::var(name)
        .ok()
        .and_then(|value| value.trim().parse::<u64>().ok())
}

fn parse_prompt_thresholds(value: &str) -> Option<[Duration; 4]> {
    let parsed = value
        .split(',')
        .map(str::trim)
        .map(str::parse::<u64>)
        .collect::<Result<Vec<_>, _>>()
        .ok()?;
    let [first, second, third, fourth]: [u64; 4] = parsed.try_into().ok()?;
    Some([first, second, third, fourth].map(Duration::from_secs))
}

#[cfg(test)]
#[path = "recording_end_reminder_tests.rs"]
mod tests;
