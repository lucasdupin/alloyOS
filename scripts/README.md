# Local Forge model assets

`python3 scripts/download_gemma.py` downloads the initial default model from
Google's official [`google/gemma-2-2b-it-GGUF`](https://huggingface.co/google/gemma-2-2b-it-GGUF)
repository. Accept the Gemma license on Hugging Face first. The script pins
revision `67803c710cb0c89bae2047a5f50a2481c9339d24`, file `2b_it_v2.gguf`,
size 10,463,413,632 bytes, and the published LFS SHA-256. This full precision
artifact needs a 16 GiB guest and approximately 21 GB host disk space for weights
plus the packaged image.

Pass only a credential reference:

```sh
python3 scripts/download_gemma.py --auth-env HF_TOKEN
python3 scripts/download_gemma.py --auth-file /private/path/hf-token
```

The token file contains just the Hugging Face token. Store it outside the
repository or under ignored `.secrets/`, with `chmod 600`. Do not type tokens
into shell command arguments. The default environment reference is `HF_TOKEN`.
Token values never appear in subprocess arguments, output, model metadata, or
the image. Redirected downloads do not forward credentials to CDN hosts.

Interrupted downloads retain `.part` and resume with HTTP Range. Servers that
ignore Range trigger a fresh download. SHA-256, pinned size, and GGUF magic are
verified before atomically installing the file. Errors are intentionally generic
to avoid printing credentials or signed download URLs. `--package-only` rebuilds
the image from an already verified local download.

Download progress shows a bar, percentage, transferred GiB, speed, and estimated
remaining time. A waiting indicator appears when no bytes arrive for five seconds.
Verification and image packaging also show progress. Terminals update in place;
redirected output emits a progress line every ten seconds.

Outputs live under ignored `.local/models/gemma-2-2b-it/`:

- `2b_it_v2.gguf`: verified weights.
- `model.img`: a read-only VirtIO model disk. The kernel displays the boot
  screen first, then loads weights directly into reserved guest RAM in polled
  16 MiB transfers. QEMU does not preload weights or make temporary image copies.
- `manifest.json`: public provenance and integrity metadata, no credentials.

After downloading, launch the UI with the model attached:

```sh
cargo run -p xtask -- run --release --model-image .local/models/gemma-2-2b-it/model.img
```

Attaching a model enables the Forge kernel backend and defaults guest RAM to
16 GiB. Use `--memory 16G` or a larger supported size to override it. A normal
launch without `--model-image` has no resident weights.

The image starts with a 4096-byte header: `FGMODEL1`, little-endian u64 payload
length at byte 8, SHA-256 at byte 16, zero-padded model ID at byte 48 (64-byte
field), then the GGUF payload at byte 4096. Sector alignment adds zero padding
at the end. Boot loading borrows the resident payload without copying it to the
small kernel heap. This model image is separate from the writable CobaltFS image.

Welder launch authentication uses environment variable names or private files
containing `NAME=VALUE` lines; unlike the downloader's token file, launch files
can carry multiple credentials. QEMU receives a private transient `fw_cfg` file
named `opt/alloyos/auth`, containing NUL-delimited entries. The guest reads it
into volatile RAM; no persistent guest filesystem receives it. Normal shutdown
and Ctrl-C remove the temporary file. Abrupt termination cannot guarantee
cleanup of files in the host temporary directory. Local process/memory inspection
remains outside this credential channel's threat boundary.

Fixture and redaction checks require no network or gated weights:

```sh
python3 -m unittest discover -s scripts -p 'test_*.py'
```

Passing these checks validates acquisition and image structure; it does not
claim that real Gemma weights were downloaded or generation succeeded.

Cloud API keys configured by setup now enable Forge providers at launch. Use
`/model claude`, `/model gemini`, or `/model openai` in chat, and `/model local`
to return to Gemma. `xtask run` discovers the provider environment variables or
same-named private `.secrets` files and removes referenced secrets from QEMU's
inherited environment. The host HTTPS relay needs `curl` and verifies certificates.
Requests may incur provider charges. Live account access is not covered by fixture tests.

The host relay retries explicit HTTP 502/503/504 failures up to three times,
with 1/2/4-second exponential delays plus up to 500 ms jitter. The entire
operation has a 50-second budget, below the guest's 55-second timeout.
Authentication, quota (429), and ambiguous transport failures are not retried.
Persistent provider outages still return the final HTTP error.

