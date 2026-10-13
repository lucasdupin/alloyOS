// Package-owned scheduling behavior, executed in a capability-scoped isolate.
function storedTasks() {
    let raw = AlloyOS.fs.read("/var/tasks/state.json");
    if (!raw) { return []; }
    return JSON.parse(raw);
}

function allTasks(includeDisabled) {
    let live = AlloyOS.scheduler.list();
    let saved = storedTasks();
    let result = [];
    for (let i = 0; i < live.length; i = i + 1) {
        let task = live[i];
        for (let j = 0; j < saved.length; j = j + 1) {
            if (saved[j].id === task.id) { task.title = saved[j].title; }
        }
        result.push(task);
    }
    if (includeDisabled) {
        for (let i = 0; i < saved.length; i = i + 1) {
            if (!saved[i].enabled) { result.push(saved[i]); }
        }
    }
    return result;
}

function saveTask(task) {
    let saved = storedTasks();
    let result = [];
    for (let i = 0; i < saved.length; i = i + 1) {
        if (saved[i].id !== task.id) { result.push(saved[i]); }
    }
    result.push(task);
    AlloyOS.fs.write("/var/tasks/state.json", JSON.stringify(result));
}

function callTool(toolName, args) {
    if (toolName === "tasks.list") {
        let tasks = allTasks(args.include_disabled !== false);
        return JSON.stringify({ count: tasks.length, tasks: tasks });
    }
    if (toolName === "tasks.create" || toolName === "tasks.edit") {
        let creating = toolName === "tasks.create";
        let path = args.script_path || args.path || "";
        let id = args.id || path;
        let previous = null;
        let tasks = allTasks(true);
        for (let i = 0; i < tasks.length; i = i + 1) {
            if (tasks[i].id === id) { previous = tasks[i]; }
        }
        if (!creating && !previous) { return JSON.stringify({ error: "unknown task: " + id }); }
        if (previous && !path) { path = previous.script_path; }
        let trigger = args.trigger || (previous && previous.trigger) || "";
        let title = args.title || (previous && previous.title) || id;
        let enabled = true;
        if (previous) { enabled = previous.enabled; }
        if (typeof args.enabled === "boolean") { enabled = args.enabled; }
        let source = args.source || (previous && previous.source) || "";
        if (!id || !path.startsWith("/scripts/") || !path.endsWith(".js") || path.includes("..") || !trigger) {
            return JSON.stringify({ error: "unknown or invalid task arguments" });
        }
        if (enabled) { AlloyOS.scheduler.create(id, path, trigger, source); }
        else { AlloyOS.scheduler.delete(id); }
        let status = "active";
        if (!enabled) { status = "paused"; }
        let task = { id: id, title: title, script_path: path, trigger: trigger, enabled: enabled, status: status, source: source };
        saveTask(task);
        let operation = "edited";
        if (creating) { operation = "created"; }
        return JSON.stringify({ status: operation, task: task });
    }
    if (toolName === "tasks.delete") {
        let deleted = AlloyOS.scheduler.delete(args.id);
        let saved = storedTasks();
        let removed = deleted.deleted;
        let result = [];
        for (let i = 0; i < saved.length; i = i + 1) {
            if (saved[i].id === args.id) { removed = true; }
            else { result.push(saved[i]); }
        }
        AlloyOS.fs.write("/var/tasks/state.json", JSON.stringify(result));
        return JSON.stringify({ status: "deleted", id: args.id, deleted: removed });
    }
    return JSON.stringify({ error: "Unknown tool: " + toolName });
}

function readResource(uri) {
    if (uri === "mcp://tasks/schedules") {
        let tasks = allTasks(true);
        return JSON.stringify({ count: tasks.length, tasks: tasks });
    }
    if (uri === "mcp://tasks/tiles") { return renderTile("tasks.board"); }
    if (uri === "mcp://tasks/skill") { return AlloyOS.fs.read("/apps/tasks/SKILL.md"); }
    return JSON.stringify({ error: "Unknown resource URI: " + uri });
}

function renderTile(tileId) {
    if (tileId === "tasks.board") {
        let tasks = allTasks(true);
        let items = [];
        for (let i = 0; i < tasks.length; i = i + 1) {
            let task = tasks[i];
            items.push({ id: task.id, title: task.title, subtitle: task.script_path, badge: task.trigger, status: task.status });
        }
        return JSON.stringify({ tile_id: "tasks.board", title: "Tasks", subtitle: tasks.length + " task(s)", refresh_resource_uri: "mcp://tasks/schedules", items: items,
            actions: [{ id: "refresh", label: "Refresh Tasks", tool_name: "tasks.list", default_args: {} }] });
    }
    return JSON.stringify({ error: "Unknown tile ID: " + tileId });
}

function getPrompt(name, args) {
    if (name !== "tasks.manage") { return "Unknown Tasks prompt"; }
    return "Use tasks.list to inspect schedules, then tasks.create, tasks.edit, or tasks.delete for the requested change. Present the tasks.board Tile.";
}
