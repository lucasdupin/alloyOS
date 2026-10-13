// alloyOS App: Package Manager (/apps/sample-inventory/main.js)
// Executed inside a capability-scoped alloyos-js isolate by alloyos-mcp.

function getPackagesInventory() {
    let raw = AlloyOS.fs.read("/proc/app-packages.json");
    if (!raw) {
        return [];
    }
    let data = JSON.parse(raw);
    return data.packages || [];
}

function callTool(toolName, args) {
    let pkgs = getPackagesInventory();

    if (toolName === "sample_inventory.list") {
        let result = [];
        for (let i = 0; i < pkgs.length; i = i + 1) {
            let p = pkgs[i];
            result.push({
                app_id: p.app_id,
                version: p.version || "unknown",
                ready: p.ready,
                revision: p.revision,
                validated_revision: p.validated_revision,
                errors: (p.diagnostics || []).length
            });
        }
        return JSON.stringify(result);
    }

    if (toolName === "sample_inventory.status") {
        let targetId = (args && args.app_id) || "";
        for (let i = 0; i < pkgs.length; i = i + 1) {
            let p = pkgs[i];
            if (p.app_id === targetId) {
                return JSON.stringify(p);
            }
        }
        return JSON.stringify({ error: "App not found: " + targetId });
    }

    if (toolName === "sample_inventory.diagnostics") {
        let errored = [];
        for (let i = 0; i < pkgs.length; i = i + 1) {
            let p = pkgs[i];
            if (!p.ready || (p.diagnostics && p.diagnostics.length > 0)) {
                errored.push(p);
            }
        }
        return JSON.stringify(errored);
    }

    return JSON.stringify({ error: "Unknown tool: " + toolName });
}

function readResource(uri) {
    let pkgs = getPackagesInventory();

    if (uri === "packages://inventory") {
        return JSON.stringify(pkgs);
    }

    if (uri.startsWith("packages://status/")) {
        let targetId = uri.substring("packages://status/".length);
        for (let i = 0; i < pkgs.length; i = i + 1) {
            let p = pkgs[i];
            if (p.app_id === targetId) {
                return JSON.stringify(p);
            }
        }
        return JSON.stringify({ error: "App not found: " + targetId });
    }

    return JSON.stringify({ error: "Unknown resource URI: " + uri });
}

function renderTile(tileId) {
    if (tileId === "sample_inventory.status") {
        let pkgs = getPackagesInventory();
        let items = [];
        let readyCount = 0;

        for (let i = 0; i < pkgs.length; i = i + 1) {
            let p = pkgs[i];
            let errorCount = (p.diagnostics || []).length;
            let statusStr = "invalid";
            if (p.ready) {
                statusStr = "ready";
                readyCount = readyCount + 1;
            }
            let errStatus = "OK";
            if (errorCount > 0) {
                errStatus = errorCount + " error(s)";
            }
            let ver = p.version || "unknown";
            let valRev = p.validated_revision || "none";
            items.push({
                id: p.app_id,
                title: p.app_id + " (v" + ver + ")",
                subtitle: "rev " + p.revision + " (val " + valRev + ")",
                badge: statusStr,
                status: errStatus
            });
        }

        return JSON.stringify({
            tile_id: "sample_inventory.status",
            title: "Installed Packages",
            subtitle: pkgs.length + " app(s) installed · " + readyCount + " ready",
            refresh_resource_uri: "packages://inventory",
            items: items,
            actions: [
                {
                    id: "refresh",
                    label: "Refresh Inventory",
                    tool_name: "sample_inventory.list",
                    default_args: {}
                }
            ]
        });
    }

    return JSON.stringify({ error: "Unknown tile ID: " + tileId });
}
