# Meeting Bot Research Appendix

Read when: evaluating providers, platform constraints, or verification details for `docs/meeting-bot-plan.md`.

## Source Findings

### Notion

Notion AI Meeting Notes is not a meeting-attendance bot in the Fireflies sense. Notion's help says the desktop app gives the best video-call experience because it captures "system audio and mic"; browser use is mostly microphone capture. It requires system audio and screen recording permissions on desktop.

Notion puts responsibility on the user to obtain consent before transcribing, offers text/audio consent-message flows, and can enforce automatic consent messaging at workspace level. It processes audio through sub-processors, then deletes processed audio; optional local audio storage keeps recent files on the recorder's device.

Implication: Notion validates a Poha-local path: calendar detection, start transcription, consent prompt, local audio retention. It does not solve "send a bot instead of me."

Source: https://www.notion.com/help/ai-meeting-notes

### Fireflies

Fireflies exposes the classic notetaker-bot model:

- Invite `fred@fireflies.ai`, enable auto-join, or add the bot to a live meeting.
- The bot joins at scheduled start time and appears as `Fireflies.ai Notetaker`.
- Host/waiting-room admission may be required.
- Fireflies says it can record without the user present if `fred@fireflies.ai` was invited and the platform permits guest bots.
- Fireflies says the bot cannot be hidden; it remains visible for compliance.
- Fireflies supports Google Calendar and Outlook/Office 365 Calendar, but not Apple Calendar directly.
- Speaker names work best when the bot joins the video meeting or the Chrome extension is used.

Implication: Poha's V1 should treat bot presence as a visible, consented recording participant. It should support manual invite and calendar rules, but default to explicit per-meeting send.

Source: https://guide.fireflies.ai/articles/9554534786-how-fireflies-joins-and-records-your-meetings-faqs

### Zoom AI Companion

Zoom now has its own cross-platform bot shape for third-party meetings. Licensed users can invite Zoom AI Companion to Google Meet or Microsoft Teams; it joins as a participant, transcribes, summarizes, and answers post-meeting questions. Zoom's consent notice model includes pre-meeting emails, chat messages, a branded video tile, owner name, and transcribing status.

Implication: The major platforms are normalizing visible assistant participants and explicit notice. Poha should align with that expectation.

Source: https://support.zoom.com/hc/en/article?id=zm_kb&sysparm_article=KB0080354

### Recall.ai

Recall.ai is a managed Meeting Bot API. Its bot is single-use and mapped to one meeting. The bot exists as a meeting participant and can access meeting features/data such as video, audio, transcription, participants, metadata, chat, screenshare, and output media. Recall supports Zoom, Google Meet, Microsoft Teams, Webex, GoTo Meeting, and Slack Huddles.

Pricing as of this research: pay-as-you-go is $0.50/hr of recording, built-in transcription is $0.15/hr, and recording storage is free for seven days then $0.05/hr retained for 30 days. It advertises US/EU/JP data residency.

Implication: Recall is the lowest-risk provider-backed Poha spike if the product can tolerate a cloud dependency and usage billing.

Sources:

- https://docs.recall.ai/docs/bot-overview
- https://www.recall.ai/pricing

### Attendee

Attendee is an open-source/self-hostable Meeting Bot API supporting Zoom, Google Meet, and Microsoft Teams. Its docs list bot states such as ready, joining, waiting room, joined-recording, recording-permission-denied, post-processing, ended, and data-deleted. Capabilities include recording, real-time transcription, speech/audio output, avatar imagery, chat, diarization, timestamps, and data deletion.

Its changelog shows recent activity through 2026: Zoom RTMS support, browser-based voice agents, per-participant realtime audio, managed Zoom OAuth, signed-in Google Meet bots, calendar integration beta, direct upload to external storage, signed-in Teams/Zoom bots, scheduled bots, chat messages, and webhooks.

Implication: Attendee is the most interesting option if Poha wants control/self-hosting later. It still adds backend and ops work.

Sources:

- https://docs.attendee.dev/guides/participant-events
- https://attendee.dev/changelog

### Meeting BaaS

Meeting BaaS provides a hosted API with optional self-hosting for Zoom, Google Meet, and Microsoft Teams. It exposes raw audio/video, transcripts, participant metadata, diarization, calendar integration on higher tiers, and SDKs. Published pricing uses token packs: raw recording is 1 token/hr, token packs range from $0.50/token down to $0.35/token, and transcription/streaming add token costs.

Implication: Similar shape to Recall/Attendee, potentially cheaper at volume, but should be validated with a direct reliability and support test before choosing.

Sources:

