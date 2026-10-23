//! JavaScriptCore-backed DOM execution environment for web pages.

use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;
use alloyos_js::JavaScriptCoreEngine;

fn escape_js_str(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            other => out.push(other),
        }
    }
    out.push('"');
    out
}

/// State of an element synced with the JS DOM environment.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JsElementState {
    pub id: String,
    pub tag: String,
    pub value: String,
    pub text_content: String,
}

/// JavaScript execution context bound to a WebKit page DOM.
pub struct PageJsRuntime {
    engine: JavaScriptCoreEngine,
    dataset: Vec<(String, String)>,
    elements: Vec<JsElementState>,
    scripts: Vec<String>,
}

impl Default for PageJsRuntime {
    fn default() -> Self {
        Self::new()
    }
}

impl PageJsRuntime {
    pub fn new() -> Self {
        Self {
            engine: JavaScriptCoreEngine::new(),
            dataset: Vec::new(),
            elements: Vec::new(),
            scripts: Vec::new(),
        }
    }

    /// Registers initial elements found in the parsed HTML.
    pub fn register_element(&mut self, id: &str, tag: &str, value: &str, text_content: &str) {
        if let Some(el) = self.elements.iter_mut().find(|e| e.id == id) {
            if tag != "div" && !tag.is_empty() {
                el.tag = String::from(tag);
            }
            el.value = String::from(value);
            el.text_content = String::from(text_content);
        } else {
            self.elements.push(JsElementState {
                id: String::from(id),
                tag: String::from(tag),
                value: String::from(value),
                text_content: String::from(text_content),
            });
        }
    }

    /// Appends a page script (from `<script>` tag).
    pub fn add_script(&mut self, script: &str) {
        self.scripts.push(String::from(script));
    }

    /// Initializes and executes all page scripts within the DOM environment.
    pub fn execute_page_scripts(&mut self) -> Result<(), String> {
        let runner = self.build_execution_bundle(None, None);
        self.evaluate_and_sync(&runner)
    }

    /// Dispatches an event (e.g. 'submit' or 'click') to an element and syncs state.
    pub fn dispatch_event(&mut self, target_id: &str, event_type: &str) -> Result<(), String> {
        let runner = self.build_execution_bundle(Some(target_id), Some(event_type));
        self.evaluate_and_sync(&runner)
    }

    /// Evaluates arbitrary JavaScript in the page context.
    pub fn eval(&mut self, code: &str) -> Result<String, String> {
        let escaped = escape_js_str(code);
        let wrapper = format!(
            r#"
            (function() {{
                {}
                try {{
                    const __res = eval({});
                    return JSON.stringify(__res !== undefined ? __res : null);
                }} catch (e) {{
                    return JSON.stringify(String(e));
                }}
            }})();
            "#,
            self.build_dom_prelude(),
            escaped
        );

        match self.engine.eval_isolated_web_script(&wrapper) {
            Ok(output) => {
                if let alloyos_js::JsValue::String(s) = output.value {
                    Ok(s)
                } else {
                    Ok(format!("{:?}", output.value))
                }
            }
            Err(e) => Err(format!("{:?}", e)),
        }
    }

    pub fn get_dataset_attr(&self, key: &str) -> Option<&str> {
        self.dataset
            .iter()
            .find(|(k, _)| k == key)
            .map(|(_, v)| v.as_str())
    }

    pub fn set_element_value(&mut self, id: &str, value: &str) {
        if let Some(el) = self.elements.iter_mut().find(|e| e.id == id) {
            el.value = String::from(value);
        } else {
            self.elements.push(JsElementState {
                id: String::from(id),
                tag: String::from("input"),
                value: String::from(value),
                text_content: String::new(),
            });
        }
    }

    pub fn get_element_value(&self, id: &str) -> Option<&str> {
        self.elements
            .iter()
            .find(|e| e.id == id)
            .map(|e| e.value.as_str())
    }

    pub fn get_element_text(&self, id: &str) -> Option<&str> {
        self.elements
            .iter()
            .find(|e| e.id == id)
            .map(|e| e.text_content.as_str())
    }

    pub fn dataset(&self) -> &[(String, String)] {
        &self.dataset
    }

