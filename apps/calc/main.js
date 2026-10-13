// alloyOS App: Calculator (/apps/calc/main.js)
// Executed inside a capability-scoped alloyos-js isolate by alloyos-mcp.

const HISTORY_PATH = "/var/calc/history.json";

function getHistory() {
    let raw = AlloyOS.fs.read(HISTORY_PATH);
    if (!raw) {
        return [];
    }
    let data = JSON.parse(raw);
    if (data && data.items) {
        return data.items;
    }
    return [];
}

function saveHistory(items) {
    let payload = JSON.stringify({ items: items });
    AlloyOS.fs.write(HISTORY_PATH, payload);
}

function appendHistory(expr, result) {
    let history = getHistory();
    history.push({
        id: "calc_" + (history.length + 1),
        expression: expr,
        result: result
    });
    if (history.length > 20) {
        history = history.slice(history.length - 20, history.length);
    }
    saveHistory(history);
}

function power(base, exp) {
    if (exp === 0) {
        return 1;
    }
    let res = 1;
    let count = 0;
    while (count < exp) {
        res = res * base;
        count = count + 1;
    }
    return res;
}

function evaluateBinary(a, op, b) {
    if (op === "+") {
        return a + b;
    }
    if (op === "-") {
        return a - b;
    }
    if (op === "*") {
        return a * b;
    }
    if (op === "/") {
        if (b === 0) {
            return 0;
        }
        return a / b;
    }
    if (op === "%") {
        if (b === 0) {
            return 0;
        }
        return a % b;
    }
    if (op === "^") {
        return power(a, b);
    }
    return a;
}

function evaluateExpression(exprStr) {
    let cleaned = exprStr.trim();
    let parts = cleaned.split(" ");
    let tokens = [];
    for (let i = 0; i < parts.length; i = i + 1) {
        let p = parts[i].trim();
        if (p.length > 0) {
            tokens.push(p);
        }
    }

    // If spaces were not used (e.g. "15+27"), split by operator
    if (tokens.length === 1) {
        for (let j = 0; j < cleaned.length; j = j + 1) {
            let c = cleaned.substring(j, j + 1);
            if (c === "+" || c === "-" || c === "*" || c === "/" || c === "%" || c === "^") {
                let leftStr = cleaned.substring(0, j).trim();
                let rightStr = cleaned.substring(j + 1, cleaned.length).trim();
                let a = parseInt(leftStr);
                let b = parseInt(rightStr);
                return evaluateBinary(a, c, b);
            }
        }
        return parseInt(tokens[0]);
    }

    // Pass 1: Exponentiation (^)
    let step1 = [];
    let i = 0;
    while (i < tokens.length) {
        if (tokens[i] === "^") {
            let prev = step1.pop();
            let prevNum = parseInt(prev);
            let nextNum = parseInt(tokens[i + 1]);
            let res = power(prevNum, nextNum);
            step1.push("" + res);
            i = i + 2;
        } else {
            step1.push(tokens[i]);
            i = i + 1;
        }
    }

    // Pass 2: Multiplication, division, modulo (*, /, %)
    let step2 = [];
    i = 0;
    while (i < step1.length) {
        let op = step1[i];
        if (op === "*" || op === "/" || op === "%") {
            let prev = step2.pop();
            let prevNum = parseInt(prev);
            let nextNum = parseInt(step1[i + 1]);
            let res = evaluateBinary(prevNum, op, nextNum);
            step2.push("" + res);
            i = i + 2;
        } else {
            step2.push(step1[i]);
            i = i + 1;
        }
    }

    // Pass 3: Addition and subtraction (+, -)
    if (step2.length === 0) {
        return 0;
    }
    let current = parseInt(step2[0]);
    i = 1;
    while (i + 1 < step2.length) {
        let op = step2[i];
        let nextVal = parseInt(step2[i + 1]);
        current = evaluateBinary(current, op, nextVal);
        i = i + 2;
    }
    return current;
}

