# Package Manager Skill

Use `sample_inventory` to inspect installed alloyOS applications, view validation health, check compiler/syntax diagnostics, and report package inventory to Welder.

## Tools
- `sample_inventory.list`: List all installed application packages, versions, and validation readiness.
- `sample_inventory.status`: Query specific package status and detailed diagnostics by `app_id`.
- `sample_inventory.diagnostics`: List all packages with errors or pending validation.

## Resources
- `packages://inventory`: Read the raw JSON system package inventory from `/proc/app-packages.json`.
- `packages://status/{app_id}`: Read package status for `{app_id}`.

## Tiles
- `sample_inventory.status`: Declarative AppTile summarizing installed packages and system health for Ferrofluid dashboard and chat streams.
