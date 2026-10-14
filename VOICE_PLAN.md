# Continuous voice implementation plan

## Approved interaction

The resting composer is a bottom-centered pair of large metaballs connected by a
soft liquid neck: microphone and keyboard. The text field is hidden. Selecting
keyboard reveals the field and slides the keyboard up while the toolbar stretches
and slides down out of view. Dismissal reverses the transition. Coordinate content
insets, focus, hit targets, and reduced motion throughout.

Microphone toggles an explicitly started continuous voice session. Show connecting,
listening, responding, muted, and error states, with live captions in the current
chat. Keyboard entry mutes capture; dismissal does not automatically unmute.
Keep listening between turns and support speaking over assistant playback.
Starting voice merges the two metaballs and fades the keyboard control. Layered
vector waves at the bottom follow a smoothed microphone RMS volume envelope;
silence settles them and mute/stop clears the level. No FFT is needed.

## Architecture and ownership

- Ferrofluid owns rendering and UI intent; shared bounded IPC carries voice state.
- Forge owns provider-neutral live sessions, Gemini Live protocol adaptation,
  credentials, audio/events, cancellation and transport boundaries.
- Welder owns chat history, context and existing tools, including voice tool calls.
- The macOS/QEMU host bridge owns microphone/speaker device I/O and secure socket
  transport. Guest agent/model decisions stay in Forge and Welder.
- Use bounded audio frames, queues, session IDs and generation IDs; reject stale
  events and clear playback immediately on interruption or stop.

## Parallel implementation

1. UI: implement metaball toolbar, coordinated composer/keyboard transitions,
   voice callbacks/states and preview/screenshot coverage.
2. Forge: implement live-session contracts and Gemini audio/event adapter with
   fixture coverage for transcripts, audio, interruptions, errors and tool calls.
3. Host audio: implement bounded audio bridge and macOS capture/playback backend,
   with explicit device lifecycle and transport integration points.
4. Integration: wire display IPC, kernel host, Welder and launcher to these
   contracts. Preserve existing text chat and process recovery.

## Milestones and verification

First verify UI and protocol/device foundations independently. Then connect a
real Gemini Live session through Forge, integrate context/tools, and validate
multi-turn conversation and interruption. Finally test disconnect/reconnect,
device permission failures, echo handling and queue pressure. Use headphones as
the initial full-duplex baseline; speaker echo handling needs separate evidence.

Run focused Rust tests and UI previews, then the applicable bare-metal build and
QEMU checks. Live microphone and billable provider checks require an available
device/account and must be reported separately from fixture validation.

## Acceptance

Tap mic once, converse for multiple turns, receive spoken replies and captions,
interrupt playback, invoke an existing Welder tool, and stop capture immediately.
Keyboard transitions must be smooth, reversible and preserve typed drafts. No
audio capture on boot, after session end, or while muted. Unsupported providers
show a clear unavailable state; no silent provider switch.

## Scope

Gemini-first two-way audio, provider-neutral interfaces, host audio for current
QEMU development. Native guest sound drivers and other providers follow later.
The existing test named gemini_live is an ordinary live-account generation test,
not an existing Gemini Live audio implementation.

## Implementation status

Implemented the connected toolbar and coordinated keyboard/composer animation,
shared display voice state/intents, Forge's bounded Gemini Live adapter, Welder
context/history/tool integration, guest UART voice framing and priority controls,
and the native macOS microphone/playback/WebSocket helper. The launcher builds
the helper when absent or stale. Slow HTTP tools run outside the host voice pump.
Voice and keyboard share Welder's tool executor, recoverable-error handling,
model-result formatting, and synthesized tile output. A fixture integration test
exercises MCP discovery, invocation, result/history recording, and tile parity.
Audio is a transport-specific Gemini Live session, not a separate MCP agent;
live turn/cancellation handling remains distinct from text generation.

Fixture/unit tests and visual golden comparisons pass; native helper compilation
and bare-metal kernel/display builds pass. Real microphone permission/capture,
provider account access, latency, speaker behavior, and sustained QEMU throughput
remain runtime acceptance checks. Acoustic echo cancellation is not implemented;
use headphones for the initial full-duplex validation. Disconnect/expiry ends the
session with a visible tap-to-reconnect error rather than automatically resuming
capture. Native guest sound and additional providers remain follow-up scope.
