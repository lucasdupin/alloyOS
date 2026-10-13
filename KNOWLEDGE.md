# Local User Knowledge

This document describes the local user knowledge repository implemented in
[`crates/alloyos-welder/src/knowledge.rs`](crates/alloyos-welder/src/knowledge.rs),
[`crates/alloyos-welder/src/tools.rs`](crates/alloyos-welder/src/tools.rs), and
[`crates/alloyos-welder/src/runtime.rs`](crates/alloyos-welder/src/runtime.rs).

## Ownership and storage

The main agent (**Welder**) maintains a local `/knowledge/` repository as the user expresses
facts, preferences, interests, and goals in conversation. Markdown files are the
storage format. The repository belongs to the device owner and is runtime data,
not personal information checked into the alloyOS source repository.

```text
/knowledge/
├── profile.md          # Stable facts explicitly provided by the user
├── preferences.md      # Communication, visual, and workflow preferences
├── interests.md        # Recurring interests and ongoing goals
├── observations.md     # Tentative patterns inferred from conversation
└── projects/           # Context scoped to individual projects
```

CobaltFS provides persistence when a disk is attached (`KnowledgeStorageKind::DurableCobaltFs`).
A RamFS fallback is explicitly identified as volatile (`KnowledgeStorageKind::VolatileRamFs`)
and is never presented as memory that survives a reboot.

## Learning and retrieval

Profile learning is a built-in responsibility of the main agent (`NativeToolRegistry::learn_from_conversation_turn`,
invoked automatically by `AgentRuntime` and `Session` as conversations happen). There is no
separate profile skill and no top-level `/skills/` directory. Skills and task
instructions are supplied by applications under `/apps/`; knowledge records
describe the user rather than instruct the agent how to perform a task.

During conversation turns, Welder identifies durable information,
merges it into existing entries (`merge_entry_into_list`), and records the source, update time, scope, and
whether it was explicitly stated (`Explicit user statement.`) or inferred (`Inferred observation.`).
A one-off or component-specific request does not automatically become a global preference:
for example, a monochrome boot-screen request is scoped to `alloyOS boot screen`, not to every future design task.

At the start of each model turn, Welder loads a bounded profile summary (`knowledge_bounded_summary`)
and retrieves additional project or scoped preference records only when relevant (`knowledge_retrieve_relevant`),
rather than putting the entire repository into every model request. Stored text is framed as untrusted
context, not authority to override current user instructions. Corrections from
the user take precedence over earlier records and tentative observations, and tentative observations
never overwrite explicit user statements.

Example entry format:

```markdown
### Visual style
- Preference: Minimal black-and-white boot screen.
- Basis: Explicit user statement.
- Source: Conversation turn identifier.
- Updated: Timestamp supplied by the runtime.
- Scope: alloyOS boot screen.
```

## Agent tools and user controls

Structured native tools registered on Welder are `knowledge_search`, `knowledge_upsert`, and
`knowledge_forget`. The storage layer validates paths and updates against an allow-list of
knowledge categories (`profile`, `preferences`, `interests`, `observations`, `projects/<slug>`),
bounds record and context sizes, and protects persisted records against interrupted writes
via staging `.tmp` writes with readback verification and rollback (`write_knowledge_markdown`).
Apps may supply scoped project context through Welder (recorded as app-sourced observations under
`/knowledge/projects/<app_id>.md`), while MCP app write permissions (`alloyos-mcp` and
`ScriptCapabilities::for_mcp_app` in `alloyos-js`) are strictly allow-list based so each app
can only write to `/var/<app_id>/`.

The user can inspect what was learned, correct or delete entries, and pause or resume
automatic learning via `/knowledge` commands (`/knowledge inspect`, `/knowledge search <query>`,
`/knowledge correct <category> | <title> | <value> [| <scope>]`, `/knowledge forget <query>`,
`/knowledge pause`, `/knowledge resume`), natural-language forget/correction requests in
conversation, or the declarative `knowledge.profile` `AppTile` (`knowledge.pause`,
`knowledge.resume`, `knowledge_search`). Forgotten entries are immediately removed from both
the Markdown repository and generated summaries/retrieval results. Profile extraction filters
secrets, API keys, tokens, and passwords so credentials are never retained in `/knowledge/`.

Local storage does not imply local model processing: retrieved context included
in a cloud provider request leaves the device. Only bounded, relevant context is
included; apps receive only the information needed for their task.
