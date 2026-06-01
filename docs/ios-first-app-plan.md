# Poha iOS First App Plan

Read when: planning or implementing Poha mobile capture, mobile imports, recording sync, or iOS app architecture.

## Goal

Build a first-class iOS app for private conversation capture. The iPhone should help capture in-person meetings, phone calls, WhatsApp/Signal/Telegram-style VoIP calls, and other conversations that happen around or through the phone. The Mac app imports the recording and keeps doing Poha's heavier work: transcription, diarization, summaries, meeting metadata, archive, and agent-safe CLI access.

This is not a full mobile clone of the macOS menu bar app.

## Non-Goals

- No meeting markers.
- No typed notes.
- No custom inbound call recorder.
- No promise of direct same-device third-party VoIP call audio capture on iOS until a ReplayKit/device spike proves it works.
- No cloud-first workspace or team product in V1.
- No Android until the iOS capture/import loop is proven.

## Plugin And Repo Findings

XcodeBuildMCP was used to inspect the local iOS build surface.

- Session defaults have no project, workspace, scheme, or simulator configured.
- Repo scan found no `.xcodeproj` or `.xcworkspace`.
- Simulator listing failed because `xcrun simctl` is unavailable in the current developer-tool path.

Implication: Poha needs a new iOS app target under this repo. Before simulator verification, fix local Xcode CLI setup so `xcrun simctl list` works.

## Product Shape

Poha iOS V1 has one job: make phone-based conversation capture reliable, even when direct capture is not technically possible on iOS.

Primary flow:

1. Open Poha iOS.
2. Tap record.
3. Capture an in-person conversation, speakerphone conversation, or imported call recording.
4. Tap stop from app or Lock Screen status.
5. Recording syncs or exports to Mac.
6. Mac Poha imports bundle into the existing session contract.
7. Mac Poha transcribes and summarizes.

Secondary flows:

- Import/share an existing audio file into Poha iOS.
- Import Apple call recordings exported from Notes, where available.
- Capture WhatsApp/Signal/Telegram conversations through supported fallback paths.
- Browse locally captured recordings with status: local only, synced, imported, failed.

## Conversation Capture Matrix

The product should present one "Record conversation" surface, but the implementation must distinguish capture modes.

| Conversation type                                             | V1 approach                                                                                                     | Reliability | Notes                                                                                                                                                                                      |
| ------------------------------------------------------------- | --------------------------------------------------------------------------------------------------------------- | ----------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------ |
| In-person meeting                                             | Poha iOS records microphone directly.                                                                           | High        | Core V1 path. Works locked/backgrounded with proper audio background mode.                                                                                                                 |
| Phone app call                                                | Import Apple's call recording from Notes/share sheet.                                                           | Medium      | iOS 18.1+ supports Phone and FaceTime audio call recording in selected regions/languages. Poha should import the exported audio, not replace Apple's call recorder.                        |
| FaceTime audio                                                | Same as Phone app call import when Apple's call recording is available.                                         | Medium      | Treat as imported evidence.                                                                                                                                                                |
| WhatsApp/Signal/Telegram call on Mac                          | Use macOS Poha to record system + mic audio.                                                                    | High        | Best path when the conversation can move to desktop.                                                                                                                                       |
| WhatsApp/Signal/Telegram call on same iPhone                  | V1 must not promise direct audio interception. Offer speaker/second-device/import paths; run a ReplayKit spike. | Low/Unknown | iOS does not expose a normal public API for one app to record another app's VoIP call audio. ReplayKit may help for screen/broadcast capture, but needs device proof with each target app. |
| WhatsApp/Signal/Telegram call on speaker using another device | Record with Mac Poha or another Poha iOS device placed nearby.                                                  | Medium      | Practical fallback. Clear UX needed so users understand this records room audio, not direct call audio.                                                                                    |
| Voice memo / external audio file                              | Share/import into Poha iOS or Mac Poha.                                                                         | High        | Same bundle/import pipeline.                                                                                                                                                               |

## WhatsApp And VoIP Call Strategy

WhatsApp call capture matters, but same-device iOS capture is the risky path. The plan should support WhatsApp conversations through multiple explicit modes instead of pretending there is one magic recorder.

V1 supported modes:

- **Desktop mode**: take the WhatsApp call on Mac, use existing Poha macOS recording.
- **Second-device mode**: keep the WhatsApp call on iPhone speaker, record room audio with Mac Poha or another Poha iOS device.
- **Import mode**: import any exported call audio or screen recording the user already has.
- **Native-call import mode**: for Phone/FaceTime calls, import Apple's own call recordings from Notes.

