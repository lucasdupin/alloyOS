---
name: calc
description: Perform deterministic arithmetic evaluation, mathematical computations, unit conversions, and manage calculation history.
---

# Calculator App Skill (`/apps/calc/SKILL.md`)

**Calculator** (`/apps/calc/`) is the deterministic mathematical and unit conversion application in alloyOS.

## Architectural Rules
1. **Zero Math Hallucination**: Large language models frequently make arithmetic errors. Welder must ALWAYS use `calc.evaluate` for calculations and `calc.convert` for unit conversions rather than computing numbers mentally.
2. **Deterministic Tape**: All calculations are appended to persistent history under `/var/calc/history.json`, readable via `mcp://calc/history` and rendered on the `calc.scratchpad` Tile.
3. **Renderer-Agnostic Tile**: Exposes the `calc.scratchpad` AppTile with calculation history and interactive clear action.

## MCP Entry Points

### Tools
- `calc.evaluate`
  - **Description**: Evaluates an arithmetic expression string (`+`, `-`, `*`, `/`, `%`, `^`).
  - **Parameters**:
    - `expression` (`string`, required): e.g. `"120 * 8"`, `"2 ^ 10"`, `"500 / 4"`, `"17 % 5"`.
- `calc.convert`
  - **Description**: Converts quantities between metric, imperial, temperature, and data units.
  - **Parameters**:
    - `value` (`number`, required): Numeric magnitude.
    - `from` (`string`, required): Source unit (`m`, `km`, `cm`, `mm`, `mi`, `ft`, `in`, `c`, `f`, `kg`, `g`, `lb`, `oz`, `b`, `kb`, `mb`, `gb`, `tb`).
    - `to` (`string`, required): Destination unit.
- `calc.clear`
  - **Description**: Clears the calculation history tape.

### Resources
- `mcp://calc/history`: JSON array of evaluated expressions and results.
- `mcp://calc/constants`: Commonly used mathematical and physical constants.

### Prompts
- `calc.solve`: Guided prompt template for math problem solving.

## Tiles
- `calc.scratchpad`:
  - Shows the most recent calculation history tape rows with expressions and calculated results.
  - Includes a `"Clear Tape"` action button.
