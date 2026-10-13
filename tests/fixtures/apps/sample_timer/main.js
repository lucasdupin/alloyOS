// alloyOS App: Clock (/apps/sample_timer/main.js)
// Executed inside a capability-scoped alloyos-js isolate by alloyos-mcp.

function getSystemUptimeMs() {
    let status = AlloyOS.sys.status();
    return status.uptimeMs;
}

function pad2(n) {
    if (n < 10) {
        return "0" + n;
    }
    return "" + n;
}

function formatUptime(uptimeMs) {
    let totalSeconds = uptimeMs / 1000;
    let hours = totalSeconds / 3600;
    let minutes = (totalSeconds % 3600) / 60;
    let seconds = totalSeconds % 60;

    return pad2(hours) + ":" + pad2(minutes) + ":" + pad2(seconds);
}

function callTool(toolName, args) {
    if (toolName === "sample_timer.now") {
        let uptime = getSystemUptimeMs();
        return JSON.stringify({
            uptime_ms: uptime,
            wall_clock_available: false
        });
    }
    return JSON.stringify({ error: "unknown Clock tool" });
}

function readResource(uri) {
    if (uri === "mcp://sample_timer/now") {
        let uptime = getSystemUptimeMs();
        return JSON.stringify({
            uptime_ms: uptime,
            wall_clock_available: false
        });
    }
    return JSON.stringify({ error: "unknown Clock resource" });
}

function renderTile(tileId) {
    if (tileId === "sample_timer.now") {
        let uptime = getSystemUptimeMs();
        let formatted = formatUptime(uptime);

        return JSON.stringify({
            tile_id: "sample_timer.now",
            title: "Clock",
            subtitle: "System uptime · wall clock unavailable",
            refresh_resource_uri: "mcp://sample_timer/now",
            items: [
                {
                    id: "uptime",
                    title: formatted,
                    subtitle: "Since boot",
                    badge: "Uptime",
                    status: "live"
                }
            ],
            actions: [
                {
                    id: "refresh",
                    label: "Refresh Clock",
                    tool_name: "sample_timer.now",
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
