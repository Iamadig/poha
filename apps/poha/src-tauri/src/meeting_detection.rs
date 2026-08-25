use std::collections::{BTreeMap, BTreeSet};
use std::hash::{Hash, Hasher};

use serde::{Deserialize, Serialize};

use crate::calendar_source::{CalendarOccurrence, MeetingProvider, select_occurrence};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub enum AutomationMode {
    #[default]
    Off,
    Ask,
    /// Calendar-assisted confirmation; never starts capture autonomously.
    CalendarAssisted,
}

#[derive(Debug, Clone)]
pub struct NativeMeetingEvidence {
    pub provider: MeetingProvider,
    /// Stable application identity such as a bundle ID, never a window title.
    pub application_id: String,
    /// The strongest coarse OS activity signal observed for this application.
    pub strength: NativeActivityStrength,
}

// Strength is observation metadata, not application identity. Keeping it out
// of equality preserves debounce, latching, and dismiss cooldowns when a
// power-only observation upgrades to duplex audio.
impl PartialEq for NativeMeetingEvidence {
    fn eq(&self, other: &Self) -> bool {
        self.provider == other.provider && self.application_id == other.application_id
    }
}

impl Eq for NativeMeetingEvidence {}

impl PartialOrd for NativeMeetingEvidence {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for NativeMeetingEvidence {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        (self.provider, &self.application_id).cmp(&(other.provider, &other.application_id))
    }
}

impl Hash for NativeMeetingEvidence {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.provider.hash(state);
        self.application_id.hash(state);
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum NativeActivityStrength {
    /// A call-correlated IOKit assertion without confirmed duplex audio. This
    /// signal may prompt, but is never sufficient for automatic recording.
    PowerAssertionOnly,
    /// CoreAudio reports both active input and output for the application.
    DuplexAudio,
}

#[derive(Debug, Clone)]
pub struct BrowserMeetingEvidence {
    /// Calendar-derived enrichment. Browser activity remains actionable when
    /// this is absent, but can only ever prompt for consent.
    pub provider: Option<MeetingProvider>,
    /// Concrete browser identity. Consent and cooldowns must not be shared.
    pub browser_id: String,
}

// Provider is calendar enrichment, not browser identity. A calendar refresh
// must not bypass an existing prompt latch or dismissal cooldown.
impl PartialEq for BrowserMeetingEvidence {
    fn eq(&self, other: &Self) -> bool {
        self.browser_id == other.browser_id
    }
}

impl Eq for BrowserMeetingEvidence {}

impl PartialOrd for BrowserMeetingEvidence {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for BrowserMeetingEvidence {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        self.browser_id.cmp(&other.browser_id)
    }
}

impl Hash for BrowserMeetingEvidence {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.browser_id.hash(state);
    }
}

/// Native sorts before browser evidence so overlap resolution is stable and a
/// browser's generic WebRTC signal cannot shadow a provider-specific native hit.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum MeetingEvidence {
    Native(NativeMeetingEvidence),
    Browser(BrowserMeetingEvidence),
}

impl MeetingEvidence {
    pub fn provider(&self) -> Option<MeetingProvider> {
        match self {
            Self::Native(evidence) => Some(evidence.provider),
            Self::Browser(evidence) => evidence.provider,
        }
    }

