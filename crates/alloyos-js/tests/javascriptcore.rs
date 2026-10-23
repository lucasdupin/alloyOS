use alloyos_js::{
    AppHandler, EngineLimits, JavaScriptCoreEngine, JsError, JsExecutionDomain,
    JsValue, VirtualHostEnv,
};

#[test]
fn test_jsc_modern_ecmascript_features() {
    let engine = JavaScriptCoreEngine::new();
    let script = r#"
        class Calculator {
            #base;
            constructor(base) {
                this.#base = base;
            }
            compute(items) {
                return items
                    .map(x => x * this.#base)
                    .reduce((acc, x) => acc + x, 0);
            }
        }
        const calc = new Calculator(3);
        const result = calc.compute([1, 2, 3, 4]);
        const set = new Set([result, 42, result]);
        print(`Calculated: ${result}, Set size: ${set.size}`);
        result;
    "#;

    let output = engine
        .eval_isolated_web_script(script)
        .expect("JSC must execute modern ECMAScript with classes and private fields");
    assert_eq!(output.value, JsValue::Number(30));
    assert_eq!(output.printed, vec!["Calculated: 30, Set size: 2".to_string()]);
}

#[test]
fn test_jsc_promises_and_typed_arrays() {
    let engine = JavaScriptCoreEngine::new();
    let script = r#"
        'use strict';
        const buffer = new ArrayBuffer(16);
        const view = new DataView(buffer);
        view.setUint32(0, 0x12345678, false);
        const u8 = new Uint8Array(buffer);

        let check = false;
        Promise.resolve(u8[0]).then(byte => {
            check = byte === 0x12;
            print(`Byte0: 0x${byte.toString(16)}, Check: ${check}`);
        });
        view.getUint32(0, false);
    "#;

    let output = engine
        .eval_isolated_web_script(script)
        .expect("JSC must evaluate DataView and typed arrays");
    assert_eq!(output.value, JsValue::Number(0x12345678));
    assert!(output.printed.iter().any(|s| s.contains("Byte0: 0x12, Check: true")));
}

#[test]
fn test_jsc_alloyos_host_bindings_and_capability_checks() {
    let engine = JavaScriptCoreEngine::new();
    let mut host = VirtualHostEnv::new();
    host.files.insert("/var/data.txt".to_string(), "hello alloyos".to_string());

    let script = r#"
        const text = AlloyOS.fs.read("/var/data.txt");
        AlloyOS.fs.write("/var/out.txt", text + " processed");
        AlloyOS.log("file written");
        AlloyOS.ui.notify("done");
        AlloyOS.fs.read("/var/out.txt");
    "#;

    let output = engine
        .eval_in_domain(JsExecutionDomain::HeadlessApp, script, &mut host)
        .expect("JSC must execute with AlloyOS bindings");
    assert_eq!(output.value, JsValue::String("hello alloyos processed".to_string()));
    assert_eq!(host.files.get("/var/out.txt").map(|s| s.as_str()), Some("hello alloyos processed"));
    assert_eq!(host.notifications, vec!["done".to_string()]);
    assert!(host.logs.iter().any(|s| s.contains("file written")));
}

#[test]
fn test_jsc_zero_shell_access_strictly_prohibited() {
    let engine = JavaScriptCoreEngine::new();
    let mut host = VirtualHostEnv::new();

    let err = engine
        .eval_in_domain(JsExecutionDomain::HeadlessApp, "AlloyOS.sh('echo pwn');", &mut host)
        .expect_err("Shell execution must be forbidden");
    assert_eq!(
        err,
        JsError::ForbiddenCapability(
            "Shell execution is strictly prohibited in alloyOS JavaScript runtime".to_string()
        )
    );
}

#[test]
fn test_jsc_isolated_web_script_domain_has_no_alloyos() {
    let engine = JavaScriptCoreEngine::new();
    let mut host = VirtualHostEnv::new();

    let err = engine
        .eval_in_domain(JsExecutionDomain::IsolatedWebScript, "AlloyOS.log('leak');", &mut host)
        .expect_err("IsolatedWebScript domain must not have AlloyOS bindings");
    assert!(matches!(err, JsError::ReferenceError(_)));
}

#[test]
fn test_jsc_app_handler_invocation() {
    let engine = JavaScriptCoreEngine::new();
    let mut host = VirtualHostEnv::new();
    let app_source = r#"
        function callTool(name, args) {
            return `Tool ${name} executed with arg: ${args.x * 2}`;
        }
    "#;

    let mut args_map = std::collections::BTreeMap::new();
    args_map.insert("x".to_string(), JsValue::Number(21));
    let args = [
        JsValue::String("compute".to_string()),
        JsValue::Object(args_map),
    ];

    let result = engine
        .invoke_app_handler(app_source, AppHandler::CallTool, &args, &mut host)
        .expect("JSC must invoke callTool handler");
    assert_eq!(result.text, "Tool compute executed with arg: 42");
}

#[test]
fn test_jsc_watchdog_time_limit_on_infinite_loop() {
    let engine = JavaScriptCoreEngine::with_limits(EngineLimits {
        max_source_bytes: 256 * 1024,
        max_steps: 100,
        max_result_bytes: 1024 * 1024,
    });

    let err = engine
        .eval_isolated_web_script("while(true) {}")
        .expect_err("Watchdog must terminate infinite loop");
    assert_eq!(err, JsError::OutOfFuel);
}
