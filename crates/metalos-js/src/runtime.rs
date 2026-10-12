use alloc::collections::BTreeMap;
use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec;
use alloc::vec::Vec;

use crate::ast::{
    ArrowBody, AssignOp, AssignTarget, BinaryOp, Expr, Stmt, UnaryOp, UpdateOp, VarKind,
};
use crate::host::{MetalOsHost, VirtualHostEnv};
use crate::parser::parse_program;
use crate::value::{json_parse, json_stringify, FunctionBody, JsError, JsFunction, JsValue};

/// Default deterministic step limit per script execution.
pub const DEFAULT_MAX_STEPS: usize = 50_000;

const SHELL_FORBIDDEN_MSG: &str = "Shell execution is disabled in metalOS";

#[derive(Debug, Clone)]
struct VarSlot {
    value: JsValue,
    is_const: bool,
}

#[derive(Debug, Clone)]
enum ControlFlow {
    None(JsValue),
    Return(JsValue),
}

/// Isolated JavaScript execution context (`JsIsolate` / `V8Isolate` / `JsRuntime`).
pub struct JsIsolate {
    scopes: Vec<BTreeMap<String, VarSlot>>,
    max_steps: usize,
    steps_executed: usize,
}

/// Alias for `JsIsolate` representing the embedded V8 isolate abstraction.
pub type V8Isolate = JsIsolate;

/// Alias for `JsIsolate` representing the JavaScript runtime.
pub type JsRuntime = JsIsolate;

impl Default for JsIsolate {
    fn default() -> Self {
        Self::new()
    }
}

impl JsIsolate {
    /// Creates a new isolated JavaScript runtime with the default step limit (`50,000`).
    pub fn new() -> Self {
        Self::with_max_steps(DEFAULT_MAX_STEPS)
    }

    /// Creates a new isolated JavaScript runtime with a custom deterministic step limit.
    pub fn with_max_steps(max_steps: usize) -> Self {
        Self {
            scopes: vec![BTreeMap::new()],
            max_steps,
            steps_executed: 0,
        }
    }

    /// Returns the number of AST evaluation steps executed in the last run.
    pub fn steps_executed(&self) -> usize {
        self.steps_executed
    }

    /// Returns the configured maximum execution steps.
    pub fn max_steps(&self) -> usize {
        self.max_steps
    }

    /// Updates the maximum execution steps (fuel limit).
    pub fn set_max_steps(&mut self, max_steps: usize) {
        self.max_steps = max_steps;
    }

    /// Defines or overwrites a global variable in the isolate's root scope.
    pub fn set_global(&mut self, name: &str, value: JsValue) {
        if let Some(global) = self.scopes.first_mut() {
            global.insert(
                name.to_string(),
                VarSlot {
                    value,
                    is_const: false,
                },
            );
        }
    }

    /// Reads a global variable from the isolate's root scope.
    pub fn get_global(&self, name: &str) -> Option<&JsValue> {
        self.scopes.first().and_then(|g| g.get(name).map(|s| &s.value))
    }

    /// Evaluates a JavaScript source string using a default in-memory `VirtualHostEnv`.
    pub fn eval(&mut self, source: &str) -> Result<JsValue, JsError> {
        let mut dummy_host = VirtualHostEnv::new();
        self.eval_with_host(source, &mut dummy_host)
    }

    /// Evaluates a JavaScript source string with capability-scoped `MetalOS.*` bindings
    /// backed by `host`.
    pub fn eval_with_host(
        &mut self,
        source: &str,
        host: &mut dyn MetalOsHost,
    ) -> Result<JsValue, JsError> {
        self.steps_executed = 0;
        let program = parse_program(source)?;

        // Hoist top-level function declarations before statement execution
        self.hoist_functions(&program);

        let mut last_val = JsValue::Undefined;
        for stmt in &program {
            match self.exec_stmt(stmt, host)? {
                ControlFlow::None(v) => last_val = v,
                ControlFlow::Return(v) => return Ok(v),
            }
        }
        Ok(last_val)
    }

    fn tick_step(&mut self) -> Result<(), JsError> {
        self.steps_executed = self.steps_executed.saturating_add(1);
        if self.steps_executed > self.max_steps {
            Err(JsError::ExecutionLimitExceeded {
                max_steps: self.max_steps,
            })
        } else {
            Ok(())
        }
    }

    fn hoist_functions(&mut self, stmts: &[Stmt]) {
        for stmt in stmts {
            if let Stmt::FunctionDecl { name, params, body } = stmt {
                let func = JsValue::Function(JsFunction {
                    name: Some(name.clone()),
                    params: params.clone(),
                    body: FunctionBody::Block(body.clone()),
                    captured_env: self.capture_environment(),
                });
                self.define_var(name.clone(), func, false);
            }
        }
    }

    fn capture_environment(&self) -> BTreeMap<String, JsValue> {
        let mut env = BTreeMap::new();
        for scope in self.scopes.iter().skip(1) {
            for (k, slot) in scope {
                env.insert(k.clone(), slot.value.clone());
            }
        }
        env
    }

    fn push_scope(&mut self) {
        self.scopes.push(BTreeMap::new());
    }

