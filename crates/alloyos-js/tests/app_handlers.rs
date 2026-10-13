use alloyos_js::{
    invoke_app_handler, validate_app_handlers, AppHandler, JsError, JsValue, VirtualHostEnv,
};
#[test]
fn typed_arguments_fresh_scopes_and_no_implicit_main() {
    let source = "let count = 0; function main() { count = 100; } function callTool(name, args) { count += 1; return name + args.value + count; }";
    let args = [
        JsValue::String("fixture.now".into()),
        JsValue::Object(
            [("value".into(), JsValue::String(":ok:".into()))]
                .into_iter()
                .collect(),
        ),
    ];
    let mut host = VirtualHostEnv::new();
    for _ in 0..2 {
        let result =
            invoke_app_handler(source, AppHandler::CallTool, &args, 500, &mut host).unwrap();
        assert_eq!(result.text, "fixture.now:ok:1");
        assert!(result.steps_executed > 0);
    }
}
#[test]
fn initialization_and_handler_share_fuel() {
    let mut host = VirtualHostEnv::new();
    for source in [
        "while (true) {} function renderTile(id) { return id; }",
        "function renderTile(id) { while (true) {} return id; }",
    ] {
        assert!(matches!(
            invoke_app_handler(
                source,
                AppHandler::RenderTile,
                &[JsValue::String("fixture.now".into())],
                100,
                &mut host
            ),
            Err(JsError::ExecutionLimitExceeded { max_steps: 100 })
        ));
    }
}
#[test]
fn validation_does_not_execute_and_checks_contract() {
    assert!(validate_app_handlers(
        "while (true) {} function renderTile(id) { return id; }",
        &[AppHandler::RenderTile]
    )
    .is_ok());
    assert!(validate_app_handlers("function renderTile() {}", &[AppHandler::RenderTile]).is_err());
    assert!(validate_app_handlers(
        "function renderTile(id) {} function renderTile(id) {}",
        &[AppHandler::RenderTile]
    )
    .is_err());
    assert!(validate_app_handlers("let renderTile = 3;", &[AppHandler::RenderTile]).is_err());
    let mut host = VirtualHostEnv::new();
    assert!(invoke_app_handler(
        "function renderTile(id) { return 42; }",
        AppHandler::RenderTile,
        &[JsValue::String("fixture.now".into())],
        100,
        &mut host
    )
    .is_err());
}

#[test]
fn arguments_have_an_aggregate_byte_budget() {
    let text = "x".repeat(600_000);
    let args = [JsValue::String(text.clone()), JsValue::String(text)];
    let mut host = VirtualHostEnv::new();
    assert!(matches!(invoke_app_handler("function callTool(name, args) { return name; }", AppHandler::CallTool, &args, 100, &mut host), Err(JsError::TypeError(message)) if message.contains("data exceeds")));
}
