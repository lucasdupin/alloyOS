# Protected setup worker: first Welder separation increment

The packaged `/system/welder` executable now renders and plans transitions for
the active first-run setup flow. It runs at EL0 with private memory and only two
supervisor-granted IPC ports: receive requests and send replies. It has no account,
installation, filesystem, network or display authority.

A versioned request contains the current setup snapshot, question revision,
Clock's reported settings and storage durability. Replies contain a bounded view
or a proposed next state and typed effects. The host independently validates
canonical answer normalization, consent choices, learning wording, the allowed
current-step transition, unchanged state fields and exact allowed effects before
writing anything. The three effect kinds are preferred-name knowledge, Clock
configuration and learning preference. The worker cannot name a path or choose a
generic tool. Profile and installation approvals remain on their existing trusted
native paths; no credentials are sent to this worker.

Messages are bounded to 128 KiB and fragmented over the existing 4 KiB IPC ABI.
Each exchange checks the actual child sender PID, request ID, wire version,
contiguous frame offsets and response/question revisions. Replayed IDs fail.
Each worker request has a one-second deadline; a stopped or hung worker returns
an error without advancing setup. A later request starts a new protected process
from the host snapshot. A partial proposal never authorizes a privileged effect.
Knowledge upsert, Clock configuration and learning flags are idempotent; a test
injects setup-save failure after the preferred-name effect and verifies that a
retry neither duplicates that record nor falsely advances the failed question.

CobaltFS remains the durable owner of setup/configuration. Conversation history
remains session-only. RAM-backed setup explicitly describes deferred progress as
session-only. Current fault restarts reuse a cached immutable executable; live
executable replacement/reload is not implemented by this increment.

This is not complete Welder separation: conversation/model/tool orchestration,
legacy deferred onboarding and persistence adapters still run in the host. The
kernel still depends on the Welder library. The roadmap's independent wizard edit
and unchanged-kernel artifact acceptance remains open.

## Reproduce the guest gate

Use an isolated checkout/workspace and an explicit temporary disk. Do not pass a
normal development volume. No model image, provider keys, microphone or OAuth
browser are required.

```sh
cargo run -p xtask -- processes --disk-image /tmp/welder-fixture.img
cargo build -p alloyos-kernel --target aarch64-unknown-none --release \
  --features process-runtime-selftest
python3 scripts/test-process-runtime.py --skip-build --accel hvf \
  --kernel "$PWD/target/aarch64-unknown-none/release/alloyos-kernel" \
  --disk /tmp/welder-fixture.img --timeout 120
```

The gate runs with 512 MiB RAM and QEMU snapshot writes. On hosts without HVF,
use the script's TCG option. The transcript must include all existing app/display
checks and this complete line, with no kernel panic:

```text
WELDER-SERVICE: protected setup transition, fault/timeout restart and replay rejection PASS
```

The Welder probe executes actual protected render/name-transition requests,
causes a real EL0 access fault, submits a nonyielding worker request to exercise
its deadline, verifies new worker processes render the retained snapshot, and
rejects a repeated request ID. These are synthetic setup inputs, not model output
or account authorization. The host contract checks run with:

```sh
cargo test -p alloyos-welder --lib --test setup_service
python3 -m unittest discover -s scripts -p test_process_acceptance.py
```