    fn pop_scope(&mut self) {
        if self.scopes.len() > 1 {
            self.scopes.pop();
        }
    }

    fn define_var(&mut self, name: String, value: JsValue, is_const: bool) {
        if let Some(current) = self.scopes.last_mut() {
            current.insert(name, VarSlot { value, is_const });
        }
    }

    fn lookup_var(&self, name: &str) -> Result<JsValue, JsError> {
        if is_forbidden_shell_ident(name) {
            return Err(JsError::ForbiddenCapability(
                SHELL_FORBIDDEN_MSG.to_string(),
            ));
        }
        for scope in self.scopes.iter().rev() {
            if let Some(slot) = scope.get(name) {
                return Ok(slot.value.clone());
            }
        }
        Err(JsError::ReferenceError(format!(
            "'{}' is not defined",
            name
        )))
    }

    fn assign_var(&mut self, name: &str, value: JsValue) -> Result<(), JsError> {
        if is_forbidden_shell_ident(name) {
            return Err(JsError::ForbiddenCapability(
                SHELL_FORBIDDEN_MSG.to_string(),
            ));
        }
        for scope in self.scopes.iter_mut().rev() {
            if let Some(slot) = scope.get_mut(name) {
                if slot.is_const {
                    return Err(JsError::TypeError(format!(
                        "Assignment to constant variable '{}'",
                        name
                    )));
                }
                slot.value = value;
                return Ok(());
            }
        }
        Err(JsError::ReferenceError(format!(
            "'{}' is not defined",
            name
        )))
    }

    fn exec_stmt(
        &mut self,
        stmt: &Stmt,
        host: &mut dyn MetalOsHost,
    ) -> Result<ControlFlow, JsError> {
        self.tick_step()?;
        match stmt {
            Stmt::VarDecl { kind, name, init } => {
                if is_forbidden_shell_ident(name) {
                    return Err(JsError::ForbiddenCapability(
                        SHELL_FORBIDDEN_MSG.to_string(),
                    ));
                }
                let val = match init {
                    Some(expr) => self.eval_expr(expr, host)?,
                    None => JsValue::Undefined,
                };
                self.define_var(name.clone(), val, *kind == VarKind::Const);
                Ok(ControlFlow::None(JsValue::Undefined))
            }
            Stmt::ExprStmt(expr) => {
                let val = self.eval_expr(expr, host)?;
                Ok(ControlFlow::None(val))
            }
            Stmt::Block(stmts) => self.exec_block(stmts, host),
            Stmt::If {
                cond,
                then_branch,
                else_branch,
            } => {
                let cond_val = self.eval_expr(cond, host)?;
                if cond_val.is_truthy() {
                    self.exec_block(then_branch, host)
                } else if let Some(else_stmts) = else_branch {
                    self.exec_block(else_stmts, host)
                } else {
                    Ok(ControlFlow::None(JsValue::Undefined))
                }
            }
            Stmt::While { cond, body } => {
                let mut last = JsValue::Undefined;
                loop {
                    self.tick_step()?;
                    let cond_val = self.eval_expr(cond, host)?;
                    if !cond_val.is_truthy() {
                        break;
                    }
                    match self.exec_block(body, host)? {
                        ControlFlow::None(v) => last = v,
                        ControlFlow::Return(v) => return Ok(ControlFlow::Return(v)),
                    }
                }
                Ok(ControlFlow::None(last))
            }
            Stmt::For {
                init,
                cond,
                update,
                body,
            } => {
                self.push_scope();
                let result = (|| {
                    if let Some(init_stmt) = init {
                        self.exec_stmt(init_stmt, host)?;
                    }
                    let mut last = JsValue::Undefined;
                    loop {
                        self.tick_step()?;
                        if let Some(cond_expr) = cond {
                            let c = self.eval_expr(cond_expr, host)?;
                            if !c.is_truthy() {
                                break;
                            }
                        }
                        match self.exec_block(body, host)? {
                            ControlFlow::None(v) => last = v,
                            ControlFlow::Return(v) => return Ok(ControlFlow::Return(v)),
                        }
                        if let Some(upd_expr) = update {
                            self.eval_expr(upd_expr, host)?;
                        }
                    }
                    Ok(ControlFlow::None(last))
                })();
                self.pop_scope();
                result
            }
            Stmt::FunctionDecl { name, params, body } => {
                let func = JsValue::Function(JsFunction {
                    name: Some(name.clone()),
                    params: params.clone(),
                    body: FunctionBody::Block(body.clone()),
                    captured_env: self.capture_environment(),
                });
                self.define_var(name.clone(), func, false);
                Ok(ControlFlow::None(JsValue::Undefined))
            }
            Stmt::Return(opt_expr) => {
                let val = match opt_expr {
                    Some(expr) => self.eval_expr(expr, host)?,
                    None => JsValue::Undefined,
                };
                Ok(ControlFlow::Return(val))
            }
        }
    }

