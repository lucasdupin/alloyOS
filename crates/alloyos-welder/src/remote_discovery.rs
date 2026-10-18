//! Discovery compatibility checks; endpoint metadata never proves connectivity.
use alloc::{string::String, vec::Vec};
use serde_json::Value;

/// Match production remote MCP transport compatibility. DNS results still need
/// host validation/pinning; metadata does not prove connectivity or authorization.
pub(crate) fn hosted_endpoint(url: &str) -> bool {
    !url.contains(['<', '>']) && alloyos_mcp::transport::remote_mcp_authority(url).is_some()
}

pub(crate) fn official_remotes(server: &Value) -> Vec<Value> {
    server["remotes"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|remote| {
            let url = remote["url"].as_str()?;
            if remote["type"] != "streamable-http" || !hosted_endpoint(url) {
                return None;
            }
            Some(serde_json::json!({"type":"streamable-http", "url":url}))
        })
        .collect()
}

pub(crate) fn smithery_endpoints(server: &Value) -> Vec<String> {
    if server["remote"] != true {
        return Vec::new();
    }
    if server
        .get("transport")
        .and_then(Value::as_str)
        .is_some_and(|kind| !matches!(kind, "http" | "streamable-http"))
    {
        return Vec::new();
    }
    let mut endpoints = Vec::new();
    if let Some(connections) = server["connections"]
        .as_array()
        .filter(|connections| !connections.is_empty())
    {
        for connection in connections {
            if !matches!(
                connection["type"].as_str(),
                Some("http" | "streamable-http")
            ) {
                continue;
            }
            if let Some(url) = connection["deploymentUrl"]
                .as_str()
                .filter(|url| hosted_endpoint(url))
            {
                if !endpoints.iter().any(|endpoint| endpoint == url) {
                    endpoints.push(url.into());
                }
            }
        }
    } else if let Some(url) = server["deploymentUrl"]
        .as_str()
        .filter(|url| hosted_endpoint(url))
    {
        endpoints.push(url.into());
    }
    endpoints
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn hosted_url_policy_rejects_local_addresses_and_malformed_authorities() {
        for url in [
            "https://gmail.example/mcp",
            "https://public.example:8443/mcp",
        ] {
            assert!(hosted_endpoint(url), "{url}");
        }
        for url in [
            "http://public.example/mcp",
            "https://8.8.8.8/mcp",
            "https://[2606:4700:4700::1111]/mcp",
            "stdio://server",
            "file:///tmp/mcp",
            "https://localhost/mcp",
            "http://LOCALHOST.:3000/mcp",
            "https://service.localhost/mcp",
            "https://service.local/mcp",
            "https://host.internal/mcp",
            "http://127.0.0.1/mcp",
            "http://127.1/mcp",
            "http://2130706433/mcp",
            "http://0x7f000001/mcp",
            "http://10.1.2.3/mcp",
            "http://192.168.1.2/mcp",
            "http://172.16.0.1/mcp",
            "http://169.254.169.254/mcp",
            "http://100.64.0.1/mcp",
            "https://[::1]/mcp",
            "https://[::ffff:127.0.0.1]/mcp",
            "https://[fc00::1]/mcp",
            "https://[fe80::1]/mcp",
            "https://user:secret@public.example/mcp",
            "https://public.example/mcp#fragment",
            "https://public.example:0/mcp",
            "https://-bad.example/mcp",
        ] {
            assert!(!hosted_endpoint(url), "{url}");
        }
    }
}
