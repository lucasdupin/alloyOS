use metalos_ferrofluid::{AgentUiToken, CallerPrincipal, FerrofluidError, SecurityGate};

#[test]
fn system_ui_gate_rejects_apps_and_invalid_agent_credentials() {
    let token = AgentUiToken::new(7, 12345);
    let gate = SecurityGate::new(token);
    let agent = token.principal();
    assert_eq!(gate.authorize(&agent, Some(&token)), Ok(()));
    assert_eq!(
        gate.authorize(&agent, None),
        Err(FerrofluidError::InvalidAgentToken)
    );
    let wrong_secret = AgentUiToken::new(token.token_id, token.capability_secret + 1);
    assert_eq!(
        gate.authorize(&agent, Some(&wrong_secret)),
        Err(FerrofluidError::InvalidAgentToken)
    );
    let wrong_principal = CallerPrincipal::AiAgent {
        token_id: token.token_id + 1,
    };
    assert_eq!(
        gate.authorize(&wrong_principal, Some(&token)),
        Err(FerrofluidError::InvalidAgentToken)
    );
    for caller in [
        CallerPrincipal::Unauthenticated,
        CallerPrincipal::DeveloperMcpApp {
            app_id: "notes".into(),
        },
    ] {
        // Possession of the token does not change a headless app's principal.
        assert_eq!(
            gate.authorize(&caller, Some(&token)),
            Err(FerrofluidError::UiCreationForbiddenForApp)
        );
    }
}