    fn exec_block(
        &mut self,
        stmts: &[Stmt],
        host: &mut dyn MetalOsHost,
    ) -> Result<ControlFlow, JsError> {
        self.push_scope();
        self.hoist_functions(stmts);
        let mut last = JsValue::Undefined;
        for stmt in stmts {
            match self.exec_stmt(stmt, host) {
                Ok(ControlFlow::None(v)) => last = v,
                Ok(ControlFlow::Return(v)) => {
                    self.pop_scope();
                    return Ok(ControlFlow::Return(v));
                }
                Err(e) => {
                    self.pop_scope();
                    return Err(e);
                }
            }
        }
        self.pop_scope();
        Ok(ControlFlow::None(last))
    }

    fn eval_expr(&mut self, expr: &Expr, host: &mut dyn MetalOsHost) -> Result<JsValue, JsError> {
        self.tick_step()?;
        match expr {
            Expr::Undefined => Ok(JsValue::Undefined),
            Expr::Null => Ok(JsValue::Null),
            Expr::Bool(b) => Ok(JsValue::Bool(*b)),
            Expr::Number(n) => Ok(JsValue::Number(*n)),
            Expr::String(s) => Ok(JsValue::String(s.clone())),
            Expr::Ident(name) => self.lookup_var(name),
            Expr::ArrayLit(items) => {
                let mut vals = Vec::with_capacity(items.len());
                for item in items {
                    vals.push(self.eval_expr(item, host)?);
                }
                Ok(JsValue::Array(vals))
            }
            Expr::ObjectLit(entries) => {
                let mut map = BTreeMap::new();
                for (k, v_expr) in entries {
                    let v = self.eval_expr(v_expr, host)?;
                    map.insert(k.clone(), v);
                }
                Ok(JsValue::Object(map))
            }
            Expr::FunctionExpr { name, params, body } => Ok(JsValue::Function(JsFunction {
                name: name.clone(),
                params: params.clone(),
                body: FunctionBody::Block(body.clone()),
                captured_env: self.capture_environment(),
            })),
            Expr::ArrowFunction { params, body } => {
                let fn_body = match body {
                    ArrowBody::Expr(e) => FunctionBody::Expr(e.clone()),
                    ArrowBody::Block(b) => FunctionBody::Block(b.clone()),
                };
                Ok(JsValue::Function(JsFunction {
                    name: None,
                    params: params.clone(),
                    body: fn_body,
                    captured_env: self.capture_environment(),
                }))
            }
            Expr::Unary { op, expr: inner } => {
                if *op == UnaryOp::Typeof {
                    if let Expr::Ident(name) = inner.as_ref() {
                        if is_forbidden_shell_ident(name) {
                            return Err(JsError::ForbiddenCapability(
                                SHELL_FORBIDDEN_MSG.to_string(),
                            ));
                        }
                        if let Ok(v) = self.lookup_var(name) {
                            return Ok(JsValue::String(v.type_of().to_string()));
                        } else {
                            return Ok(JsValue::String("undefined".to_string()));
                        }
                    }
                }
                let val = self.eval_expr(inner, host)?;
                match op {
                    UnaryOp::Not => Ok(JsValue::Bool(!val.is_truthy())),
                    UnaryOp::Neg => Ok(JsValue::Number(val.to_number()?.wrapping_neg())),
                    UnaryOp::Pos => Ok(JsValue::Number(val.to_number()?)),
                    UnaryOp::Typeof => Ok(JsValue::String(val.type_of().to_string())),
                }
            }
            Expr::Update { op, target, prefix } => {
                let current = self.read_assign_target(target, host)?.to_number()?;
                let updated = match op {
                    UpdateOp::Inc => current.wrapping_add(1),
                    UpdateOp::Dec => current.wrapping_sub(1),
                };
                self.write_assign_target(target, JsValue::Number(updated), host)?;
                if *prefix {
                    Ok(JsValue::Number(updated))
                } else {
                    Ok(JsValue::Number(current))
                }
            }
            Expr::Binary { op, left, right } => self.eval_binary(*op, left, right, host),
            Expr::Assign { op, target, value } => {
                let rhs = self.eval_expr(value, host)?;
                let final_val = match op {
                    AssignOp::Assign => rhs,
                    AssignOp::AddAssign => {
                        let cur = self.read_assign_target(target, host)?;
                        eval_add(&cur, &rhs)?
                    }
                    AssignOp::SubAssign => {
                        let cur = self.read_assign_target(target, host)?;
                        JsValue::Number(cur.to_number()?.wrapping_sub(rhs.to_number()?))
                    }
                };
                self.write_assign_target(target, final_val.clone(), host)?;
                Ok(final_val)
            }
            Expr::Member { object, property } => {
                // Check for forbidden MetalOS shell properties before evaluating object
                if let Expr::Ident(obj_name) = object.as_ref() {
                    if obj_name == "MetalOS" && is_forbidden_shell_prop(property) {
                        return Err(JsError::ForbiddenCapability(
                            SHELL_FORBIDDEN_MSG.to_string(),
                        ));
                    }
                }
                let obj_val = self.eval_expr(object, host)?;
                self.eval_member_access(&obj_val, property)
            }
            Expr::Index { object, index } => {
                let obj_val = self.eval_expr(object, host)?;
                let idx_val = self.eval_expr(index, host)?;
                self.eval_index_access(&obj_val, &idx_val)
            }
            Expr::Call { callee, args } => self.eval_call(callee, args, host),
        }
    }

