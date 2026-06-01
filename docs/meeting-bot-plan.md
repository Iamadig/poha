# Poha Meeting Bot Plan

Read when: planning a Poha feature that sends a bot/notetaker to Zoom, Google Meet, Microsoft Teams, or another online meeting without requiring the Poha desktop app to be actively recording.

## Goal

Let a Poha user send a visible meeting notetaker to a scheduled or live online meeting, then import the resulting transcript, audio, chat, participant metadata, and summary into Poha's existing local meeting archive.

The user-facing promise should be precise:

- "Send Poha Notetaker to record and summarize this meeting if admitted."
- Not "Poha attends as you" or "Poha can silently record anything."
- Not "Poha can answer for you" in V1.

## Research Summary

There are two common product shapes.

1. Local/no-bot capture, like Notion AI Meeting Notes.
   - The app records local microphone and system audio from the user's own device.
   - It is low-intrusion because no extra meeting participant appears.
   - It cannot attend when the user is absent.
   - It depends on local OS permissions, active app state, and audio routing.

2. Visible meeting bot, like Fireflies, Otter-style notetakers, Recall.ai, Attendee, Meeting BaaS, and Zoom AI Companion in third-party meetings.
   - A cloud service joins the meeting as a participant.
   - The bot captures meeting audio/video/transcripts/chat/metadata.
   - The host or meeting settings may require admitting the bot.
   - It can sometimes record when the user is absent, but this varies by platform, tenant settings, and authorization model.

For Poha, the realistic V1 is a provider-backed visible notetaker plus local-first import into Poha's session contract.

## Research Notes

The detailed source review, provider notes, and platform constraints live in `docs/meeting-bot-research.md`.

Important conclusions:

- Notion validates a no-bot local capture path, not bot attendance.
- Fireflies validates visible participant bots, calendar/manual join, host admission, and "record without me" when the platform allows it.
- Zoom's March 2, 2026 authorization shift means external Zoom bot joins need careful provider validation.
- Google Meet's official Media API is still constrained enough that a consumer V1 should not depend on it.
- Teams native real-time media is heavyweight; provider-backed capture is the pragmatic V1 path.

## Product Decision

Build Poha V1 around a provider-backed notetaker import, not custom bot infrastructure.

Why:

- Poha is currently a local-first macOS menu bar recorder, not a cloud conferencing infrastructure company.
- Cross-platform bot joining is mostly reliability work: lobbies, guest restrictions, auth, calendar drift, recurring meetings, platform UI changes, provider policy changes, and post-meeting artifact handling.
- Zoom's 2026 authorization changes make naive Meeting SDK bots riskier.
- Google Meet's official media API is not broadly available enough for a consumer/product V1.
- Teams native real-time media is heavyweight and Windows/Azure-biased.

Recommended V1 provider order:

1. Recall.ai for fastest managed spike and platform coverage.
2. Attendee for self-host/control spike if local-first/data-control matters more than speed.
3. Meeting BaaS as a cost/alternative spike.
4. Cronofy only if calendar-agent semantics become central and its Zoom presence constraint is acceptable.

## Non-Goals

- No hidden recording.
- No impersonating the user.
- No joining meetings that explicitly block bots or recording.
- No default auto-join for every meeting.
- No bot speech, voice agent, or answer-on-my-behalf behavior in V1.
- No direct implementation of Zoom/Meet/Teams bot stacks in V1.
- No cloud-first workspace/social sharing product.

## User Experience

### Manual Send

1. User opens Poha.
2. User selects "Send Notetaker".
3. User pastes a Zoom/Meet/Teams link or selects an upcoming calendar event.
4. Poha shows:
   - Platform.
   - Start time.
   - Bot display name: `Poha Notetaker for Adi`.
   - Whether user presence may be required.
   - Consent/notice text that will be sent.
   - Retention policy.
5. User clicks "Send".
6. Status updates:
   - Scheduled.
   - Joining.
   - Waiting room.
   - Recording.
   - Permission denied.
   - Left.
   - Processing.
   - Imported.
   - Failed.
