use metalos_js::{JsError, JsRuntime, JsValue, VirtualHostEnv};

#[test]
fn named_handler_receives_arguments_without_running_main() {
    let mut runtime = JsRuntime::new();
    let mut host = VirtualHostEnv::new();
    let source = "let calls = 0; function callTool(name) { calls = calls + 1; return name; } function main() { calls = calls + 10; }";
    let result = runtime
        .call_function(
            source,
            "callTool",
            &[JsValue::String("clock.now".into())],
            &mut host,
        )
        .unwrap();
    assert_eq!(result, JsValue::String("clock.now".into()));
    assert_eq!(runtime.get_global("calls"), Some(&JsValue::Number(1)));
}

#[test]
fn main_handler_runs_once() {
    let mut runtime = JsRuntime::new();
    let mut host = VirtualHostEnv::new();
    let result = runtime
        .call_function(
            "let calls = 0; function main() { calls = calls + 1; return calls; }",
            "main",
            &[],
            &mut host,
        )
        .unwrap();
    assert_eq!(result, JsValue::Number(1));
}

#[test]
fn non_callable_handler_is_a_type_error() {
    let mut runtime = JsRuntime::new();
    let mut host = VirtualHostEnv::new();
    let error = runtime
        .call_function("let callTool = 42;", "callTool", &[], &mut host)
        .unwrap_err();
    assert!(
        matches!(error, JsError::TypeError(message) if message.contains("not a function") && message.contains("number"))
    );
}
