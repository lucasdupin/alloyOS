//! Discovery compatibility checks; endpoint metadata never proves connectivity.
use alloc::{string::String, vec::Vec};
use serde_json::Value;

/// Web-hosted HTTP(S), excluding local/private addresses. Reuse manifest URL
/// validation before applying the narrower hosted-server product boundary.
pub(crate) fn hosted_endpoint(url: &str) -> bool {
    if url.len() > 8192 || url.contains(['{', '}', '<', '>']) {
        return false;
    }
    let manifest = alloyos_mcp::AppManifest::new_remote(
        "candidate",
        "Candidate",
        "1",
        alloc::vec![
            alloyos_mcp::AppCapability::ExposeMcpTools,
            alloyos_mcp::AppCapability::ScopedNetwork,
        ],
        url,
        Some("streamable-http".into()),
    );
    if alloyos_mcp::validate_app_manifest(&manifest).is_err() {
        return false;
    }
    let Some((_, rest)) = url.split_once("://") else {
        return false;
    };
    let authority = rest.split(['/', '?']).next().unwrap_or("");
    if authority.starts_with('[') {
        let Some(end) = authority.find(']') else {
            return false;
        };
        let Ok(ip) = authority[1..end].parse::<core::net::Ipv6Addr>() else {
            return false;
        };
        // Global-unicast IPv6 only; excludes loopback, mapped IPv4, local/link local.
        return ip.segments()[0] & 0xe000 == 0x2000;
    }
    let host = authority
        .split(':')
        .next()
        .unwrap_or("")
        .trim_end_matches('.');
    if let Ok(ip) = host.parse::<core::net::Ipv4Addr>() {
        let [a, b, _, _] = ip.octets();
        return !(matches!(a, 0 | 10 | 127)
            || a >= 224
            || (a == 169 && b == 254)
            || (a == 172 && (16..=31).contains(&b))
            || (a == 192 && b == 168)
            || (a == 100 && (64..=127).contains(&b))
            || (a == 198 && matches!(b, 18 | 19)));
    }
    let lower = host.to_ascii_lowercase();
    if host.len() > 253
        || !host.contains('.')
        || host.bytes().all(|b| b.is_ascii_digit() || b == b'.')
        || [
            "localhost",
            "local",
            "localdomain",
            "internal",
            "lan",
            "home",
        ]
        .iter()
        .any(|suffix| lower == *suffix || lower.ends_with(&alloc::format!(".{suffix}")))
    {
        return false;
    }
    host.split('.').all(|label| {
        !label.is_empty()
            && label.len() <= 63
            && !label.starts_with('-')
            && !label.ends_with('-')
            && label
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
    })
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
            "http://public.example:8080/mcp",
            "https://8.8.8.8/mcp",
            "https://[2606:4700:4700::1111]/mcp",
        ] {
            assert!(hosted_endpoint(url), "{url}");
        }
        for url in [
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
