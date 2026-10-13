// alloyOS Canonical App: Tasks (/apps/tasks/main.js)
// Executed inside a capability-scoped alloyos-js isolate by alloyos-mcp.
// Tasks is the ONLY app granted ScriptSchedulerAccess (AlloyOS.scheduler.*).

function callTool(toolName, args) {
    if (toolName === "tasks.list") {
        let tasks = AlloyOS.scheduler.list();
        return JSON.stringify(tasks);
    }

    if (toolName === "tasks.create") {
        let id = args.id;
        let scriptPath = args.script_path;
        let trigger = args.trigger;
        let source = args.source;
        let created = AlloyOS.scheduler.create(id, scriptPath, trigger, source);
        return JSON.stringify(created);
    }

    if (toolName === "tasks.edit") {
        let id = args.id;
        let scriptPath = args.script_path || "";
        let trigger = args.trigger || "";
        let source = args.source || "";
        let updated = AlloyOS.scheduler.edit(id, scriptPath, trigger, source);
        return JSON.stringify(updated);
    }

    if (toolName === "tasks.delete") {
        let id = args.id;
        let deleted = AlloyOS.scheduler.delete(id);
        return JSON.stringify(deleted);
    }

    return JSON.stringify({ error: "Unknown tool: " + toolName });
}

function readResource(uri) {
    if (uri === "mcp://tasks/schedules") {
        let tasks = AlloyOS.scheduler.list();
        return JSON.stringify(tasks);
    }
    return JSON.stringify({ error: "Unknown resource URI: " + uri });
}

function renderTile(tileId) {
    if (tileId === "tasks.board") {
        let tasks = AlloyOS.scheduler.list();
        let items = [];
        for (let i = 0; i < tasks.length; i = i + 1) {
            let t = tasks[i];
            items.push({
                id: t.id,
                title: t.id,
                subtitle: t.script_path,
                badge: t.trigger + " · runs: " + t.run_count,
                status: t.last_result || "pending"
            });
        }
        return JSON.stringify({
            tile_id: "tasks.board",
            title: "Scheduled Tasks",
            subtitle: tasks.length + " active task(s)",
            refresh_resource_uri: "mcp://tasks/schedules",
            items: items,
            actions: [
                {
                    id: "refresh",
                    label: "Refresh Tasks",
                    tool_name: "tasks.list",
                    default_args: {}
                }
            ]
        });
    }
    return JSON.stringify({ error: "Unknown tile ID: " + tileId });
}

function getPrompt(name, args) {
    if (name !== "tasks.manage") {
        return "Unknown Tasks prompt";
    }
    return "Use tasks.list to inspect schedules, then tasks.create, tasks.edit, or tasks.delete for the requested change. Present the tasks.board Tile.";
}
