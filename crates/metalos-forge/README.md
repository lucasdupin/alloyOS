# Forge

Forge is MetalOS's resident, local, multi-model inference service. Gemma 2 2B IT
is the initial default. Loading a backend retains its borrowed weights in guest
RAM; requests allocate bounded, independent KV caches. No network or canned
fallback exists. Other architectures implement `InferenceBackend` and register
under independent model IDs.

The portable CPU backend executes Gemma2 F32/F16 GGUF tensors, with SentencePiece
BPE, grouped-query attention, alternating sliding/full causal attention, NeoX
RoPE, four layer RMS normalizations, gated GELU and attention/output soft caps.
GGUF-converted normalization weights already include Gemma's unit offset.
Quantized tensor formats fail with `UnsupportedEncoding` rather than silently
producing incorrect results. Greedy decoding is deterministic. Context is capped
at 512 total prompt and output tokens to bound the initial guest heap footprint.
This scalar reference backend favors correctness and portability, not throughput;
2B generation in emulated QEMU may be very slow and is synchronous today.

```
MetalOS.forge("Explain this status", {model: "gemma-2-2b-it", max_tokens: 64});
```

The JavaScript host explicitly grants this capability with `set_forge_handler`.
Absent capability, weights, unknown models and unsupported backends return errors.
The AI console selects Forge by default, with cloud providers explicit alternatives.

`metalos-forge` owns model loading, inference, cloud provider adapters, and model
selection. `metalos-welder` is the agent runtime that routes requests through
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

`cargo test -p metalos-forge -p metalos-js -p metalos-welder` checks strict framing,
UTF-8 round trips, missing-weight errors, numeric primitives, malformed GGUF,
actual tiny GGUF forward/generation, two resident models, budgets, deterministic
requests, and JS capability validation. The tiny model is a numeric fixture,
not evidence that official 2B weights have been run. Full-model validation needs
the gated weights and a sufficiently large guest and remains a separate check.

Graph and format references:
- https://github.com/ggml-org/llama.cpp/blob/master/src/models/gemma2.cpp
- https://github.com/ggml-org/llama.cpp/blob/master/conversion/gemma.py
- https://github.com/ggml-org/ggml/blob/master/docs/gguf.md
