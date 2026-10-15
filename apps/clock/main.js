// alloyOS App: Clock (/apps/clock/main.js)
// Executed inside a capability-scoped alloyos-js isolate by alloyos-mcp.

function preferences() {
    let raw = AlloyOS.fs.read("/var/clock/preferences.json");
    if (raw === null) {
        return { timezone: "UTC", clock_format: "24h", error: null };
    }
    let saved = JSON.parse(raw);
    let error = preferenceError(saved.timezone, saved.clock_format);
    return { timezone: saved.timezone, clock_format: saved.clock_format, error: error };
}

function preferenceError(timezone, clockFormat) {
    if (!AlloyOS.sys.timeZones().includes(timezone)) {
        return "Unsupported time zone";
    }
    if (clockFormat !== "12h" && clockFormat !== "24h") {
        return "Clock format must be 12h or 24h";
    }
    return null;
}

function clockDisplay() {
    let settings = preferences();
    return AlloyOS.sys.localTime(AlloyOS.sys.status().wallClockMs, settings.timezone, settings.clock_format);
}

function callTool(toolName, args) {
    if (toolName === "clock.settings") {
        return JSON.stringify(preferences());
    }
    if (toolName === "clock.configure") {
        let error = preferenceError(args.timezone, args.clock_format);
        if (error !== null) {
            return JSON.stringify({ ok: false, error: error });
        }
        let saved = { timezone: args.timezone, clock_format: args.clock_format };
        AlloyOS.fs.write("/var/clock/preferences.json", JSON.stringify(saved));
        return JSON.stringify({ ok: true, timezone: saved.timezone, clock_format: saved.clock_format, error: null });
    }
    if (toolName === "clock.now") {
        let status = AlloyOS.sys.status();
        let uptime = status.uptimeMs;
        let display = clockDisplay();
        return JSON.stringify({
            uptime_ms: uptime,
            wall_clock_available: status.wallClockMs !== null,
            unix_time_ms: status.wallClockMs,
            timezone: display.timezone,
            clock_format: display.clock_format,
            local_time: display.local_time,
            local_date: display.local_date,
            utc_offset_seconds: display.utc_offset_seconds,
            error: display.error
        });
    }
    return JSON.stringify({ error: "unknown Clock tool" });
}

function readResource(uri) {
    if (uri === "mcp://clock/now") {
        let status = AlloyOS.sys.status();
        let uptime = status.uptimeMs;
        let display = clockDisplay();
        return JSON.stringify({
            uptime_ms: uptime,
            wall_clock_available: status.wallClockMs !== null,
            unix_time_ms: status.wallClockMs,
            timezone: display.timezone,
            clock_format: display.clock_format,
            local_time: display.local_time,
            local_date: display.local_date,
            utc_offset_seconds: display.utc_offset_seconds,
            error: display.error
        });
    }
    return JSON.stringify({ error: "unknown Clock resource" });
}

function renderTile(tileId) {
    if (tileId === "clock.now") {
        let display = clockDisplay();
        let wallTime = display.local_time;
        let wallSubtitle = display.timezone + " · " + display.clock_format + " · " + display.local_date;
        if (display.error !== null) {
            wallSubtitle = display.error;
        }

        return JSON.stringify({
            tile_id: "clock.now",
            title: "Clock",
            subtitle: "Local wall clock",
            refresh_resource_uri: "mcp://clock/now",
            items: [
                {
                    id: "wall-clock",
                    title: wallTime,
                    subtitle: wallSubtitle,
                    badge: "Wall clock",
                    status: "live"
                }
            ],
            actions: [
                {
                    id: "refresh",
                    label: "Refresh Clock",
                    tool_name: "clock.now",
                    default_args: {}
                }
            ]
        });
    }
    return JSON.stringify({ error: "unknown Clock tile" });
}

function getPrompt(name, args) {
    return "Clock has no prompts";
}
