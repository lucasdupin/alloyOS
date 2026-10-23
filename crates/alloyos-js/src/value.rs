use alloc::boxed::Box;
use alloc::collections::BTreeMap;
use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec::Vec;
use core::fmt;

use crate::ast::{Expr, Stmt};

/// Error type returned by the JavaScript lexer, parser, or runtime evaluator.
#[derive(Debug, Clone, Eq)]
pub enum JsError {
    /// Lexical or grammatical syntax error.
    SyntaxError(String),
    /// Reference to an undeclared variable.
    ReferenceError(String),
    /// Invalid type operation or call on non-function.
    TypeError(String),
    /// Uncaught runtime error (`new Error(...)` or `throw ...`).
    RuntimeError(String),
    /// Deterministic fuel limit (`max_steps`) exceeded.
    ExecutionLimitExceeded { max_steps: usize },
    /// Deterministic step fuel exhausted (unit alias for fuel limit errors).
    OutOfFuel,
    /// Attempt to invoke a forbidden capability (such as shell execution).
    ForbiddenCapability(String),
    /// Error propagated from a `AlloyOS.*` host binding.
    HostError(String),
}

impl PartialEq for JsError {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (JsError::SyntaxError(a), JsError::SyntaxError(b)) => a == b,
            (JsError::ReferenceError(a), JsError::ReferenceError(b)) => a == b,
            (JsError::TypeError(a), JsError::TypeError(b)) => a == b,
            (JsError::RuntimeError(a), JsError::RuntimeError(b)) => a == b,
            (
                JsError::ExecutionLimitExceeded { max_steps: a },
                JsError::ExecutionLimitExceeded { max_steps: b },
            ) => a == b,
            (JsError::OutOfFuel, JsError::OutOfFuel)
            | (JsError::ExecutionLimitExceeded { .. }, JsError::OutOfFuel)
            | (JsError::OutOfFuel, JsError::ExecutionLimitExceeded { .. }) => true,
            (JsError::ForbiddenCapability(a), JsError::ForbiddenCapability(b)) => a == b,
            (JsError::HostError(a), JsError::HostError(b)) => a == b,
            _ => false,
        }
    }
}

impl fmt::Display for JsError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            JsError::SyntaxError(msg) => write!(f, "SyntaxError: {}", msg),
            JsError::ReferenceError(msg) => write!(f, "ReferenceError: {}", msg),
            JsError::TypeError(msg) => write!(f, "TypeError: {}", msg),
            JsError::RuntimeError(msg) => write!(f, "Error: {}", msg),
            JsError::ExecutionLimitExceeded { max_steps } => {
                write!(f, "ExecutionLimitExceeded: exceeded {} steps", max_steps)
            }
            JsError::OutOfFuel => write!(f, "OutOfFuel: execution step limit exceeded"),
            JsError::ForbiddenCapability(msg) => write!(f, "ForbiddenCapability: {}", msg),
            JsError::HostError(msg) => write!(f, "HostError: {}", msg),
        }
    }
}

/// Body of a user-defined JavaScript function or arrow function.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FunctionBody {
    Block(Vec<Stmt>),
    Expr(Box<Expr>),
}

/// Representation of a JavaScript function value with lexical closure state.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JsFunction {
    pub name: Option<String>,
    pub params: Vec<String>,
    pub body: FunctionBody,
    pub captured_env: BTreeMap<String, JsValue>,
}

/// Dynamic JavaScript runtime value inside `alloyos-js`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum JsValue {
    Undefined,
    Null,
    Bool(bool),
    Number(i64),
    String(String),
    Array(Vec<JsValue>),
    Object(BTreeMap<String, JsValue>),
    Function(JsFunction),
}

impl JsValue {
    /// Returns the JavaScript truthiness of this value.
    pub fn is_truthy(&self) -> bool {
        match self {
            JsValue::Undefined | JsValue::Null => false,
            JsValue::Bool(b) => *b,
            JsValue::Number(n) => *n != 0,
            JsValue::String(s) => !s.is_empty(),
            JsValue::Array(_) | JsValue::Object(_) | JsValue::Function(_) => true,
        }
    }

