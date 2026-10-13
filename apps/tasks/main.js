// metalOS Canonical App: Tasks (/apps/tasks/main.js)
// Executed inside a capability-scoped metalos-js isolate by metalos-mcp.
// Tasks is the ONLY app granted ScriptSchedulerAccess (MetalOS.scheduler.*).

function callTool(toolName, args) {
    if (toolName === "tasks.list") {
        let tasks = MetalOS.scheduler.list();
        return JSON.stringify(tasks);
    }

    if (toolName === "tasks.create") {
        let id = args.id;
        let scriptPath = args.script_path;
        let trigger = args.trigger;
        let source = args.source;
        let created = MetalOS.scheduler.create(id, scriptPath, trigger, source);
        return JSON.stringify(created);
    }

    if (toolName === "tasks.edit") {
        let id = args.id;
        let scriptPath = args.script_path || "";
        let trigger = args.trigger || "";
        let source = args.source || "";
        let updated = MetalOS.scheduler.edit(id, scriptPath, trigger, source);
        return JSON.stringify(updated);
    }

    if (toolName === "tasks.delete") {
        let id = args.id;
        let deleted = MetalOS.scheduler.delete(id);
        return JSON.stringify(deleted);
    }

    return JSON.stringify({ error: "Unknown tool: " + toolName });
}

function readResource(uri) {
    if (uri === "mcp://tasks/scheduled") {
        let tasks = MetalOS.scheduler.list();
        return JSON.stringify(tasks);
    }
    return JSON.stringify({ error: "Unknown resource URI: " + uri });
}

function renderTile(tileId) {
    if (tileId === "tasks.board") {
        let tasks = MetalOS.scheduler.list();
        let items = [];
        for (let i = 0; i < tasks.length; i = i + 1) {
            let t = tasks[i];
            items.push({
                id: t.id,
                label: t.id + " (" + t.script_path + ")",
                value: t.trigger + " · runs: " + t.run_count,
                status: t.last_result || "pending"
            });
        }
        return JSON.stringify({
            tile_id: "tasks.board",
            app_id: "tasks",
            title: "Scheduled Tasks",
            subtitle: tasks.length + " active task(s)",
            placements: ["ChatStream", "Dashboard"],
            items: items,
            actions: [
                {
                    id: "refresh",
                    label: "Refresh Tasks",
                    tool_name: "tasks.list",
                    arguments_json: "{}"
                }
            ]
        });
    }
    return JSON.stringify({ error: "Unknown tile ID: " + tileId });
}
