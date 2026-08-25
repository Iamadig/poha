use std::cmp::Reverse;

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum MeetingProvider {
    Zoom,
    Teams,
    Meet,
    Webex,
}

/// The only calendar data that may cross Poha's persistence boundary.
///
/// Titles, raw URLs, attendees, locations, descriptions, and provider secrets
/// are deliberately absent. Runtime adapters should discard their source event
/// after constructing this value.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CalendarOccurrence {
    pub starts_at_unix_ms: i64,
    pub ends_at_unix_ms: i64,
    pub provider: MeetingProvider,
    pub occurrence_id_hash: String,
}

/// Transient calendar input. This type intentionally accepts neither attendee
/// data nor event notes, keeping the ingestion surface as narrow as possible.
#[derive(Debug, Clone, Copy)]
pub struct CalendarEventInput<'a> {
    pub starts_at_unix_ms: i64,
    pub ends_at_unix_ms: i64,
    pub occurrence_id: &'a str,
    pub url: Option<&'a str>,
    pub location: Option<&'a str>,
    pub is_all_day: bool,
    pub is_cancelled: bool,
}

/// Ephemeral recognition result suitable for diagnostics. The redacted value
/// never contains a query, fragment, meeting identifier, password, or token.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecognizedMeetingLink {
    pub provider: MeetingProvider,
    pub redacted_url: String,
}

pub fn sanitize_calendar_event(input: CalendarEventInput<'_>) -> Option<CalendarOccurrence> {
    if input.is_all_day
        || input.is_cancelled
        || input.ends_at_unix_ms <= input.starts_at_unix_ms
        || input.occurrence_id.trim().is_empty()
    {
        return None;
    }

    let link = input
        .url
        .and_then(recognize_meeting_link)
        .or_else(|| input.location.and_then(recognize_meeting_link))?;
    Some(CalendarOccurrence {
        starts_at_unix_ms: input.starts_at_unix_ms,
        ends_at_unix_ms: input.ends_at_unix_ms,
        provider: link.provider,
        occurrence_id_hash: hash_occurrence_id(input.occurrence_id.trim()),
    })
}

pub fn recognize_meeting_link(value: &str) -> Option<RecognizedMeetingLink> {
    recognize_single_url(value).or_else(|| {
        value
            .split_whitespace()
            .map(trim_url_wrapper)
            .find_map(recognize_single_url)
    })
}

/// Selects one matching occurrence deterministically, even when calendars
/// contain duplicates or overlapping meetings.
///
/// An event currently in progress wins over one admitted only by tolerance;
/// then the nearest start, latest start, earliest end, and hashed ID break ties.
pub fn select_occurrence<'a>(
    occurrences: &'a [CalendarOccurrence],
    provider: MeetingProvider,
    now_unix_ms: i64,
    early_tolerance_ms: i64,
    late_tolerance_ms: i64,
) -> Option<&'a CalendarOccurrence> {
    let early_tolerance_ms = early_tolerance_ms.max(0);
    let late_tolerance_ms = late_tolerance_ms.max(0);

    occurrences
        .iter()
        .filter(|occurrence| occurrence.provider == provider)
        .filter(|occurrence| {
            now_unix_ms
                >= occurrence
                    .starts_at_unix_ms
                    .saturating_sub(early_tolerance_ms)
                && now_unix_ms <= occurrence.ends_at_unix_ms.saturating_add(late_tolerance_ms)
        })
        .min_by_key(|occurrence| {
            let in_progress = now_unix_ms >= occurrence.starts_at_unix_ms
                && now_unix_ms <= occurrence.ends_at_unix_ms;
            (
                u8::from(!in_progress),
                now_unix_ms.abs_diff(occurrence.starts_at_unix_ms),
                Reverse(occurrence.starts_at_unix_ms),
                occurrence.ends_at_unix_ms,
                occurrence.occurrence_id_hash.as_str(),
            )
        })
}