    /// Returns the JavaScript `typeof` string for this value.
    pub fn type_of(&self) -> &'static str {
        match self {
            JsValue::Undefined => "undefined",
            JsValue::Null => "object",
            JsValue::Bool(_) => "boolean",
            JsValue::Number(_) => "number",
            JsValue::String(_) => "string",
            JsValue::Array(_) | JsValue::Object(_) => "object",
            JsValue::Function(_) => "function",
        }
    }

    /// Converts the value to its JavaScript string representation (`ToString`).
    pub fn to_js_string(&self) -> String {
        match self {
            JsValue::Undefined => "undefined".to_string(),
            JsValue::Null => "null".to_string(),
            JsValue::Bool(true) => "true".to_string(),
            JsValue::Bool(false) => "false".to_string(),
            JsValue::Number(n) => format!("{}", n),
            JsValue::String(s) => s.clone(),
            JsValue::Array(items) => {
                let mut out = String::new();
                for (i, item) in items.iter().enumerate() {
                    if i > 0 {
                        out.push(',');
                    }
                    match item {
                        JsValue::Undefined | JsValue::Null => {}
                        other => out.push_str(&other.to_js_string()),
                    }
                }
                out
            }
            JsValue::Object(map) => {
                if let Some(JsValue::String(cls)) = map.get("__class__") {
                    if matches!(
                        cls.as_str(),
                        "Error" | "SyntaxError" | "TypeError" | "ReferenceError"
                    ) {
                        let name = map
                            .get("name")
                            .map(|v| v.to_js_string())
                            .unwrap_or_else(|| cls.clone());
                        let msg = map
                            .get("message")
                            .map(|v| v.to_js_string())
                            .unwrap_or_default();
                        return if msg.is_empty() {
                            name
                        } else {
                            format!("{}: {}", name, msg)
                        };
                    }
                }
                "[object Object]".to_string()
            }
            JsValue::Function(func) => {
                let name = func.name.as_deref().unwrap_or("anonymous");
                format!("[Function: {}]", name)
            }
        }
    }

    /// Converts the value to a numeric `i64` where applicable.
    pub fn to_number(&self) -> Result<i64, JsError> {
        match self {
            JsValue::Number(n) => Ok(*n),
            JsValue::Bool(true) => Ok(1),
            JsValue::Bool(false) | JsValue::Null => Ok(0),
            JsValue::String(s) => {
                let trimmed = s.trim();
                if trimmed.is_empty() {
                    Ok(0)
                } else {
                    trimmed.parse::<i64>().map_err(|_| {
                        JsError::TypeError(format!("Cannot convert string '{}' to number", s))
                    })
                }
            }
            JsValue::Undefined => Err(JsError::TypeError(
                "Cannot convert undefined to integer number".to_string(),
            )),
            JsValue::Array(_) | JsValue::Object(_) | JsValue::Function(_) => Err(
                JsError::TypeError("Cannot convert object/function to number".to_string()),
            ),
        }
    }

    /// Strict equality (`===`).
    pub fn strict_eq(&self, other: &Self) -> bool {
        self == other
    }

    /// Loose equality (`==`).
    pub fn abstract_eq(&self, other: &Self) -> bool {
        match (self, other) {
            (JsValue::Undefined, JsValue::Null) | (JsValue::Null, JsValue::Undefined) => true,
            (JsValue::Number(a), JsValue::String(b)) => {
                b.trim().parse::<i64>().map(|n| n == *a).unwrap_or(false)
            }
            (JsValue::String(a), JsValue::Number(b)) => {
                a.trim().parse::<i64>().map(|n| n == *b).unwrap_or(false)
            }
            (JsValue::Bool(a), JsValue::Number(b)) => (*a as i64) == *b,
            (JsValue::Number(a), JsValue::Bool(b)) => *a == (*b as i64),
            _ => self == other,
        }
    }

    /// Convenience helper to extract an object property.
    pub fn get_prop(&self, key: &str) -> Option<&JsValue> {
        match self {
            JsValue::Object(map) => map.get(key),
            _ => None,
        }
    }
}

impl fmt::Display for JsValue {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.to_js_string())
    }
}

/// Serializes a `JsValue` into a JSON string (`JSON.stringify`).
pub fn json_stringify(value: &JsValue) -> String {
    match value {
        JsValue::Undefined | JsValue::Function(_) => "null".to_string(),
        JsValue::Null => "null".to_string(),
        JsValue::Bool(true) => "true".to_string(),
        JsValue::Bool(false) => "false".to_string(),
        JsValue::Number(n) => format!("{}", n),
        JsValue::String(s) => escape_json_string(s),
        JsValue::Array(items) => {
            let mut out = String::from("[");
            for (i, item) in items.iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                out.push_str(&json_stringify(item));
            }
            out.push(']');
            out
        }
        JsValue::Object(map) => {
            let mut out = String::from("{");
            let mut first = true;
            for (k, v) in map.iter() {
                if k.starts_with("__") || matches!(v, JsValue::Undefined | JsValue::Function(_)) {
                    continue;
                }
                if !first {
                    out.push(',');
                }
                first = false;
                out.push_str(&escape_json_string(k));
                out.push(':');
                out.push_str(&json_stringify(v));
            }
            out.push('}');
            out
        }
    }
}