To verify live Gemini generation through QEMU, fw_cfg credentials, and the
production HTTPS relay (incurs API usage), build the kernel and run:

```sh
cargo test -p alloyos-dev-harness --test gemini_live -- --ignored
```

This uses `GEMINI_API_KEY` from the environment or `.secrets/GEMINI_API_KEY`;
credentials and provider response bodies are not printed.


Run a guest-to-host API integration check without network access or real keys:

```sh
cargo build -p alloyos-kernel --target aarch64-unknown-none
python3 scripts/check_cloud_qemu.py --kernel target/aarch64-unknown-none/debug/alloyos-kernel
```

This loads fixture credentials through fw_cfg and exercises all three provider
requests and responses over the real UART path. It does not validate API accounts.

Large-file model loading is covered by kernel unit tests using small headers
and numeric sizes and offsets. These check multi-gigabyte bounds without
allocating model buffers, creating large files, or launching QEMU:

```sh
cargo test -p alloyos-kernel --lib drivers::forge::tests::model_
```

To check actual Gemma chat responsiveness and the first streamed reply through the
keyboard UI, run (requires the full model image and macOS HVF):

```sh
python3 scripts/check_local_chat_qemu.py --kernel target/aarch64-unknown-none/release/alloyos-kernel --model-image .local/models/gemma-2-2b-it/model.img
```

The test verifies continued rendering and keyboard input while Forge's independent
kernel task executes inference, then waits for the first text in Welder's chat.
# Protected-process guest acceptance

`python3 scripts/test-process-isolation.py` builds the `process-selftest` kernel
variant and runs bounded EL0 probes in headless QEMU with TCG. Use
`--gic-version 3` to exercise GICv3, or `--accel hvf --gic-version 3` on Apple
Silicon. The runner requires the guest PASS marker and terminates its own QEMU
instance; it does not change the filesystem disk or run production apps.

The probes verify restricted syscalls, ABI discovery, private-stack output,
invalid-pointer rejection, denied shared-descriptor access, granted IPC,
privileged-memory fault containment, and progress alongside a non-yielding
process. They establish the low-level memory and syscall boundaries.

Run `python3 scripts/test-process-runtime.py` for the protected production runtime.
It packages `/system/ferrofluid` and `/bin/app-runner` on CobaltFS, then exercises
real app handlers, capability-scoped host operations, app resource containment and
Ferrofluid crash/restart with host conversation/pinned-Tile restoration. Use
`--gic-version 3` for GICv3 and `--accel hvf` on Apple Silicon.

`cargo xtask build --release` builds the kernel and protected executables. For UI
iteration, `cargo xtask processes --release` rebuilds and packages executables,
skipping the explicit guest kernel build. Image tools share `alloyos-fs` with the
kernel; xtask no longer imports the host kernel library. The opt-in
[M9 build evidence gate](test-build-independence.md) checks the planned boundary
and records unchanged-kernel evidence. Existing small development volumes migrate to
128 MiB while preserving files and keeping an original backup. Restart the guest
to load an updated executable; running guest storage must not be overwritten.

## Gmail MCP search acceptance

`python3 scripts/check_mcp_search_qemu.py --kernel /absolute/path/to/alloyos-kernel --log /tmp/mcp-search.jsonl`
boots a fresh, diskless QEMU VM and exercises the UI/Welder chat path with two
Gmail searches. By default the model is simulated and the official public MCP
Registry is live. Add `--live-model` to use the configured Gemini key; this sends
OS tool definitions, synthetic questions and public registry metadata to Google
and incurs API usage. Each search permits at most eight model calls, each with a
2,048-token output budget. It never loads existing conversations or personal files.

The stage log records token counts, finish reasons, registry counts, chat results
and UI frame progress without credentials, prompts or raw response text. Passing
requires fresh registry lookups and final answers containing a returned service
name. Discovery returns service metadata; Gmail account access requires a separate
authenticated connection. External server choices are restricted to web-hosted
endpoints; local-only stdio/Node/Python/MCPB listings are not offered. Native
alloyOS JavaScript packages and their protected app runner are separate from
external MCP server hosting.

For graphical input/restart verification, build the normal release kernel and run
`python3 scripts/test-process-ui.py`. It sends `/apps` through the real VirtIO
keyboard, captures the reply, then sends `/display restart` and waits for a fresh
process frame. Review native PNGs in `target/process-validation`; native PNG avoids
the installed QEMU PPM capture’s RGB row-padding issue at the 1206-pixel width.
Protected user executables are optimized/stripped even with the default debug
kernel so they fit the packaged volume and loader quota.