pub fn hash_occurrence_id(occurrence_id: &str) -> String {
    sha256(occurrence_id.as_bytes())
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

fn recognize_single_url(value: &str) -> Option<RecognizedMeetingLink> {
    let parsed = ParsedHttpsUrl::parse(trim_url_wrapper(value))?;
    let path = parsed.path.to_ascii_lowercase();

    let (provider, redacted_path) = if host_matches(&parsed.host, "zoom.us")
        && ["/j/", "/my/", "/w/"]
            .iter()
            .any(|prefix| path.starts_with(prefix))
    {
        let prefix = path.split('/').nth(1)?;
        (MeetingProvider::Zoom, format!("/{prefix}/REDACTED"))
    } else if (parsed.host == "teams.microsoft.com" || parsed.host == "teams.live.com")
        && (path.starts_with("/l/meetup-join/") || path.starts_with("/meet/"))
    {
        let prefix = if path.starts_with("/l/meetup-join/") {
            "/l/meetup-join"
        } else {
            "/meet"
        };
        (MeetingProvider::Teams, format!("{prefix}/REDACTED"))
    } else if parsed.host == "meet.google.com" && is_google_meet_path(&path) {
        let prefix = if path.starts_with("/lookup/") {
            "/lookup"
        } else {
            ""
        };
        (MeetingProvider::Meet, format!("{prefix}/REDACTED"))
    } else if host_matches(&parsed.host, "webex.com") && is_webex_path(&path, parsed.query) {
        let prefix = if path.ends_with("/j.php") {
            "/j.php"
        } else if path.starts_with("/webappng/sites/") {
            "/webappng/sites/REDACTED"
        } else {
            path.split('/').nth(1).map_or("/meet", |part| match part {
                "join" => "/join",
                "m" => "/m",
                _ => "/meet",
            })
        };
        (MeetingProvider::Webex, format!("{prefix}/REDACTED"))
    } else {
        return None;
    };

    Some(RecognizedMeetingLink {
        provider,
        redacted_url: format!("https://{}{}", parsed.host, redacted_path),
    })
}

fn host_matches(host: &str, domain: &str) -> bool {
    host == domain || host.ends_with(&format!(".{domain}"))
}

fn is_google_meet_path(path: &str) -> bool {
    if path.starts_with("/lookup/") && path.len() > "/lookup/".len() {
        return true;
    }
    let code = path.trim_matches('/');
    let parts = code.split('-').collect::<Vec<_>>();
    parts.len() == 3
        && [3, 4, 3].iter().zip(parts).all(|(length, part)| {
            part.len() == *length && part.bytes().all(|byte| byte.is_ascii_lowercase())
        })
}

fn is_webex_path(path: &str, query: Option<&str>) -> bool {
    path.starts_with("/meet/")
        || path.starts_with("/join/")
        || path.starts_with("/m/")
        || path.starts_with("/webappng/sites/")
        || (path.ends_with("/j.php")
            && query.is_some_and(|query| query.to_ascii_lowercase().contains("mtid=")))
}

fn trim_url_wrapper(value: &str) -> &str {
    value.trim().trim_matches(|character: char| {
        matches!(
            character,
            '<' | '>' | '(' | ')' | '[' | ']' | '{' | '}' | '\'' | '"' | ',' | ';'
        )
    })
}

struct ParsedHttpsUrl<'a> {
    host: String,
    path: &'a str,
    query: Option<&'a str>,
}

impl<'a> ParsedHttpsUrl<'a> {
    fn parse(value: &'a str) -> Option<Self> {
        let scheme = value.get(..8)?;
        if !scheme.eq_ignore_ascii_case("https://") {
            return None;
        }
        let rest = value.get(8..)?;
        let authority_end = first_index_of(rest, &['/', '?', '#']).unwrap_or(rest.len());
        let authority = &rest[..authority_end];
        if authority.is_empty() || authority.contains('@') {
            return None;
        }
        let host = authority
            .split(':')
            .next()?
            .trim_end_matches('.')
            .to_ascii_lowercase();
        if host.is_empty()
            || !host
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || byte == b'.' || byte == b'-')
        {
            return None;
        }

        let tail = &rest[authority_end..];
        let fragment_index = tail.find('#').unwrap_or(tail.len());
        let before_fragment = &tail[..fragment_index];
        let query_index = before_fragment.find('?');
        let path = query_index.map_or(before_fragment, |index| &before_fragment[..index]);
        let query = query_index.map(|index| &before_fragment[index + 1..]);

        Some(Self {
            host,
            path: if path.is_empty() { "/" } else { path },
            query,
        })
    }
}

fn first_index_of(value: &str, needles: &[char]) -> Option<usize> {
    value
        .char_indices()
        .find_map(|(index, character)| needles.contains(&character).then_some(index))
}

