---
name: clock
description: Show host-provided monotonic system uptime in a Clock Tile.
---

# Clock App Skill (`/apps/clock/SKILL.md`)

Clock is a headless MCP application exposing system uptime and the declarative `clock.now` Tile. A wall-clock source is currently unavailable. Do not describe uptime as local time, UTC, or the current time of day. Timezones and 12h/24h wall-clock formatting are unsupported until a real wall-clock source is connected.

Only Welder invokes Clock tools and resources. Clock returns semantic descriptors; Ferrofluid renders them. Any future stored preferences must remain under `/var/clock`.

## MCP entry points

- `clock.now`: No parameters. Returns `uptime_ms` and `wall_clock_available: false`.
- `mcp://clock/now`: The same live JSON snapshot.
- `/apps/clock/SKILL.md`: Guidance exposed through the app skill registry.

## Tile

`clock.now` shows elapsed hours, minutes, and seconds since boot, labeled Uptime and Since boot. Refresh Clock invokes `clock.now` through Welder and updates the rendered tile with the latest host uptime. It has no wall-clock or timezone controls.