    fn read_assign_target(
        &mut self,
        target: &AssignTarget,
        host: &mut dyn MetalOsHost,
    ) -> Result<JsValue, JsError> {
        match target {
            AssignTarget::Ident(name) => self.lookup_var(name),
            AssignTarget::Member { object, property } => {
                let obj = self.eval_expr(object, host)?;
                self.eval_member_access(&obj, property)
            }
            AssignTarget::Index { object, index } => {
                let obj = self.eval_expr(object, host)?;
                let idx = self.eval_expr(index, host)?;
                self.eval_index_access(&obj, &idx)
            }
        }
    }

    fn write_assign_target(
        &mut self,
        target: &AssignTarget,
        value: JsValue,
        host: &mut dyn MetalOsHost,
    ) -> Result<(), JsError> {
        match target {
            AssignTarget::Ident(name) => self.assign_var(name, value),
            AssignTarget::Member { object, property } => {
                if let Expr::Ident(var_name) = object.as_ref() {
                    let mut container = self.lookup_var(var_name)?;
                    match &mut container {
                        JsValue::Object(map) => {
                            map.insert(property.clone(), value);
                        }
                        _ => {
                            return Err(JsError::TypeError(format!(
                                "Cannot set property '{}' on non-object",
                                property
                            )))
                        }
                    }
                    self.assign_var(var_name, container)
                } else {
                    Err(JsError::TypeError(
                        "Nested member assignment requires an identifier receiver".to_string(),
                    ))
                }
            }
            AssignTarget::Index { object, index } => {
                let idx_val = self.eval_expr(index, host)?;
                if let Expr::Ident(var_name) = object.as_ref() {
                    let mut container = self.lookup_var(var_name)?;
                    match (&mut container, idx_val) {
                        (JsValue::Array(items), JsValue::Number(i)) => {
                            if i < 0 {
                                return Err(JsError::TypeError(
                                    "Negative array index assignment".to_string(),
                                ));
                            }
                            let idx = i as usize;
                            if idx >= items.len() {
                                items.resize(idx + 1, JsValue::Undefined);
                            }
                            items[idx] = value;
                        }
                        (JsValue::Object(map), key_val) => {
                            map.insert(key_val.to_js_string(), value);
                        }
                        _ => {
                            return Err(JsError::TypeError(
                                "Invalid index assignment target".to_string(),
                            ))
                        }
                    }
                    self.assign_var(var_name, container)
                } else {
                    Err(JsError::TypeError(
                        "Index assignment requires an identifier receiver".to_string(),
                    ))
                }
            }
        }
    }

    fn eval_member_access(&self, obj: &JsValue, property: &str) -> Result<JsValue, JsError> {
        match obj {
            JsValue::Array(items) => {
                if property == "length" {
                    Ok(JsValue::Number(items.len() as i64))
                } else {
                    Ok(JsValue::Undefined)
                }
            }
            JsValue::String(s) => {
                if property == "length" {
                    Ok(JsValue::Number(s.chars().count() as i64))
                } else {
                    Ok(JsValue::Undefined)
                }
            }
            JsValue::Object(map) => Ok(map.get(property).cloned().unwrap_or(JsValue::Undefined)),
            JsValue::Undefined | JsValue::Null => Err(JsError::TypeError(format!(
                "Cannot read property '{}' of {:?}",
                property, obj
            ))),
            _ => Ok(JsValue::Undefined),
        }
    }

    fn eval_index_access(&self, obj: &JsValue, idx: &JsValue) -> Result<JsValue, JsError> {
        match (obj, idx) {
            (JsValue::Array(items), JsValue::Number(i)) => {
                if *i < 0 {
                    Ok(JsValue::Undefined)
                } else {
                    Ok(items.get(*i as usize).cloned().unwrap_or(JsValue::Undefined))
                }
            }
            (JsValue::String(s), JsValue::Number(i)) => {
                if *i < 0 {
                    Ok(JsValue::Undefined)
                } else {
                    Ok(s.chars()
                        .nth(*i as usize)
                        .map(|c| JsValue::String(c.to_string()))
                        .unwrap_or(JsValue::Undefined))
                }
            }
            (JsValue::Object(map), key) => {
                let k = key.to_js_string();
                Ok(map.get(&k).cloned().unwrap_or(JsValue::Undefined))
            }
            (JsValue::Undefined | JsValue::Null, _) => Err(JsError::TypeError(
                "Cannot index into undefined or null".to_string(),
            )),
            _ => Ok(JsValue::Undefined),
        }
    }

