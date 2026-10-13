---
name: sample_timer
description: Show host-provided monotonic system uptime in a Sample Timer Tile.
---

# Sample Timer App Skill (`/apps/sample_timer/SKILL.md`)

Sample Timer is a headless MCP application exposing system uptime and the declarative `sample_timer.now` Tile. A wall-clock source is currently unavailable. Do not describe uptime as local time, UTC, or the current time of day. Timezones and 12h/24h wall-clock formatting are unsupported until a real wall-clock source is connected.

Only Welder invokes Sample Timer tools and resources. Sample Timer returns semantic descriptors; Ferrofluid renders them. Any future stored preferences must remain under `/var/sample_timer`.

## MCP entry points

- `sample_timer.now`: No parameters. Returns `uptime_ms` and `wall_clock_available: false`.
- `mcp://sample_timer/now`: The same live JSON snapshot.
- `/apps/sample_timer/SKILL.md`: Guidance exposed through the app skill registry.

## Tile

`sample_timer.now` shows elapsed hours, minutes, and seconds since boot, labeled Uptime and Since boot. Refresh Sample Timer invokes `sample_timer.now` through Welder and updates the rendered tile with the latest host uptime. It has no wall-clock or timezone controls.
