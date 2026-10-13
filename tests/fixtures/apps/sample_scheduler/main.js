// Package-owned scheduling behavior, executed in a capability-scoped isolate.
function storedTasks() {
    let raw = AlloyOS.fs.read("/var/sample_scheduler/state.json");
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
    AlloyOS.fs.write("/var/sample_scheduler/state.json", JSON.stringify(result));
}

function callTool(toolName, args) {
    if (toolName === "sample_scheduler.list") {
        let sample_scheduler = allTasks(args.include_disabled !== false);
        return JSON.stringify({ count: sample_scheduler.length, sample_scheduler: sample_scheduler });
    }
    if (toolName === "sample_scheduler.create" || toolName === "sample_scheduler.edit") {
        let creating = toolName === "sample_scheduler.create";
        let path = args.script_path || args.path || "";
        let id = args.id || path;
        let previous = null;
        let sample_scheduler = allTasks(true);
        for (let i = 0; i < sample_scheduler.length; i = i + 1) {
            if (sample_scheduler[i].id === id) { previous = sample_scheduler[i]; }
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
    if (toolName === "sample_scheduler.delete") {
        let deleted = AlloyOS.scheduler.delete(args.id);
        let saved = storedTasks();
        let removed = deleted.deleted;
        let result = [];
        for (let i = 0; i < saved.length; i = i + 1) {
            if (saved[i].id === args.id) { removed = true; }
            else { result.push(saved[i]); }
        }
        AlloyOS.fs.write("/var/sample_scheduler/state.json", JSON.stringify(result));
        return JSON.stringify({ status: "deleted", id: args.id, deleted: removed });
    }
    return JSON.stringify({ error: "Unknown tool: " + toolName });
}

function readResource(uri) {
    if (uri === "mcp://sample_scheduler/schedules") {
        let sample_scheduler = allTasks(true);
        return JSON.stringify({ count: sample_scheduler.length, sample_scheduler: sample_scheduler });
    }
    if (uri === "mcp://sample_scheduler/tiles") { return renderTile("sample_scheduler.board"); }
    if (uri === "mcp://sample_scheduler/skill") { return AlloyOS.fs.read("/apps/sample_scheduler/SKILL.md"); }
    return JSON.stringify({ error: "Unknown resource URI: " + uri });
}

function renderTile(tileId) {
    if (tileId === "sample_scheduler.board") {
        let sample_scheduler = allTasks(true);
        let items = [];
        for (let i = 0; i < sample_scheduler.length; i = i + 1) {
            let task = sample_scheduler[i];
            items.push({ id: task.id, title: task.title, subtitle: task.script_path, badge: task.trigger, status: task.status });
        }
        return JSON.stringify({ tile_id: "sample_scheduler.board", title: "Sample Scheduler", subtitle: sample_scheduler.length + " task(s)", refresh_resource_uri: "mcp://sample_scheduler/schedules", items: items,
            actions: [{ id: "refresh", label: "Refresh Sample Scheduler", tool_name: "sample_scheduler.list", default_args: {} }] });
    }
    return JSON.stringify({ error: "Unknown tile ID: " + tileId });
}

function getPrompt(name, args) {
    if (name !== "sample_scheduler.manage") { return "Unknown Sample Scheduler prompt"; }
    return "Use sample_scheduler.list to inspect schedules, then sample_scheduler.create, sample_scheduler.edit, or sample_scheduler.delete for the requested change. Present the sample_scheduler.board Tile.";
}
