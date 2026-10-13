---
name: clock
description: Tell the current system time, monotonic uptime, and formatted timezone offsets, and present a glanceable Clock Tile.
---

# Clock App Skill (`/apps/clock/SKILL.md`)

**Clock** (`/apps/clock/`) is a headless MCP application that reports the current time, UTC/timezone-adjusted wall clock, and system monotonic uptime, and exposes a declarative `clock.now` Tile.

## Architectural Rules
1. **Zero App-Owned UI**: `clock` never renders pixels or opens windows. It returns structured MCP values and declarative `AppTile` descriptors (`clock.now`) that Welder routes to Ferrofluid.
2. **Centralized Orchestration**: Only Welder invokes `clock.*` tools and resources.
3. **Scoped Storage**: `clock` stores user-selected display preferences (such as 12h/24h format or favorite UTC offsets) exclusively under `/var/clock`.

## MCP Entry Points

### Tools
- `clock.now`
  - **Description**: Return the current UTC time, optional UTC offset time, and monotonic system uptime.
  - **Parameters**:
    - `utc_offset_hours` (`integer`, optional, default `0`): Signed hour offset from UTC (e.g. `-7` for PDT, `9` for JST).
    - `format_24h` (`boolean`, optional, default `true`): Format the human-readable clock string in 24-hour (`17:06:45`) or 12-hour (`5:06:45 PM`) notation.

### Resources
- `mcp://clock/now`: Live JSON snapshot of the current time and monotonic uptime.
- `mcp://clock/tiles`: Declarative `clock.now` Tile descriptor.
- `mcp://clock/skill`: This `SKILL.md` instruction document.

### Prompts
- `clock.tell_time`: Prompt template for answering natural-language time and timezone queries concisely.

## Tiles
- `clock.now`:
  - Displays the current formatted time, active UTC offset badge, and system uptime row, with a quick `Refresh` action mapped to `clock.now`.
