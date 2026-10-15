//! Generate a setup-controller preview from real Welder state transitions.
//! This intentionally does not claim to exercise LLM interpretation or guest boot.
use alloyos_mcp::display::SetupAnswer;
use alloyos_welder::{AgentRuntime, SharedMapVfsBackend, ToolVfsBackend, KnowledgeEntry, KnowledgeCategory, KnowledgeBasis};
use serde_json::json;

fn main() {
    let backend = SharedMapVfsBackend::shared();
    for (name, content) in [
        ("clock.mcp", include_str!("../../../apps/clock/clock.mcp")),
        ("main.js", include_str!("../../../apps/clock/main.js")),
        ("tools.json", include_str!("../../../apps/clock/tools.json")),
        ("resources.json", include_str!("../../../apps/clock/resources.json")),
        ("tiles.json", include_str!("../../../apps/clock/tiles.json")),
        ("SKILL.md", include_str!("../../../apps/clock/SKILL.md")),
    ] {
        backend.borrow_mut().write_file(&format!("/apps/clock/{name}"), content).unwrap();
    }
    if std::env::args().nth(3).is_some() {
        for name in ["weather.mcp","main.js","tools.json","resources.json","tiles.json","prompts.json","SKILL.md"] {
            let content=std::fs::read_to_string(format!("apps/weather/{name}")).unwrap();
            backend.borrow_mut().write_file(&format!("/apps/weather/{name}"), &content).unwrap();
        }
    }
    let mut agent = AgentRuntime::with_vfs_backend(backend);
    if let Some(weather_output)=std::env::args().nth(3) {
        let tile=agent.tool_registry.mcp_registry().render_tile("weather.card").unwrap();
        std::fs::write(weather_output,serde_json::to_string_pretty(&json!({
            "tile":tile,
            "provenance":"Actual bundled Weather app JavaScript rendered through MCP; initial cached San Francisco20C sample, not live forecast. No network request or LLM."
        })).unwrap()).unwrap();
    }
    let opening = agent.startup_greeting().unwrap();
    let mut items = vec![json!({"role":"assistant","text":opening})];
    let mut scenes = vec![json!({"label":"Welcome","items":items,"hold_ms":2200})];
    for (shown, normalized) in [
        ("Alex", "Alex"),
        ("Nova", "Nova"),
        ("Los Angeles, United States", "America/Los_Angeles"),
        ("12-hour", "12h"),
        ("Yes, learn my preferences", "learn"),
        ("Yes, looks good", "finish"),
    ] {
        let revision = agent.setup_view().expect("active setup").revision;
        let reply = agent.submit_setup_answer(&SetupAnswer { revision, value:normalized.into() }).unwrap();
        items.push(json!({"role":"user","text":shown}));
        items.push(json!({"role":"assistant","text":reply.chat_text()}));
        scenes.push(json!({"label":reply.title,"items":items,"hold_ms":2300}));
    }
    assert_eq!(agent.agent_name(), "Nova");
    assert!(!agent.setup_active());
    for (title,value) in [
        ("Communication style", "Keep answers concise and lead with the decision."),
        ("Travel pace", "Allow time for rest breaks and unhurried photo stops."),
        ("Visual style", "Use a calm monochrome interface with readable text."),
        ("Planning", "Show practical next steps and explain important tradeoffs."),
        ("Review preference", "Correct factual mistakes directly and objectively."),
    ] {
        agent.tool_registry.knowledge_upsert(KnowledgeEntry::new(KnowledgeCategory::Preferences,
            title,value,KnowledgeBasis::Explicit,"Capture demo","Demo","global"), None).unwrap();
    }
    let active_tile = agent.tool_registry.render_knowledge_tile();
    agent.tool_registry.set_knowledge_learning_paused(true);
    let paused_tile = agent.tool_registry.render_knowledge_tile();
    if let Some(memory_output) = std::env::args().nth(2) {
        std::fs::write(memory_output, serde_json::to_string_pretty(&json!({
            "provenance":"Actual Welder knowledge store and generated tiles; demo records; in-memory host runtime",
            "active":active_tile,"paused":paused_tile
        })).unwrap()).unwrap();
    }
    let output = std::env::args().nth(1).expect("output JSON path");
    std::fs::write(output, serde_json::to_string_pretty(&json!({
        "provenance":"Actual Welder setup controller and Clock app; normalized answers supplied by capture harness; no LLM or guest boot",
        "scenes":scenes
    })).unwrap()).unwrap();
}