    fn eval_binary(
        &mut self,
        op: BinaryOp,
        left: &Expr,
        right: &Expr,
        host: &mut dyn MetalOsHost,
    ) -> Result<JsValue, JsError> {
        // Short-circuit logical operators
        if op == BinaryOp::And {
            let l = self.eval_expr(left, host)?;
            if !l.is_truthy() {
                return Ok(l);
            }
            return self.eval_expr(right, host);
        }
        if op == BinaryOp::Or {
            let l = self.eval_expr(left, host)?;
            if l.is_truthy() {
                return Ok(l);
            }
            return self.eval_expr(right, host);
        }

        let l = self.eval_expr(left, host)?;
        let r = self.eval_expr(right, host)?;

        match op {
            BinaryOp::Add => eval_add(&l, &r),
            BinaryOp::Sub => Ok(JsValue::Number(
                l.to_number()?.wrapping_sub(r.to_number()?),
            )),
            BinaryOp::Mul => Ok(JsValue::Number(
                l.to_number()?.wrapping_mul(r.to_number()?),
            )),
            BinaryOp::Div => {
                let denom = r.to_number()?;
                if denom == 0 {
                    return Err(JsError::TypeError("Division by zero".to_string()));
                }
                Ok(JsValue::Number(l.to_number()?.wrapping_div(denom)))
            }
            BinaryOp::Rem => {
                let denom = r.to_number()?;
                if denom == 0 {
                    return Err(JsError::TypeError("Modulo by zero".to_string()));
                }
                Ok(JsValue::Number(l.to_number()?.wrapping_rem(denom)))
            }
            BinaryOp::EqEq => Ok(JsValue::Bool(l.abstract_eq(&r))),
            BinaryOp::NotEq => Ok(JsValue::Bool(!l.abstract_eq(&r))),
            BinaryOp::StrictEq => Ok(JsValue::Bool(l.strict_eq(&r))),
            BinaryOp::StrictNotEq => Ok(JsValue::Bool(!l.strict_eq(&r))),
            BinaryOp::Lt => match (&l, &r) {
                (JsValue::String(a), JsValue::String(b)) => Ok(JsValue::Bool(a < b)),
                _ => Ok(JsValue::Bool(l.to_number()? < r.to_number()?)),
            },
            BinaryOp::Le => match (&l, &r) {
                (JsValue::String(a), JsValue::String(b)) => Ok(JsValue::Bool(a <= b)),
                _ => Ok(JsValue::Bool(l.to_number()? <= r.to_number()?)),
            },
            BinaryOp::Gt => match (&l, &r) {
                (JsValue::String(a), JsValue::String(b)) => Ok(JsValue::Bool(a > b)),
                _ => Ok(JsValue::Bool(l.to_number()? > r.to_number()?)),
            },
            BinaryOp::Ge => match (&l, &r) {
                (JsValue::String(a), JsValue::String(b)) => Ok(JsValue::Bool(a >= b)),
                _ => Ok(JsValue::Bool(l.to_number()? >= r.to_number()?)),
            },
            BinaryOp::And | BinaryOp::Or => unreachable!(),
        }
    }

    fn eval_call(
        &mut self,
        callee: &Expr,
        arg_exprs: &[Expr],
        host: &mut dyn MetalOsHost,
    ) -> Result<JsValue, JsError> {
        // 1. Check direct identifier calls (including forbidden shell functions and built-ins)
        if let Expr::Ident(name) = callee {
            if is_forbidden_shell_ident(name) {
                return Err(JsError::ForbiddenCapability(
                    SHELL_FORBIDDEN_MSG.to_string(),
                ));
            }
            if name == "parseInt" {
                let mut args = Vec::with_capacity(arg_exprs.len());
                for a in arg_exprs {
                    args.push(self.eval_expr(a, host)?);
                }
                let first = args.first().unwrap_or(&JsValue::Undefined);
                return Ok(JsValue::Number(first.to_number()?));
            }
        }

        // 2. Check namespace / host calls (`MetalOS.*`, `JSON.*`, `Object.*`, `console.log`)
        if let Some(host_res) = self.try_eval_builtin_namespace_call(callee, arg_exprs, host)? {
            return Ok(host_res);
        }

        // 3. Check method calls on a receiver (`receiver.method(args)`)
        if let Expr::Member { object, property } = callee {
            let mut args = Vec::with_capacity(arg_exprs.len());
            for a in arg_exprs {
                args.push(self.eval_expr(a, host)?);
            }
            let receiver_val = self.eval_expr(object, host)?;

            // If `receiver_val` is an Object that actually has a Function property with that name, call it
            if let JsValue::Object(map) = &receiver_val {
                if let Some(JsValue::Function(func)) = map.get(property) {
                    return self.invoke_js_function(func.clone(), &args, host);
                }
            }

            return self.eval_value_method(object, receiver_val, property, &args, host);
        }

        // 4. General callable expression
        let callee_val = self.eval_expr(callee, host)?;
        let mut args = Vec::with_capacity(arg_exprs.len());
        for a in arg_exprs {
            args.push(self.eval_expr(a, host)?);
        }

        match callee_val {
            JsValue::Function(func) => self.invoke_js_function(func, &args, host),
            other => Err(JsError::TypeError(format!(
                "'{}' is not a function",
                other.to_js_string()
            ))),
        }
    }

