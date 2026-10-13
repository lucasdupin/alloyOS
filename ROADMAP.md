# alloyOS Roadmap

**alloyOS** is an AI-first, single-user, bare-metal ARM64 operating system written in `#![no_std]` Rust for QEMU `virt`, with native Apple Silicon acceleration through HVF. Ferrofluid provides the Slint system interface, Welder orchestrates the agent, Forge supplies resident and cloud model access, and CobaltFS provides persistent storage. Gemma 2 2B IT is the default local model; Claude, Gemini, and OpenAI are explicit cloud alternatives.

### Core Architectural Invariants
- **Single-User by Design**: Personal devices are single-owner devices. alloyOS intentionally omits multi-user support (no UIDs/GIDs, user session switching, `/home/<user>` hierarchies, or cross-user permission matrices). Security and isolation boundaries exist strictly between **the AI Agent** and **capability-scoped JS isolates / headless MCP apps**, eliminating an entire class of state-synchronization and permission bugs.
- **No Shell for the AI**: The AI agent cannot see or execute shell commands (`sh`, `exec`, `spawn`). It interacts with the OS exclusively through typed native tools, local JavaScript (`alloyos-js`), and MCP servers.
- **Agent-Exclusive UI & Declarative Tiles**: Developer apps are strictly headless (`alloyos-mcp`) and never link against or depend on any UI toolkit. Each app can expose **MCP entry points** (tools, resources, prompts), **SKILLs** (text files under `/apps/<app_id>/SKILL.md`), and declarative **Tiles** (`AppTile` schemas that can be embedded in the chat stream or pinned to the **Dashboard**). Only Welder and **Ferrofluid (`ferrofluidd`)** synthesize and render UI surfaces.
- **One Process per App (required; pending)**: Every JavaScript app runs in its own independently scheduled OS process with a private virtual address space. `alloyos-mcp` invokes app handlers through IPC; manifest capabilities remain enforced inside each app process. An app fault or hang must not stop the host, Welder, Ferrofluid, or other apps.
- **Welder-Centralized App Orchestration & Exclusive `Tasks` Scheduler Access**: No app can access or invoke another app; **only Welder centralizes app access and orchestration**. Furthermore, the canonical **Tasks** app (`app_id = "tasks"`) is the **only** app permitted to access `ScriptScheduler` (`/etc/schedules.conf` and `/scripts/*.js`), and Welder must route scheduled-task operations through `Tasks`.
- **Agent-Owned User Knowledge**: The planned knowledge service maintains local user context under `/knowledge/`, with source tracking and user correction/deletion controls. Skills are supplied by `/apps/`, independently of knowledge; no top-level `/skills/` directory is planned.

---

## Architecture Overview

The architecture centers on Welder as the agent, Forge as the model gateway,
Ferrofluid as the presentation layer, and CobaltFS as persistent storage.
Solid arrows show current relationships; dashed arrows show intended integrations
that are not yet connected end to end.

