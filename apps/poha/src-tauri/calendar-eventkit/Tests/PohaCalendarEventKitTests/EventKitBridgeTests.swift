import Foundation
import Testing

@testable import PohaCalendarEventKit

@Test func recognizesSupportedProvidersWithoutKeepingSecrets() {
  let fixtures: [(String, String)] = [
    ("https://tenant.zoom.us/j/123456?pwd=secret#join", "https://zoom.us/j/REDACTED"),
    (
      "Join <https://teams.microsoft.com/l/meetup-join/19%3ameeting_secret?context=token>",
      "https://teams.microsoft.com/l/meetup-join/REDACTED"
    ),
    ("https://meet.google.com/abc-defg-hij?authuser=2", "https://meet.google.com/aaa-bbbb-ccc"),
    ("https://tenant.webex.com/meet/person?token=secret", "https://webex.com/meet/REDACTED"),
    ("https://tenant.webex.com/site/j.php?MTID=secret", "https://webex.com/j.php?mtid=REDACTED"),
  ]

  for (source, expected) in fixtures {
    let redacted = redactedRecognizedMeetingLink(source)
    #expect(redacted == expected)
    #expect(redacted?.contains("secret") == false)
    #expect(redacted?.contains("123456") == false)
    #expect(redacted?.contains("tenant") == false)
  }
}

@Test func rejectsInsecureHomepageAndLookalikeLinks() {
  for source in [
    "http://zoom.us/j/123",
    "https://evilzoom.us/j/123",
    "https://zoom.us/",
    "https://meet.google.com/",
    "https://user@meet.google.com/abc-defg-hij",
  ] {
    #expect(redactedRecognizedMeetingLink(source) == nil)
  }
}

@Test func bridgePayloadContainsNoRawURLOrForbiddenCalendarFields() throws {
  let event = NativeCalendarEvent(
    startsAtUnixMs: 1_000,
    endsAtUnixMs: 2_000,
    occurrenceId: "opaque-event-id|1000",
    recognizedLink: "https://zoom.us/j/REDACTED")
  let data = try JSONEncoder().encode(event)
  let value = try #require(JSONSerialization.jsonObject(with: data) as? [String: Any])

  #expect(
    Set(value.keys)
      == Set([
        "startsAtUnixMs", "endsAtUnixMs", "occurrenceId", "recognizedLink",
      ]))
  let encoded = String(decoding: data, as: UTF8.self).lowercased()
  for forbidden in ["title", "attendee", "notes", "location", "123456", "secret", "pwd="] {
    #expect(!encoded.contains(forbidden))
  }
}

@Test func nativeWindowIsFixedNearCurrentRange() {
  let now = Date(timeIntervalSince1970: 1_000_000)
  let (start, end) = nativeQueryWindow(now: now)
  #expect(start == now.addingTimeInterval(-6 * 60 * 60))
  #expect(end == now.addingTimeInterval(12 * 60 * 60))
}
