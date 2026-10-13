# Forge

Forge is AlloyOS's resident, local, multi-model inference service. Gemma 2 2B IT
is the initial default. Loading a backend retains its borrowed weights in guest
RAM; requests retain a bounded KV prefix cache and reuse identical input tokens. Cloud providers are explicit alternatives to local inference; no canned
fallback exists. Other architectures implement `InferenceBackend` and register
under independent model IDs.

The portable CPU backend executes Gemma2 F32/F16/Q4_0 GGUF tensors, with SentencePiece
BPE, grouped-query attention, alternating sliding/full causal attention, NeoX
RoPE, four layer RMS normalizations, gated GELU and attention/output soft caps.
GGUF-converted normalization weights already include Gemma's unit offset.
Other quantized tensor formats fail with `UnsupportedEncoding` rather than silently
producing incorrect results. Greedy decoding is deterministic. Context is capped
at 512 total prompt and output tokens to bound the initial guest heap footprint.
Prompt prefill runs in batches of at most 32 tokens, bounding temporary projection
buffers independently of conversation length within the context limit.
The CPU backend batches prompt projections and uses ARM NEON for F32 and Q4_0 dot products. Q4_0 projections quantize activations once to 8 bits and reuse them across weight rows.
Tensor kernels use explicit unaligned SIMD loads for Normal RAM: Q4_0 blocks
have an 18-byte stride, and the bare-metal target's default strict alignment
otherwise splits those loads into smaller accesses. MMU initialization clears
SCTLR_EL1.A. Device MMIO still requires naturally aligned accesses. The compiler's
default alignment policy remains intact outside these checked tensor kernels.
Matrix projections yield progress every 32 rows and retain timer preemption.
Generation runs in a timer-preempted Forge kernel task, with its own stack and
saved integer/SIMD context. Ferrofluid submits bounded messages and reads streamed
replies; inference has no per-frame time budget. Both tasks still share the kernel
address space. Private user-process address spaces remain future work.
2B generation under emulated QEMU may still be slow.

```
AlloyOS.forge("Explain this status", {model: "gemma-2-2b-it", max_tokens: 64});
```

The JavaScript host explicitly grants this capability with `set_forge_handler`.
Absent capability, weights, unknown models and unsupported backends return errors.
The AI console selects Gemini by default. Gemma remains available with `/model local`; saved provider selections override the initial default.

`alloyos-forge` owns model loading, inference, cloud provider adapters, and model
selection. `alloyos-welder` is the agent runtime that routes requests through
Forge and manages conversation history, native tools, configuration persistence,
and UI synthesis. Forge is the service name;
Gemma is the initial model, identified by `gemma-2-2b-it`.

## Wire protocol

Single UTF-8 lines with tab-separated fields, at most 32768 bytes:

```
FORGE1\t<ID>\tstatus
FORGE1\t<ID>\tmodels
FORGE1\t<ID>\tgenerate\t<MODEL or ->\t<MAX_TOKENS>\t<HEX_UTF8_PROMPT>
FORGE1\t<ID>\tok\t<HEX_UTF8_TEXT>
FORGE1\t<ID>\terror\t<CODE>\t<HEX_UTF8_MESSAGE>
```

`-` chooses the resident default model. Prompt limit is 8192 UTF-8 bytes; output
limit is 256 tokens. Hex encoding prevents embedded newlines/tabs corrupting UART
frames. Status/models text is JSON with `default_model` and `models` entries
containing `id`, `state`, and `weight_bytes`. Ready is only reported after an
executable backend attaches successfully.

## Validation

`cargo test -p alloyos-forge -p alloyos-js -p alloyos-welder` checks strict framing,
UTF-8 round trips, missing-weight errors, numeric primitives, malformed GGUF,
actual tiny GGUF forward/generation, two resident models, budgets, deterministic
requests, and JS capability validation. The tiny model is a numeric fixture,
not evidence that official 2B weights have been run. Full-model validation needs
the gated weights and a sufficiently large guest and remains a separate check.

Graph and format references:
- https://github.com/ggml-org/llama.cpp/blob/master/src/models/gemma2.cpp
- https://github.com/ggml-org/llama.cpp/blob/master/conversion/gemma.py
- https://github.com/ggml-org/ggml/blob/master/docs/gguf.md

## Cloud APIs

Forge supports Claude Messages, Gemini GenerateContent, and OpenAI Responses.
The defaults are `claude-sonnet-4-6`, `gemini-3.8-flash`, and `gpt-4.1`.
OpenAI's chat provider alias remains accepted as `/model chatgpt`, alongside
`/model openai`. Selecting a provider does not change the resident Gemma default
and failed requests never silently switch providers.

For `xtask run`, keys are discovered from `ANTHROPIC_API_KEY`, `GEMINI_API_KEY`,
`OPENAI_API_KEY`, or same-named owner-only files under `.secrets/`. Explicit
`--auth-env NAME` and `--auth-file PATH` references are also accepted. Files can
contain `NAME=VALUE` lines; same-named key files may contain a single raw key.
The interactive launcher prepares these references. Credentials cross into
volatile guest RAM through QEMU fw_cfg, not command-line key values or AI config.

Forge constructs the request and parses provider JSON. A bounded UART capability
relays HTTPS through the host's `curl`, using its certificate verification and
fixed provider destinations. The host handles no model routing or tool execution.
There are no redirects, insecure TLS mode, automatic retries, or credential logs.
Welder receives generated text or sanitized errors. Cloud requests run cooperatively with bounded transport work per frame, so
input and rendering continue while waiting. Replies are non-streaming. Local
Gemma inference remains synchronous. API charges are separate
from consumer subscriptions.

This QEMU bridge is required today because guest-native authenticated TLS is
not implemented. Direct QEMU launches must supply an equivalent HTTPS capability.
Offline fixtures cover all APIs, status failures, malformed replies, token bounds,
explicit provider selection, and the HTTPS relay; they do not verify account access.

API contracts: [Claude Messages](https://platform.claude.com/docs/en/api/messages/create),
[Gemini GenerateContent](https://ai.google.dev/api/generate-content),
[OpenAI Responses](https://developers.openai.com/api/docs/guides/text).


Model boot uses a read-only VirtIO disk. The boot logotype is presented before
any weight reads; the boot UI stays active while polled 16 MiB DMA transfers
load reserved RAM. The UI transitions to the workspace when initialization
finishes. No whole-image QEMU ROM preload or temporary copy is needed.

## Local quantization experiment

Convert the existing full-precision model without another download (requires NumPy):

```sh
python3 scripts/quantize_gemma.py .local/models/gemma-2-2b-it/model.img .local/models/gemma-2-2b-it/q4_0.gguf
python3 scripts/check_local_chat_qemu.py --kernel target/aarch64-unknown-none/release/alloyos-kernel --model-image .local/models/gemma-2-2b-it/q4_0.img --memory 4G
```

The conversion keeps normalization vectors in full precision and reduces the
model payload to approximately 1.48 GB. Original weights remain intact. Q4_0
changes model precision; conversational quality still needs evaluation.

The host-only `benchmark` example measures the same portable backend without
the guest or UI. It is a diagnostic, not a host inference bridge:

```sh
cargo run --release -p alloyos-forge --example benchmark -- .local/models/gemma-2-2b-it/q4_0.gguf 'Hello! Reply in one short sentence.'
```

`./run.sh` prefers an existing `q4_0.img`, with 4G guest RAM unless overridden.
Without that image it retains the full-precision acquisition path.