```mermaid
flowchart TB
    Boot["Bootloader / ARM64 entry<br/>Current: QEMU loads kernel ELF directly"]
    Kernel["Kernel foundation<br/>Memory, tasks, syscalls, drivers and VFS"]
    User["User"]
    Ferrofluid["Ferrofluid<br/>System UI, Dashboard, boot screen and input<br/>Owns rendering and display"]
    Welder["Welder<br/>Centralized app orchestrator, conversation and authorized tools<br/>Owns agent decisions and app interaction"]
    Forge["Forge<br/>Model registry, selection, inference and credentials<br/>Single gateway for model access"]
    Cobalt["CobaltFS<br/>Persistent filesystem behind kernel VFS"]

    subgraph Models["Model access — owned by Forge"]
        Local["Resident local models<br/>Gemma 2 2B IT by default<br/>Weights loaded from a read-only model disk into RAM"]
        Transport["Cloud transport<br/>Current: QEMU host HTTPS bridge<br/>Guest-native authenticated TLS pending"]
        Cloud["Cloud providers<br/>Claude / Gemini / OpenAI"]
        Transport --> Cloud
    end

    subgraph Apps["App architecture — headless, Welder-centralized, capability scoped"]
        Tasks["Tasks App (/apps/tasks)<br/>MCP entry points, SKILL.md & declarative Tiles<br/>Sole app with ScriptScheduler access"]
        OtherMCP["Future MCP Apps<br/>MCP entry points, SKILLs & Tiles<br/>Zero cross-app or scheduler access"]
        JS["JavaScript automation (alloyos-js)<br/>Isolated interpreter and ScriptScheduler"]
    end

    Boot -->|"Initialize"| Kernel
    Kernel -->|"Start System UI"| Ferrofluid
    Kernel -->|"Initialize agent"| Welder
    Kernel -->|"Initialize model service"| Forge
    Kernel -->|"Mount storage"| Cobalt
    User -->|"Input"| Ferrofluid
    Ferrofluid -->|"Requests"| Welder
    Welder -->|"Responses & Tile descriptors"| Ferrofluid
    Welder -->|"Model requests and results"| Forge
    Forge -->|"Local inference"| Local
    Forge -->|"Explicit cloud selection"| Transport
    Welder -->|"Sole app orchestrator"| Tasks
    Welder -.->|"Sole app orchestrator"| OtherMCP
    Tasks -->|"Exclusive schedule CRUD"| JS
    Welder -->|"Create and run scripts"| JS
    JS -->|"Opt-in AlloyOS.forge capability"| Forge
    Welder -.->|"Conversation, config and /knowledge/ persistence"| Cobalt
    Tasks -->|"Scoped /etc/schedules.conf & /scripts"| Cobalt
```

The current boot path uses the kernel's ARM64 entry code rather than a separate
bootloader executable. Ferrofluid owns the single system window; Welder supplies
chat responses and directs agent-generated views and declarative App Tiles. Each
app can expose MCP entry points, SKILL text files (`/apps/<app_id>/SKILL.md`),
and UI-agnostic Tiles (`AppTile`) that can be embedded in the conversation
stream or pinned to the **Dashboard**. No app can access another app — only
Welder centralizes app orchestration — and `Tasks` is the sole app allowed to
access `ScriptScheduler`.


---

## Phase 1: ARM64 Boot, Kernel & CobaltFS

The kernel provides the hardware and storage foundation for the system. Its
ARM64 entry code establishes the execution environment, initializes memory and
drivers, and starts Ferrofluid, Forge, and Welder. CobaltFS provides persistent
disk storage through the kernel VFS; RamFS and DevFS provide volatile files and
device access. The current QEMU boot path loads the kernel ELF directly.

- [x] **ARM64 boot and hardware support**
  - EL2-to-EL1 entry, exception vectors, MMU mappings, PL011 serial and PSCI shutdown
  - QEMU `virt` support with Apple Silicon HVF acceleration
- [x] **Kernel services**
  - Heap and frame allocation, task scheduling, syscall dispatch and device drivers
  - Userland syscall wrappers in `alloyos-user`; serial debug shell via `--debug-shell`
- [x] **Storage foundation**
  - VFS mount table, persistent CobaltFS, RamFS and DevFS
  - VirtIO block transport for filesystem disks
- [ ] **Agent storage integration**
  - Connect Welder's in-memory file tools and configuration to kernel VFS storage
  - Verify durable agent data across reboot and distinguish volatile fallback

---

## Phase 2: Networking & Authenticated Model Transport

The networking layer provides guest TCP/IP connectivity through VirtIO-Net and
smoltcp. Forge owns cloud API requests, credentials, and response parsing.
Cloud model access currently uses a bounded UART capability to a QEMU host
relay, which performs certificate-verified HTTPS. Guest-native authenticated
TLS remains a separate integration requirement.

- [x] **Guest network foundation**
  - VirtIO-Net transport, smoltcp stack and socket syscalls
  - DNS support and hardware RNG/CSPRNG exposed through `/dev/urandom`
- [x] **Forge cloud HTTPS capability**
  - Fixed provider destinations, bounded UART frames and host `curl` certificate verification
  - Cooperative transport polling keeps input and rendering active during cloud requests
  - Credentials enter volatile guest memory through fw_cfg; Welder receives text or sanitized errors
- [x] **Networking protocol libraries**
  - HTTP parsing, chunked transfer/SSE helpers and TLS-related primitives in `alloyos-user`
