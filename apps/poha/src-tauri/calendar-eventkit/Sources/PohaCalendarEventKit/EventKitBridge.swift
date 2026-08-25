import Darwin
import Dispatch
import EventKit
import Foundation

let pohaCalendarLookbackSeconds: TimeInterval = 6 * 60 * 60
let pohaCalendarLookaheadSeconds: TimeInterval = 12 * 60 * 60
let pohaCalendarMaximumEvents = 128

private enum PohaCalendarAuthorizationCode: Int32 {
  case unknown = -1
  case notDetermined = 0
  case restricted = 1
  case denied = 2
  case fullAccess = 3
  case writeOnly = 4
}

/// The only event-shaped value that crosses the Swift/Rust bridge.
///
/// `recognizedLink` is a fixed redacted placeholder. Raw meeting URLs,
/// titles, locations, attendees, organizers, alarms, and notes are never encoded.
struct NativeCalendarEvent: Encodable {
  let startsAtUnixMs: Int64
  let endsAtUnixMs: Int64
  let occurrenceId: String
  let recognizedLink: String
}

private struct NativeCalendarQueryResponse: Encodable {
  let authorizationStatus: Int32
  let queriedAtUnixMs: Int64
  let events: [NativeCalendarEvent]
}

public typealias PohaCalendarAccessCallback = @convention(c) (
  UnsafeMutableRawPointer?, Int32, Int32
) -> Void

@_cdecl("poha_eventkit_authorization_status")
public func pohaEventKitAuthorizationStatus() -> Int32 {
  authorizationCode(EKEventStore.authorizationStatus(for: .event)).rawValue
}

/// Requests full event access through EventKit's system permission sheet.
///
/// Callers should invoke this from a visible user action. Dispatching to the
/// main queue ensures macOS owns and presents the consent UI in the app.
@_cdecl("poha_eventkit_request_full_access")
public func pohaEventKitRequestFullAccess(
  _ context: UnsafeMutableRawPointer?,
  _ callback: @escaping PohaCalendarAccessCallback
) {
  let current = EKEventStore.authorizationStatus(for: .event)
  guard current == .notDetermined else {
    callback(context, authorizationCode(current).rawValue, 0)
    return
  }

  DispatchQueue.main.async {
    let eventStore = EKEventStore()
    eventStore.requestFullAccessToEvents { _, error in
      // Keep the store alive until EventKit completes the permission request.
      withExtendedLifetime(eventStore) {
        let status = authorizationCode(EKEventStore.authorizationStatus(for: .event))
        callback(context, status.rawValue, error == nil ? 0 : 1)
      }
    }
  }
}

/// Returns a malloc-owned UTF-8 JSON response for a fixed near-current range.
/// The caller must release it with `poha_eventkit_free_string`.
@_cdecl("poha_eventkit_copy_near_current_events_json")
public func pohaEventKitCopyNearCurrentEventsJSON() -> UnsafeMutablePointer<CChar>? {
  let authorization = EKEventStore.authorizationStatus(for: .event)
  let now = Date()
  let queriedAtUnixMs = unixMilliseconds(now)

  guard authorization == .fullAccess else {
    return copyJSON(
      NativeCalendarQueryResponse(
        authorizationStatus: authorizationCode(authorization).rawValue,
        queriedAtUnixMs: queriedAtUnixMs,
        events: []))
  }

  let (rangeStart, rangeEnd) = nativeQueryWindow(now: now)
  let eventStore = EKEventStore()
  let predicate = eventStore.predicateForEvents(
    withStart: rangeStart,
    end: rangeEnd,
    calendars: nil)

  let events = eventStore.events(matching: predicate)
    .lazy
    .filter { !$0.isAllDay && $0.status != .canceled }
    .compactMap(nativeCalendarEvent)
    .prefix(pohaCalendarMaximumEvents)

  return copyJSON(
    NativeCalendarQueryResponse(
      authorizationStatus: PohaCalendarAuthorizationCode.fullAccess.rawValue,
      queriedAtUnixMs: queriedAtUnixMs,
      events: Array(events)))
}

@_cdecl("poha_eventkit_free_string")
public func pohaEventKitFreeString(_ pointer: UnsafeMutablePointer<CChar>?) {
  free(pointer)
}

func nativeQueryWindow(now: Date) -> (Date, Date) {
  (
    now.addingTimeInterval(-pohaCalendarLookbackSeconds),
    now.addingTimeInterval(pohaCalendarLookaheadSeconds)
  )
}