V1 technical spike:

- Build a small ReplayKit broadcast/upload prototype on a physical iPhone.
- Test WhatsApp audio call, WhatsApp video call, Signal call, Telegram call, FaceTime, and Phone call.
- Measure whether buffers include remote audio, local mic, both, screen only, or silence.
- Check behavior when the foreground app owns the microphone.
- Record OS/device/app versions with results.

Product rule:

- If ReplayKit fails or is inconsistent, Poha should not ship it as a primary capture mode.
- If ReplayKit works for some apps, label it as "screen/call capture beta" and keep direct mic/import paths as the reliable defaults.

## How Similar Apps Achieve Call Capture

Most apps are not bypassing iOS audio restrictions. They pick one of a few capture mechanisms and shape the product around that mechanism.

### Granola

Granola uses different capture paths on Mac and iPhone.

- Mac: captures system audio, so it can work with Zoom, Google Meet, Discord, Slack, FaceTime, WhatsApp Desktop, and other call apps.
- iPhone: records in-person microphone audio and outbound phone calls made through Granola's built-in dialer.
- iPhone inbound calls are not supported; Granola recommends asking the caller to let you call them back.

Implication for Poha: this validates a split architecture. Mac Poha should stay the best path for WhatsApp Desktop and all computer audio. iOS should not promise same-phone third-party app capture unless a device spike proves it.

### Three-Way Phone Call Recorders

Apps such as TapeACall and REKK record normal carrier phone calls by adding a recording line as a third participant.

- User calls a recording access number or the app does that step.
- User adds/receives the real call.
- User taps Merge Calls.
- The recording service records from its side of the conference call.
- Carrier support for three-way calling is required.
- Recordings are usually processed and stored by the service.

This works for PSTN phone calls, not WhatsApp/Signal/Telegram calls that do not use the carrier phone network. TapeACall's own support says third-party calling services such as Skype, WhatsApp, and FaceTime are outside that model.

Implication for Poha: an outbound dialer or recording-line feature would be a separate telephony product. It would not solve WhatsApp.

### Apple Native Call Recording

Apple's built-in call recording is the cleanest iPhone-native path for regular Phone app calls where available.

- User starts recording during a Phone app call.
- Both participants hear a recording notice.
- Recording and transcript are saved to Notes.
- Availability depends on region and language.

Implication for Poha: support import from Notes/Files/Share Sheet rather than recreating this path.

### Hardware Recorders

Plaud, Notta Memo, Magmo, and similar devices avoid app-audio APIs by recording physical audio/vibration from the phone.

- Device attaches to the back of the phone, usually MagSafe or adhesive ring.
- Phone-call mode uses a vibration, contact, bone-conduction, or similar sensor to pick up the other party from the phone earpiece/speaker vibration.
- Device also captures the user's voice with onboard microphones.
- Because it is physical capture, it can work with WhatsApp calls similarly to normal calls.
- Headphones/AirPods usually break the model because the phone body is no longer producing the remote party's sound.
- Audio syncs to the companion app for transcription and summaries.

Implication for Poha: reliable same-iPhone WhatsApp capture at Plaud/Notta quality likely requires hardware, not just an iOS app. Without hardware, Poha should rely on Mac capture, second-device capture, or import.

### Mobile Transcription Apps

Otter, Fireflies, VOMO, Notta mobile, and similar apps mostly use:

- iPhone microphone recording.
- Bluetooth/external mic recording.
- Upload/import of existing audio or video.
- Meeting bots for Zoom/Meet/Teams where supported.
- Desktop capture for system audio when they have a desktop app.

Otter's support docs are especially direct: Apple and Google restrict third-party apps from recording phone calls directly, so calls must be recorded separately and imported; Otter live recording pauses during a phone call.

Implication for Poha: do not over-read "record any call" marketing. On iOS, it often means speakerphone/mic recording, file import, a telephony bridge, or hardware.

### ReplayKit And Open Source Evidence

ReplayKit is the one software path worth testing for same-device iPhone audio. Apple's Broadcast Upload Extension API can deliver video and audio buffers to an extension, including app audio and microphone audio sample types. It is user-visible and consented through the system broadcast UI, not hidden capture.

Evidence:

