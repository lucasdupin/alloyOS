// Headless web search using Gemini Google Search Grounding.
const CACHE_PATH = "/var/search/latest.json";
const API_KEY = "__GEMINI_API_KEY__";
const GEMINI_URL = "https://generativelanguage.googleapis.com/v1beta/models/gemini-3.8-flash:generateContent";

function failure(status, message) {
    return {ok: false, status: status, message: message, presentation: "text", provider: "Google", results: []};
}

function parseGeminiResponse(raw, limit) {
    if (!raw) {
        return failure("network_error", "No response received from Gemini Search.");
    }
    let data = JSON.tryParse(raw);
    if (!data) {
        return failure("parse_error", "Failed to parse Gemini Search response.");
    }
    if (data.error) {
        let msg = data.error.message || "Gemini Search API error";
        return failure("api_error", msg);
    }
    let candidate = data.candidates && data.candidates[0];
    if (!candidate) {
        return {ok: true, status: "empty", provider: "Google", results: []};
    }

    let chunks = [];
    if (candidate.groundingMetadata && candidate.groundingMetadata.groundingChunks) {
        chunks = candidate.groundingMetadata.groundingChunks;
    }

    let results = [];
    let seen = [];
    for (let i = 0; i < chunks.length && results.length < limit; i = i + 1) {
        let chunk = chunks[i];
        if (chunk.web && chunk.web.uri) {
            let url = chunk.web.uri;
            let title = chunk.web.title || url;
            let snippet = chunk.web.snippet || "";
            if (!seen.includes(url)) {
                seen.push(url);
                results.push({title: title, url: url, snippet: snippet});
            }
        }
    }

    let status = "empty";
    if (results.length > 0) {
        status = "success";
    }
    let metadata = candidate.groundingMetadata || {};
    return {ok: true, status: status, provider: "Google", results: results,
        groundingSupports: metadata.groundingSupports || [],
        searchEntryPoint: metadata.searchEntryPoint || {},
        webSearchQueries: metadata.webSearchQueries || []};
}

function latest() {
    let raw = AlloyOS.fs.read(CACHE_PATH);
    if (!raw) { return failure("no_search", "No saved search. Ask Search to find something on the web."); }
    let data = JSON.tryParse(raw);
    if (!data) { return failure("cache_error", "Saved search is unreadable. Run a fresh search."); }
    data.cached = true;
    return data;
}

function callTool(name, args) {
    if (name === "search.latest") { return JSON.stringify(latest()); }
    if (name === "search.clear") {
        AlloyOS.fs.delete(CACHE_PATH);
        return JSON.stringify({ok: true, status: "cleared", presentation: "text"});
    }
    if (name !== "search.web") { return JSON.stringify({error: "Unknown tool: " + name}); }
    if (!args || typeof args.query !== "string" || !args.query.trim() || args.query.length > 500) {
        return JSON.stringify(failure("invalid_arguments", "query must contain 1 to 500 characters"));
    }
    let limit = 5;
    if (args.limit !== undefined) { limit = args.limit; }
    if (typeof limit !== "number" || limit < 1 || limit > 10 || limit % 1 !== 0) {
        return JSON.stringify(failure("invalid_arguments", "limit must be an integer from 1 to 10"));
    }
    let query = args.query.trim();

    if (!API_KEY || API_KEY === "__GEMINI_API_KEY__") {
        return JSON.stringify(failure("not_configured", "Build Search with GEMINI_API_KEY set, then rebuild the app image."));
    }
    let headers = [["Content-Type", "application/json"], ["x-goog-api-key", API_KEY]];

    let part = {text: "Search the web for the following query and provide sources: " + query};
    let parts = [part];
    let content = {parts: parts};
    let contents = [content];
    let tool = {google_search: {}};
    let tools = [tool];
    let payload = JSON.stringify({
        contents: contents,
        tools: tools
    });

    let rawResponse = AlloyOS.net.post(GEMINI_URL, payload, headers);
    let data = parseGeminiResponse(rawResponse, limit);
    data.query = query;
    data.presentation = "text";
    data.cached = false;
    if (data.ok) { AlloyOS.fs.write(CACHE_PATH, JSON.stringify(data)); }
    return JSON.stringify(data);
}

function readResource(uri) {
    if (uri === "mcp://search/latest") { return JSON.stringify(latest()); }
    return JSON.stringify({error: "Unknown resource URI: " + uri});
}
