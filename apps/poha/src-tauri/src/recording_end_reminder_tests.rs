use super::*;

fn test_config() -> RecordingEndReminderConfig {
    RecordingEndReminderConfig {
        min_recording_duration: Duration::from_secs(5),
        prompt_after: [1, 2, 3, 4].map(Duration::from_secs),
        system_auto_stop_after: Duration::from_secs(5),
        mic_auto_stop_after: Duration::from_secs(2),
        snooze_duration: Duration::from_secs(15),
        quiet_level: 20,
        tick_interval: Duration::from_millis(250),
    }
}

fn quiet() -> AudioLevels {
    AudioLevels { mic: 0, speaker: 0 }
}

#[test]
fn waits_for_min_recording_duration_before_prompting() {
    let config = test_config();
    let started = Instant::now();
    let mut state = RecordingEndReminderState::default();
    state.reset_for_recording(started);

    let actions = state.observe_audio(
        started + Duration::from_secs(4),
        Some(started),
        quiet(),
        &config,
    );

    assert!(actions.is_empty());
}

#[test]
fn emits_prompt_levels_on_continuous_quiet() {
    let config = test_config();
    let started = Instant::now();
    let mut state = RecordingEndReminderState::default();
    state.reset_for_recording(started);

    assert_eq!(
        state.observe_audio(
            started + Duration::from_secs(6),
            Some(started),
            quiet(),
            &config,
        ),
        vec![RecordingEndReminderAction::Notify {
            level: 1,
            system_quiet_for: Duration::from_secs(1),
            mic_quiet_for: Duration::from_secs(1),
            auto_stop_in: Duration::from_secs(4),
        }]
    );

    assert_eq!(
        state.evaluate(started + Duration::from_secs(7), Some(started), &config),
        vec![RecordingEndReminderAction::Notify {
            level: 2,
            system_quiet_for: Duration::from_secs(2),
            mic_quiet_for: Duration::from_secs(2),
            auto_stop_in: Duration::from_secs(3),
        }]
    );
}

#[test]
fn resets_when_system_audio_activity_returns() {
    let config = test_config();
    let started = Instant::now();
    let mut state = RecordingEndReminderState::default();
    state.reset_for_recording(started);
    let _ = state.observe_audio(
        started + Duration::from_secs(6),
        Some(started),
        quiet(),
        &config,
    );
    state.mark_response_requested();

    let actions = state.observe_audio(
        started + Duration::from_secs(7),
        Some(started),
        AudioLevels {
            mic: 0,
            speaker: 21,
        },
        &config,
    );

    assert!(actions.is_empty());
    assert!(!state.needs_response());
}

#[test]
fn mic_activity_blocks_auto_stop_without_resetting_system_silence() {
    let config = test_config();
    let started = Instant::now();
    let mut state = RecordingEndReminderState::default();
    state.reset_for_recording(started);
    let _ = state.observe_audio(
        started + Duration::from_secs(6),
        Some(started),
        quiet(),
        &config,
    );
    let _ = state.evaluate(started + Duration::from_secs(7), Some(started), &config);
    let _ = state.evaluate(started + Duration::from_secs(8), Some(started), &config);
    let _ = state.evaluate(started + Duration::from_secs(9), Some(started), &config);
    state.mark_response_requested();

    assert!(
        state
            .observe_audio(
                started + Duration::from_secs(10),
                Some(started),
                AudioLevels {
                    mic: 21,
                    speaker: 0,
                },
                &config,
            )
            .is_empty()
    );
    assert!(state.needs_response());
    assert!(
        state
            .evaluate(started + Duration::from_secs(11), Some(started), &config)
            .is_empty()
    );
    assert_eq!(
        state.evaluate(started + Duration::from_secs(12), Some(started), &config),
        vec![RecordingEndReminderAction::AutoStop {
            system_quiet_for: Duration::from_secs(7),
            mic_quiet_for: Duration::from_secs(2),
        }]
    );
}

#[test]
fn auto_stops_only_after_response_was_requested() {
    let config = test_config();
    let started = Instant::now();
    let mut state = RecordingEndReminderState::default();
    state.reset_for_recording(started);
    let _ = state.observe_audio(
        started + Duration::from_secs(6),
        Some(started),
        quiet(),
        &config,
    );
    let _ = state.evaluate(started + Duration::from_secs(7), Some(started), &config);
    let _ = state.evaluate(started + Duration::from_secs(8), Some(started), &config);
    let _ = state.evaluate(started + Duration::from_secs(9), Some(started), &config);

    assert!(
        state
            .evaluate(started + Duration::from_secs(10), Some(started), &config)
            .is_empty()
    );

    state.mark_response_requested();
    assert_eq!(
        state.evaluate(started + Duration::from_secs(10), Some(started), &config),
        vec![RecordingEndReminderAction::AutoStop {
            system_quiet_for: Duration::from_secs(5),
            mic_quiet_for: Duration::from_secs(5),
        }]
    );
}

#[test]
fn keep_recording_snoozes_current_quiet_cycle() {
    let config = test_config();
    let started = Instant::now();
    let mut state = RecordingEndReminderState::default();
    state.reset_for_recording(started);
    let _ = state.observe_audio(
        started + Duration::from_secs(6),
        Some(started),
        quiet(),
        &config,
    );
    state.mark_response_requested();
    state.snooze(started + Duration::from_secs(6), &config);

    assert!(
        state
            .evaluate(started + Duration::from_secs(10), Some(started), &config)
            .is_empty()
    );
    assert!(!state.needs_response());
    assert!(
        state
            .evaluate(started + Duration::from_secs(22), Some(started), &config)
            .is_empty()
    );
    assert_eq!(
        state.evaluate(started + Duration::from_secs(23), Some(started), &config),
        vec![RecordingEndReminderAction::Notify {
            level: 1,
            system_quiet_for: Duration::from_secs(1),
            mic_quiet_for: Duration::from_secs(1),
            auto_stop_in: Duration::from_secs(4),
        }]
    );
}

#[test]
fn reminder_status_detail_keeps_tray_actionable() {
    let config = test_config();

    assert_eq!(
        reminder_status_detail(
            SilenceDurations {
                system_quiet_for: Duration::from_secs(1),
                mic_quiet_for: Duration::from_secs(1),
                auto_stop_in: Duration::from_secs(4),
            },
            &config,
        ),
        Some(
            "System quiet 1s; mic quiet 1s; auto-stop in 4s; Keep Recording available".to_string()
        )
    );
    assert_eq!(
        reminder_status_detail(
            SilenceDurations {
                system_quiet_for: Duration::from_secs(4),
                mic_quiet_for: Duration::from_secs(1),
                auto_stop_in: Duration::from_secs(1),
            },
            &config,
        ),
        Some(
            "System quiet 4s; mic quiet 1s; auto-stop in 1s; Keep Recording available".to_string()
        )
    );
}