- Twilio's ReplayKit example shows a production-style screen-share implementation, but also warns about physical-device-only testing, extension memory limits, app audio restrictions, audio mixing complexity, and historical app-audio delay/leak issues.
- VoicePing's open-source iOS/macOS offline transcription app claims a working ReplayKit Broadcast Upload Extension path where `.audioApp` buffers from other apps are written to an App Group ring buffer for transcription.
- None of the open evidence found proves reliable WhatsApp/Signal/Telegram call audio capture. The open-source evidence proves ReplayKit can expose some other-app audio, not that encrypted VoIP call audio is consistently available.

Implication for Poha: ReplayKit should become a dedicated spike. If it captures WhatsApp remote audio, it is potentially valuable. If it captures silence, mic only, or inconsistent buffers, it should not shape V1.

## Product Implication

For Poha iOS, the honest product options are:

1. Ship excellent direct microphone recording and import.
2. Make Mac Poha the recommended path for WhatsApp Desktop and all desktop calls.
3. Support Phone/FaceTime call imports from Apple Notes/Files.
4. Offer second-device/speakerphone capture as a fallback.
5. Run ReplayKit experiments, but do not base V1 on them.
6. Consider a hardware accessory only if same-iPhone WhatsApp capture becomes core.

## iOS App Scope

V1 screens:

- Recorder: large record/stop control, elapsed time, input route, sync state.
- Recordings: recent recordings, duration, date, import/sync status.
- Capture Mode: direct recording, import audio, or setup instructions for phone-call/WhatsApp fallback.
- Settings: sync folder, privacy text, export diagnostics.

V1 system capabilities:

- Microphone permission.
- Background audio recording.
- Lock Screen / Dynamic Island Live Activity for elapsed recording and stop affordance.
- iCloud Drive export folder or document-based export.
- Share extension intake for audio files if low-risk after core recorder works.
- ReplayKit prototype only if the physical-device spike shows useful call audio capture.

V1 data captured:

- Audio file.
- Start/end timestamps.
- Duration.
- Device name.
- App version/schema version.
- Capture mode.
- Source app hint when user provides one, such as `in_person`, `phone`, `facetime`, `whatsapp`, `signal`, `telegram`, `voice_memo`, or `unknown`.
- Optional calendar event reference if calendar integration is added later.

## Bundle Contract

Mobile produces a directory or package that Mac Poha can import.

```text
poha-mobile-<uuid>/
  mobile_session.json
  audio.m4a
```

`mobile_session.json`:

```json
{
  "schemaVersion": 1,
  "id": "uuid",
  "source": "ios",
  "startedAt": "2026-05-27T18:00:00Z",
  "endedAt": "2026-05-27T18:32:10Z",
  "durationMs": 1930000,
  "deviceName": "Adi's iPhone",
  "appVersion": "0.1.0",
  "captureMode": "directMic",
  "sourceApp": "in_person",
  "audioFile": "audio.m4a",
  "calendarEventId": null,
  "title": null
}
```

Mac import converts this to the existing Poha session layout:

```text
sessions/<session-id>/
  session.json
  meeting.json
  audio.m4a
  transcript.md
  transcript.json
  summary.md
```

## Mac Import Work

Add CLI and app import support:

- `poha-cli meetings import-mobile-bundle <path>`
- App command for importing a selected bundle/folder.
- Optional watcher for an iCloud Drive `Poha Inbox` folder.

Import behavior:

- Validate `mobile_session.json`.
- Copy audio without rewriting evidence.
- Write `session.json` with `status: imported` or `status: queued`.
- Write `meeting.json` with `source: ios`.
- Rebuild `.poha/meetings.sqlite`.
- Queue existing transcription pipeline.

Known code change:

- `transcription.rs` copies `audio.mp3`, `audio.wav`, `audio.ogg`, `audio_mic.wav`, and `audio_spk.wav`; add `audio.m4a` to keep iOS audio artifacts.

## Architecture

Recommended repo layout:

```text
apps/
  poha/
    src-tauri/
  poha-ios/
    Poha.xcodeproj
    Poha/
    PohaTests/
    PohaUITests/
    PohaSessionKit/
```

Recommended iOS implementation:

- SwiftUI app shell.
- `AVAudioRecorder` or `AVAudioEngine` capture service.
- `AVAudioSession` category configured for recording.
- `UIBackgroundModes = audio`.
- ActivityKit extension for active recording state.
- Optional ReplayKit experiment target, gated behind physical-device results.
- File-backed `RecordingStore`, with SwiftData only for list/index metadata if needed.
- `PohaSessionKit` for bundle schema encoding/decoding and tests.

Avoid Tauri mobile for V1. Tauri mobile can build iOS apps, but this feature depends on native audio/backgrounding/ActivityKit/share/iCloud behavior. Native SwiftUI reduces risk and keeps the product closer to Apple APIs.