7. When complete, Poha imports the meeting into the archive.

### Calendar Send

V1 should be opt-in per meeting:

- Upcoming meetings list from Google Calendar and/or Microsoft Calendar.
- Toggle: "Send Poha Notetaker".
- Recurring options:
  - This meeting only.
  - This and following.
  - All instances.
- Default: off.

Later optional rules:

- Auto-send only meetings I own.
- Auto-send only meetings with specific domains.
- Never send to external domains.
- Never send to 1:1s.
- Never send when title contains blocked keywords.

### Live Meeting Send

1. User pastes active meeting URL.
2. Poha warns that late join depends on lobby/admission and may miss earlier audio.
3. Provider attempts join immediately.
4. Poha imports partial recording if the bot is removed or joins late.

### Not Attending Flow

This is the core differentiator, but the UI must be honest.

Copy:

> Poha can record this meeting if the meeting platform and host allow the notetaker to join. Some Zoom and enterprise meetings require you or the host to be present or to admit the bot.

Possible states:

- `can_join_without_user`: provider says yes, meeting settings unknown.
- `requires_host_admit`: lobby/restricted access likely.
- `requires_user_present`: platform/provider restriction.
- `blocked_by_policy`: provider/platform reported recording/bot denied.
- `unknown_until_join`: default for external meetings.

## Consent And Notice

Default posture: all-party consent.

Requirements:

- Bot name must identify the recorder: `Poha Notetaker for Adi`.
- Bot video tile/avatar should show Poha branding and "Recording".
- Bot sends a chat notice on join:
  - "Poha Notetaker for Adi is recording and transcribing this meeting for private notes. Please ask the host to remove it if you do not consent."
- Optional pre-meeting email when calendar invitees are available.
- If a participant opts out or the host removes the bot, Poha stops recording and imports the partial artifact.
- Store an audit record:
  - consent mode
  - notice text
  - notice delivery channel
  - notice sent at
  - bot visible display name
  - host/participant removal event if known

No hidden mode. If a provider offers non-bot capture, expose it separately as local capture.

## Architecture

### Required Backend

A real "send a bot even if I am absent" feature needs a backend. The local Poha app cannot reliably host a meeting bot if the Mac sleeps, loses network, or is not running.

V1 components:

- Poha desktop app:
  - meeting picker
  - manual URL send
  - job status
  - local archive import
  - provider-auth setup
- Poha bot broker service:
  - provider API credentials
  - calendar OAuth tokens, if enabled
  - bot scheduling API
  - webhook receiver
  - artifact downloader
  - provider data deletion
  - signed local import manifest
- Bot provider:
  - Recall/Attendee/Meeting BaaS
  - joins meeting
  - records/transcribes
  - sends webhooks
- Local import inbox:
  - `~/Library/Application Support/Poha/Bot Inbox/`
  - or iCloud Drive/Dropbox-style folder for early prototypes

### Why Not Local-Only Bot Hosting

Local-only bot hosting is possible only for a narrower "Mac must be awake" product:

- Poha app schedules a local worker.
- Local worker opens a meeting link or provider SDK.
- Local capture records system audio.

This is closer to Notion/Granola and should be treated as a separate feature. It does not solve absence.

## Data Model

### New Session Layout

```text
sessions/<session-id>/
  session.json
  meeting.json
  bot.json
  audio.m4a
  transcript.md
  transcript.json
  transcript.source.json
  chat.json
  participants.json
  summary.md
```

Evidence rule:

- Raw provider artifacts are evidence.
- Poha may convert them into its normalized transcript format.
- Agents may write `summary.md` and `meeting.json`.
- Agents must not rewrite `audio.*`, `transcript.source.json`, `chat.json`, `participants.json`, or `session.json`.

### `bot.json`