/// Recognizes only supported meeting URLs and returns a canonical placeholder.
/// No meeting code, password, query value, tenant, or fragment survives.
func redactedRecognizedMeetingLink(_ value: String?) -> String? {
  guard let value else { return nil }

  let candidates =
    [value]
    + value
    .split(whereSeparator: { $0.isWhitespace })
    .map(String.init)

  for candidate in candidates {
    let trimmed = candidate.trimmingCharacters(in: urlWrapperCharacters)
    guard
      let components = URLComponents(string: trimmed),
      components.scheme?.lowercased() == "https",
      components.user == nil,
      components.password == nil,
      let host = components.host?.lowercased().trimmingCharacters(
        in: CharacterSet(charactersIn: "."))
    else {
      continue
    }

    let path = components.percentEncodedPath.lowercased()
    if hostMatches(host, domain: "zoom.us") {
      for prefix in ["/j/", "/my/", "/w/"] where path.hasPrefix(prefix) {
        return "https://zoom.us\(prefix)REDACTED"
      }
    }

    if host == "teams.microsoft.com" || host == "teams.live.com" {
      if path.hasPrefix("/l/meetup-join/") {
        return "https://teams.microsoft.com/l/meetup-join/REDACTED"
      }
      if path.hasPrefix("/meet/") {
        return "https://teams.microsoft.com/meet/REDACTED"
      }
    }

    if host == "meet.google.com" {
      if path.hasPrefix("/lookup/") && path.count > "/lookup/".count {
        return "https://meet.google.com/lookup/REDACTED"
      }
      if isGoogleMeetPath(path) {
        return "https://meet.google.com/aaa-bbbb-ccc"
      }
    }

    if hostMatches(host, domain: "webex.com") {
      if path.hasPrefix("/meet/") {
        return "https://webex.com/meet/REDACTED"
      }
      if path.hasPrefix("/join/") {
        return "https://webex.com/join/REDACTED"
      }
      if path.hasPrefix("/m/") {
        return "https://webex.com/m/REDACTED"
      }
      if path.hasPrefix("/webappng/sites/") {
        return "https://webex.com/webappng/sites/REDACTED"
      }
      if path.hasSuffix("/j.php"), hasWebexMeetingIdentifier(components) {
        return "https://webex.com/j.php?mtid=REDACTED"
      }
    }
  }

  return nil
}

private func nativeCalendarEvent(_ event: EKEvent) -> NativeCalendarEvent? {
  let startsAtUnixMs = unixMilliseconds(event.startDate)
  let endsAtUnixMs = unixMilliseconds(event.endDate)
  guard endsAtUnixMs > startsAtUnixMs else { return nil }

  let recognizedLink =
    redactedRecognizedMeetingLink(event.url?.absoluteString)
    ?? redactedRecognizedMeetingLink(event.location)
  guard let recognizedLink else { return nil }

  let identifier = event.eventIdentifier ?? event.calendarItemIdentifier
  guard !identifier.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty else {
    return nil
  }

  return NativeCalendarEvent(
    startsAtUnixMs: startsAtUnixMs,
    endsAtUnixMs: endsAtUnixMs,
    occurrenceId: String("\(identifier)|\(startsAtUnixMs)".prefix(512)),
    recognizedLink: recognizedLink)
}

private func authorizationCode(_ status: EKAuthorizationStatus) -> PohaCalendarAuthorizationCode {
  switch status {
  case .notDetermined:
    return .notDetermined
  case .restricted:
    return .restricted
  case .denied:
    return .denied
  case .fullAccess:
    return .fullAccess
  case .writeOnly:
    return .writeOnly
  @unknown default:
    return .unknown
  }
}

private func unixMilliseconds(_ date: Date) -> Int64 {
  Int64((date.timeIntervalSince1970 * 1_000).rounded())
}

private func copyJSON<T: Encodable>(_ value: T) -> UnsafeMutablePointer<CChar>? {
  let encoder = JSONEncoder()
  encoder.outputFormatting = [.sortedKeys, .withoutEscapingSlashes]
  guard
    let data = try? encoder.encode(value),
    let json = String(data: data, encoding: .utf8)
  else {
    return nil
  }
  return strdup(json)
}

private func hostMatches(_ host: String, domain: String) -> Bool {
  host == domain || host.hasSuffix(".\(domain)")
}

private func isGoogleMeetPath(_ path: String) -> Bool {
  let code = path.trimmingCharacters(in: CharacterSet(charactersIn: "/"))
  let parts = code.split(separator: "-", omittingEmptySubsequences: false)
  guard parts.map(\.count) == [3, 4, 3] else { return false }
  return parts.allSatisfy { part in
    part.utf8.allSatisfy { byte in
      byte >= Character("a").asciiValue! && byte <= Character("z").asciiValue!
    }
  }
}

private func hasWebexMeetingIdentifier(_ components: URLComponents) -> Bool {
  components.queryItems?.contains { item in
    item.name.lowercased() == "mtid" && !(item.value ?? "").isEmpty
  } ?? false
}

private let urlWrapperCharacters = CharacterSet.whitespacesAndNewlines.union(
  CharacterSet(charactersIn: "<>()[]{}'\";,"))
