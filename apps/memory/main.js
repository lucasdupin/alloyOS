// alloyOS App: Memory (/apps/memory/main.js)
// Executed inside a capability-scoped alloyos-js isolate by alloyos-mcp.

const STORAGE_PATH = "/var/memory/items.json";

function getMemoryItems() {
    let raw = AlloyOS.fs.read(STORAGE_PATH);
    if (!raw) {
        return [];
    }
    let data = JSON.parse(raw);
    if (data && data.items) {
        return data.items;
    }
    return [];
}

function saveMemoryItems(items) {
    let payload = JSON.stringify({ items: items });
    AlloyOS.fs.write(STORAGE_PATH, payload);
}

function callTool(toolName, args) {
    let items = getMemoryItems();

    if (toolName === "memory.remember") {
        let content = (args && args.content) || "";
        let category = (args && args.category) || "fact";
        let source = (args && args.source) || "user";

        let newId = "mem_" + (items.length + 1);
        let record = {
            id: newId,
            content: content,
            category: category,
            source: source
        };
        items.push(record);
        saveMemoryItems(items);

        return JSON.stringify({
            status: "recorded",
            item: record
        });
    }

    if (toolName === "memory.list") {
        let categoryFilter = (args && args.category) || "";
        if (!categoryFilter) {
            return JSON.stringify(items);
        }

        let filtered = [];
        for (let i = 0; i < items.length; i = i + 1) {
            let item = items[i];
            if (item.category === categoryFilter) {
                filtered.push(item);
            }
        }
        return JSON.stringify(filtered);
    }

    if (toolName === "memory.search") {
        let q = ((args && args.query) || "").toLowerCase();
        let matches = [];
        for (let i = 0; i < items.length; i = i + 1) {
            let item = items[i];
            let text = (item.content || "").toLowerCase();
            if (text.includes(q)) {
                matches.push(item);
            }
        }
        return JSON.stringify(matches);
    }

    if (toolName === "memory.forget") {
        let targetId = (args && args.id) || "";
        let remaining = [];
        let deleted = false;
        for (let i = 0; i < items.length; i = i + 1) {
            let item = items[i];
            if (item.id === targetId) {
                deleted = true;
            } else {
                remaining.push(item);
            }
        }
        if (deleted) {
            saveMemoryItems(remaining);
            return JSON.stringify({ status: "deleted", id: targetId });
        }
        return JSON.stringify({ error: "Memory item not found: " + targetId });
    }

    return JSON.stringify({ error: "Unknown tool: " + toolName });
}

function readResource(uri) {
    if (uri === "mcp://memory/items") {
        let items = getMemoryItems();
        return JSON.stringify(items);
    }
    return JSON.stringify({ error: "Unknown resource URI: " + uri });
}

function renderTile(tileId) {
    if (tileId === "memory.profile") {
        let items = getMemoryItems();
        let tileItems = [];

        if (items.length === 0) {
            tileItems.push({
                id: "empty",
                title: "No Memorized Facts",
                subtitle: "Tell Welder what you want remembered",
                badge: "Empty",
                status: "idle"
            });
        } else {
            let start = 0;
            if (items.length > 5) {
                start = items.length - 5;
            }
            for (let i = start; i < items.length; i = i + 1) {
                let item = items[i];
                tileItems.push({
                    id: item.id,
                    title: item.content,
                    subtitle: "Source: " + (item.source || "user"),
                    badge: item.category || "fact",
                    status: "saved"
                });
            }
        }

        return JSON.stringify({
            tile_id: "memory.profile",
            title: "Memory & Context",
            subtitle: items.length + " item(s) in persistent memory",
            refresh_resource_uri: "mcp://memory/items",
            items: tileItems,
            actions: [
                {
                    id: "refresh",
                    label: "Refresh Memory",
                    tool_name: "memory.list",
                    default_args: {}
                }
            ]
        });
    }

    return JSON.stringify({ error: "Unknown tile ID: " + tileId });
}

function getPrompt(name, args) {
    if (name === "memory.curate") {
        let cat = (args && args.category) || "all";
        return "Review and curate user knowledge records for category: " + cat;
    }
    return "Memory prompt: " + name;
}