## Crash diagnostics

Bare-metal builds retain frame pointers. Kernel panics and synchronous faults
print `[crash]` records with fault registers, process identity and up to 32 stack
frames. Corrupted or inaccessible stacks stop with a reason instead of following
arbitrary pointers. Panic output bypasses the serial mutex and allocator. EL0
Ferrofluid and app-runner panics report their message before a breakpoint; the
kernel prints the private stack and contains the fault as before.

The development harness displays and appends only crash records to
`target/alloyos-crash.log`; set `ALLOYOS_CRASH_LOG` to choose another path. Raw
Forge/MCP HTTP envelopes are excluded. The serial-shell runner uses its ordinary
terminal output, which can be redirected to a file.

Resolve addresses against the **exact ELF from that guest**:

```sh
python3 scripts/symbolicate_crash.py --elf target/aarch64-unknown-none/debug/alloyos-kernel --log target/alloyos-crash.log
python3 scripts/check_crash_qemu.py --log /tmp/alloyos-crash.log
python3 scripts/test-process-isolation.py --accel hvf --gic-version 3
```

The crash acceptance test uses an isolated build directory and diskless VM,
intentionally panics while holding the serial lock, and requires symbolicated
parent frames. It saves the matching ELF next to the log. Use `--accel tcg` on
hosts without HVF. Symbolication discovers `llvm-nm` from PATH or the Rust
`llvm-tools-preview` component and prints demangled function names and offsets.
For user faults, pass the matching `ferrofluid` or `app-runner` ELF: all protected
processes use overlapping virtual addresses. Keep that ELF when collecting logs;
rebuilding can change addresses. Optimized/inlined calls may not have separate
frames; source line lookup is not provided by this helper.

Display recovery also emits a persisted `[crash] display-stopped` record with
the PID, exit code, reason and monotonic uptime in nanoseconds. `fault` (-11)
should have preceding exception/stack records; `execution-timeout` (-110) and
`requested-restart` (-125) do not pass through the exception handler and have no
fault stack. This distinguishes a crashed renderer from a deliberate restart
or an expired execution budget even after the renderer recovers.

### Secure entropy gate

```sh
cargo build -p alloyos-kernel --target aarch64-unknown-none --release --offline
python3 scripts/check_secure_entropy_qemu.py \
  --kernel target/aarch64-unknown-none/release/alloyos-kernel \
  --output /tmp/alloyos-secure-entropy-evidence
```

This diskless HVF/GICv3 gate boots without an entropy device, then with legacy
and modern VirtIO-RNG devices. It checks the actual cryptographic API fails
closed without a trusted seed and succeeds after the complete device seed is
received; logs and the exact kernel SHA are retained. It never prints random
bytes or accesses accounts, networks or disks. Standard xtask QEMU launches attach
VirtIO-RNG. `/dev/urandom`, `getrandom` and OAuth use the fail-closed generator;
timer-derived general randomness is separate and cannot enable it. A future
hardware port must supply a trusted entropy source; this gate does not validate
Raspberry Pi entropy hardware. The virtual entropy source trusts its hypervisor.

### OS OAuth guest integration fixture

```sh
cargo build -p alloyos-kernel --target aarch64-unknown-none --release \
  --features oauth-selftest --offline
python3 scripts/check_oauth_guest_qemu.py \
  --kernel target/aarch64-unknown-none/release/alloyos-kernel \
  --disk /tmp/alloyos-native-consent-acceptance.img \
  --output /tmp/alloyos-os-oauth-evidence
```

Supply a formatted CobaltFS image (for example, create a separate image with
`cargo xtask processes --release --disk-image /tmp/alloyos-native-consent-acceptance.img`).
The gate copies it and boots three times with synthetic UART responses. It runs
the production OS broker, cryptographic entropy, PKCE, private store, browser and
HTTP ports; checks authenticated MCP discovery/call, restoration after reboot,
refresh and local revocation despite a provider failure. A final reboot must not
restore revoked authority. Logs omit callback, token and authenticated request
frames. The fixture uses no real accounts, browser, provider, DNS or network;
it does not establish Gmail readiness or validate a provider's OAuth deployment.
The self-test feature is opt-in and absent from ordinary builds.