- [x] **Guest-native authenticated HTTPS**
  - Complete and validate TLS handshake, certificate authentication and encrypted transport
  - Connect Forge to native HTTPS without requiring the QEMU host relay
- [x] **Live response streaming**
  - Deliver incremental model output through Forge, Welder and Ferrofluid
  - Cloud SSE and local generation replies stream incrementally into the UI

---

## Phase 3: Boot Presentation & Model Readiness

Boot presentation makes system startup visible while storage and model services
initialize. Ferrofluid displays a alloyOS logotype with liquid turbulence in the same Slint
window that becomes the workspace. Forge loads local model weights from a
read-only VirtIO model disk into reserved guest RAM while the boot UI remains
active, then signals completion or reports a loading error.

- [x] **Early boot display**
  - VirtIO-GPU scanout and Slint boot logotype with liquid turbulence before model weight reads
  - Shared window for boot presentation and the interactive workspace
- [x] **Model loading and transition**
  - Polled DMA transfers into reserved RAM without a whole-image ROM preload
  - Resident backend attachment, readiness/error reporting and workspace transition
- [x] **Liquid animation library**
  - Metaball and mercury shading helpers in `alloyos-boot`
  - The active boot screen applies layered turbulence to the Slint logotype
- [x] **Boot logo turbulence**
  - Subtle layered displacement and bilinear sampling preserve smooth text edges
  - White droplets stretch from glyph edges, detach and fall during loading
  - Repaint the logo region while loading and stop the effect on workspace transition

---

## Phase 4: Ferrofluid System UI, Dashboard & Native Input

Ferrofluid owns the single fullscreen System UI, its bare-metal platform
adapter, and rendering. The kernel supplies input, monotonic time, and GPU
scanout. Ferrofluid forwards user submissions to Welder and presents replies
and model selection. The independent `alloyos-keyboard` component supplies the
soft keyboard and liquid displacement effects.

- [x] **Platform and rendering**
  - Software rendering, damage tracking, embedded Quicksand font and Fluent dark controls
  - All UI operations remain on the boot/UI execution context
- [x] **Conversation and keyboard input**
  - Conversation viewport, text editing, submission and soft keyboard
  - Welder request/reply bridge, active model display and pending-reply state
- [x] **Agent UI authorization**
  - Capability gate rejects app-owned UI and invalid agent credentials
- [ ] **The Dashboard (Customizable App Launcher & Dynamic Pinned Tiles)**
  - Customizable home surface and app launcher presented every time the user opens the device
  - Two-column bento layout with as many rows as needed; larger cards can span both columns
  - Hosts dynamic, live-updating **Tiles** pinned from installed apps (such as `tasks.board` from `Tasks`)
  - Implemented increment: pin/unpin/order state, persistent pin preferences, periodic resource refresh and built-in cards; app launcher and historical chat placement remain pending
  - The Dashboard is part of the conversation itself: conversations flow seamlessly underneath the Dashboard as the user scrolls and interacts
- [ ] **Agent-generated views & embedded chat Tiles**
  - Render Welder's synthesized views and declarative `AppTile` descriptors inline in the chat stream or pinned to the Dashboard, routing tile actions back to Welder
- [ ] **Interrupt-driven wakeups**
  - Replace the current input/timer loop, which polls at up to 60 Hz
- [ ] **Voice input**
  - Microphone capture and speech-to-text into the focused editor
  - Start/stop controls and recording/transcription status

---

## Phase 5: Welder Agent & Forge Model Access

Welder owns conversation, orchestration, authorized tool use, and centralized app
interaction. Forge is its model gateway: it owns model discovery and selection,
resident inference, cloud adapters, credentials, and provider transport. Gemma 2
2B IT is the resident default; cloud models require configured access and
explicit selection. Missing backends and failed requests return errors without
silently switching providers. Model selection remains manual through the
console model commands; task complexity does not change the selected provider.

- [x] **Forge resident inference**
  - Multi-model registry, bounded requests and resident backend lifecycle
  - Gemma2 F32/F16 GGUF CPU backend with weights retained in guest RAM
  - Local generation is synchronous; unsupported tensor encodings fail explicitly