function convertUnits(val, fromUnit, toUnit) {
    let f = fromUnit.toLowerCase();
    let t = toUnit.toLowerCase();

    // Length (base = meters)
    let lengthToM = {};
    lengthToM["m"] = 1;
    lengthToM["km"] = 1000;
    lengthToM["cm"] = 0; // integer representation
    lengthToM["mi"] = 1609;
    lengthToM["ft"] = 0;

    if (f === "km" && t === "m") {
        return val * 1000;
    }
    if (f === "m" && t === "km") {
        return val / 1000;
    }
    if (f === "mi" && t === "km") {
        return (val * 1609) / 1000;
    }
    if (f === "km" && t === "mi") {
        return (val * 1000) / 1609;
    }
    if (f === "m" && t === "ft") {
        return (val * 328) / 100;
    }
    if (f === "ft" && t === "m") {
        return (val * 100) / 328;
    }

    // Temperature (C <-> F)
    if (f === "c" && t === "f") {
        return (val * 9) / 5 + 32;
    }
    if (f === "f" && t === "c") {
        return ((val - 32) * 5) / 9;
    }

    // Weight (base = grams)
    if (f === "kg" && t === "g") {
        return val * 1000;
    }
    if (f === "g" && t === "kg") {
        return val / 1000;
    }
    if (f === "kg" && t === "lb") {
        return (val * 220) / 100;
    }
    if (f === "lb" && t === "kg") {
        return (val * 100) / 220;
    }

    // Data (bytes, kb, mb, gb)
    if (f === "kb" && t === "b") {
        return val * 1024;
    }
    if (f === "mb" && t === "kb") {
        return val * 1024;
    }
    if (f === "gb" && t === "mb") {
        return val * 1024;
    }
    if (f === "mb" && t === "b") {
        return val * 1024 * 1024;
    }
    if (f === "b" && t === "kb") {
        return val / 1024;
    }
    if (f === "kb" && t === "mb") {
        return val / 1024;
    }
    if (f === "mb" && t === "gb") {
        return val / 1024;
    }

    return val;
}

function callTool(toolName, args) {
    if (toolName === "calc.evaluate") {
        let expr = (args && args.expression) || "0";
        let res = evaluateExpression(expr);
        appendHistory(expr, res);
        return JSON.stringify({
            expression: expr,
            result: res
        });
    }

    if (toolName === "calc.convert") {
        let val = (args && args.value) || 0;
        let fromU = (args && args.from) || "";
        let toU = (args && args.to) || "";
        let converted = convertUnits(val, fromU, toU);
        let summary = val + " " + fromU + " = " + converted + " " + toU;
        appendHistory(summary, converted);
        return JSON.stringify({
            value: val,
            from: fromU,
            to: toU,
            result: converted
        });
    }

    if (toolName === "calc.clear") {
        saveHistory([]);
        return JSON.stringify({ cleared: true });
    }

    return JSON.stringify({ error: "Unknown tool: " + toolName });
}

function readResource(uri) {
    if (uri === "mcp://calc/history") {
        let history = getHistory();
        return JSON.stringify(history);
    }

    if (uri === "mcp://calc/constants") {
        return JSON.stringify({
            pi: 3,
            e: 2,
            speed_of_light_m_s: 299792458,
            gravity_m_s2: 9
        });
    }

    return JSON.stringify({ error: "Unknown resource URI: " + uri });
}

function renderTile(tileId) {
    if (tileId === "calc.scratchpad") {
        let history = getHistory();
        let items = [];

        if (history.length === 0) {
            items.push({
                id: "empty",
                title: "No Calculations",
                subtitle: "History is empty",
                badge: "Ready",
                status: "idle"
            });
        } else {
            let start = 0;
            if (history.length > 5) {
                start = history.length - 5;
            }
            for (let i = start; i < history.length; i = i + 1) {
                let item = history[i];
                items.push({
                    id: item.id,
                    title: item.expression,
                    subtitle: "= " + item.result,
                    badge: "Result",
                    status: "done"
                });
            }
        }

        return JSON.stringify({
            tile_id: "calc.scratchpad",
            title: "Calculator Tape",
            subtitle: history.length + " calculation(s) recorded",
            refresh_resource_uri: "mcp://calc/history",
            items: items,
            actions: [
                {
                    id: "clear",
                    label: "Clear Tape",
                    tool_name: "calc.clear",
                    default_args: {}
                }
            ]
        });
    }

    return JSON.stringify({ error: "Unknown tile ID: " + tileId });
}

function getPrompt(name, args) {
    if (name === "calc.solve") {
        let prob = (args && args.problem) || "";
        return "Please solve this calculation deterministically using calc.evaluate: " + prob;
    }
    return "Calculator prompt: " + name;
}