    fn try_eval_builtin_namespace_call(
        &mut self,
        callee: &Expr,
        arg_exprs: &[Expr],
        host: &mut dyn MetalOsHost,
    ) -> Result<Option<JsValue>, JsError> {
        // Two-level member call: e.g. `MetalOS.log(...)`, `JSON.stringify(...)`, `JSON.parse(...)`, `console.log(...)`
        if let Expr::Member { object, property } = callee {
            if let Expr::Ident(ns) = object.as_ref() {
                match ns.as_str() {
                    "MetalOS" => {
                        if is_forbidden_shell_prop(property) {
                            return Err(JsError::ForbiddenCapability(
                                SHELL_FORBIDDEN_MSG.to_string(),
                            ));
                        }
                        let mut args = Vec::with_capacity(arg_exprs.len());
                        for a in arg_exprs {
                            args.push(self.eval_expr(a, host)?);
                        }
                        return match property.as_str() {
                            "log" => {
                                let msg = args
                                    .iter()
                                    .map(|v| v.to_js_string())
                                    .collect::<Vec<_>>()
                                    .join(" ");
                                host.log(&msg);
                                Ok(Some(JsValue::Undefined))
                            }
                            other => Err(JsError::TypeError(format!(
                                "Unknown MetalOS method 'MetalOS.{}'",
                                other
                            ))),
                        };
                    }
                    "console" if property == "log" => {
                        let mut args = Vec::with_capacity(arg_exprs.len());
                        for a in arg_exprs {
                            args.push(self.eval_expr(a, host)?);
                        }
                        let msg = args
                            .iter()
                            .map(|v| v.to_js_string())
                            .collect::<Vec<_>>()
                            .join(" ");
                        host.log(&msg);
                        return Ok(Some(JsValue::Undefined));
                    }
                    "JSON" => {
                        let mut args = Vec::with_capacity(arg_exprs.len());
                        for a in arg_exprs {
                            args.push(self.eval_expr(a, host)?);
                        }
                        return match property.as_str() {
                            "stringify" => {
                                let first = args.first().unwrap_or(&JsValue::Undefined);
                                Ok(Some(JsValue::String(json_stringify(first))))
                            }
                            "parse" => {
                                let first = args.first().unwrap_or(&JsValue::Undefined);
                                let s = first.to_js_string();
                                Ok(Some(json_parse(&s)?))
                            }
                            other => Err(JsError::TypeError(format!(
                                "JSON.{} is not a function",
                                other
                            ))),
                        };
                    }
                    "Object" => {
                        let mut args = Vec::with_capacity(arg_exprs.len());
                        for a in arg_exprs {
                            args.push(self.eval_expr(a, host)?);
                        }
                        return match property.as_str() {
                            "keys" => {
                                if let Some(JsValue::Object(map)) = args.first() {
                                    let keys =
                                        map.keys().cloned().map(JsValue::String).collect();
                                    Ok(Some(JsValue::Array(keys)))
                                } else {
                                    Ok(Some(JsValue::Array(Vec::new())))
                                }
                            }
                            "values" => {
                                if let Some(JsValue::Object(map)) = args.first() {
                                    let vals = map.values().cloned().collect();
                                    Ok(Some(JsValue::Array(vals)))
                                } else {
                                    Ok(Some(JsValue::Array(Vec::new())))
                                }
                            }
                            other => Err(JsError::TypeError(format!(
                                "Object.{} is not a function",
                                other
                            ))),
                        };
                    }
                    _ => {}
                }
            }

            // Three-level member call: `MetalOS.<subsystem>.<method>(...)`
            if let Expr::Member {
                object: root_obj,
                property: subsystem,
            } = object.as_ref()
            {
                if let Expr::Ident(root_name) = root_obj.as_ref() {
                    if root_name == "MetalOS" {
                        if is_forbidden_shell_prop(subsystem) || is_forbidden_shell_prop(property) {
                            return Err(JsError::ForbiddenCapability(
                                SHELL_FORBIDDEN_MSG.to_string(),
                            ));
                        }
                        let mut args = Vec::with_capacity(arg_exprs.len());
                        for a in arg_exprs {
                            args.push(self.eval_expr(a, host)?);
                        }
                        let val =
                            self.eval_metalos_host_call(subsystem, property, &args, host)?;
                        return Ok(Some(val));
                    }
                }
            }
        }
        Ok(None)
    }