fn sha256(input: &[u8]) -> [u8; 32] {
    const INITIAL: [u32; 8] = [
        0x6a09e667, 0xbb67ae85, 0x3c6ef372, 0xa54ff53a, 0x510e527f, 0x9b05688c, 0x1f83d9ab,
        0x5be0cd19,
    ];
    const K: [u32; 64] = [
        0x428a2f98, 0x71374491, 0xb5c0fbcf, 0xe9b5dba5, 0x3956c25b, 0x59f111f1, 0x923f82a4,
        0xab1c5ed5, 0xd807aa98, 0x12835b01, 0x243185be, 0x550c7dc3, 0x72be5d74, 0x80deb1fe,
        0x9bdc06a7, 0xc19bf174, 0xe49b69c1, 0xefbe4786, 0x0fc19dc6, 0x240ca1cc, 0x2de92c6f,
        0x4a7484aa, 0x5cb0a9dc, 0x76f988da, 0x983e5152, 0xa831c66d, 0xb00327c8, 0xbf597fc7,
        0xc6e00bf3, 0xd5a79147, 0x06ca6351, 0x14292967, 0x27b70a85, 0x2e1b2138, 0x4d2c6dfc,
        0x53380d13, 0x650a7354, 0x766a0abb, 0x81c2c92e, 0x92722c85, 0xa2bfe8a1, 0xa81a664b,
        0xc24b8b70, 0xc76c51a3, 0xd192e819, 0xd6990624, 0xf40e3585, 0x106aa070, 0x19a4c116,
        0x1e376c08, 0x2748774c, 0x34b0bcb5, 0x391c0cb3, 0x4ed8aa4a, 0x5b9cca4f, 0x682e6ff3,
        0x748f82ee, 0x78a5636f, 0x84c87814, 0x8cc70208, 0x90befffa, 0xa4506ceb, 0xbef9a3f7,
        0xc67178f2,
    ];

    let bit_len = (input.len() as u64).wrapping_mul(8);
    let mut padded = input.to_vec();
    padded.push(0x80);
    while padded.len() % 64 != 56 {
        padded.push(0);
    }
    padded.extend_from_slice(&bit_len.to_be_bytes());

    let mut hash = INITIAL;
    for chunk in padded.chunks_exact(64) {
        let mut words = [0u32; 64];
        for (index, bytes) in chunk.chunks_exact(4).enumerate() {
            words[index] = u32::from_be_bytes(bytes.try_into().expect("four-byte SHA word"));
        }
        for index in 16..64 {
            let s0 = words[index - 15].rotate_right(7)
                ^ words[index - 15].rotate_right(18)
                ^ (words[index - 15] >> 3);
            let s1 = words[index - 2].rotate_right(17)
                ^ words[index - 2].rotate_right(19)
                ^ (words[index - 2] >> 10);
            words[index] = words[index - 16]
                .wrapping_add(s0)
                .wrapping_add(words[index - 7])
                .wrapping_add(s1);
        }

        let [mut a, mut b, mut c, mut d, mut e, mut f, mut g, mut h] = hash;
        for index in 0..64 {
            let sum1 = h
                .wrapping_add(e.rotate_right(6) ^ e.rotate_right(11) ^ e.rotate_right(25))
                .wrapping_add((e & f) ^ (!e & g))
                .wrapping_add(K[index])
                .wrapping_add(words[index]);
            let sum0 = (a.rotate_right(2) ^ a.rotate_right(13) ^ a.rotate_right(22))
                .wrapping_add((a & b) ^ (a & c) ^ (b & c));
            h = g;
            g = f;
            f = e;
            e = d.wrapping_add(sum1);
            d = c;
            c = b;
            b = a;
            a = sum0.wrapping_add(sum1);
        }
        for (slot, value) in hash.iter_mut().zip([a, b, c, d, e, f, g, h]) {
            *slot = slot.wrapping_add(value);
        }
    }

    let mut output = [0u8; 32];
    for (chunk, value) in output.chunks_exact_mut(4).zip(hash) {
        chunk.copy_from_slice(&value.to_be_bytes());
    }
    output
}

#[cfg(test)]
mod tests {
    use super::*;

    fn input<'a>(url: Option<&'a str>, location: Option<&'a str>) -> CalendarEventInput<'a> {
        CalendarEventInput {
            starts_at_unix_ms: 1_000,
            ends_at_unix_ms: 2_000,
            occurrence_id: "calendar/event/occurrence-7",
            url,
            location,
            is_all_day: false,
            is_cancelled: false,
        }
    }

    #[test]
    fn recognizes_all_supported_providers_and_redacts_secrets() {
        let fixtures = [
            (
                "https://acme.zoom.us/j/123456?pwd=secret#join",
                MeetingProvider::Zoom,
            ),
            (
                "https://teams.microsoft.com/l/meetup-join/19%3ameeting_token?context=secret",
                MeetingProvider::Teams,
            ),
            (
                "https://meet.google.com/abc-defg-hij?authuser=2",
                MeetingProvider::Meet,
            ),
            (
                "https://acme.webex.com/meet/person?token=secret",
                MeetingProvider::Webex,
            ),
        ];

        for (url, provider) in fixtures {
            let recognized = recognize_meeting_link(url).expect("recognized provider URL");
            assert_eq!(recognized.provider, provider);
            assert!(!recognized.redacted_url.contains('?'));
            assert!(!recognized.redacted_url.contains('#'));
            assert!(!recognized.redacted_url.contains("secret"));
            assert!(!recognized.redacted_url.contains("123456"));
            assert!(recognized.redacted_url.contains("REDACTED"));
        }
    }

