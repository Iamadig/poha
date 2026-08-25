//! Permission-light meeting application activity collection.
//!
//! The collector deliberately observes only stable application identities and
//! coarse operating-system activity signals. It never reads window titles,
//! pixels, URLs, or meeting content.

use std::collections::BTreeMap;
use std::error::Error;
use std::fmt;

use crate::calendar_source::MeetingProvider;
use crate::meeting_detection::{
    BrowserMeetingEvidence, MeetingEvidence, NativeActivityStrength, NativeMeetingEvidence,
};

#[cfg(target_os = "macos")]
#[path = "native_meeting_activity/macos.rs"]
mod macos;

/// An application with strong enough activity evidence to be considered a
/// possible live meeting application.
///
/// Browser activity intentionally carries no provider. A sanitized calendar
/// occurrence may enrich it later, but browser evidence always flows through
/// the policy's consent prompt even when no provider can be established.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum ActiveMeetingApplication {
    Native(NativeMeetingEvidence),
    Browser { bundle_id: String },
}

impl ActiveMeetingApplication {
    /// Converts native activity directly and optionally enriches browser
    /// activity with a separately established provider (normally calendar
    /// evidence). Browser conversion never fails solely for lack of provider.
    pub fn to_meeting_evidence(
        &self,
        browser_provider: Option<MeetingProvider>,
    ) -> Option<MeetingEvidence> {
        match self {
            Self::Native(evidence) => Some(MeetingEvidence::Native(evidence.clone())),
            Self::Browser { bundle_id } => Some(MeetingEvidence::Browser(BrowserMeetingEvidence {
                provider: browser_provider,
                browser_id: bundle_id.clone(),
            })),
        }
    }
}

/// A privacy-safe collection error. It exposes only the failed subsystem and
/// an optional OS status, never process identities or user data.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NativeActivityCollectionError {
    component: &'static str,
    status: Option<i32>,
}

impl NativeActivityCollectionError {
    pub fn component(&self) -> &'static str {
        self.component
    }

    pub fn status(&self) -> Option<i32> {
        self.status
    }

    pub(super) const fn unavailable(component: &'static str) -> Self {
        Self {
            component,
            status: None,
        }
    }

    pub(super) const fn os_status(component: &'static str, status: i32) -> Self {
        Self {
            component,
            status: Some(status),
        }
    }
}

impl fmt::Display for NativeActivityCollectionError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.status {
            Some(status) => write!(
                formatter,
                "{} failed with OS status {status}",
                self.component
            ),
            None => write!(formatter, "{} is unavailable", self.component),
        }
    }
}

impl Error for NativeActivityCollectionError {}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct ProcessActivity {
    pub bundle_id: String,
    pub audio_input_active: bool,
    pub audio_output_active: bool,
    pub qualifying_power_assertion: bool,
}

trait ActivitySource {
    fn snapshot(&self) -> Result<Vec<ProcessActivity>, NativeActivityCollectionError>;
}

/// Collects strong meeting-application activity evidence.
///
/// On unsupported systems, or if every native source fails, this returns an
/// error rather than guessing from process names.
pub fn try_collect_active_meeting_applications()
-> Result<Vec<ActiveMeetingApplication>, NativeActivityCollectionError> {
    #[cfg(target_os = "macos")]
    {
        collect_from_source(&macos::MacosActivitySource)
    }

    #[cfg(not(target_os = "macos"))]
    {
        Err(NativeActivityCollectionError::unavailable(
            "native meeting activity collection",
        ))
    }
}

/// Fail-closed convenience wrapper for polling loops.
pub fn collect_active_meeting_applications() -> Vec<ActiveMeetingApplication> {
    match try_collect_active_meeting_applications() {
        Ok(applications) => applications,
        Err(error) => {
            tracing::debug!(%error, "meeting activity poll produced no evidence");
            Vec::new()
        }
    }
}

/// Compatibility helper for callers that only consume existing native policy
/// evidence. Browser activity is intentionally omitted from this native-only
/// view, regardless of whether calendar enrichment is available.
pub fn collect_native_meeting_evidence() -> Vec<MeetingEvidence> {
    collect_active_meeting_applications()
        .iter()
        .filter_map(|application| match application {
            ActiveMeetingApplication::Native(evidence) => {
                Some(MeetingEvidence::Native(evidence.clone()))
            }
            ActiveMeetingApplication::Browser { .. } => None,
        })
        .collect()
}

fn collect_from_source(
    source: &impl ActivitySource,
) -> Result<Vec<ActiveMeetingApplication>, NativeActivityCollectionError> {
    let snapshot = source.snapshot()?;
    Ok(classify_snapshot(&snapshot))
}