    fn eval_metalos_host_call(
        &mut self,
        subsystem: &str,
        method: &str,
        args: &[JsValue],
        host: &mut dyn MetalOsHost,
    ) -> Result<JsValue, JsError> {
        match (subsystem, method) {
            ("fs", "read") => {
                let path = args
                    .first()
                    .ok_or_else(|| {
                        JsError::TypeError("MetalOS.fs.read requires a path argument".to_string())
                    })?
                    .to_js_string();
                match host.fs_read(&path) {
                    Some(content) => Ok(JsValue::String(content)),
                    None => Ok(JsValue::Null),
                }
            }
            ("fs", "write") => {
                if args.len() < 2 {
                    return Err(JsError::TypeError(
                        "MetalOS.fs.write requires (path, content) arguments".to_string(),
                    ));
                }
                let path = args[0].to_js_string();
                let content = args[1].to_js_string();
                host.fs_write(&path, &content)
                    .map_err(JsError::HostError)?;
                Ok(JsValue::Bool(true))
            }
            ("fs", "delete") => {
                let path = args
                    .first()
                    .ok_or_else(|| {
                        JsError::TypeError(
                            "MetalOS.fs.delete requires a path argument".to_string(),
                        )
                    })?
                    .to_js_string();
                let deleted = host.fs_delete(&path).map_err(JsError::HostError)?;
                Ok(JsValue::Bool(deleted))
            }
            ("fs", "list") => {
                let dir_path = args
                    .first()
                    .map(|v| v.to_js_string())
                    .unwrap_or_else(|| "/".to_string());
                let entries = host
                    .fs_list(&dir_path)
                    .into_iter()
                    .map(JsValue::String)
                    .collect();
                Ok(JsValue::Array(entries))
            }
            ("net", "fetch") => {
                let url = args
                    .first()
                    .ok_or_else(|| {
                        JsError::TypeError("MetalOS.net.fetch requires a url argument".to_string())
                    })?
                    .to_js_string();
                let body = host.net_fetch(&url).map_err(JsError::HostError)?;
                Ok(JsValue::String(body))
            }
            ("sys", "status") => {
                let st = host.sys_status();
                let mut map = BTreeMap::new();
                map.insert("uptimeMs".to_string(), JsValue::Number(st.uptime_ms as i64));
                map.insert(
                    "memUsedBytes".to_string(),
                    JsValue::Number(st.mem_used_bytes as i64),
                );
                map.insert(
                    "memFreeBytes".to_string(),
                    JsValue::Number(st.mem_free_bytes as i64),
                );
                map.insert(
                    "activeTasks".to_string(),
                    JsValue::Number(st.active_tasks as i64),
                );
                Ok(JsValue::Object(map))
            }
            ("ui", "notify") => {
                let msg = args
                    .first()
                    .ok_or_else(|| {
                        JsError::TypeError(
                            "MetalOS.ui.notify requires a message argument".to_string(),
                        )
                    })?
                    .to_js_string();
                host.ui_notify(&msg);
                Ok(JsValue::Bool(true))
            }
            _ => Err(JsError::TypeError(format!(
                "Unknown host capability 'MetalOS.{}.{}'",
                subsystem, method
            ))),
        }
    }

    fn eval_value_method(
        &mut self,
        receiver_expr: &Expr,
        receiver_val: JsValue,
        method: &str,
        args: &[JsValue],
        host: &mut dyn MetalOsHost,
    ) -> Result<JsValue, JsError> {
        match receiver_val {
            JsValue::Array(mut items) => match method {
                "push" => {
                    for arg in args {
                        items.push(arg.clone());
                    }
                    let new_len = items.len() as i64;
                    if let Expr::Ident(var_name) = receiver_expr {
                        self.assign_var(var_name, JsValue::Array(items))?;
                    }
                    Ok(JsValue::Number(new_len))
                }
                "pop" => {
                    let popped = items.pop().unwrap_or(JsValue::Undefined);
                    if let Expr::Ident(var_name) = receiver_expr {
                        self.assign_var(var_name, JsValue::Array(items))?;
                    }
                    Ok(popped)
                }
                "join" => {
                    let sep = args
                        .first()
                        .map(|v| v.to_js_string())
                        .unwrap_or_else(|| ",".to_string());
                    let joined = items
                        .iter()
                        .map(|v| v.to_js_string())
                        .collect::<Vec<_>>()
                        .join(&sep);
                    Ok(JsValue::String(joined))
                }
                "includes" => {
                    let needle = args.first().unwrap_or(&JsValue::Undefined);
                    Ok(JsValue::Bool(items.iter().any(|x| x == needle)))
                }
                "indexOf" => {
                    let needle = args.first().unwrap_or(&JsValue::Undefined);
                    let idx = items
                        .iter()
                        .position(|x| x == needle)
                        .map(|i| i as i64)
                        .unwrap_or(-1);
                    Ok(JsValue::Number(idx))
                }
                "slice" => {
                    let len = items.len() as i64;
                    let start = args
                        .first()
                        .and_then(|v| v.to_number().ok())
                        .unwrap_or(0)
                        .clamp(0, len) as usize;
                    let end = args
                        .get(1)
                        .and_then(|v| v.to_number().ok())
                        .unwrap_or(len)
                        .clamp(start as i64, len) as usize;
                    Ok(JsValue::Array(items[start..end].to_vec()))
                }
                "map" => {
                    let Some(JsValue::Function(cb)) = args.first() else {
                        return Err(JsError::TypeError(
                            "Array.prototype.map requires a callback function".to_string(),
                        ));
                    };
                    let mut mapped = Vec::with_capacity(items.len());
                    for (idx, item) in items.iter().enumerate() {
                        let res = self.invoke_js_function(
                            cb.clone(),
                            &[item.clone(), JsValue::Number(idx as i64)],
                            host,
                        )?;
                        mapped.push(res);
                    }
                    Ok(JsValue::Array(mapped))
                }
                "filter" => {
                    let Some(JsValue::Function(cb)) = args.first() else {
                        return Err(JsError::TypeError(
                            "Array.prototype.filter requires a callback function".to_string(),
                        ));
                    };
                    let mut filtered = Vec::new();
                    for (idx, item) in items.iter().enumerate() {
                        let keep = self.invoke_js_function(
                            cb.clone(),
                            &[item.clone(), JsValue::Number(idx as i64)],
                            host,
                        )?;
                        if keep.is_truthy() {
                            filtered.push(item.clone());
                        }
                    }
                    Ok(JsValue::Array(filtered))
                }
                other => Err(JsError::TypeError(format!(
                    "Array.prototype.{} is not a function",
                    other
                ))),
            },
            JsValue::String(s) => match method {
                "includes" => {
                    let sub = args
                        .first()
                        .map(|v| v.to_js_string())
                        .unwrap_or_default();
                    Ok(JsValue::Bool(s.contains(&sub)))
                }
                "indexOf" => {
                    let sub = args
                        .first()
                        .map(|v| v.to_js_string())
                        .unwrap_or_default();
                    let pos = s.find(&sub).map(|i| i as i64).unwrap_or(-1);
                    Ok(JsValue::Number(pos))
                }
                "split" => {
                    let sep = args
                        .first()
                        .map(|v| v.to_js_string())
                        .unwrap_or_default();
                    let parts = if sep.is_empty() {
                        s.chars().map(|c| JsValue::String(c.to_string())).collect()
                    } else {
                        s.split(&sep)
                            .map(|p| JsValue::String(p.to_string()))
                            .collect()
                    };
                    Ok(JsValue::Array(parts))
                }
                "trim" => Ok(JsValue::String(s.trim().to_string())),
                "toUpperCase" => Ok(JsValue::String(s.to_ascii_uppercase())),
                "toLowerCase" => Ok(JsValue::String(s.to_ascii_lowercase())),
                "startsWith" => {
                    let prefix = args
                        .first()
                        .map(|v| v.to_js_string())
                        .unwrap_or_default();
                    Ok(JsValue::Bool(s.starts_with(&prefix)))
                }
                "endsWith" => {
                    let suffix = args
                        .first()
                        .map(|v| v.to_js_string())
                        .unwrap_or_default();
                    Ok(JsValue::Bool(s.ends_with(&suffix)))
                }
                "replace" => {
                    let from = args
                        .first()
                        .map(|v| v.to_js_string())
                        .unwrap_or_default();
                    let to = args
                        .get(1)
                        .map(|v| v.to_js_string())
                        .unwrap_or_default();
                    Ok(JsValue::String(s.replacen(&from, &to, 1)))
                }
                "substring" | "slice" => {
                    let chars: Vec<char> = s.chars().collect();
                    let len = chars.len() as i64;
                    let start = args
                        .first()
                        .and_then(|v| v.to_number().ok())
                        .unwrap_or(0)
                        .clamp(0, len) as usize;
                    let end = args
                        .get(1)
                        .and_then(|v| v.to_number().ok())
                        .unwrap_or(len)
                        .clamp(start as i64, len) as usize;
                    let sub: String = chars[start..end].iter().collect();
                    Ok(JsValue::String(sub))
                }
                other => Err(JsError::TypeError(format!(
                    "String.prototype.{} is not a function",
                    other
                ))),
            },
            other => Err(JsError::TypeError(format!(
                "Property '{}' is not callable on {:?}",
                method, other
            ))),
        }
    }

