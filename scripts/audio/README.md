# macOS voice device and socket helper

`./scripts/audio/build.sh` builds an ad-hoc signed native helper at
`target/alloyos-voice-helper`. The QEMU launcher rebuilds it when missing or when its source changes; set
`ALLOYOS_VOICE_HELPER` to override the executable. Rebuild after source changes.
No devices or network connections open until an explicit `start` command.
The embedded microphone usage description supports macOS permission prompts.

The helper uses AVAudioEngine for mono capture (resampled to PCM16LE 16 kHz)
and playback (PCM16LE 24 kHz, converted to the mixer format), and Foundation's
certificate-verified WebSocket transport. The only permitted destination is the
Gemini Live endpoint. Credentials are passed in private inherited pipes and
never printed or placed in process arguments. Logs and crash records exclude
all VOICE1 traffic.

Rust `voice::HostCommand` and `voice::HostEvent` define the JSON contract. UART
frames are `VOICE1<TAB>session<TAB>hex-json<NEWLINE>`; inner/outer session IDs
must match. Commands and provider messages are bounded, queues have finite
capacities, muted input is discarded, and session/generation checks discard
stale capture/playback. Stop/EOF closes the device engine and WebSocket.

No acoustic echo cancellation is enabled. Use headphones for initial duplex
validation. Real microphone permissions, speaker behavior, network reliability,
and billable provider sessions require manual runtime verification; fixture
checks do not demonstrate these behaviors.

The native pipe regression runs with `cargo test -p alloyos-dev-harness
native_helper_dispatches_short_commands_with_stdin_open` on macOS, or manually
with `sh scripts/audio/build.sh && python3 scripts/audio/test_helper.py`.
It keeps stdin open and checks short, fragmented, and batched commands using
only stop and invalid-endpoint start commands; no microphone or API is used.

For local device troubleshooting, run `target/alloyos-voice-helper --probe-audio`
from the same Terminal used to launch QEMU. It requests microphone permission,
starts the audio engine, discards captured audio, and stops after three seconds.
It never opens a network connection or outputs/retains PCM, and exits within
ten seconds if permission/setup does not finish. macOS permissions may differ
between Terminal and Codex, so a successful Terminal grant does not grant Codex.

Startup emits fixed `progress` stages: `permission_pending`,
`permission_granted`, `audio_starting`, `audio_ready`, `socket_open`, and
`setup_sent`. The host logs only these names and session IDs; captions, PCM,
credentials, provider messages, and raw network error descriptions stay private.
Connection failures expose only fixed failure descriptions to the guest.

`target/alloyos-voice-helper --probe-network` accepts the normal private
start/send JSON commands but skips all audio devices, isolating the Gemini
WebSocket/setup handshake. Use a bounded private-pipe caller, stop after
`setupComplete`, and never print the command or raw provider messages.
Production capture frames are withheld until the provider acknowledges setup,
so microphone traffic cannot congest the guest UART during connection setup.

During capture, `audio_stats` emits one numeric summary per second:
`captured_frames`, `captured_samples`, `emitted_frames`, `sent_frames`,
`input_transcripts`, `conversion_errors`, `level` (RMS envelope, 0–1000),
`muted`, and `provider_ready`. `sent_frames` counts successful WebSocket sends,
not just queued guest requests. `input_transcripts` counts provider events
without exposing their contents. `--probe-audio` prints these same summaries
without transmitting audio. `--test-converter` checks the production resampler
using synthetic mono 44.1 kHz and stereo 48 kHz signals without any devices.

With `start.direct_audio: true` (the QEMU coordinator default), media never
crosses UART: microphone PCM is encoded into `realtimeInput.audio` on the host,
and received PCM `inlineData` is played on the host. The helper removes only
handled PCM fields before forwarding the provider message, preserving sibling
text, transcripts, tool calls, and cancellation messages. Forge/Welder still
create setup/context/tools and handle every tool call/result. UART carries
control, provider text/tool messages, `audio_level` at most 4 Hz, and
`playback_active` transitions. Microphone capture and WebSocket send queues
remain bounded; transport overload terminates the session visibly.

`--test-direct-audio` explicitly enables a device-free synthetic 16 kHz input
source (1,600 samples every 100 ms), permits a localhost mock WebSocket endpoint,
and simulates speaker completion. Normal launches always enforce the production
endpoint. The test mode shares the production PCM serialization, WebSocket send,
provider audio filtering, mute, stop, and session validation paths; it cannot
validate actual microphone permission or physical speaker playback.

Reproducible acceptance commands:

- `python3 scripts/audio/check_native_media.py`: localhost model boundary,
  sustained native PCM, mute/resume/stop; no actual devices or Gemini.
- `cargo build -p alloyos-dev-harness --example voice_qemu_probe`, then
  `python3 scripts/audio/check_qemu_media.py`: actual QEMU and native helper,
  localhost model, production Forge/Welder `clock.now` MCP action.
- `scripts/audio/check_live_device.command`: explicit human Terminal run using
  actual microphone and Gemini for about 44 seconds; sanitized summary at
  `/private/tmp/alloyos-voice-live-summary.json`. Physical speaker audibility
  requires human confirmation. `python3 scripts/audio/check_qemu_media.py
  --live-device --preflight` checks artifacts without devices or network.

Normal and fixture starts require `direct_audio: true`. A missing flag fails
before microphone or network access with an actionable rebuild message. This
prevents stale guest kernels from silently reverting to PCM over UART. Capture
fixtures invoke the same `captureData` handler as the native converter callback,
including all session, mute, provider-readiness, and transport-mode gates.

Host telemetry uses two latest-only slots, so old level/stat snapshots cannot
queue ahead of provider control or tool messages. Levels begin only after
provider readiness; connecting stats remain host-local. Control messages use a
separate bounded queue and are delivered before replaceable telemetry.
