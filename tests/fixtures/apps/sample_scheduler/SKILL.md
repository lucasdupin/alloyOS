---
name: sample_scheduler
description: List, create, edit, and delete scheduled sample_scheduler backed by alloyOS ScriptScheduler, and present interactive Sample Scheduler Tiles.
---

# Sample Scheduler App Skill (`/apps/sample_scheduler/SKILL.md`)

**Sample Scheduler** (`/apps/sample_scheduler/`) is the canonical scheduled-automation application in alloyOS.
It is the **only** application permitted to access `ScriptScheduler` (`/etc/schedules.conf` and `/scripts/*.js`). Welder must always route schedule listing, creation, editing, and deletion through the `Sample Scheduler` MCP entry points.

## Architectural Rules
1. **Exclusive Scheduler Access**: Only `/apps/sample_scheduler/` holds `ScriptSchedulerAccess`. No other app may access `ScriptScheduler` or `/etc/schedules.conf`.
2. **Centralized Orchestration**: Apps cannot invoke or access other apps. Only Welder orchestrates across apps.
3. **Renderer-Agnostic Tiles**: `Sample Scheduler` exposes declarative `AppTile` schemas (`sample_scheduler.board` and `sample_scheduler.detail`) that Ferrofluid renders.

## MCP Entry Points

### Tools
- `sample_scheduler.list`
  - **Description**: List all scheduled sample_scheduler managed by `ScriptScheduler`.
  - **Parameters**:
    - `include_disabled` (`boolean`, optional, default `true`): Whether to include paused/disabled sample_scheduler.
- `sample_scheduler.create`
  - **Description**: Create and schedule a new task in `ScriptScheduler`.
  - **Parameters**:
    - `title` (`string`, required): Human-readable task name.
    - `script_path` (`string`, required): Target JS module path under `/scripts/*.js` (e.g. `/scripts/backup.js`).
    - `trigger` (`string`, required): Schedule trigger (`every:60s`, `interval:5000ms`, `once:10s`, `cron:*/5 * * * *`, or OS event `boot`, `fs:create:/inbox/*`, `fs:modify:/etc`, `net:online`, `net:request:8080`).
    - `id` (`string`, optional): Custom schedule identifier (auto-generated if omitted).
    - `source` (`string`, optional): JavaScript ES module source code to register in `ScriptStore`.
    - `enabled` (`boolean`, optional, default `true`): Whether the schedule is immediately active in `ScriptScheduler`.
- `sample_scheduler.edit`
  - **Description**: Edit an existing scheduled task's title, trigger, script path, JS source, or enabled state.
  - **Parameters**:
    - `id` (`string`, required): Identifier of the scheduled task to edit.
    - `title` (`string`, optional): Updated human-readable title.
    - `trigger` (`string`, optional): Updated schedule trigger specification.
    - `script_path` (`string`, optional): Updated `/scripts/*.js` target path.
    - `source` (`string`, optional): Updated JavaScript source code.
    - `enabled` (`boolean`, optional): Pause (`false`) or resume (`true`) the task in `ScriptScheduler`.
- `sample_scheduler.delete`
  - **Description**: Delete an existing scheduled task by ID from `ScriptScheduler`.
  - **Parameters**:
    - `id` (`string`, required): Identifier of the scheduled task to remove.

### Resources
- `mcp://sample_scheduler/schedules`: Live JSON snapshot of all scheduled sample_scheduler and `/etc/schedules.conf` state.
- `mcp://sample_scheduler/tiles`: Declarative Tile descriptors (`sample_scheduler.board`).
- `mcp://sample_scheduler/skill`: This `SKILL.md` instruction document.

### Prompts
- `sample_scheduler.manage`: Guided prompt template for creating or editing a scheduled task from natural language.

## Tiles
- `sample_scheduler.board`:
  - Displays all scheduled sample_scheduler with their title, script path, trigger badge, and active/paused status.
