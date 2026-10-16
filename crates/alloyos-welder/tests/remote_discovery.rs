use alloyos_forge::provider::ToolCall;
use alloyos_welder::{AgentUiSynthesizer, NativeToolRegistry};
use serde_json::{json, Value};

fn execute(registry: &mut NativeToolRegistry, tool: &str, args: Value) -> Result<Value, String> {
    let output = registry
        .execute_tool(
            &ToolCall::new("fixture", tool, args.to_string()),
            &mut AgentUiSynthesizer::new(),
        )
        .map_err(|error| error.to_string())?;
    serde_json::from_str(&output.output_json).map_err(|error| error.to_string())
}

#[test]
fn official_discovery_filters_local_only_and_preserves_only_compatible_hosted_remotes() {
    let mut registry = NativeToolRegistry::new();
    registry.register_http_response("https://registry.modelcontextprotocol.io/v0.1/servers?search=gmail&limit=5", &json!({"servers":[
        {"server":{"name":"local-gmail","packages":[{"registryType":"npm","identifier":"gmail-mcp","transport":{"type":"stdio"}}]}},
        {"server":{"name":"legacy","remotes":[{"type":"sse","url":"https://legacy.example/sse"}]}},
        {"server":{"name":"localhost","remotes":[{"type":"streamable-http","url":"http://localhost:3000/mcp"}]}},
        {"server":{"name":"mixed","version":"1","packages":[{"registryType":"pypi","identifier":"gmail"}],"remotes":[
            {"type":"stdio","url":"https://not-http.example/mcp"},
            {"type":"streamable-http","url":"http://127.0.0.1:5000/mcp"},
            {"type":"streamable-http","url":"https://gmail.example/mcp"},
            {"type":"sse","url":"https://gmail.example/sse"}
        ]}},
        {"server":{"name":"remote","remotes":[{"type":"streamable-http","url":"http://public.example/mcp"}]}}
    ]}).to_string());
    let result = execute(
        &mut registry,
        "mcp_search_services",
        json!({"query":"gmail"}),
    )
    .unwrap();
    assert_eq!(result["count"], 2);
    assert_eq!(result["servers"][0]["name"], "mixed");
    assert_eq!(
        result["servers"][0]["remotes"],
        json!([{"type":"streamable-http","url":"https://gmail.example/mcp"}])
    );
    for server in result["servers"].as_array().unwrap() {
        assert!(server.get("packages").is_none());
        assert_eq!(server["connectivity_verified"], false);
        assert_eq!(server["connection_status"], "not_connected");
    }
}

#[test]
fn smithery_discovery_never_exposes_local_launch_metadata_or_unsupported_connections() {
    let mut registry = NativeToolRegistry::new();
    registry.register_http_response("https://api.smithery.ai/servers?q=gmail", &json!({"servers":[
        {"qualifiedName":"local/npm","remote":false,"command":"npx","args":["gmail"]},
        {"qualifiedName":"missing/endpoint","remote":true},
        {"qualifiedName":"private/server","remote":true,"deploymentUrl":"https://192.168.1.2/mcp"},
        {"qualifiedName":"legacy/sse","remote":true,"connections":[{"type":"sse","deploymentUrl":"https://legacy.example/sse"}]},
        {"qualifiedName":"mixed/gmail","remote":true,"displayName":"Gmail","command":"python","packages":["local"],"connections":[
            {"type":"stdio","command":"npx"},
            {"type":"http","deploymentUrl":"http://localhost/mcp"},
            {"type":"http","deploymentUrl":"https://gmail.example/mcp"}
        ]},
        {"qualifiedName":"hosted/mail","remote":true,"deploymentUrl":"https://mail.example/mcp"}
    ],"pagination":{"totalCount":6}}).to_string());
    let result = execute(&mut registry, "smithery_search", json!({"query":"gmail"})).unwrap();
    assert_eq!(result["count"], 2);
    assert_eq!(result["pagination"]["totalCount"], 6); // Registry pagination remains upstream metadata.
    let mixed = &result["servers"][0];
    assert_eq!(
        mixed["connections"],
        json!([{"type":"http","deploymentUrl":"https://gmail.example/mcp"}])
    );
    assert!(mixed.get("command").is_none() && mixed.get("packages").is_none());
    assert_eq!(mixed["connection_status"], "not_connected");
    assert_eq!(mixed["connectivity_verified"], false);
}

#[test]
fn local_or_incompatible_install_details_cannot_create_a_remote_package() {
    for details in [
        json!({"remote":false,"deploymentUrl":"https://gmail.example/mcp"}),
        json!({"remote":true,"deploymentUrl":"http://localhost:1234/mcp"}),
        json!({"remote":true,"deploymentUrl":"http://127.1/mcp"}),
        json!({"remote":true,"deploymentUrl":"https://server.local/mcp"}),
        json!({"remote":true,"deploymentUrl":"https://user:secret@gmail.example/mcp"}),
        json!({"remote":true,"connections":[{"type":"sse","deploymentUrl":"https://gmail.example/sse"}]}),
        json!({"remote":true,"transport":"stdio","deploymentUrl":"https://gmail.example/mcp"}),
    ] {
        let mut registry = NativeToolRegistry::new();
        let mut details = details;
        details["qualifiedName"] = json!("acme/gmail");
        details["displayName"] = json!("Gmail");
        details["tools"] = json!([]);
        registry.register_http_response(
            "https://api.smithery.ai/servers/acme%2Fgmail",
            &details.to_string(),
        );
        assert!(
            execute(
                &mut registry,
                "smithery_install",
                json!({"server":"acme/gmail"})
            )
            .is_err(),
            "{details}"
        );
        assert!(registry.vfs_read("/apps/gmail/gmail.mcp").is_none());
    }
}

#[test]
fn install_uses_only_an_advertised_hosted_endpoint_without_claiming_connection() {
    let mut registry = NativeToolRegistry::new();
    registry.register_http_response("https://api.smithery.ai/servers/acme%2Fgmail", &json!({
        "qualifiedName":"acme/gmail","displayName":"Gmail","remote":true,"tools":[],"connections":[
            {"type":"stdio","command":"npx"},
            {"type":"http","deploymentUrl":"http://localhost/mcp"},
            {"type":"streamable-http","deploymentUrl":"https://gmail.example/mcp"}
        ]
    }).to_string());
    assert!(execute(
        &mut registry,
        "smithery_install",
        json!({"server":"acme/gmail","endpoint":"https://invented.example/mcp"})
    )
    .is_err());
    assert!(registry.vfs_read("/apps/gmail/gmail.mcp").is_none());
    let result = execute(
        &mut registry,
        "smithery_install",
        json!({"server":"acme/gmail","endpoint":"https://gmail.example/mcp"}),
    )
    .unwrap();
    assert_eq!(result["endpoint"], "https://gmail.example/mcp");
    assert_eq!(result["connectivity_verified"], false);
    assert_eq!(result["connection_status"], "not_connected");
    let manifest = registry.vfs_read("/apps/gmail/gmail.mcp").unwrap();
    assert!(manifest.contains("https://gmail.example/mcp"));
    assert!(!manifest.contains("command") && !manifest.contains("localhost"));
}