## Sync Strategy

V1: iCloud Drive folder or manual export.

- Lowest backend complexity.
- Easy to inspect files.
- Preserves Poha's local-first posture.
- Lets Mac importer be file-based.

V2: local network pairing.

- Mac shows pairing code.
- iPhone sends bundle over local network.
- Useful for users avoiding iCloud.

V3: CloudKit private database.

- Better status sync and background reliability.
- More entitlement and merge complexity.
- Use only after V1 file import proves daily value.

## Transcription Strategy

V1:

- iOS captures audio only.
- Mac Poha transcribes with current pipeline.

Optional later:

- iOS 26+ `SpeechAnalyzer` rough transcript for immediate mobile preview.
- Keep Mac transcript authoritative.

Avoid `SFSpeechRecognizer` for long meeting transcription in V1. Apple's speech recognition task limits and cloud/on-device availability make it a poor core dependency for full-length meetings.

## Call Recording Strategy

Do not build a phone dialer first, and do not frame Poha as an invisible WhatsApp call recorder.

Use Apple's native call recording path where available: iOS can record and transcribe calls in selected regions/languages, with recordings saved in Notes. Poha should support importing the exported audio into the same mobile bundle/import flow.

For WhatsApp and other VoIP apps, prioritize explicit user-controlled capture paths:

- If possible, take the call on Mac and use macOS Poha.
- If the call must happen on iPhone, use speaker/second-device capture or import an existing recording.
- Investigate ReplayKit, but require proof before committing product UX.

Custom outbound calling is a later business decision because it adds phone verification, telephony provider cost, consent UX, support burden, and compliance surface. It still would not solve WhatsApp/Signal/Telegram calls.

## Milestones

### M0: Mac Import Spine

- Add `audio.m4a` artifact preservation.
- Add mobile bundle schema parser.
- Add `poha-cli meetings import-mobile-bundle`.
- Add `captureMode` and `sourceApp` handling in imported `meeting.json`.
- Add regression tests for valid bundle, missing audio, invalid JSON, duplicate import.
- Verify imported mobile sessions appear in `meetings list` and can transcribe.

### M1: iOS Project Skeleton

- Create `apps/poha-ios`.
- Add SwiftUI recorder shell.
- Add `PohaSessionKit` schema tests.
- Configure bundle id, microphone purpose string, background audio mode.
- Fix local Xcode CLI setup; verify with XcodeBuildMCP simulator build.
- Add a placeholder capture-mode model with direct mic, import, phone recording import, and VoIP fallback modes.

### M2: Reliable Recording

- Implement recording service.
- Persist interrupted/unfinished recordings safely.
- Show active recording state.
- Export mobile bundle.
- Support source labels for in-person, WhatsApp, Signal, Telegram, Phone, FaceTime, and other.
- Add unit tests around session metadata and file naming.

### M3: Lock Screen Recording

- Add ActivityKit extension.
- Show elapsed time and recording state.
- Stop control if feasible; otherwise deep-link to app stop screen.
- Test lock/unlock/background interruption cases on device.

### M4: Sync

- Add iCloud Drive `Poha Inbox` export.
- Add Mac import UI or watcher.
- Add import status handling and duplicate detection.

### M5: Polish And Daily Use

- Improve failure recovery.
- Add battery/storage warnings.
- Add basic local recordings list.
- Add import from Files/share sheet if still needed.

### M6: WhatsApp/VoIP Capture Spike

- Build a ReplayKit prototype on a physical iPhone.
- Test target apps and document exact audio buffers captured.
- Decide whether direct same-device VoIP capture is shippable, beta-only, or unsupported.
- If unsupported, polish fallback UX around Mac capture, second-device capture, and import.

Spike checklist:

- Test routes: speaker, earpiece, AirPods, wired headset.
- Test apps: WhatsApp, Signal, Telegram, FaceTime, Phone, Zoom, YouTube, Spotify.
- Capture streams separately: `.audioApp`, `.audioMic`, and mixed output.
- Log RMS, peak, sample count, and silence ratio every second.
- Label each test by device, iOS version, target app version, call type, and audio route.
- Confirm which streams contain local voice, remote voice, both, media playback, or silence.
- Test foreground, background, locked screen, incoming interruption, and app switch.
- Validate App Store posture: visible consent, broadcast picker, no hidden capture.

## Verification Gates

Mac:

- `pnpm exec dprint fmt`
- `pnpm cargo:check`
- `pnpm test:cli`
- `pnpm test:lib`
- Focused import/transcription tests.

