# Welder

Welder owns the on-device agent: bounded model/tool orchestration, first-boot
setup, conversation context, app capabilities, native tools, and agent-exclusive
UI synthesis. It has no provider credentials or network transports.

`AgentRuntime` is the conversational console facade. It keeps history and the
native tool/UI registries, persists `/etc/ai.conf` in its virtual store, and
submits inference through Forge's `ModelGateway`. The kernel passes this runtime
to Ferrofluid's System UI. `Welder` provides the bounded tool loop; `Session`
adds setup, context, and capability-scoped app connections.

Forge owns model adapters and execution. Gemma (`gemma-2-2b-it`) remains the local
default. Selecting a cloud provider is explicit; generation requires a registered
model and attached transport in the shared Forge service. Missing weights or
transports return errors rather than canned replies.

The former `metalos-ai` crate has been retired: its agent code lives here, and
its provider wire types and Gemini/Claude/ChatGPT adapters live in Forge.

Validation: `cargo test -p metalos-forge -p metalos-welder`.