    fn invoke_js_function(
        &mut self,
        func: JsFunction,
        args: &[JsValue],
        host: &mut dyn MetalOsHost,
    ) -> Result<JsValue, JsError> {
        self.tick_step()?;
        self.push_scope();

        // Populate captured lexical environment for variables not already in active local scopes
        for (k, v) in &func.captured_env {
            let exists_in_outer = self.scopes.iter().skip(1).any(|s| s.contains_key(k));
            if !exists_in_outer {
                self.define_var(k.clone(), v.clone(), false);
            }
        }

        // Bind named function to its own scope for recursion
        if let Some(ref fn_name) = func.name {
            self.define_var(fn_name.clone(), JsValue::Function(func.clone()), false);
        }

        // Bind positional parameters
        for (i, param) in func.params.iter().enumerate() {
            let arg_val = args.get(i).cloned().unwrap_or(JsValue::Undefined);
            self.define_var(param.clone(), arg_val, false);
        }

        let res = match &func.body {
            FunctionBody::Expr(expr) => self.eval_expr(expr, host),
            FunctionBody::Block(stmts) => {
                self.hoist_functions(stmts);
                let mut ret = JsValue::Undefined;
                let mut err = None;
                for stmt in stmts {
                    match self.exec_stmt(stmt, host) {
                        Ok(ControlFlow::None(_)) => {}
                        Ok(ControlFlow::Return(v)) => {
                            ret = v;
                            break;
                        }
                        Err(e) => {
                            err = Some(e);
                            break;
                        }
                    }
                }
                if let Some(e) = err {
                    Err(e)
                } else {
                    Ok(ret)
                }
            }
        };

        self.pop_scope();
        res
    }
}

fn eval_add(l: &JsValue, r: &JsValue) -> Result<JsValue, JsError> {
    if matches!(l, JsValue::String(_)) || matches!(r, JsValue::String(_)) {
        let mut s = l.to_js_string();
        s.push_str(&r.to_js_string());
        Ok(JsValue::String(s))
    } else {
        Ok(JsValue::Number(
            l.to_number()?.wrapping_add(r.to_number()?),
        ))
    }
}

fn is_forbidden_shell_ident(name: &str) -> bool {
    matches!(name, "sh" | "exec" | "spawn" | "child_process" | "system")
}

fn is_forbidden_shell_prop(prop: &str) -> bool {
    matches!(
        prop,
        "sh" | "exec" | "spawn" | "child_process" | "shell" | "system"
    )
}