- [x] **Forge cloud adapters**
  - Claude Messages, Gemini GenerateContent and OpenAI Responses
  - `/model local`, `/model claude`, `/model gemini` and `/model openai` selection
  - Credentials and network transport remain inside Forge's access boundary
- [x] **Welder runtime libraries**
  - `AgentRuntime` connects the graphical console to Forge and manages conversation and tool dispatch
  - Separate bounded `Welder` model/tool loop with routing policy and tool authorization
  - Native tool interfaces for files, status, scripts, MCP and agent UI synthesis
- [x] **Development access and model packaging**
  - Versioned Forge UART protocol and optional loopback inference endpoint
  - Host download/image packaging, authentication references and model-sized guest memory
- [ ] **Complete console agent integration**
  - Connect the graphical console to the bounded multi-step tool loop and real OS/app capabilities
  - Persist configuration, conversation and first-boot preferences on CobaltFS
- [ ] **Full-model acceptance and performance**
  - Verify readiness and generation with official Gemma weights in the target guest
  - Library fixtures establish interface and numeric behavior, not full-model acceptance

---

## Phase 6: JavaScript Automation & Scheduling

`alloyos-js` provides a custom `no_std` JavaScript interpreter for local automation.
Each isolate has deterministic execution limits and capability-scoped host
bindings. The scheduler supports time and event triggers; scripts can access
Forge only through an explicitly granted `AlloyOS.forge(...)` capability.

- [x] **Interpreter and host API**
  - Parsing, isolated execution and instruction fuel limits
  - `AlloyOS.*` interfaces for files, network, status and notifications; shell access is forbidden
  - `V8Isolate` is a compatibility alias for the custom interpreter
- [x] **Scheduling library**
  - One-shot timers, intervals, five-field cron expressions and event matching
  - Schedule serialization and host-backed save/load interfaces
  - Kernel boot dispatches the system boot event
- [x] **Live automation integration**
  - Connect Welder script tools to interpreter execution and the scheduler (routed through the `Tasks` app)
  - Wire ongoing kernel time, filesystem, network and lifecycle events
  - Supply real capability-scoped OS host bindings in place of virtual host data
- [x] **Durable scripts and schedules**
  - Store `/scripts/*.js` and `/etc/schedules.conf` through CobaltFS
  - Restore and verify automations across reboot
- [ ] **Boa JavaScript engine support (deferred; low priority)**
  - Keep the current `alloyos-js` subset interpreter for near-term automation and headless apps; broader ECMAScript compatibility is a later developer-platform improvement, not a prerequisite for current milestones
  - Introduce a shared engine boundary for source validation and execution, preserving `AlloyOS.*` capability policies, Welder-only app orchestration, and Tasks-only scheduler access
  - Resolve Boa's upstream `std` dependency for the bare-metal ARM64 environment before committing to integration; measure binary size, startup cost, and memory use on the guest
  - Validate module loading, promises/async host I/O, and execution/memory limits within protected app processes before enabling Boa for developer apps
  - Define supported language and host APIs explicitly; Boa support does not imply Node.js or unrestricted npm package compatibility

---

## Phase 7: Headless MCP Apps, SKILLs, Declarative Tiles & Tasks App

Every alloyOS application is a headless package under `/apps/<app_id>/` that can expose three declarative surfaces:
1. **MCP Entry Points**: Structured tools (`tools/list`, `tools/call`), resources (`resources/list`, `resources/read`), and prompts (`prompts/list`, `prompts/get`).
2. **SKILLs**: Plain-text Markdown instruction files (`/apps/<app_id>/SKILL.md`, exposed via `skills/list` and `skills/read`) teaching Welder how to orchestrate the app's capabilities.
3. **Tiles**: UI-framework-agnostic `AppTile` descriptors (`tiles/list`, `tiles/render`) that Ferrofluid can either **embed inline in the chat stream** or **pin to the Dashboard**.

