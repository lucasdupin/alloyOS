# Local User Knowledge

This document describes the planned user knowledge repository. Profile learning,
retrieval tools, and persistent storage are not implemented yet.

## Ownership and storage

The main agent maintains a local `/knowledge/` repository as the user expresses
facts, preferences, interests, and goals in conversation. Markdown files are the
initial storage format; a database is not required for the first implementation.
The repository belongs to the device owner and is runtime data, not personal
information checked into the alloyOS source repository.

```text
/knowledge/
├── profile.md          # Stable facts explicitly provided by the user
├── preferences.md      # Communication, visual, and workflow preferences
├── interests.md        # Recurring interests and ongoing goals
├── observations.md     # Tentative patterns inferred from conversation
└── projects/           # Context scoped to individual projects
```

CobaltFS provides persistence when a disk is attached. A RamFS fallback is
volatile and must not be presented as memory that survives a reboot.

## Learning and retrieval

Profile learning is a built-in responsibility of the main agent. There is no
separate profile skill and no top-level `/skills/` directory. Skills and task
instructions are supplied by applications under `/apps/`; knowledge records
describe the user rather than instruct the agent how to perform a task.

After relevant conversation turns, the agent identifies durable information,
merges it into existing entries, and records the source, update time, scope, and
whether it was explicitly stated or inferred. A one-off request should not
automatically become a global preference. For example, a monochrome boot-screen
request is initially scoped to alloyOS, not to every future design task.

At the start of a conversation, the agent loads a bounded profile summary. It
retrieves additional project or preference records only when relevant, rather
than putting the entire repository into every model request. Stored text is
context, not authority to override current user instructions. Corrections from
the user take precedence over earlier records and tentative observations.

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

Planned structured tools are `knowledge_search`, `knowledge_upsert`, and
`knowledge_forget`. The storage layer validates paths and updates, bounds record
and context sizes, and protects persisted records against interrupted writes.
Apps may supply relevant context through the agent, but do not directly rewrite
the user profile or gain unrestricted access to the knowledge repository.

The user can inspect what was learned, correct or delete entries, and pause
automatic learning. Forgotten entries must also be removed from generated
summaries and retrieval results. Profile extraction should not retain secrets
or turn uncertain observations into asserted facts.

Local storage does not imply local model processing: retrieved context included
in a cloud provider request leaves the device. Only relevant context should be
included; apps should receive only the information needed for their task.

## Implementation sequence

1. Connect Ferrofluid submissions and responses to the main agent runtime.
2. Connect the AI filesystem tools to the real kernel VFS. Their current
   `NativeToolRegistry` storage is an in-memory map, not persistent CobaltFS.
3. Create the Markdown repository and implement validated read, update, and
   delete operations with reboot-persistence tests.
4. Add bounded retrieval and post-turn profile learning, with source tracking
   and a distinction between explicit statements and inferred observations.
5. Expose inspection, correction, deletion, and learning controls in Ferrofluid.
6. Add app-provided skill discovery under `/apps/` independently of user knowledge.
