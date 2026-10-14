# macOS voice device and socket helper

`./scripts/audio/build.sh` builds an ad-hoc signed native helper at
`target/alloyos-voice-helper`. The QEMU launcher builds it if missing; set
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