Strict isolation and orchestration rules govern all apps:
- **Tasks (`app_id = "tasks"`) is our first canonical app**: It allows Ferrofluid and Welder to list (`tasks.list`), create (`tasks.create`), edit (`tasks.edit`), and delete (`tasks.delete`) scheduled tasks, and exposes `/apps/tasks/SKILL.md` and the `tasks.board` Tile.
- **Exclusive `ScriptScheduler` Access**: `Tasks` is the **only** app allowed to declare `ScriptSchedulerAccess` or touch `/etc/schedules.conf` and `/scripts/*.js`. Welder routes all schedule management through `Tasks`.
- **Welder-Centralized App Orchestration**: No app can access or invoke another app (`McpCaller::App` is rejected by `McpRegistry`). Only Welder (`McpCaller::Welder`) centralizes app access and cross-app workflows.

- [x] **MCP platform library (`alloyos-mcp`)**
  - JSON-RPC protocol (`tools/*`, `resources/*`, `prompts/*`, `skills/*`, `tiles/*`), app registry and `/apps/<app_id>/*.mcp` manifest discovery
  - Declarative, renderer-agnostic `AppTile` and `AppSkill` schemas
- [x] **First canonical app: `Tasks` (`/apps/tasks/`)**
  - Full CRUD over `ScriptScheduler` (`tasks.list`, `tasks.create`, `tasks.edit`, `tasks.delete`), `/apps/tasks/SKILL.md`, and `tasks.board` Tile (`ChatStream` + `Dashboard` placement)
  - Connected to Welder (`NativeToolRegistry`) with shared `ScriptScheduler` and VFS persistence
- [x] **Manifest capability enforcement**
  - Manifest validation and runtime guards reject UI, raw-input, shell, cross-app (`CrossAppAccessForbidden`), and unauthorized scheduler (`SchedulerAccessForbidden`) capabilities
  - Enforce that only `Tasks` can access `ScriptScheduler` and only Welder can invoke apps
- [ ] **Default app storage and network access**
  - Automatically provide read/write access to `/var/<app_id>/` and outbound network access; apps do not need manifest grants for either
  - Keep `/apps/` read-only to apps and deny access to other apps' data directories; package installation and updates remain OS-managed
  - Update manifest-to-runtime policies and regression tests to enforce these defaults and boundaries
- [ ] **Watch app packages and expose validation status to Welder**
  - Add an OS-owned recursive watcher for `/apps/*/`, covering package creation/deletion and changes to manifests, `main.js`, skills, and other package files; scan installed packages at startup too
  - Implemented increment: settled recursive VFS snapshots, package inventory/diagnostics at `/proc/app-packages.json`, static handler checks and revision-bound registry gates; legacy built-in adapters are not yet bound to package revisions
  - Coalesce related writes and validate a consistent package snapshot before registration or reload; notify Welder whenever a package changes so it can review validation results and explain failures
  - Check package layout, manifest/schema validity, JavaScript syntax and handler contracts, and permission boundaries; run execution checks in a bounded app process
  - Track each package's installed version, validated revision, readiness, and actionable errors with file locations; retain invalid packages in the inventory, but prevent invalid revisions from executing
  - Let users ask Welder which apps are installed and what errors each app has, including whether validation is pending or its result is stale
  - Verify nested edits, additions/removals, partial writes, invalid packages, and recovery after a fix with watcher and package-validation regression tests
- [ ] **PackageManager app (proposed)**
  - Provide a JavaScript app under `/apps/package-manager/` with MCP tools/resources and a status Tile for installed apps, per-package errors, and revalidation, following the app pattern used by Tasks
  - Back its inventory and lifecycle operations with a typed OS package service; Welder orchestrates requests, and PackageManager does not directly invoke other apps or write `/apps/`
  - Keep the watcher, validation records, and diagnostic service available even if the PackageManager app itself is invalid or unavailable
- [ ] **One protected OS process per JavaScript app**
  - Current apps execute inside the MCP host; JavaScript isolates and shared-address-space kernel tasks do not satisfy this requirement
  - Implemented foundation: private ARM64 page tables, bounded caller-bound IPC, EL0 context metadata and fault handling; app launch remains disabled pending syscall validation and resource ownership
  - Add user-mode app processes with per-process page tables, private heaps/stacks, and preemptive scheduling; prevent access to kernel and other app memory
  - Run `alloyos-js` inside each app process, loading `/apps/<app_id>/main.js` under its manifest-derived capability policy
  - Route MCP requests, results, and authorized OS operations over bounded IPC; derive caller identity from the process rather than trusting app-supplied IDs
  - Enforce memory and execution limits, request timeouts/cancellation, and process teardown/restart so a faulty app cannot block other apps or the system UI
  - Add QEMU regression tests for cross-process memory protection, denied capabilities, and continued host/UI/other-app operation when an app crashes, hangs, or exceeds its limits
