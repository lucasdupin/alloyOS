---
name: memory
description: Manage durable user facts, preferences, goals, and personal context stored persistently in alloyOS.
---

# Memory App Skill (`/apps/memory/SKILL.md`)

**Memory** (`/apps/memory/`) is the user-facing memory management application in alloyOS.

## Architectural Rules
1. **Durable Knowledge Store**: Stores explicit user statements, goals, preferences, and personal facts under `/var/memory/items.json`.
2. **Centralized Agent Memory**: Welder uses `memory.remember`, `memory.search`, and `memory.list` whenever the user explicitly asks the agent to remember or recall personal context ("Remember that I prefer TypeScript", "What is my project deadline?").
3. **Declarative Profile Tile**: Exposes `memory.profile`, presenting an organized view of active facts, preferences, and goals on the Dashboard.

## MCP Entry Points

### Tools
- `memory.remember`
  - **Description**: Records a new durable fact or preference.
  - **Parameters**:
    - `content` (`string`, required): Detail to record.
    - `category` (`string`, optional, default `"fact"`): `"fact"`, `"preference"`, `"goal"`, or `"bookmark"`.
    - `source` (`string`, optional): Originating context or project name.
- `memory.list`
  - **Description**: Lists remembered items, optionally filtered by category.
- `memory.search`
  - **Description**: Searches remembered items by query substring.
- `memory.forget`
  - **Description**: Removes an item by its ID.

### Resources
- `mcp://memory/items`: JSON snapshot of all remembered records.

### Prompts
- `memory.curate`: Template for reviewing and organizing stored knowledge.

## Tiles
- `memory.profile`:
  - Visual summary of remembered items grouped with badges (`Fact`, `Preference`, `Goal`, `Bookmark`).
  - Action button to refresh or review items.