    #[test]
    fn rejects_insecure_homepage_and_lookalike_links() {
        for url in [
            "http://zoom.us/j/123",
            "https://evilzoom.us/j/123",
            "https://zoom.us/",
            "https://meet.google.com/",
            "https://user@meet.google.com/abc-defg-hij",
        ] {
            assert_eq!(recognize_meeting_link(url), None, "unexpected match: {url}");
        }
    }

    #[test]
    fn finds_a_link_in_location_text_without_accepting_notes() {
        let occurrence = sanitize_calendar_event(input(
            None,
            Some("Room 4 — join <https://meet.google.com/abc-defg-hij?authuser=1>"),
        ))
        .expect("location link");
        assert_eq!(occurrence.provider, MeetingProvider::Meet);
    }

    #[test]
    fn persisted_occurrence_has_only_the_safe_fields() {
        let source = input(
            Some("https://zoom.us/j/987654?pwd=dont-persist"),
            Some("Board room"),
        );
        let occurrence = sanitize_calendar_event(source).expect("sanitized event");
        let value = serde_json::to_value(&occurrence).expect("serialize occurrence");
        let keys = value
            .as_object()
            .expect("object")
            .keys()
            .cloned()
            .collect::<Vec<_>>();

        assert_eq!(
            keys,
            [
                "endsAtUnixMs",
                "occurrenceIdHash",
                "provider",
                "startsAtUnixMs",
            ]
        );
        let serialized = serde_json::to_string(&occurrence).expect("serialize occurrence");
        for forbidden in [
            "attendee",
            "notes",
            "url",
            "location",
            "987654",
            "dont-persist",
        ] {
            assert!(!serialized.to_ascii_lowercase().contains(forbidden));
        }
    }

    #[test]
    fn rejects_cancelled_all_day_invalid_and_non_meeting_events() {
        let mut all_day = input(Some("https://meet.google.com/abc-defg-hij"), None);
        all_day.is_all_day = true;
        assert_eq!(sanitize_calendar_event(all_day), None);

        let mut cancelled = input(Some("https://meet.google.com/abc-defg-hij"), None);
        cancelled.is_cancelled = true;
        assert_eq!(sanitize_calendar_event(cancelled), None);

        let mut invalid_time = input(Some("https://meet.google.com/abc-defg-hij"), None);
        invalid_time.ends_at_unix_ms = invalid_time.starts_at_unix_ms;
        assert_eq!(sanitize_calendar_event(invalid_time), None);

        assert_eq!(
            sanitize_calendar_event(input(Some("https://example.com"), None)),
            None
        );
    }

    #[test]
    fn overlap_resolution_is_deterministic_and_provider_specific() {
        let occurrences = vec![
            CalendarOccurrence {
                starts_at_unix_ms: 1_000,
                ends_at_unix_ms: 4_000,
                provider: MeetingProvider::Zoom,
                occurrence_id_hash: "b".to_string(),
            },
            CalendarOccurrence {
                starts_at_unix_ms: 2_000,
                ends_at_unix_ms: 3_000,
                provider: MeetingProvider::Zoom,
                occurrence_id_hash: "a".to_string(),
            },
            CalendarOccurrence {
                starts_at_unix_ms: 2_400,
                ends_at_unix_ms: 2_600,
                provider: MeetingProvider::Teams,
                occurrence_id_hash: "0".to_string(),
            },
        ];

        let selected = select_occurrence(&occurrences, MeetingProvider::Zoom, 2_500, 0, 0)
            .expect("overlap match");
        assert_eq!(selected.occurrence_id_hash, "a");

        let mut reversed = occurrences.clone();
        reversed.reverse();
        assert_eq!(
            select_occurrence(&reversed, MeetingProvider::Zoom, 2_500, 0, 0)
                .map(|event| event.occurrence_id_hash.as_str()),
            Some("a")
        );
    }

    #[test]
    fn occurrence_hash_is_stable_sha256_and_hides_the_identifier() {
        assert_eq!(
            hash_occurrence_id("abc"),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
        let hashed = hash_occurrence_id("private-occurrence-id");
        assert_eq!(hashed.len(), 64);
        assert!(!hashed.contains("private-occurrence-id"));
    }
}