#[cfg(test)]
fn collect_fail_closed_from_source(source: &impl ActivitySource) -> Vec<ActiveMeetingApplication> {
    collect_from_source(source).unwrap_or_default()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum SupportedApplication {
    Native {
        provider: MeetingProvider,
        canonical_bundle_id: &'static str,
    },
    Browser {
        canonical_bundle_id: &'static str,
    },
}

#[derive(Debug, Clone, Copy, Default)]
struct AggregateActivity {
    audio_input_active: bool,
    audio_output_active: bool,
    qualifying_power_assertion: bool,
}

impl AggregateActivity {
    fn include(&mut self, activity: &ProcessActivity) {
        self.audio_input_active |= activity.audio_input_active;
        self.audio_output_active |= activity.audio_output_active;
        self.qualifying_power_assertion |= activity.qualifying_power_assertion;
    }

    fn native_strength(self) -> Option<NativeActivityStrength> {
        if self.audio_input_active && self.audio_output_active {
            Some(NativeActivityStrength::DuplexAudio)
        } else if self.qualifying_power_assertion {
            Some(NativeActivityStrength::PowerAssertionOnly)
        } else {
            None
        }
    }

    fn browser_is_active(self) -> bool {
        self.audio_input_active && self.audio_output_active
    }
}

fn classify_snapshot(snapshot: &[ProcessActivity]) -> Vec<ActiveMeetingApplication> {
    let mut applications = BTreeMap::<SupportedApplication, AggregateActivity>::new();
    for process in snapshot {
        let Some(application) = supported_application(&process.bundle_id) else {
            continue;
        };
        applications
            .entry(application)
            .or_default()
            .include(process);
    }

    applications
        .into_iter()
        .filter_map(|(application, activity)| match application {
            SupportedApplication::Native {
                provider,
                canonical_bundle_id,
            } => activity.native_strength().map(|strength| {
                ActiveMeetingApplication::Native(NativeMeetingEvidence {
                    provider,
                    application_id: canonical_bundle_id.to_owned(),
                    strength,
                })
            }),
            SupportedApplication::Browser {
                canonical_bundle_id,
            } if activity.browser_is_active() => Some(ActiveMeetingApplication::Browser {
                bundle_id: canonical_bundle_id.to_owned(),
            }),
            _ => None,
        })
        .collect()
}

/// Exact allowlist only. Helper identities below are vendor-shipped call/media
/// hosts and normalize to their parent application's stable identity.
fn supported_application(bundle_id: &str) -> Option<SupportedApplication> {
    let application = match bundle_id {
        "us.zoom.xos" | "us.zoom.CptHost" | "us.zoom.zCCIMeetingHost" => {
            SupportedApplication::Native {
                provider: MeetingProvider::Zoom,
                canonical_bundle_id: "us.zoom.xos",
            }
        }
        "com.microsoft.teams2" | "com.microsoft.teams" => SupportedApplication::Native {
            provider: MeetingProvider::Teams,
            canonical_bundle_id: "com.microsoft.teams2",
        },
        "Cisco-Systems.Spark"
        | "Cisco-Systems.Spark.media"
        | "com.cisco.washost"
        | "com.cisco.webexmeetingsapp" => SupportedApplication::Native {
            provider: MeetingProvider::Webex,
            canonical_bundle_id: "Cisco-Systems.Spark",
        },
        "com.apple.Safari" => SupportedApplication::Browser {
            canonical_bundle_id: "com.apple.Safari",
        },
        "com.google.Chrome" | "com.google.Chrome.helper" | "com.google.Chrome.helper.renderer" => {
            SupportedApplication::Browser {
                canonical_bundle_id: "com.google.Chrome",
            }
        }
        "com.microsoft.edgemac"
        | "com.microsoft.edgemac.helper"
        | "com.microsoft.edgemac.helper.renderer" => SupportedApplication::Browser {
            canonical_bundle_id: "com.microsoft.edgemac",
        },
        "org.mozilla.firefox" => SupportedApplication::Browser {
            canonical_bundle_id: "org.mozilla.firefox",
        },
        "com.brave.Browser" | "com.brave.Browser.helper" | "com.brave.Browser.helper.renderer" => {
            SupportedApplication::Browser {
                canonical_bundle_id: "com.brave.Browser",
            }
        }
        "company.thebrowser.Browser"
        | "company.thebrowser.Browser.helper"
        | "company.thebrowser.Browser.helper.renderer" => SupportedApplication::Browser {
            canonical_bundle_id: "company.thebrowser.Browser",
        },
        _ => return None,
    };
    Some(application)
}

#[cfg(test)]
mod tests {
    use super::*;

    struct MockSource(Result<Vec<ProcessActivity>, NativeActivityCollectionError>);

    impl ActivitySource for MockSource {
        fn snapshot(&self) -> Result<Vec<ProcessActivity>, NativeActivityCollectionError> {
            self.0.clone()
        }
    }

    fn activity(
        bundle_id: &str,
        audio_input_active: bool,
        audio_output_active: bool,
        qualifying_power_assertion: bool,
    ) -> ProcessActivity {
        ProcessActivity {
            bundle_id: bundle_id.to_owned(),
            audio_input_active,
            audio_output_active,
            qualifying_power_assertion,
        }
    }

    #[test]
    fn detects_supported_native_apps_from_duplex_audio() {
        let applications = classify_snapshot(&[
            activity("us.zoom.xos", true, true, false),
            activity("com.microsoft.teams2", true, true, false),
            activity("Cisco-Systems.Spark", true, true, false),
        ]);

        assert_eq!(applications.len(), 3);
        assert!(applications.iter().any(|application| matches!(
            application,
            ActiveMeetingApplication::Native(evidence)
                if evidence.provider == MeetingProvider::Zoom
                    && evidence.application_id == "us.zoom.xos"
                    && evidence.strength == NativeActivityStrength::DuplexAudio
        )));
        assert!(applications.iter().any(|application| matches!(
            application,
            ActiveMeetingApplication::Native(evidence)
                if evidence.provider == MeetingProvider::Teams
                    && evidence.application_id == "com.microsoft.teams2"
                    && evidence.strength == NativeActivityStrength::DuplexAudio
        )));
        assert!(applications.iter().any(|application| matches!(
            application,
            ActiveMeetingApplication::Native(evidence)
                if evidence.provider == MeetingProvider::Webex
                    && evidence.application_id == "Cisco-Systems.Spark"
                    && evidence.strength == NativeActivityStrength::DuplexAudio
        )));
    }

    #[test]
    fn power_assertion_is_a_native_fallback_but_not_a_browser_signal() {
        let applications = classify_snapshot(&[
            activity("us.zoom.xos", false, false, true),
            activity("com.google.Chrome", false, false, true),
        ]);

        assert!(matches!(
            applications.as_slice(),
            [ActiveMeetingApplication::Native(NativeMeetingEvidence {
                provider: MeetingProvider::Zoom,
                application_id,
                strength: NativeActivityStrength::PowerAssertionOnly,
            })] if application_id == "us.zoom.xos"
        ));
    }

    #[test]
    fn aggregates_vendor_helpers_but_rejects_one_sided_activity() {
        let applications = classify_snapshot(&[
            activity("us.zoom.xos", true, false, false),
            activity("us.zoom.CptHost", false, true, false),
            activity("com.microsoft.teams2", false, true, false),
        ]);

        assert!(matches!(
            applications.as_slice(),
            [ActiveMeetingApplication::Native(NativeMeetingEvidence {
                provider: MeetingProvider::Zoom,
                application_id,
                strength: NativeActivityStrength::DuplexAudio,
            })] if application_id == "us.zoom.xos"
        ));
    }

    #[test]
    fn ignores_unknown_and_lookalike_bundle_ids() {
        assert!(
            classify_snapshot(&[
                activity("evil.us.zoom.xos", true, true, true),
                activity("com.microsoft.teams2.preview", true, true, true),
            ])
            .is_empty()
        );
    }

    #[test]
    fn browser_requires_duplex_audio_but_not_a_provider() {
        let applications = classify_snapshot(&[
            activity("com.google.Chrome", true, true, false),
            activity("com.apple.Safari", false, true, false),
        ]);
        assert_eq!(
            applications,
            vec![ActiveMeetingApplication::Browser {
                bundle_id: "com.google.Chrome".to_owned(),
            }]
        );
        assert_eq!(
            applications[0].to_meeting_evidence(None),
            Some(MeetingEvidence::Browser(BrowserMeetingEvidence {
                provider: None,
                browser_id: "com.google.Chrome".to_owned(),
            }))
        );
        assert_eq!(
            applications[0].to_meeting_evidence(Some(MeetingProvider::Meet)),
            Some(MeetingEvidence::Browser(BrowserMeetingEvidence {
                provider: Some(MeetingProvider::Meet),
                browser_id: "com.google.Chrome".to_owned(),
            }))
        );
    }

    #[test]
    fn browser_audio_helpers_are_canonicalized_to_the_parent_identity() {
        let applications = classify_snapshot(&[
            activity("com.google.Chrome.helper.renderer", true, false, false),
            activity("com.google.Chrome.helper", false, true, false),
        ]);
        assert_eq!(
            applications,
            vec![ActiveMeetingApplication::Browser {
                bundle_id: "com.google.Chrome".to_owned(),
            }]
        );
    }

    #[test]
    fn source_error_fails_closed() {
        let error = NativeActivityCollectionError::os_status("mock source", -1);
        let source = MockSource(Err(error.clone()));

        assert_eq!(collect_from_source(&source), Err(error));
        assert!(collect_fail_closed_from_source(&source).is_empty());
    }

    #[test]
    fn duplicate_rows_are_deduplicated_deterministically() {
        let source = MockSource(Ok(vec![
            activity("Cisco-Systems.Spark", true, true, false),
            activity("Cisco-Systems.Spark.media", true, true, true),
        ]));

        let applications = collect_from_source(&source).unwrap();
        assert!(matches!(
            applications.as_slice(),
            [ActiveMeetingApplication::Native(NativeMeetingEvidence {
                provider: MeetingProvider::Webex,
                application_id,
                strength: NativeActivityStrength::DuplexAudio,
            })] if application_id == "Cisco-Systems.Spark"
        ));
    }
}
