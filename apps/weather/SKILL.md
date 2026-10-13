---
name: weather
description: Query real-time weather conditions, forecasts, and manage location preferences via headless network fetch.
---

# Weather App Skill (`/apps/weather/SKILL.md`)

**Weather** (`/apps/weather/`) provides meteorological context and forecasts in alloyOS.

## Architectural Rules
1. **Headless Data Retrieval**: Retrieves forecasts using `AlloyOS.net.fetch` and caches results under `/var/weather/cache.json`.
2. **Offline-Resilient**: If the guest is offline or network fetch fails, graceful fallback conditions and cached observations ensure responses never hard-fail.
3. **Declarative Tile**: Exposes the `weather.card` Tile on the Bento Dashboard with temperature, condition badge, high/low range, and a refresh action.

## MCP Entry Points

### Tools
- `weather.current`
  - **Description**: Returns current temperature, conditions, humidity, and wind.
  - **Parameters**:
    - `city` (`string`, optional): Target city (defaults to configured city).
    - `units` (`string`, optional, default `"celsius"`): `"celsius"` or `"fahrenheit"`.
- `weather.forecast`
  - **Description**: Returns multi-day weather outlook.
  - **Parameters**:
    - `city` (`string`, optional): Target city.
    - `days` (`number`, optional, default `3`): Forecast horizon.
- `weather.set_location`
  - **Description**: Sets the default city for weather queries.
  - **Parameters**:
    - `city` (`string`, required): Default city.

### Resources
- `mcp://weather/current`: Live JSON snapshot of current weather conditions.
- `mcp://weather/config`: Location configuration and default units.

### Prompts
- `weather.briefing`: Guided prompt for delivering daily weather outlooks.

## Tiles
- `weather.card`:
  - Bento Dashboard card with current temperature, condition badge, wind/humidity items, and refresh action.