```json
{
  "schemaVersion": 1,
  "source": "meeting_bot",
  "provider": "recall",
  "providerBotId": "bot_123",
  "providerMeetingId": "optional",
  "platform": "zoom",
  "joinUrlHash": "sha256:...",
  "displayName": "Poha Notetaker for Adi",
  "state": "imported",
  "scheduledAt": "2026-05-30T15:30:00Z",
  "joinRequestedAt": "2026-05-30T15:29:30Z",
  "joinedAt": "2026-05-30T15:30:12Z",
  "recordingStartedAt": "2026-05-30T15:30:20Z",
  "leftAt": "2026-05-30T16:02:03Z",
  "completedAt": "2026-05-30T16:04:50Z",
  "leaveReason": "meeting_ended",
  "consent": {
    "mode": "chat_notice",
    "noticeText": "Poha Notetaker for Adi is recording and transcribing this meeting for private notes. Please ask the host to remove it if you do not consent.",
    "sentAt": "2026-05-30T15:30:21Z"
  },
  "artifacts": {
    "audio": "audio.m4a",
    "transcriptSource": "transcript.source.json",
    "chat": "chat.json",
    "participants": "participants.json"
  },
  "providerRetention": {
    "deleteRequestedAt": "2026-05-30T16:05:10Z",
    "deleteConfirmedAt": "2026-05-30T16:05:30Z"
  },
  "warnings": []
}
```

### `meeting.json`

Extend current `MeetingMetadata` conservatively:

```json
{
  "schemaVersion": 1,
  "id": "2026-05-30-zoom-customer-sync",
  "title": "Customer sync",
  "company": "Example Co",
  "context": "Example Co",
  "contextKind": "company",
  "people": ["Asha", "Ravi"],
  "speakerMap": {},
  "speakerStatus": "pending",
  "source": "meeting_bot",
  "originalId": "recall:bot_123",
  "importedFrom": "poha-bot-broker",
  "importedAt": "2026-05-30T16:05:00Z",
  "updatedAt": "2026-05-30T16:05:00Z"
}
```

Avoid adding a broad untyped blob to `meeting.json`; keep provider detail in `bot.json`.

### `session.json`

Use current manifest shape where possible:

```json
{
  "id": "2026-05-30-zoom-customer-sync",
  "status": "done",
  "startedAt": "2026-05-30T15:30:20Z",
  "endedAt": "2026-05-30T16:02:03Z",
  "captureDir": "/path/to/imported/provider/artifacts",
  "audioPath": "/path/to/session/audio.m4a",
  "transcriptPath": "/path/to/session/transcript.md",
  "transcription": {
    "engine": "provider",
    "status": "done",
    "model": "provider-native",
    "fallbackModel": "",
    "transcriptJsonPath": "/path/to/session/transcript.json",
    "transcriptMarkdownPath": "/path/to/session/transcript.md",
    "updatedAt": "2026-05-30T16:05:00Z"
  },
  "error": null
}
```

## Import Pipeline

1. Bot broker receives provider webhook.
2. Broker fetches artifacts:
   - raw audio/video if available
   - provider transcript
   - participant metadata
   - chat messages
   - provider job log/status
3. Broker writes a signed import bundle:

```text
poha-bot-import-<id>/
  import_manifest.json
  bot.json
  audio.m4a
  transcript.source.json
  chat.json
  participants.json
```

4. Poha imports bundle into `sessions/<session-id>/`.
5. Poha converts `transcript.source.json` into normalized `transcript.json` and `transcript.md`.
6. Poha writes `meeting.json` with `source: meeting_bot`.
7. Poha rebuilds `.poha/meetings.sqlite`.
8. Poha queues summary/enrichment.
9. Broker requests provider-side deletion after local import, if user retention policy requires it.

### Normalized Transcript Mapping

Poha `TranscriptSegment` requires:

- `speaker`
- `speakerId`
- `source`
- `startMs`
- `endMs`
- `text`

Mapping rules:

- Provider named speaker -> `speaker`.
- Provider participant ID -> `speakerId`.
- Provider unknown speaker -> `Speaker 1`, `Speaker 2`.
- Source -> `meeting_bot:<provider>`.
- Preserve provider word-level timestamps in `transcript.source.json` even if normalized segment loses detail.

