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

A synthetic fixture larger than 2 GiB always exercises QEMU loading in
`xtask ci`, without downloading weights. The full-size QEMU loader regression runs automatically in `xtask ci` (including
the pre-push hook) when the local model image exists. It checks actual guest RAM
at the header, sampled boundaries, and the end of the image, and requires the
boot screen to appear within five seconds before model loading starts. Without
the asset, CI prints an explicit skip and does not download weights.

Run it directly with:

```sh
cargo test -p alloyos-dev-harness --test qemu_model_loader -- --ignored full_model_loads_into_qemu_at_contiguous_addresses
```

Set `ALLOYOS_TEST_MODEL_IMAGE` to test a different packaged image larger than
2 GiB. An explicitly configured missing image fails CI. Build a Forge-enabled
kernel before running the test directly:

```sh
cargo build -p alloyos-kernel --target aarch64-unknown-none --features forge
```

Use `ALLOYOS_TEST_KERNEL` to select another Forge-enabled kernel. The test needs
QEMU and 16 GiB guest RAM. It performs disk-to-RAM loading after splash without
duplicating the model on disk.

To check actual Gemma chat responsiveness and the first streamed reply through the
keyboard UI, run (requires the full model image and macOS HVF):

```sh
python3 scripts/check_local_chat_qemu.py --kernel target/aarch64-unknown-none/release/alloyos-kernel --model-image .local/models/gemma-2-2b-it/model.img
```

The test verifies continued rendering and keyboard input while Forge's independent
kernel task executes inference, then waits for the first text in Welder's chat.

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
authenticated connection.
