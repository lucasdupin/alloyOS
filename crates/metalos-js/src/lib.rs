#![cfg_attr(not(test), no_std)]

#[cfg(test)]
extern crate std;

extern crate alloc;

pub mod ast;
pub mod host;
pub mod lexer;
pub mod parser;
pub mod runtime;
pub mod scheduler;
pub mod value;

pub use host::{
    CapabilityRequest, CapabilityScopedHost, MetalOsHost, OsEvent, ScriptCapabilities,
    ScriptCapability, SystemStatus, VirtualHostEnv,
};
pub use parser::parse_program;
pub use runtime::{JsIsolate, JsRuntime, V8Isolate, DEFAULT_MAX_STEPS};
pub use scheduler::{
    glob_match, CalendarTime, CronExpr, CronField, CronParseError, EventPattern, ScheduleTrigger,
    ScheduledScript, ScriptExecutionReport, ScriptScheduler, ScriptStore, SCHEDULES_CONF_PATH,
};
pub use value::{json_parse, json_stringify, FunctionBody, JsError, JsFunction, JsValue};

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::string::ToString;
    use alloc::vec;

    #[test]
    fn test_js_variables_operators_and_control_flow() {
        let mut rt = JsRuntime::new();
        let script = r#"
            let total = 0;
            const factor = 3;
            for (let i = 1; i <= 5; i++) {
                if (i % 2 === 1) {
                    total += i * factor;
                } else {
                    total -= 1;
                }
            }
            let count = 3;
            while (count > 0) {
                total += 2;
                count--;
            }
            total;
        "#;
        // Odd i in 1..=5: 1*3 + 3*3 + 5*3 = 3 + 9 + 15 = 27
        // Even i in 1..=5: 2, 4 -> -2 => 25
        // while loop adds 2 * 3 = 6 => 31
        let val = rt.eval(script).expect("script should succeed");
        assert_eq!(val, JsValue::Number(31));
    }

    #[test]
    fn test_js_functions_recursion_arrows_and_closures() {
        let mut rt = V8Isolate::new();
        let script = r#"
            function fib(n) {
                if (n <= 1) {
                    return n;
                }
                return fib(n - 1) + fib(n - 2);
            }

            function makeMultiplier(factor) {
                return (x) => x * factor;
            }

            const triple = makeMultiplier(3);
            const nums = [1, 2, 3, 4];
            const mapped = nums.map(x => triple(x) + 1);
            const filtered = mapped.filter(v => v > 7);

            fib(7) + filtered.length * 100 + filtered[0];
        "#;
        // fib(7) = 13
        // mapped = [4, 7, 10, 13]
        // filtered = [10, 13] -> length = 2, filtered[0] = 10
        // 13 + 200 + 10 = 223
        let val = rt.eval(script).expect("functions and closures should evaluate");
        assert_eq!(val, JsValue::Number(223));
    }

    #[test]
    fn test_js_arrays_objects_strings_and_json() {
        let mut rt = JsIsolate::new();
        let script = r#"
            let items = ["alpha", "beta"];
            items.push("gamma");
            let joined = items.join("-");

            let raw = '{"service":"scriptd","jobs":[10,20,30],"active":true}';
            let parsed = JSON.parse(raw);
            parsed.summary = joined.toUpperCase();
            parsed.count = parsed.jobs.length;

            JSON.stringify(parsed);
        "#;

        let val = rt.eval(script).expect("JSON and object manipulation should work");
        let JsValue::String(json_out) = val else {
            panic!("Expected JSON string output");
        };
        let reparsed = json_parse(&json_out).expect("valid JSON");
        assert_eq!(
            reparsed.get_prop("service"),
            Some(&JsValue::String("scriptd".to_string()))
        );
        assert_eq!(reparsed.get_prop("count"), Some(&JsValue::Number(3)));
        assert_eq!(
            reparsed.get_prop("summary"),
            Some(&JsValue::String("ALPHA-BETA-GAMMA".to_string()))
        );
    }

    #[test]
    fn test_deterministic_fuel_limit_halts_infinite_loops() {
        let mut rt = JsIsolate::with_max_steps(250);
        let err = rt
            .eval("let i = 0; while (true) { i++; }")
            .expect_err("infinite loop must hit fuel limit");
        assert_eq!(err, JsError::ExecutionLimitExceeded { max_steps: 250 });
    }

    #[test]
    fn test_zero_shell_access_prohibition() {
        let mut rt = JsIsolate::new();
        let mut host = VirtualHostEnv::new();

        let forbidden_snippets = [
            r#"sh("ls -la")"#,
            r#"exec("/bin/sh")"#,
            r#"spawn("rm -rf /")"#,
            r#"child_process.exec("whoami")"#,
            r#"MetalOS.sh("uname -a")"#,
            r#"MetalOS.exec("cat /etc/passwd")"#,
            r#"let x = MetalOS.sh;"#,
        ];

        for snippet in forbidden_snippets {
            let err = rt
                .eval_with_host(snippet, &mut host)
                .expect_err("shell access must be forbidden");
            assert_eq!(
                err,
                JsError::ForbiddenCapability(
                    "Shell execution is disabled in metalOS".to_string()
                ),
                "Snippet failed to trigger ForbiddenCapability: {}",
                snippet
            );
        }
    }

    #[test]
    fn test_metalos_host_bindings_fs_net_sys_ui_log() {
        let mut rt = JsIsolate::new();
        let mut host = VirtualHostEnv::new();
        host.status = SystemStatus {
            uptime_ms: 42_500,
            mem_used_bytes: 8 * 1024 * 1024,
            mem_free_bytes: 120 * 1024 * 1024,
            active_tasks: 7,
        };
        host.register_http_response(
            "http://api.internal/config",
            r#"{"mode":"autonomous","version":2}"#,
        );

        let script = r#"
            const body = MetalOS.net.fetch("http://api.internal/config");
            const cfg = JSON.parse(body);
            const st = MetalOS.sys.status();

            const report = {
                mode: cfg.mode,
                uptime: st.uptimeMs,
                tasks: st.activeTasks
            };

            MetalOS.fs.write("/var/report.json", JSON.stringify(report));
            MetalOS.fs.write("/var/notes.txt", "ready");
            const files = MetalOS.fs.list("/var");
            MetalOS.ui.notify("Report generated: " + files.join(","));
            MetalOS.log("completed host test");
            MetalOS.fs.read("/var/report.json");
        "#;

        let res = rt
            .eval_with_host(script, &mut host)
            .expect("host bindings script should succeed");
        assert_eq!(
            res,
            JsValue::String(r#"{"mode":"autonomous","tasks":7,"uptime":42500}"#.to_string())
        );
        assert_eq!(
            host.notifications,
            vec!["Report generated: notes.txt,report.json".to_string()]
        );
        assert_eq!(host.logs, vec!["completed host test".to_string()]);

        let emitted = host.drain_emitted_events();
        assert_eq!(
            emitted,
            vec![
                OsEvent::FsCreated {
                    path: "/var/report.json".to_string()
                },
                OsEvent::FsCreated {
                    path: "/var/notes.txt".to_string()
                },
            ]
        );
    }

    #[test]
    fn test_cron_parser_and_matching() {
        // Every 15 minutes during business hours (9-17) on Mon-Fri (1-5) in Jan,Jun,Dec (1,6,12)
        let cron = CronExpr::parse("*/15 9-17 * 1,6,12 1-5").expect("valid 5-field cron");

        // Matches: Monday (1), June (6), 10th, 09:30
        assert!(cron.matches(2026, 6, 10, 9, 30, 1));
        // Matches: Friday (5), December (12), 1st, 17:45
        assert!(cron.matches(2026, 12, 1, 17, 45, 5));
        // Does not match minute 14
        assert!(!cron.matches(2026, 6, 10, 9, 14, 1));
        // Does not match hour 18
        assert!(!cron.matches(2026, 6, 10, 18, 0, 1));
        // Does not match Sunday (0 or 7)
        assert!(!cron.matches(2026, 6, 10, 9, 30, 0));
        assert!(!cron.matches(2026, 6, 10, 9, 30, 7));
        // Does not match month 5
        assert!(!cron.matches(2026, 5, 10, 9, 30, 1));
    }

    #[test]
    fn test_time_based_scheduling_once_interval_and_cron() {
        let mut scheduler = ScriptScheduler::new();
        let mut host = VirtualHostEnv::new();

        scheduler.register_script_source(
            "/scripts/once_init.js",
            r#"MetalOS.log("once@" + EVENT.nowMs);"#,
        );
        scheduler.register_script_source(
            "/scripts/heartbeat.js",
            r#"MetalOS.log("hb@" + EVENT.nowMs);"#,
        );
        scheduler.register_script_source(
            "/scripts/hourly_cleanup.js",
            r#"MetalOS.log("cron@" + EVENT.hour + ":" + EVENT.minute);"#,
        );

        scheduler.schedule_script(
            "init-job",
            "/scripts/once_init.js",
            ScheduleTrigger::OnceAtMs(1_000),
        );
        scheduler.schedule_script(
            "hb-job",
            "/scripts/heartbeat.js",
            ScheduleTrigger::IntervalMs { every_ms: 500 },
        );
        scheduler.schedule_script(
            "cron-job",
            "/scripts/hourly_cleanup.js",
            ScheduleTrigger::Cron(CronExpr::parse("0 */2 * * *").unwrap()),
        );

        // t = 0ms (arms interval timer for t=500ms, cron at 01:15 does not match)
        let r0 = scheduler.tick_time(
            0,
            Some(CalendarTime::new(2026, 10, 12, 1, 15, 1)),
            &mut host,
        );
        assert!(r0.is_empty());

        // t = 500ms (interval fires, once not yet due, cron at 02:00 matches!)
        let r1 = scheduler.tick_time(
            500,
            Some(CalendarTime::new(2026, 10, 12, 2, 0, 1)),
            &mut host,
        );
        assert_eq!(r1.len(), 2);

        // Duplicate tick within the same minute 02:00 at t = 600ms should NOT re-fire cron
        let r2 = scheduler.tick_time(
            600,
            Some(CalendarTime::new(2026, 10, 12, 2, 0, 1)),
            &mut host,
        );
        assert!(r2.is_empty());

        // t = 1000ms (once fires + interval fires)
        let r3 = scheduler.tick_time(
            1_000,
            Some(CalendarTime::new(2026, 10, 12, 2, 1, 1)),
            &mut host,
        );
        assert_eq!(r3.len(), 2);

        // t = 1500ms (only interval fires; once already fired)
        let r4 = scheduler.tick_time(
            1_500,
            Some(CalendarTime::new(2026, 10, 12, 2, 2, 1)),
            &mut host,
        );
        assert_eq!(r4.len(), 1);

        assert_eq!(
            host.logs,
            vec![
                "hb@500".to_string(),
                "cron@2:0".to_string(),
                "once@1000".to_string(),
                "hb@1000".to_string(),
                "hb@1500".to_string(),
            ]
        );
    }

    #[test]
    fn test_event_driven_scheduling_and_persistence() {
        let mut scheduler = ScriptScheduler::new();
        let mut host = VirtualHostEnv::new();

        // Model writes an automation script to process incoming JSON files in `/inbox/*`
        let inbox_processor = r#"
            const raw = MetalOS.fs.read(EVENT.path);
            const data = JSON.parse(raw);
            const out = {
                source: EVENT.path,
                customer: data.customer.toUpperCase(),
                totalWithTax: data.amount + 15,
                processed: true
            };
            MetalOS.fs.write("/processed/out.json", JSON.stringify(out));
            MetalOS.ui.notify("Processed " + EVENT.path);
            out.totalWithTax;
        "#;

        scheduler.register_script_source("/scripts/on_upload.js", inbox_processor);
        scheduler.schedule_script(
            "inbox-watcher",
            "/scripts/on_upload.js",
            ScheduleTrigger::OnEvent(EventPattern::FsCreated("/inbox/*".to_string())),
        );

        // Persist schedules to `/etc/schedules.conf` and reload into a new scheduler
        scheduler
            .save_to_host(&mut host)
            .expect("should write /etc/schedules.conf");
        let mut restored_scheduler = ScriptScheduler::new();
        restored_scheduler.register_script_source("/scripts/on_upload.js", inbox_processor);
        let loaded_count = restored_scheduler
            .load_from_host(&mut host)
            .expect("should parse /etc/schedules.conf");
        assert_eq!(loaded_count, 1);

        // Clear initial write event from saving `/etc/schedules.conf`
        let _ = host.drain_emitted_events();

        // Write a file to `/inbox/order_42.json` and dispatch the emitted FsCreated event
        host.fs_write(
            "/inbox/order_42.json",
            r#"{"customer":"acme","amount":100}"#,
        )
        .unwrap();
        let events = host.drain_emitted_events();
        assert_eq!(events.len(), 1);

        let reports = restored_scheduler.dispatch_event(&events[0], &mut host);
        assert_eq!(reports.len(), 1);
        assert_eq!(reports[0].schedule_id, "inbox-watcher");
        assert_eq!(reports[0].result, Ok(JsValue::Number(115)));

        let processed = host
            .fs_read("/processed/out.json")
            .expect("/processed/out.json should be written by triggered script");
        assert_eq!(
            processed,
            r#"{"customer":"ACME","processed":true,"source":"/inbox/order_42.json","totalWithTax":115}"#
        );
        assert_eq!(
            host.notifications,
            vec!["Processed /inbox/order_42.json".to_string()]
        );
    }

    #[test]
    fn test_capability_scoping_fs_net_sys_ui_and_traversal_blocking() {
        let mut host = VirtualHostEnv::with_capabilities(
            ScriptCapabilities::none()
                .with_fs_read("/inbox")
                .with_fs_write("/outbox")
                .with_net_domain("api.metalos.local")
                .with_sys_status(false)
                .with_ui_notify(true),
        );
        host.files.insert("/inbox/msg.txt".to_string(), "ok".to_string());
        host.files.insert("/etc/secret.conf".to_string(), "top-secret".to_string());
        host.register_http_response("https://api.metalos.local/ping", "pong");
        host.register_http_response("https://evil.example.com/exfil", "nope");

        let mut rt = JsRuntime::new();

        // Allowed read under /inbox
        assert_eq!(
            rt.eval_with_host("MetalOS.fs.read('/inbox/msg.txt');", &mut host),
            Ok(JsValue::String("ok".to_string()))
        );
        // Denied read outside /inbox or via .. traversal
        assert!(matches!(
            rt.eval_with_host("MetalOS.fs.read('/etc/secret.conf');", &mut host),
            Err(JsError::ForbiddenCapability(_))
        ));
        assert!(matches!(
            rt.eval_with_host("MetalOS.fs.read('/inbox/../etc/secret.conf');", &mut host),
            Err(JsError::ForbiddenCapability(_))
        ));

        // Allowed write under /outbox
        assert_eq!(
            rt.eval_with_host("MetalOS.fs.write('/outbox/result.txt', 'done');", &mut host),
            Ok(JsValue::Bool(true))
        );
        // Denied write outside /outbox
        assert!(matches!(
            rt.eval_with_host("MetalOS.fs.write('/etc/ai.conf', 'pwn');", &mut host),
            Err(JsError::ForbiddenCapability(_))
        ));

        // Allowed net.fetch to api.metalos.local, denied to evil.example.com
        assert_eq!(
            rt.eval_with_host(
                "MetalOS.net.fetch('https://api.metalos.local/ping');",
                &mut host
            ),
            Ok(JsValue::String("pong".to_string()))
        );
        assert!(matches!(
            rt.eval_with_host(
                "MetalOS.net.fetch('https://evil.example.com/exfil');",
                &mut host
            ),
            Err(JsError::ForbiddenCapability(_))
        ));

        // SysStatus is disabled, UiNotify is enabled
        assert!(matches!(
            rt.eval_with_host("MetalOS.sys.status();", &mut host),
            Err(JsError::ForbiddenCapability(_))
        ));
        assert_eq!(
            rt.eval_with_host("MetalOS.ui.notify('Scoped ok');", &mut host),
            Ok(JsValue::Bool(true))
        );
    }

    #[test]
    fn test_shorthand_triggers_and_export_main_execution() {
        let mut host = VirtualHostEnv::new();
        let mut sched = ScriptScheduler::new();
        sched.register_script_source(
            "/scripts/heartbeat.js",
            "export function main() { return MetalOS.sys.status().uptimeMs; }",
        );
        let trigger = ScheduleTrigger::deserialize("every:60s").unwrap();
        assert_eq!(trigger, ScheduleTrigger::IntervalMs { every_ms: 60_000 });
        sched.schedule_script("hb", "/scripts/heartbeat.js", trigger);

        host.status.uptime_ms = 60_000;
        let reports = sched.tick_time(60_000, None, &mut host);
        assert_eq!(reports.len(), 1);
        assert_eq!(reports[0].result, Ok(JsValue::Number(60_000)));
    }
}

#[cfg(test)]
mod forge_tests {
    use super::*;
    use alloc::{boxed::Box,string::ToString};
    #[test]
    fn local_forge_capability_is_opt_in_and_options_are_validated() {
        let mut rt=JsRuntime::new();
        let mut host=VirtualHostEnv::new();
        assert!(rt.eval_with_host("MetalOS.forge('hello');",&mut host).is_err());
        host.set_forge_handler(Box::new(|prompt,model,max_tokens| {
            assert_eq!(prompt,"hello");assert_eq!(model,Some("test-model"));assert_eq!(max_tokens,4);
            Ok("local result".to_string())
        }));
        assert_eq!(rt.eval_with_host("MetalOS.forge('hello', {model:'test-model', max_tokens:4});",&mut host).unwrap(),JsValue::String("local result".to_string()));
        for script in ["MetalOS.forge('hello', {max_tokens:0});","MetalOS.forge('hello', {max_tokens:257});","MetalOS.forge('hello', {auth_token:'secret'});","MetalOS.forge(42);","MetalOS.forge('hello', false);"] {
            assert!(rt.eval_with_host(script,&mut host).is_err());
        }
    }
}