    fn has_identity(&self) -> bool {
        match self {
            Self::Native(evidence) => !evidence.application_id.trim().is_empty(),
            Self::Browser(evidence) => !evidence.browser_id.trim().is_empty(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AutomationPolicyConfig {
    pub mode: AutomationMode,
    pub required_consecutive_hits: u8,
    pub cooldown_ms: i64,
    pub schedule_early_tolerance_ms: i64,
    pub schedule_late_tolerance_ms: i64,
}

impl Default for AutomationPolicyConfig {
    fn default() -> Self {
        Self {
            mode: AutomationMode::Off,
            required_consecutive_hits: 2,
            cooldown_ms: 10 * 60 * 1_000,
            schedule_early_tolerance_ms: 10 * 60 * 1_000,
            schedule_late_tolerance_ms: 5 * 60 * 1_000,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PromptReason {
    AskMode,
    ScheduledNativeRequiresConsent,
    NativeMeetingIsUnscheduled,
    NativeEvidenceRequiresConsent,
    BrowserRequiresConsent,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NoActionReason {
    Off,
    NoEvidence,
    Debouncing,
    Cooldown,
    SuppressedUntilEvidenceEnds,
    AlreadyHandled,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AutomationDecision {
    NoAction(NoActionReason),
    Prompt {
        evidence: MeetingEvidence,
        scheduled_occurrence: Option<CalendarOccurrence>,
        reason: PromptReason,
    },
}

#[derive(Debug, Clone)]
pub struct MeetingAutomationPolicy {
    config: AutomationPolicyConfig,
    consecutive_hits: BTreeMap<MeetingEvidence, u8>,
    latched: BTreeSet<MeetingEvidence>,
    cooldown_until: BTreeMap<MeetingEvidence, i64>,
    global_cooldown_until: Option<i64>,
    suppress_until_no_evidence: bool,
    consecutive_empty_hits: u8,
}

impl Default for MeetingAutomationPolicy {
    fn default() -> Self {
        Self::new(AutomationPolicyConfig::default())
    }
}

impl MeetingAutomationPolicy {
    pub fn new(mut config: AutomationPolicyConfig) -> Self {
        config.required_consecutive_hits = config.required_consecutive_hits.max(1);
        config.cooldown_ms = config.cooldown_ms.max(0);
        config.schedule_early_tolerance_ms = config.schedule_early_tolerance_ms.max(0);
        config.schedule_late_tolerance_ms = config.schedule_late_tolerance_ms.max(0);
        Self {
            config,
            consecutive_hits: BTreeMap::new(),
            latched: BTreeSet::new(),
            cooldown_until: BTreeMap::new(),
            global_cooldown_until: None,
            suppress_until_no_evidence: false,
            consecutive_empty_hits: 0,
        }
    }

    pub fn config(&self) -> &AutomationPolicyConfig {
        &self.config
    }

    pub fn set_mode(&mut self, mode: AutomationMode) {
        if self.config.mode != mode {
            self.config.mode = mode;
            self.reset_transient_state();
        }
    }

    pub fn reset_observations(&mut self) {
        self.reset_transient_state();
    }

    /// Observes one detector poll. Duplicate evidence in the same poll counts
    /// once, and any missed poll resets that evidence's debounce counter.
    pub fn observe(
        &mut self,
        evidence: &[MeetingEvidence],
        calendar: &[CalendarOccurrence],
        now_unix_ms: i64,
    ) -> AutomationDecision {
        if self.config.mode == AutomationMode::Off {
            self.reset_transient_state();
            return AutomationDecision::NoAction(NoActionReason::Off);
        }

        self.expire_cooldowns(now_unix_ms);
        let observed = evidence
            .iter()
            .filter(|evidence| evidence.has_identity())
            .cloned()
            .collect::<BTreeSet<_>>();

        if self.suppress_until_no_evidence {
            self.reset_transient_state();
            if observed.is_empty() {
                self.consecutive_empty_hits = self.consecutive_empty_hits.saturating_add(1);
                if self.consecutive_empty_hits < self.config.required_consecutive_hits {
                    return AutomationDecision::NoAction(
                        NoActionReason::SuppressedUntilEvidenceEnds,
                    );
                }
                self.suppress_until_no_evidence = false;
                self.consecutive_empty_hits = 0;
                self.global_cooldown_until =
                    Some(now_unix_ms.saturating_add(self.config.cooldown_ms));
                return AutomationDecision::NoAction(NoActionReason::Cooldown);
            }
            self.consecutive_empty_hits = 0;
            return AutomationDecision::NoAction(NoActionReason::SuppressedUntilEvidenceEnds);
        }

        self.consecutive_hits
            .retain(|candidate, _| observed.contains(candidate));
        self.latched
            .retain(|candidate| observed.contains(candidate));

        if observed.is_empty() {
            return AutomationDecision::NoAction(NoActionReason::NoEvidence);
        }

        let global_cooldown = self
            .global_cooldown_until
            .is_some_and(|until| now_unix_ms < until);
        let mut saw_cooldown = false;
        for candidate in &observed {
            let candidate_cooldown = self
                .cooldown_until
                .get(candidate)
                .is_some_and(|until| now_unix_ms < *until);
            if global_cooldown || candidate_cooldown {
                saw_cooldown = true;
                self.consecutive_hits.remove(candidate);
                continue;
            }
            let count = self.consecutive_hits.entry(candidate.clone()).or_default();
            *count = count.saturating_add(1);
        }

        let mut saw_debouncing = false;
        let mut saw_latched = false;
        let mut ready = Vec::new();
        for candidate in observed {
            if global_cooldown || self.cooldown_until.contains_key(&candidate) {
                continue;
            }
            if self.consecutive_hits.get(&candidate).copied().unwrap_or(0)
                < self.config.required_consecutive_hits
            {
                saw_debouncing = true;
                continue;
            }
            if self.latched.contains(&candidate) {
                saw_latched = true;
                continue;
            }

            let scheduled = candidate.provider().and_then(|provider| {
                select_occurrence(
                    calendar,
                    provider,
                    now_unix_ms,
                    self.config.schedule_early_tolerance_ms,
                    self.config.schedule_late_tolerance_ms,
                )
                .cloned()
            });
            ready.push((candidate, scheduled));
        }

        ready.sort_by(|left, right| left.0.cmp(&right.0));
        if let Some((candidate, scheduled)) = ready.into_iter().next() {
            let decision = self.decision_for(candidate.clone(), scheduled);
            self.latched.insert(candidate);
            return decision;
        }

        if saw_cooldown {
            AutomationDecision::NoAction(NoActionReason::Cooldown)
        } else if saw_debouncing {
            AutomationDecision::NoAction(NoActionReason::Debouncing)
        } else if saw_latched {
            AutomationDecision::NoAction(NoActionReason::AlreadyHandled)
        } else {
            AutomationDecision::NoAction(NoActionReason::NoEvidence)
        }
    }

    /// Suppresses a dismissed prompt for this concrete application/browser.
    pub fn dismiss(&mut self, evidence: &MeetingEvidence, now_unix_ms: i64) {
        self.cooldown(evidence, now_unix_ms);
    }

    /// A manual stop remains in force until every meeting signal disappears.
    /// Only then does the ordinary global cooldown begin. This prevents a long
    /// call from being offered again merely because a timer elapsed.
    pub fn manual_stop(&mut self) {
        self.suppress_until_no_evidence = true;
        self.consecutive_empty_hits = 0;
        self.global_cooldown_until = None;
        self.reset_transient_state();
    }

    fn decision_for(
        &self,
        evidence: MeetingEvidence,
        scheduled_occurrence: Option<CalendarOccurrence>,
    ) -> AutomationDecision {
        match evidence {
            MeetingEvidence::Browser(evidence) => AutomationDecision::Prompt {
                evidence: MeetingEvidence::Browser(evidence),
                scheduled_occurrence,
                reason: PromptReason::BrowserRequiresConsent,
            },
            MeetingEvidence::Native(evidence) => match self.config.mode {
                // Calendar provider + time is useful prompt context, but cannot
                // bind the active process to that occurrence. Consent is always
                // required until a trustworthy identity binding exists.
                AutomationMode::CalendarAssisted => AutomationDecision::Prompt {
                    reason: if evidence.strength == NativeActivityStrength::PowerAssertionOnly {
                        PromptReason::NativeEvidenceRequiresConsent
                    } else if scheduled_occurrence.is_some() {
                        PromptReason::ScheduledNativeRequiresConsent
                    } else {
                        PromptReason::NativeMeetingIsUnscheduled
                    },
                    evidence: MeetingEvidence::Native(evidence),
                    scheduled_occurrence,
                },
                AutomationMode::Ask => AutomationDecision::Prompt {
                    evidence: MeetingEvidence::Native(evidence),
                    scheduled_occurrence,
                    reason: PromptReason::AskMode,
                },
                AutomationMode::Off => AutomationDecision::NoAction(NoActionReason::Off),
            },
        }
    }

    fn cooldown(&mut self, evidence: &MeetingEvidence, now_unix_ms: i64) {
        self.cooldown_until.insert(
            evidence.clone(),
            now_unix_ms.saturating_add(self.config.cooldown_ms),
        );
        self.consecutive_hits.remove(evidence);
        self.latched.remove(evidence);
    }

    fn expire_cooldowns(&mut self, now_unix_ms: i64) {
        self.cooldown_until.retain(|_, until| now_unix_ms < *until);
        if self
            .global_cooldown_until
            .is_some_and(|until| now_unix_ms >= until)
        {
            self.global_cooldown_until = None;
        }
    }

    fn reset_transient_state(&mut self) {
        self.consecutive_hits.clear();
        self.latched.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn config(mode: AutomationMode) -> AutomationPolicyConfig {
        AutomationPolicyConfig {
            mode,
            required_consecutive_hits: 2,
            cooldown_ms: 100,
            schedule_early_tolerance_ms: 10,
            schedule_late_tolerance_ms: 10,
        }
    }

    fn native(provider: MeetingProvider) -> MeetingEvidence {
        MeetingEvidence::Native(NativeMeetingEvidence {
            provider,
            application_id: format!("com.example.{provider:?}"),
            strength: NativeActivityStrength::DuplexAudio,
        })
    }

    fn browser(provider: MeetingProvider) -> MeetingEvidence {
        MeetingEvidence::Browser(BrowserMeetingEvidence {
            provider: Some(provider),
            browser_id: "com.google.Chrome".to_string(),
        })
    }

    fn providerless_browser() -> MeetingEvidence {
        MeetingEvidence::Browser(BrowserMeetingEvidence {
            provider: None,
            browser_id: "com.google.Chrome".to_string(),
        })
    }

    fn power_only_native(provider: MeetingProvider) -> MeetingEvidence {
        MeetingEvidence::Native(NativeMeetingEvidence {
            provider,
            application_id: format!("com.example.{provider:?}"),
            strength: NativeActivityStrength::PowerAssertionOnly,
        })
    }

    fn occurrence(provider: MeetingProvider, start: i64, end: i64, id: &str) -> CalendarOccurrence {
        CalendarOccurrence {
            starts_at_unix_ms: start,
            ends_at_unix_ms: end,
            provider,
            occurrence_id_hash: id.to_string(),
        }
    }

    #[test]
    fn automation_defaults_off() {
        let policy = MeetingAutomationPolicy::default();
        assert_eq!(policy.config().mode, AutomationMode::Off);
        assert_eq!(AutomationMode::default(), AutomationMode::Off);
    }

    #[test]
    fn off_mode_ignores_evidence_and_resets_debounce() {
        let mut policy = MeetingAutomationPolicy::default();
        let evidence = native(MeetingProvider::Zoom);
        assert_eq!(
            policy.observe(&[evidence.clone()], &[], 1),
            AutomationDecision::NoAction(NoActionReason::Off)
        );
        policy.set_mode(AutomationMode::Ask);
        assert_eq!(
            policy.observe(&[evidence], &[], 2),
            AutomationDecision::NoAction(NoActionReason::Debouncing)
        );
    }

    #[test]
    fn consecutive_hits_are_required_and_a_missed_poll_resets_them() {
        let mut policy = MeetingAutomationPolicy::new(config(AutomationMode::Ask));
        let evidence = native(MeetingProvider::Teams);
        assert_eq!(
            policy.observe(&[evidence.clone()], &[], 1),
            AutomationDecision::NoAction(NoActionReason::Debouncing)
        );
        assert_eq!(
            policy.observe(&[], &[], 2),
            AutomationDecision::NoAction(NoActionReason::NoEvidence)
        );
        assert_eq!(
            policy.observe(&[evidence.clone(), evidence.clone()], &[], 3),
            AutomationDecision::NoAction(NoActionReason::Debouncing),
            "a duplicate hit in one poll must count once"
        );
        assert!(matches!(
            policy.observe(&[evidence], &[], 4),
            AutomationDecision::Prompt {
                reason: PromptReason::AskMode,
                ..
            }
        ));
    }

    #[test]
    fn calendar_data_alone_can_never_start() {
        let mut policy = MeetingAutomationPolicy::new(config(AutomationMode::CalendarAssisted));
        let calendar = [occurrence(MeetingProvider::Zoom, 90, 200, "zoom")];
        for now in [90, 100, 150] {
            assert_eq!(
                policy.observe(&[], &calendar, now),
                AutomationDecision::NoAction(NoActionReason::NoEvidence)
            );
        }
    }

    #[test]
    fn calendar_assisted_mode_prompts_for_matching_native_evidence() {
        let mut policy = MeetingAutomationPolicy::new(config(AutomationMode::CalendarAssisted));
        let evidence = native(MeetingProvider::Zoom);
        let calendar = [
            occurrence(MeetingProvider::Teams, 90, 200, "teams"),
            occurrence(MeetingProvider::Zoom, 90, 200, "zoom"),
        ];
        policy.observe(&[evidence.clone()], &calendar, 100);
        assert!(matches!(
            policy.observe(&[evidence], &calendar, 101),
            AutomationDecision::Prompt {
                evidence: MeetingEvidence::Native(NativeMeetingEvidence {
                    provider: MeetingProvider::Zoom,
                    ..
                }),
                scheduled_occurrence: Some(CalendarOccurrence {
                    occurrence_id_hash,
                    ..
                }),
                reason: PromptReason::ScheduledNativeRequiresConsent,
            } if occurrence_id_hash == "zoom"
        ));
    }

    #[test]
    fn unscheduled_native_evidence_prompts_in_calendar_assisted_mode() {
        let mut policy = MeetingAutomationPolicy::new(config(AutomationMode::CalendarAssisted));
        let evidence = native(MeetingProvider::Webex);
        policy.observe(&[evidence.clone()], &[], 100);
        assert!(matches!(
            policy.observe(&[evidence], &[], 101),
            AutomationDecision::Prompt {
                reason: PromptReason::NativeMeetingIsUnscheduled,
                ..
            }
        ));
    }

    #[test]
    fn calendar_context_never_changes_the_consent_requirement() {
        let mut policy = MeetingAutomationPolicy::new(config(AutomationMode::CalendarAssisted));
        let zoom = native(MeetingProvider::Zoom);
        let teams = native(MeetingProvider::Teams);
        let calendar = [occurrence(MeetingProvider::Teams, 90, 200, "teams")];
        let evidence = [zoom.clone(), teams.clone()];

        policy.observe(&evidence, &calendar, 100);
        assert!(matches!(
            policy.observe(&evidence, &calendar, 101),
            AutomationDecision::Prompt {
                evidence: MeetingEvidence::Native(_),
                ..
            }
        ));
    }

    #[test]
    fn browser_evidence_always_prompts_even_when_scheduled() {
        let mut policy = MeetingAutomationPolicy::new(config(AutomationMode::CalendarAssisted));
        let evidence = browser(MeetingProvider::Meet);
        let calendar = [occurrence(MeetingProvider::Meet, 90, 200, "meet")];
        policy.observe(&[evidence.clone()], &calendar, 100);
        assert!(matches!(
            policy.observe(&[evidence], &calendar, 101),
            AutomationDecision::Prompt {
                evidence: MeetingEvidence::Browser(_),
                scheduled_occurrence: Some(_),
                reason: PromptReason::BrowserRequiresConsent,
            }
        ));
    }

    #[test]
    fn providerless_browser_evidence_still_prompts() {
        let mut policy = MeetingAutomationPolicy::new(config(AutomationMode::CalendarAssisted));
        let evidence = providerless_browser();
        let unrelated_calendar = [occurrence(MeetingProvider::Meet, 90, 200, "meet")];
        policy.observe(&[evidence.clone()], &unrelated_calendar, 100);

        assert!(matches!(
            policy.observe(&[evidence], &unrelated_calendar, 101),
            AutomationDecision::Prompt {
                evidence: MeetingEvidence::Browser(BrowserMeetingEvidence { provider: None, .. }),
                scheduled_occurrence: None,
                reason: PromptReason::BrowserRequiresConsent,
            }
        ));
    }

    #[test]
    fn scheduled_power_assertion_only_native_evidence_requires_consent() {
        let mut policy = MeetingAutomationPolicy::new(config(AutomationMode::CalendarAssisted));
        let evidence = power_only_native(MeetingProvider::Zoom);
        let calendar = [occurrence(MeetingProvider::Zoom, 90, 200, "zoom")];
        policy.observe(&[evidence.clone()], &calendar, 100);

        assert!(matches!(
            policy.observe(&[evidence], &calendar, 101),
            AutomationDecision::Prompt {
                evidence: MeetingEvidence::Native(NativeMeetingEvidence {
                    strength: NativeActivityStrength::PowerAssertionOnly,
                    ..
                }),
                scheduled_occurrence: Some(CalendarOccurrence {
                    occurrence_id_hash,
                    ..
                }),
                reason: PromptReason::NativeEvidenceRequiresConsent,
            } if occurrence_id_hash == "zoom"
        ));
    }

    #[test]
    fn duplex_upgrade_is_used_without_resetting_the_debounce_identity() {
        let mut policy = MeetingAutomationPolicy::new(config(AutomationMode::CalendarAssisted));
        let power_only = power_only_native(MeetingProvider::Zoom);
        let duplex = native(MeetingProvider::Zoom);
        let calendar = [occurrence(MeetingProvider::Zoom, 90, 200, "zoom")];

        assert_eq!(
            policy.observe(&[power_only], &calendar, 100),
            AutomationDecision::NoAction(NoActionReason::Debouncing)
        );
        assert!(matches!(
            policy.observe(&[duplex], &calendar, 101),
            AutomationDecision::Prompt {
                evidence: MeetingEvidence::Native(NativeMeetingEvidence {
                    strength: NativeActivityStrength::DuplexAudio,
                    ..
                }),
                reason: PromptReason::ScheduledNativeRequiresConsent,
                scheduled_occurrence: Some(_),
            }
        ));
    }

    #[test]
    fn strength_and_calendar_enrichment_do_not_bypass_identity_cooldowns() {
        let mut policy = MeetingAutomationPolicy::new(config(AutomationMode::CalendarAssisted));
        let power_only = power_only_native(MeetingProvider::Zoom);
        let duplex = native(MeetingProvider::Zoom);
        let calendar = [occurrence(MeetingProvider::Zoom, 90, 200, "zoom")];
        policy.observe(&[power_only.clone()], &calendar, 1);
        policy.observe(&[power_only.clone()], &calendar, 2);
        policy.dismiss(&power_only, 10);
        assert_eq!(
            policy.observe(&[duplex], &calendar, 50),
            AutomationDecision::NoAction(NoActionReason::Cooldown)
        );

        let providerless = providerless_browser();
        let enriched = browser(MeetingProvider::Meet);
        assert_eq!(providerless, enriched);
    }

    #[test]
    fn a_prompt_is_latched_instead_of_repeated_every_poll() {
        let mut policy = MeetingAutomationPolicy::new(config(AutomationMode::Ask));
        let evidence = browser(MeetingProvider::Meet);
        policy.observe(&[evidence.clone()], &[], 1);
        assert!(matches!(
            policy.observe(&[evidence.clone()], &[], 2),
            AutomationDecision::Prompt { .. }
        ));
        assert_eq!(
            policy.observe(&[evidence], &[], 3),
            AutomationDecision::NoAction(NoActionReason::AlreadyHandled)
        );
    }

    #[test]
    fn dismiss_applies_per_identity_cooldown_then_redebounces() {
        let mut policy = MeetingAutomationPolicy::new(config(AutomationMode::Ask));
        let evidence = browser(MeetingProvider::Meet);
        policy.observe(&[evidence.clone()], &[], 1);
        policy.observe(&[evidence.clone()], &[], 2);
        policy.dismiss(&evidence, 10);

        assert_eq!(
            policy.observe(&[evidence.clone()], &[], 109),
            AutomationDecision::NoAction(NoActionReason::Cooldown)
        );
        assert_eq!(
            policy.observe(&[evidence.clone()], &[], 110),
            AutomationDecision::NoAction(NoActionReason::Debouncing)
        );
        assert!(matches!(
            policy.observe(&[evidence], &[], 111),
            AutomationDecision::Prompt { .. }
        ));
    }

    #[test]
    fn manual_stop_suppresses_for_the_evidence_lifetime_then_cools_down() {
        let mut policy = MeetingAutomationPolicy::new(config(AutomationMode::Ask));
        let evidence = native(MeetingProvider::Zoom);
        policy.manual_stop();
        assert_eq!(
            policy.observe(&[evidence.clone()], &[], 10_000),
            AutomationDecision::NoAction(NoActionReason::SuppressedUntilEvidenceEnds),
            "an active call must stay suppressed even long after the cooldown duration"
        );
        assert_eq!(
            policy.observe(&[], &[], 10_001),
            AutomationDecision::NoAction(NoActionReason::SuppressedUntilEvidenceEnds),
            "one empty poll is not enough to prove that the call ended"
        );
        assert_eq!(
            policy.observe(&[], &[], 10_002),
            AutomationDecision::NoAction(NoActionReason::Cooldown),
            "the cooldown begins only after all evidence disappears"
        );
        assert_eq!(
            policy.observe(&[evidence.clone()], &[], 10_101),
            AutomationDecision::NoAction(NoActionReason::Cooldown)
        );
        assert_eq!(
            policy.observe(&[evidence.clone()], &[], 10_102),
            AutomationDecision::NoAction(NoActionReason::Debouncing)
        );
        assert!(matches!(
            policy.observe(&[evidence], &[], 10_103),
            AutomationDecision::Prompt { .. }
        ));
    }

    #[test]
    fn simultaneous_native_and_browser_hits_resolve_to_native_regardless_of_input_order() {
        let schedule = [occurrence(MeetingProvider::Meet, 90, 200, "meet")];
        let native = native(MeetingProvider::Meet);
        let browser = browser(MeetingProvider::Meet);

        for evidence in [
            vec![browser.clone(), native.clone()],
            vec![native.clone(), browser.clone()],
        ] {
            let mut policy = MeetingAutomationPolicy::new(config(AutomationMode::CalendarAssisted));
            policy.observe(&evidence, &schedule, 100);
            assert!(matches!(
                policy.observe(&evidence, &schedule, 101),
                AutomationDecision::Prompt {
                    evidence: MeetingEvidence::Native(NativeMeetingEvidence { .. }),
                    ..
                }
            ));
        }
    }
}