fn escape_json_string(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for ch in s.chars() {
        match ch {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

/// Parses a JSON string into a `JsValue` (`JSON.parse`).
pub fn json_parse(input: &str) -> Result<JsValue, JsError> {
    let mut parser = JsonParser {
        chars: input.chars().collect(),
        pos: 0,
    };
    parser.skip_ws();
    let value = parser.parse_value()?;
    parser.skip_ws();
    if parser.pos < parser.chars.len() {
        return Err(JsError::SyntaxError(
            "Unexpected trailing characters in JSON".to_string(),
        ));
    }
    Ok(value)
}

struct JsonParser {
    chars: Vec<char>,
    pos: usize,
}

impl JsonParser {
    fn skip_ws(&mut self) {
        while self.pos < self.chars.len() && self.chars[self.pos].is_whitespace() {
            self.pos += 1;
        }
    }

    fn peek(&self) -> Option<char> {
        self.chars.get(self.pos).copied()
    }

    fn next(&mut self) -> Option<char> {
        let ch = self.peek()?;
        self.pos += 1;
        Some(ch)
    }

    fn parse_value(&mut self) -> Result<JsValue, JsError> {
        self.skip_ws();
        match self.peek() {
            Some('"') => self.parse_string().map(JsValue::String),
            Some('{') => self.parse_object(),
            Some('[') => self.parse_array(),
            Some('t') => self.parse_literal("true", JsValue::Bool(true)),
            Some('f') => self.parse_literal("false", JsValue::Bool(false)),
            Some('n') => self.parse_literal("null", JsValue::Null),
            Some(c) if c == '-' || c.is_ascii_digit() => self.parse_number(),
            Some(c) => Err(JsError::SyntaxError(format!(
                "Unexpected character '{}' in JSON",
                c
            ))),
            None => Err(JsError::SyntaxError("Unexpected end of JSON input".to_string())),
        }
    }

    fn parse_literal(&mut self, expected: &str, val: JsValue) -> Result<JsValue, JsError> {
        for exp_ch in expected.chars() {
            match self.next() {
                Some(c) if c == exp_ch => {}
                _ => {
                    return Err(JsError::SyntaxError(format!(
                        "Invalid JSON literal, expected {}",
                        expected
                    )))
                }
            }
        }
        Ok(val)
    }

    fn parse_number(&mut self) -> Result<JsValue, JsError> {
        let mut s = String::new();
        if self.peek() == Some('-') {
            s.push('-');
            self.pos += 1;
        }
        while let Some(c) = self.peek() {
            if c.is_ascii_digit() {
                s.push(c);
                self.pos += 1;
            } else {
                break;
            }
        }
        // If there is a fractional part in JSON, consume digits and truncate to i64
        if self.peek() == Some('.') {
            self.pos += 1;
            while let Some(c) = self.peek() {
                if c.is_ascii_digit() {
                    self.pos += 1;
                } else {
                    break;
                }
            }
        }
        let n = s
            .parse::<i64>()
            .map_err(|_| JsError::SyntaxError(format!("Invalid JSON number '{}'", s)))?;
        Ok(JsValue::Number(n))
    }

    fn parse_string(&mut self) -> Result<String, JsError> {
        if self.next() != Some('"') {
            return Err(JsError::SyntaxError("Expected '\"' in JSON string".to_string()));
        }
        let mut out = String::new();
        while let Some(ch) = self.next() {
            match ch {
                '"' => return Ok(out),
                '\\' => {
                    let esc = self.next().ok_or_else(|| {
                        JsError::SyntaxError("Unterminated escape in JSON string".to_string())
                    })?;
                    match esc {
                        '"' => out.push('"'),
                        '\\' => out.push('\\'),
                        '/' => out.push('/'),
                        'n' => out.push('\n'),
                        'r' => out.push('\r'),
                        't' => out.push('\t'),
                        other => out.push(other),
                    }
                }
                c => out.push(c),
            }
        }
        Err(JsError::SyntaxError("Unterminated JSON string".to_string()))
    }

    fn parse_array(&mut self) -> Result<JsValue, JsError> {
        self.next(); // consume '['
        self.skip_ws();
        let mut items = Vec::new();
        if self.peek() == Some(']') {
            self.next();
            return Ok(JsValue::Array(items));
        }
        loop {
            let val = self.parse_value()?;
            items.push(val);
            self.skip_ws();
            match self.peek() {
                Some(',') => {
                    self.next();
                    self.skip_ws();
                }
                Some(']') => {
                    self.next();
                    break;
                }
                _ => {
                    return Err(JsError::SyntaxError(
                        "Expected ',' or ']' in JSON array".to_string(),
                    ))
                }
            }
        }
        Ok(JsValue::Array(items))
    }

    fn parse_object(&mut self) -> Result<JsValue, JsError> {
        self.next(); // consume '{'
        self.skip_ws();
        let mut map = BTreeMap::new();
        if self.peek() == Some('}') {
            self.next();
            return Ok(JsValue::Object(map));
        }
        loop {
            self.skip_ws();
            let key = self.parse_string()?;
            self.skip_ws();
            if self.next() != Some(':') {
                return Err(JsError::SyntaxError(
                    "Expected ':' after key in JSON object".to_string(),
                ));
            }
            let val = self.parse_value()?;
            map.insert(key, val);
            self.skip_ws();
            match self.peek() {
                Some(',') => {
                    self.next();
                    self.skip_ws();
                }
                Some('}') => {
                    self.next();
                    break;
                }
                _ => {
                    return Err(JsError::SyntaxError(
                        "Expected ',' or '}' in JSON object".to_string(),
                    ))
                }
            }
        }
        Ok(JsValue::Object(map))
    }
}