- https://www.meetingbaas.com/en
- https://pricing.meetingbaas.com/pricing

### Cronofy Meeting Agents

Cronofy exposes an "on behalf of attendee" scheduling model with meeting URL, join time, display name, image, and webhooks. It supports Microsoft Teams, Google Meet, and Zoom URLs. Its docs call out an important Zoom limitation: Zoom Meeting Agents must be attributed to an attendee and can only be active when that attendee is present.

Implication: This is a good reminder that "bot attends instead of me" is not universally supported. Poha needs platform/provider capability checks per meeting, especially for Zoom.

Source: https://docs.cronofy.com/developers/api/meeting-agents/schedule/

## Platform APIs And Constraints

### Zoom

- Zoom Meeting SDK offers raw video/audio access on native platforms.
- Beginning March 2, 2026, apps joining meetings outside their account must be authorized by ZAK, OBF, or RTMS.
- Zoom's OBF FAQ says OBF tokens are for external account joins and require an authorized Zoom user who is already present in the meeting for the join to succeed.
- For persistent recording/data access, Zoom points developers toward RTMS.

Sources:

- https://developers.zoom.us/docs/meeting-sdk/
- https://developers.zoom.us/docs/meeting-sdk/obf-faq/
- https://developers.zoom.us/blog/transition-to-obf-token-meetingsdk-apps/

### Google Meet

- Google's Meet Media API can access real-time Meet media, but it is in Developer Preview.
- The Cloud project, OAuth principal, and all meeting participants must be enrolled in the preview.
- Host/admin settings, encryption, watermarking, underage accounts, and consumer account requirements can block connection.
- Google Workspace admins can restrict third-party recording apps by blocking domains, extensions, API access, and forcing restrictive meeting access.

Sources:

- https://developers.google.com/workspace/meet/media-api/guides/overview
- https://knowledge.workspace.google.com/admin/support/troubleshooting/external-apps-are-recording-meet-meetings

### Microsoft Teams

- Teams supports real-time media bots via Microsoft Graph/Teams bot infrastructure.
- Application-hosted media bots need Microsoft Graph media libraries and Windows Server/Azure hosting.
- Microsoft says real-time media bots are not recommended for AI agent scenarios; it recommends Copilot Studio agents or Graph transcript APIs for many meeting-intelligence use cases.

Sources:

- https://learn.microsoft.com/en-us/microsoftteams/platform/bots/calls-and-meetings/real-time-media-concepts
- https://learn.microsoft.com/en-us/microsoftteams/platform/bots/calls-and-meetings/calls-meetings-bots-overview

## Provider Selection Spike

Run a one-week spike before committing.

Providers:

- Recall.ai
- Attendee hosted or self-hosted
- Meeting BaaS

Required tests:

- Create bot by URL.
- Schedule bot for future time.
- Join live meeting.
- Join without user present.
- Waiting room/admit flow.
- Bot denied/removed flow.
- Late meeting start.
- Meeting overrun.
- Recurring meeting instance.
- Provider webhook delivery.
- Audio download.
- Transcript download with speaker/timestamps.
- Chat download.
- Participant metadata.
- Provider-side deletion.
- Data residency/retention controls.
- Zoom external meeting after March 2, 2026 auth requirements.

Decision scorecard:

| Criterion                            | Weight |
| ------------------------------------ | -----: |
| Zoom/Meet/Teams join reliability     |     25 |
| Can record when user absent          |     20 |
| Consent/visible identity controls    |     10 |
| Artifact quality and export fidelity |     15 |
| Data deletion and retention controls |     10 |
| Cost                                 |      5 |
| Self-host/control path               |     10 |
| API/docs/support quality             |      5 |

## Verification Matrix

Platforms:

- Zoom internal meeting.
- Zoom external meeting.
- Google Meet personal/consumer.
- Google Meet Workspace restricted access.
- Microsoft Teams tenant meeting.
- Microsoft Teams external meeting.

Scenarios:

- User attends.
- User absent.
- Host absent.
- Host admits bot.
- Host denies bot.
- Bot removed mid-meeting.
- Participant opts out.
- Meeting starts late.
- Meeting overruns.
- Meeting ends early.
- Recurring event.
- Meeting URL changes.
- E2EE/watermark/enterprise restriction.

Outputs:

- `bot.json` state accurate.
- `session.json` valid.
- `meeting.json` indexed.
- `audio.*` preserved.
- `transcript.source.json` preserved.
- `transcript.json` normalized.
- `transcript.md` readable.
- `summary.md` generated or queued.
- `.poha/meetings.sqlite` rebuilt.
- Provider data deletion request recorded.