    fn build_dom_prelude(&self) -> String {
        let mut elements_json = String::from("{");
        for (i, el) in self.elements.iter().enumerate() {
            if i > 0 {
                elements_json.push(',');
            }
            let escaped_val = el.value.replace('\\', "\\\\").replace('"', "\\\"");
            let escaped_text = el
                .text_content
                .replace('\\', "\\\\")
                .replace('"', "\\\"")
                .replace('\n', "\\n");
            elements_json.push_str(&format!(
                r#""{}": new DOMElement("{}", "{}", "{}", "{}")"#,
                el.id, el.id, el.tag, escaped_val, escaped_text
            ));
        }
        elements_json.push('}');

        let mut dataset_json = String::from("{");
        for (i, (k, v)) in self.dataset.iter().enumerate() {
            if i > 0 {
                dataset_json.push(',');
            }
            dataset_json.push_str(&format!(r#""{}": "{}""#, k, v));
        }
        dataset_json.push('}');

        format!(
            r#"
            const __listeners = {{}};
            class DOMElement {{
                constructor(id, tag, value, textContent) {{
                    this.id = id;
                    this.tagName = (tag || "div").toUpperCase();
                    this.value = value || "";
                    this.textContent = textContent || "";
                    this.dataset = {{}};
                    this.style = {{}};
                }}
                addEventListener(type, listener) {{
                    if (!__listeners[this.id]) __listeners[this.id] = {{}};
                    if (!__listeners[this.id][type]) __listeners[this.id][type] = [];
                    __listeners[this.id][type].push(listener);
                }}
            }}

            const __elements = {};
            const __dataset = {};

            const documentElement = {{ dataset: __dataset }};
            const document = {{
                documentElement: documentElement,
                title: "",
                getElementById: function(id) {{
                    if (!__elements[id]) {{
                        __elements[id] = new DOMElement(id, 'div', '', '');
                    }}
                    return __elements[id];
                }}
            }};

            globalThis.document = document;
            globalThis.window = globalThis;
            globalThis.self = globalThis;
            globalThis.location = {{ href: "", hostname: "", pathname: "/", search: "", hash: "" }};
            globalThis.navigator = {{ userAgent: "Mozilla/5.0 alloyOS-WebKit/1.0" }};
            globalThis.localStorage = {{ getItem: () => null, setItem: () => {{}}, removeItem: () => {{}} }};
            globalThis.sessionStorage = {{ getItem: () => null, setItem: () => {{}}, removeItem: () => {{}} }};
            globalThis.console = {{ log: () => {{}}, warn: () => {{}}, error: () => {{}}, info: () => {{}} }};
            globalThis.setTimeout = (fn) => {{ try {{ fn(); }} catch(e) {{}} return 1; }};
            globalThis.clearTimeout = () => {{}};
            globalThis.setInterval = () => 1;
            globalThis.clearInterval = () => {{}};
            window.addEventListener = (type, listener) => {{}};
            document.addEventListener = (type, listener) => {{}};
            "#,
            elements_json, dataset_json
        )
    }

    fn build_execution_bundle(
        &self,
        dispatch_target: Option<&str>,
        dispatch_event: Option<&str>,
    ) -> String {
        let mut bundle = String::from("(function() {\n");
        bundle.push_str(&self.build_dom_prelude());

        // Append page scripts
        for s in &self.scripts {
            bundle.push_str("\ntry {\n(function() {\n");
            bundle.push_str(s);
            bundle.push_str("\n})();\n} catch (e) {}\n");
        }

        // If dispatch requested
        if let (Some(target), Some(ev)) = (dispatch_target, dispatch_event) {
            bundle.push_str(&format!(
                r#"
                if (__listeners["{target}"] && __listeners["{target}"]["{ev}"]) {{
                    const eventObj = {{
                        type: "{ev}",
                        preventDefault: function() {{}}
                    }};
                    for (const fn of __listeners["{target}"]["{ev}"]) {{
                        fn(eventObj);
                    }}
                }}
                "#,
                target = target,
                ev = ev
            ));
        }

        // Return serialized state
        bundle.push_str(
            r#"
            return JSON.stringify({
                dataset: __dataset,
                elements: Object.keys(__elements).map(k => ({
                    id: k,
                    tag: (__elements[k].tagName || "div").toLowerCase(),
                    value: __elements[k].value,
                    textContent: __elements[k].textContent
                }))
            });
            })();
            "#,
        );

        bundle
    }

    fn evaluate_and_sync(&mut self, runner_code: &str) -> Result<(), String> {
        let output = self
            .engine
            .eval_isolated_web_script(runner_code)
            .map_err(|e| format!("{:?}", e))?;

        let json_str = match output.value {
            alloyos_js::JsValue::String(s) => s,
            _ => return Err(String::from("Expected JSON string from runner")),
        };

        // Parse JSON sync output using serde_json
        if let Ok(val) = serde_json::from_str::<serde_json::Value>(&json_str) {
            if let Some(ds) = val.get("dataset").and_then(|d| d.as_object()) {
                self.dataset.clear();
                for (k, v) in ds {
                    if let Some(str_val) = v.as_str() {
                        self.dataset.push((k.clone(), String::from(str_val)));
                    } else if let Some(b) = v.as_bool() {
                        self.dataset.push((k.clone(), String::from(if b { "true" } else { "false" })));
                    }
                }
            }

            if let Some(els) = val.get("elements").and_then(|e| e.as_array()) {
                for el_val in els {
                    if let Some(id) = el_val.get("id").and_then(|i| i.as_str()) {
                        let tag = el_val
                            .get("tag")
                            .and_then(|t| t.as_str())
                            .unwrap_or("div");
                        let text = el_val
                            .get("textContent")
                            .and_then(|t| t.as_str())
                            .unwrap_or("");
                        let value = el_val.get("value").and_then(|v| v.as_str()).unwrap_or("");
                        self.register_element(id, tag, value, text);
                    }
                }
            }
        }

        Ok(())
    }
}
