# alloyOS development harness

The development harness launches a headless alloyOS guest and forwards bounded local HTTP requests
to its resident Forge service over PL011 serial. Requests are real guest operations;
a missing model or unsupported backend returns an error.

```
cargo xtask forge-bridge --model-image target/models/gemma2.img --memory 16G
curl http://127.0.0.1:7070/v1/status
curl http://127.0.0.1:7070/v1/models
curl -X POST http://127.0.0.1:7070/v1/generate \
  -H 'Content-Type: application/json' \
  -d '{"prompt":"Hello","max_tokens":16}'
```

`model` is optional in generation requests; Forge selects its default when omitted.
Prompts are bounded to 8192 UTF-8 bytes; generation is bounded to 256 tokens.
HTTP bodies are bounded to 32768 bytes, headers to 8192 bytes. Serial framing is
`FORGE1`, request id, operation and tab-separated arguments; prompts and responses
use hex-encoded UTF-8. Replies are correlated by request id and bounded by
`--timeout` seconds (default 120). HTTP reads/writes time out after five seconds.
The endpoint binds exclusively to loopback and serves one request at a time.
It is a development endpoint accessible to local processes; it has no remote auth.

The Forge build is distinct from ordinary UI/debug-shell builds. Default guest RAM
is 16 GiB because official Gemma 2 2B GGUF contains full-precision weights. QEMU
loads the packaged FGMODEL1 image at the reserved guest address 0x80000000.
Missing weights are reported explicitly; a status response confirms transport,
while each model's state reports actual readiness.

Authentication launch parameters contain references only:
`--auth-env VARIABLE_NAME` or `--auth-file /private/path`. Credentials go through a
transient private fw_cfg file, removed when the guest exits. Token values must
never be supplied directly on command lines, committed, or written to model images.
The host download script takes its own auth reference; tokens are not needed for
local inference once model weights exist.

The host also provides a bounded HTTPS capability for Forge API requests.
`https::run_ui` services this capability while QEMU displays Ferrofluid; the
loopback development endpoint services it during a cloud generation request.
The host forwards only the fixed Claude/Gemini/OpenAI HTTPS endpoints, through
certificate-verified curl with redirects disabled and a 45-second request timeout.
Credentials and JSON bodies cross private stdin/stdout pipes and are not logged.
Forge owns model selection and API semantics; Welder has no network access.
