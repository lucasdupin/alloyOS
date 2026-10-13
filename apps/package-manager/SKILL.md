# Package Manager Skill

Use `package_manager` to inspect installed alloyOS applications, view validation health, check compiler/syntax diagnostics, and report package inventory to Welder.

## Tools
- `package_manager.list`: List all installed application packages, versions, and validation readiness.
- `package_manager.status`: Query specific package status and detailed diagnostics by `app_id`.
- `package_manager.diagnostics`: List all packages with errors or pending validation.

## Resources
- `packages://inventory`: Read the raw JSON system package inventory from `/proc/app-packages.json`.
- `packages://status/{app_id}`: Read package status for `{app_id}`.

## Tiles
- `package_manager.status`: Declarative AppTile summarizing installed packages and system health for Ferrofluid dashboard and chat streams.