- [x] **Agent presentation library**
  - Welder synthesizes structured views and attaches declarative `AppTile` payloads from MCP tool/resource outputs
- [ ] **Ferrofluid Dashboard & Tile renderer integration**
  - Render declarative `AppTile` descriptors inline in the Ferrofluid chat stream and pinned inside the customizable Dashboard
  - Route interactive `TileAction` triggers from Ferrofluid back through Welder to the owning MCP app

---

## Phase 8: Local User Knowledge & Profile Learning

Welder's planned knowledge service maintains durable user context as Markdown
under `/knowledge/` on CobaltFS. It distinguishes explicit facts and preferences
from tentative observations, records sources, and retrieves only relevant
context. User controls provide inspection, correction, deletion, and paused
learning. See [KNOWLEDGE.md](KNOWLEDGE.md) for the design.

- [ ] **Persistent knowledge storage**
  - Validate updates, protect interrupted writes and verify records across reboot
  - Distinguish durable CobaltFS storage from volatile RamFS fallback
- [ ] **Conversation-driven learning**
  - Extract and merge durable information after relevant agent turns
  - Track sources and timestamps; separate inference from explicit statements
- [ ] **Bounded retrieval and tools**
  - `knowledge_search`, `knowledge_upsert` and `knowledge_forget`
  - Compact summary plus relevant retrieval; stored context remains data
- [ ] **User controls and app boundaries**
  - Inspect, correct, delete and pause learning through Ferrofluid
  - Remove forgotten records from summaries and retrieval
  - Apps supply context through Welder; they cannot freely rewrite the profile
  - Share only relevant context needed for model requests

---

## Milestone Summary

| Milestone | Deliverable | Target Crate(s) | Status |
|---|---|---|---|
| **M1: ARM64 Core & CobaltFS** | Boot entry, kernel services, VFS and persistent storage | `alloyos-kernel`, `alloyos-user` | Foundation implemented; agent persistence pending |
| **M2: Networking & Model Transport** | Guest TCP/IP and authenticated Forge cloud access | `alloyos-kernel`, `alloyos-user`, `alloyos-forge`, `alloyos-dev-harness` | Implemented |
| **M3: Boot & Model Readiness** | Liquid boot logo, resident weight loading and workspace transition | `alloyos-boot`, `alloyos-kernel`, `alloyos-ferrofluid` | Logo turbulence and loading implemented |
| **M4: Ferrofluid System UI & Dashboard** | System UI rendering, customizable Dashboard with dynamic Tiles, chat bridge and native input | `alloyos-ferrofluid`, `alloyos-keyboard`, `alloyos-kernel` | Chat and keyboard connected; Dashboard, inline Tile rendering, IRQ wakeups and voice pending |
| **M5: Welder & Forge** | Centralized app orchestration, resident inference and cloud adapters | `alloyos-welder`, `alloyos-forge` | Runtime libraries and model access implemented; complete agent integration and full-model acceptance pending |
| **M6: JS Automation** | Capability-scoped interpreter and time/event scheduler | `alloyos-js`, `alloyos-welder`, `alloyos-kernel` | Current interpreter and scheduler implemented; Boa support deferred, low priority |
| **M7: Headless Apps (MCP, SKILLs & Tiles)** | JavaScript apps in separate protected processes, MCP entry points, `SKILL.md` files, declarative Tiles, and Welder-only orchestration | `alloyos-mcp`, `alloyos-js`, `alloyos-kernel`, `alloyos-welder`, `alloyos-ferrofluid` | MCP surfaces and manifest guards implemented; per-app process isolation and Ferrofluid Dashboard/chat Tile rendering pending |
| **M8: User Knowledge** | Durable profile, retrieval, learning and user controls | `alloyos-welder`, `alloyos-kernel`, `alloyos-ferrofluid` | Planned |