iOS:

- XcodeBuildMCP project discovery sees `apps/poha-ios/Poha.xcodeproj`.
- XcodeBuildMCP simulator build succeeds.
- Device test: 30-minute locked-screen recording.
- Device test: incoming call/interruption recovery.
- Device test: WhatsApp call while Poha records directly; document whether recording continues, interrupts, or captures silence.
- Device test: ReplayKit prototype on WhatsApp/Signal/Telegram if built.
- Device test: low battery/storage warning path.
- Bundle import test into Mac Poha.

## Risks

- Background audio recording requires correct Info.plist modes and App Review justification.
- Simulator cannot validate real microphone/background behavior; physical-device tests are required.
- iCloud Drive sync latency may confuse status; V1 should show "exported", not promise "processed".
- ActivityKit is useful for state, but the recording engine must not depend on Live Activity updates.
- Long recordings need defensive file finalization and corruption recovery.
- iOS may interrupt Poha microphone recording when a VoIP app owns the microphone.
- ReplayKit may capture screen and some audio buffers, but third-party call apps may omit or block useful call audio.
- Speaker/second-device capture is less clean than direct audio and needs clear consent/quality guidance.

## Open Decisions

- Minimum iOS version: likely iOS 18 for call-recording-era devices, or iOS 17 if wider install base matters.
- Sync default: iCloud Drive folder vs manual export first.
- iOS app identity: same `com.iamadig.poha` family with a new suffix, for example `com.iamadig.poha.ios`.
- Whether to make the mobile bundle a visible folder or a package extension such as `.pohaRecording`.
- Whether WhatsApp/VoIP capture is positioned as a primary use case, a beta mode, or fallback-only after the ReplayKit spike.

## References

- Apple `AVAudioSession`: https://developer.apple.com/documentation/avfaudio/avaudiosession
- Apple recording/background note: https://developer.apple.com/documentation/avfaudio/avaudiosession/category-swift.struct/record
- Apple ActivityKit: https://developer.apple.com/documentation/ActivityKit/
- Apple ReplayKit security: https://support.apple.com/guide/security/replaykit-security-seca5fc039dd/web
- Apple ReplayKit sample handler: https://developer.apple.com/documentation/replaykit/rpbroadcastsamplehandler
- Apple CallKit: https://developer.apple.com/documentation/callkit
- Apple shared data/app groups: https://developer.apple.com/documentation/technologyoverviews/shared-data
- Apple document picker: https://developer.apple.com/documentation/uikit/uidocumentpickerviewcontroller
- Apple SpeechAnalyzer: https://developer.apple.com/documentation/speech/speechanalyzer
- Apple call recording support: https://support.apple.com/en-us/guide/iphone/record-and-transcribe-a-call-iph57c6590e9/ios
- Meta note on WhatsApp personal calls/messages encryption: https://about.fb.com/news/2022/08/new-privacy-features-on-whatsapp/
- Granola iPhone differences: https://docs.granola.ai/help-center/ios/getting-started
- Granola phone calls: https://docs.granola.ai/help-center/ios/phone-calls
- TapeACall three-way recording: https://support.tapeacall.com/hc/en-us/articles/38577452382356-The-Merge-Calls-button-isn-t-working-What-should-I-do
- TapeACall compatibility: https://tapeacall.com/support/compatibility
- REKK FAQ: https://rekk.io/en/faq
- Otter phone call import: https://help.otter.ai/hc/en-us/articles/37814850589975-Record-and-transcribe-a-phone-call
- VoicePing ReplayKit transcription example: https://github.com/voiceping-ai/ios-mac-offline-transcribe
- REKK App Store three-way recording note: https://apps.apple.com/us/app/rekk-call-recorder/id1475739728
- Plaud phone call recording mode: https://support.plaud.ai/hc/en-us/articles/50837232018585-What-is-the-difference-between-Note-Recording-and-Phone-Call-Recording
- Plaud headphones limitation: https://support.plaud.ai/hc/en-us/articles/50837313885337-Can-I-use-headphones-earphones-when-I-record-a-phone-call-with-Plaud-Note
- Notta Memo call recording mode: https://support.notta.ai/hc/en-us/articles/38281365010715-How-to-switch-recording-mode-and-start-recording-on-Notta-Memo
- Notta Memo recording quality: https://support.notta.ai/hc/en-us/articles/38281517913371-How-to-improve-recording-quality-with-Notta-Memo
- Tauri mobile CLI reference: https://v2.tauri.app/reference/cli/
