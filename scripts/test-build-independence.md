# M9 build independence gate (opt-in)

This gate tests the build portion of ROADMAP Phase 5/M9. It deliberately is not
part of default CI while service separation is planned. Run its fixture tests:

```sh
python3 scripts/test_build_independence.py
python3 scripts/test-build-independence.py architecture
```

The architecture check reads Cargo metadata without building. It rejects normal
or build dependency paths from xtask to kernel and kernel to Welder/Ferrofluid,
and requires a Welder executable target. Dev-only dependencies are allowed.
The filesystem/image library extraction removes xtask's kernel dependency.
Remaining prerequisites are unmet: kernel imports Welder, and Welder is a
library. This gate does not implement the missing service.

After service packaging exists, make a full baseline build. Record the guest
kernel and any host kernel library artifacts whose identity must remain stable:

```sh
python3 scripts/test-build-independence.py record \
  --kernel target/aarch64-unknown-none/release/alloyos-kernel \
  --baseline /tmp/m9-before.json
```

Edit only setup text/logic, then execute the implementation's Welder build and
packaging command. Save complete Cargo stdout using `--message-format=json`;
capture stderr separately. For nested Cargo invocations, preserve their JSON
output as well, including the invocation that builds xtask. Do not reuse logs
from a baseline build. Then run:

```sh
python3 scripts/test-build-independence.py verify \
  --baseline /tmp/m9-before.json --cargo-log /tmp/m9-cargo.jsonl \
  --component alloyos-welder
```

Repeat from a new baseline after a Ferrofluid-only edit with component
`alloyos-ferrofluid`. Verification requires a newly compiled component binary,
successful Cargo completion, unchanged SHA-256 and nanosecond mtime for every
recorded kernel artifact, and no kernel compiler artifacts marked non-fresh.
An empty, cached-only, library-only or unrelated build cannot pass. Evidence
must cover the whole build/package operation; this script cannot establish log
provenance or observe omitted subprocess output.

Passing proves only the supplied build evidence. Separately boot/restart the
service and verify the changed wizard/UI, setup persistence, conversation
restoration, authorization, fault recovery and ABI compatibility. No claim of
runtime acceptance follows from this gate.