### Re-Transcription Option

If provider transcript quality is weak or missing:

- Preserve provider transcript as evidence.
- Run Poha's existing transcription pipeline over `audio.m4a`.
- Mark `transcription.engine` as `mlx-whisper` or current configured engine.

Known local issue to handle when building: `transcription.rs` currently detects `audio.m4a` as a possible output artifact but `copy_audio_artifacts` does not copy `audio.m4a`. Any bot/mobile import path that uses M4A needs a regression test around artifact preservation.

## Provider Selection Spike

Run a one-week spike before committing. Compare Recall.ai, Attendee, and Meeting BaaS against join reliability, user-absent behavior, artifact fidelity, consent controls, retention controls, and provider deletion.

The detailed test list and scorecard live in `docs/meeting-bot-research.md`.

## Milestones

### M0: Spec And Provider Spike

- Run the provider selection spike.
- Record evidence with links/screenshots/logs.
- Pick one V1 provider and one fallback.
- Confirm whether "user absent" works by platform.

### M1: Bot Import Spine

- Add a bot import bundle parser.
- Add `bot.json` schema.
- Add provider transcript -> Poha transcript conversion.
- Preserve raw provider artifacts.
- Add `source: meeting_bot` handling in meeting list/detail.
- Add tests:
  - valid bundle import
  - missing required artifact
  - duplicate import
  - provider transcript conversion
  - provider delete audit status
  - evidence files are not rewritten

### M2: Manual Send

- Add minimal bot broker service.
- Add API credentials storage outside repo.
- Add Poha UI: paste URL, bot name, consent notice, send.
- Add job status polling/webhooks.
- Import finished jobs into local archive.

### M3: Consent And Retention

- Add configurable notice text.
- Add provider data deletion after import.
- Add local audit fields.
- Add "participant opted out / host removed bot" state.
- Add hard no-hidden-recording guardrails.

### M4: Calendar

- Add Google Calendar and Microsoft Calendar OAuth.
- Show upcoming online meetings.
- Per-meeting send toggle.
- Recurring meeting controls.
- Allow/deny rules.
- Pre-meeting email/notification if provider supports it.

### M5: Reliability

- Retry late starts.
- Handle lobbies and timeouts.
- Mark partial meetings.
- Better failure messages by provider/platform.
- Add export/debug bundle for failed bot joins.

### M6: Interactive Agent Later

Only after V1 recording/import works:

- Read meeting chat.
- Answer questions post-meeting.
- Optional live chat replies.
- Optional voice/avatar agent.

This is a different product: "Poha participates," not just "Poha records."

## Verification Matrix

The verification matrix lives in `docs/meeting-bot-research.md`.

Minimum before shipping:

- Zoom, Google Meet, and Microsoft Teams each pass user-present and user-absent tests.
- Bot denied/removed/late-start paths produce clear partial or failed states.
- Imported sessions preserve raw evidence and index into `.poha/meetings.sqlite`.
- Provider-side deletion is requested and audited after local import when retention policy requires it.

## Risks

- User expects the bot to "represent" them, but V1 only records.
- External hosts may reject notetakers.
- Enterprise admins may block third-party recording apps.
- Zoom external meetings may require user attribution/presence depending on provider path.
- Provider artifact quality can vary by platform.
- Provider outages break scheduled recordings.
- Cloud dependency conflicts with Poha's local-first posture.
- Consent laws vary by jurisdiction; safest UX is all-party consent.
- Calendar auto-join can create trust failures if defaults are too aggressive.

## Recommendation

Do not build bot infrastructure from scratch first.

Build a thin, explicit, provider-backed Poha Notetaker:

1. Manual send by URL.
2. Visible bot identity and consent notice.
3. Import all artifacts into Poha's local session contract.
4. Delete provider-side artifacts after import where possible.
5. Add calendar later.

This gets the product learning fast while preserving Poha's differentiator: local archive, stable `poha-cli`, agent-safe files, and user control over meeting evidence.
