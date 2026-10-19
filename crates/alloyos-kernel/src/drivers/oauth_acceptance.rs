//! Opt-in hermetic guest integration gate. Synthetic account only; host replies
//! are fixtures. Production OS core, crypto, private store and UART ports run.
use crate::fs::accounts::KernelCredentialStore;
use alloc::{boxed::Box, string::String, vec};
use alloyos_mcp::account::{
    AccountOperation, AccountOutcome, AccountRequest, AccountResponse, CredentialReference,
    SecretStorage,
};
use alloyos_welder::{
    accounts::AccountBroker,
    oauth::{ClientProfile, OsAccountBroker},
};
use core::task::Poll;
const APP: &str = "oauthfixture";
const ENDPOINT: &str = "https://resource.example.com/mcp";
const ISSUER: &str = "https://issuer.example.com";
fn execute(
    broker: &mut OsAccountBroker,
    operation: AccountOperation,
) -> Result<AccountResponse, String> {
    let request = AccountRequest {
        request_id: 1,
        app_id: APP.into(),
        endpoint: ENDPOINT.into(),
        operation,
    };
    let deadline = super::rng::monotonic_time_ns() + 30_000_000_000;
    let mut response = broker.start(&request);
    loop {
        match response {
            Poll::Ready(value) => return value.map_err(|_| "account operation rejected".into()),
            Poll::Pending => {
                if super::rng::monotonic_time_ns() > deadline {
                    return Err("account operation timed out".into());
                }
                response = broker.poll();
                core::hint::spin_loop();
            }
        }
    }
}
fn credential(response: AccountResponse) -> Result<CredentialReference, String> {
    match response.outcome {
        AccountOutcome::Authorized {
            credential,
            storage: SecretStorage::OsPrivateStore,
            ..
        } => Ok(credential),
        _ => Err("authorization not durably stored".into()),
    }
}
fn resource_probe(bearer: String) -> Result<(), String> {
    use alloc::{collections::BTreeMap, rc::Rc};
    use alloyos_mcp::{AppCapability, AppManifest, McpApp, RemoteMcpApp};
    use core::cell::RefCell;
    let mut client =
        alloyos_mcp::transport::HttpClient::new(Rc::new(RefCell::new(BTreeMap::new())));
    client.set_transport(Box::new(super::mcp_https::HostRegistryHttp::default()));
    let mut app = RemoteMcpApp::new(
        AppManifest::new_remote(
            APP,
            "OAuth fixture",
            "1",
            vec![AppCapability::ScopedNetwork, AppCapability::ExposeMcpTools],
            ENDPOINT,
            Some("streamable-http".into()),
        ),
        vec![],
        vec![],
        vec![],
        vec![],
    )
    .map_err(|_| "fixture manifest invalid")?
    .with_http_client(Rc::new(RefCell::new(client)));
    app.set_trusted_bearer(Some(bearer))
        .map_err(|_| "trusted attachment failed")?;
    let deadline = super::rng::monotonic_time_ns() + 30_000_000_000;
    loop {
        if super::rng::monotonic_time_ns() > deadline {
            return Err("resource probe timeout".into());
        }
        match app.poll_call_tool("oauthfixture.read", &BTreeMap::new()) {
            Poll::Pending => core::hint::spin_loop(),
            Poll::Ready(Ok(_)) => break,
            Poll::Ready(Err(_)) => return Err("authenticated resource probe failed".into()),
        }
    }
    app.set_trusted_bearer(None)
        .map_err(|_| "credential detach failed")?;
    Ok(())
}
pub fn run() {
    let result = (|| -> Result<(), String> {
        let mode = super::fw_cfg::read_file("opt/alloyos/oauth-probe", 64)
            .ok_or("missing fixture mode")?;
        let mode = core::str::from_utf8(&mode)
            .map_err(|_| "invalid fixture mode")?
            .trim();
        let mut store = KernelCredentialStore::global();
        if mode == "connect" {
            store
                .save_profile(APP, &ClientProfile {
                    endpoint: ENDPOINT.into(),
                    issuer: ISSUER.into(),
                    client_id: "fixture-client".into(),
                    allowed_scopes: vec!["read".into()],
                    loopback_port: 39173,
                    client_secret: None,
                })
                .map_err(|_| "fixture profile save failed")?;
        }
        let mut broker = OsAccountBroker::new(
            Box::new(store),
            Box::new(super::mcp_accounts::HostBrowserAssistant::default()),
            Box::new(super::mcp_https::OAuthHttp::default()),
            Box::new(super::mcp_accounts::KernelOAuthCrypto),
        );
        if mode == "revoked" {
            if execute(&mut broker, AccountOperation::Status).is_ok() {
                return Err("disconnected account revived after reboot".into());
            }
            crate::serial_println!("OAUTH-GUEST: reboot preserves local revocation PASS");
            return Ok(());
        }
        let reference = if mode == "connect" {
            let result = execute(
                &mut broker,
                AccountOperation::Begin {
                    scopes: vec!["read".into()],
                },
            )?;
            let attempt = match result.outcome {
                AccountOutcome::Authorizing { attempt } => attempt,
                _ => return Err("fixture browser not authorizing".into()),
            };
            credential(execute(&mut broker, AccountOperation::Poll { attempt })?)?
        } else if mode == "restore" {
            credential(execute(&mut broker, AccountOperation::Status)?)?
        } else {
            return Err("invalid fixture mode".into());
        };
        let bearer = broker
            .bearer(APP, ENDPOINT, &reference)
            .map_err(|_| "bound credential resolution failed")?;
        if bearer != "fixture-access" {
            return Err("unexpected synthetic credential".into());
        }
        if broker.bearer("other", ENDPOINT, &reference).is_ok()
            || broker
                .bearer(APP, "https://other.example.com/mcp", &reference)
                .is_ok()
        {
            return Err("credential escaped resource binding".into());
        }
        resource_probe(bearer)?;
        crate::serial_println!(
            "OAUTH-GUEST: private credential binding and authenticated MCP verified PASS"
        );
        if mode == "restore" {
            let refreshed = credential(execute(
                &mut broker,
                AccountOperation::Refresh {
                    credential: reference,
                },
            )?)?;
            if broker
                .bearer(APP, ENDPOINT, &refreshed)
                .map_err(|_| "refresh resolution failed")?
                != "fixture-refreshed"
            {
                return Err("refresh failed".into());
            }
            let result = execute(
                &mut broker,
                AccountOperation::Disconnect {
                    credential: Some(refreshed),
                },
            )?;
            if !matches!(
                result.outcome,
                AccountOutcome::Disconnected {
                    remote_revoked: false
                }
            ) {
                return Err("provider failure falsely claimed revoked".into());
            }
            if execute(&mut broker, AccountOperation::Status).is_ok() {
                return Err("local authority survived disconnect".into());
            }
            crate::serial_println!(
                "OAUTH-GUEST: refresh and failed-provider-revoke local disconnect PASS"
            );
        }
        Ok(())
    })();
    match result {
        Ok(()) => {
            crate::serial_println!("OAUTH-GUEST: integration PASS");
        }
        Err(error) => {
            crate::serial_println!("OAUTH-GUEST: FAIL {}", error);
        }
    }
}
