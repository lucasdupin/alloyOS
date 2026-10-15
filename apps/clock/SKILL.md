---
name: clock
description: Show local wall-clock time in a Clock Tile and expose system uptime through the API.
---

Clock is a headless MCP app exposing `clock.now` and `mcp://clock/now`.
Both return `uptime_ms`, `wall_clock_available`, `unix_time_ms` (Unix epoch
milliseconds, or null when unavailable), `timezone`, `clock_format`, `local_time`,
`local_date`, `utc_offset_seconds`, and an explicit `error` when conversion is unavailable.

The `clock.now` Tile shows only the current local date/time using its saved time zone and 12/24-hour preference. Uptime remains available through the tool and resource. Refresh Clock reads both clocks
again through Welder. Uptime is monotonic and is never a time of day.
Wall time comes from the host RTC; when unavailable, report that explicitly.

Clock owns its preferences in its scoped local storage, `/var/clock/preferences.json`.
Use `clock.settings` to read them and `clock.configure` with `timezone` and
`clock_format` (`12h` or `24h`) to validate and save them. Invalid values are rejected
without changing saved preferences. First use defaults to UTC and 24-hour time.
The OS setup wizard invokes these app tools; it does not own a clock config file.

The native `AlloyOS.sys.localTime(epochMs, timezone, clockFormat)` helper only converts
explicit arguments; it never reads or saves preferences. `AlloyOS.sys.timeZones()`
lists the 12 supported IANA zones. Bundled transitions cover UTC dates from 1970
through 2100. Unsupported zones/dates are reported explicitly. Future DST rules
are projections from the bundled IANA snapshot and need regeneration when governments
change rules.
