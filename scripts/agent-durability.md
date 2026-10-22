# Agent and pinned Tile durability gate

Create a separate clean packaged seed, then run the opt-in guest gate:

```sh
cargo build -p alloyos-kernel --target aarch64-unknown-none --release --offline
cargo xtask processes --release --disk-image /tmp/alloyos-durability-seed.img
python3 scripts/check_agent_durability_qemu.py \
  --kernel target/aarch64-unknown-none/release/alloyos-kernel \
  --disk /tmp/alloyos-durability-seed.img \
  --output /tmp/alloyos-agent-durability-evidence
```

Requires Apple Silicon HVF, QEMU, Python and Pillow. The disk argument is required;
the script copies it and the kernel into a temporary directory before booting, never attaches
that input directly, and checks its SHA-256 and modification time afterward.
No developer volume is selected by default. Use a clean seed: existing setup or
calculator state intentionally fails the acceptance assumptions.

The gate runs production kernel, Ferrofluid, Welder and native SDK JavaScript
apps. Gemini-shaped responses are deterministic local fixtures with an invalid
fixture key. Unexpected account, browser, voice or MCP network requests fail the
run; no provider HTTP client is invoked.

Three durable boots check:

- Validated setup names survive an interrupted wizard; the next boot resumes the
  correct question and selected model configuration.
- Clock time-zone/format and learning preferences survive completed setup.
- Actual pointer pin/unpin operations establish a persisted Calculator/Clock
  order; calculation data is restored into the native pinned Tile after reboot.
- Idle Clock content refreshes without input. A bounded pixel comparison checks
  its value/preferences region, away from the animated backdrop.
- Clicking the actual Clear Tape action changes the app's persisted history.
- A prior conversation marker is absent from every subsequent model request.
  Conversation restoration is not part of the feature.

PNG captures preserve the actual rendered setup/greeting, full Tile contents and
before/after action states for visual review. These are not OCR assertions. Pin
order is established through unpin/re-pin, not a dedicated reorder control.

Two diskless boots exercise the actual headless agent loop over its existing
private console port. The gate verifies reported RamFS storage, session-only
`/model`, `/knowledge pause/resume` and `/setup` acknowledgements, an in-session setup write, and loss of
that model/setup state after reboot. It requires unavailable
`/system/ferrofluid` and does **not** claim a diskless graphical setup test.
The generic kernel `Slint window online` proxy diagnostic does not establish
that the EL0 renderer loaded; only its own ready marker can establish that.

The script retains stage logs and `result.json` with the kernel/seed hashes.
`--inspect` pauses captures for developer inspection and optional explicit
pointer clicks; use the normal invocation for reproducible acceptance.
