use alloc::collections::{BTreeMap, VecDeque};
use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec;
use alloc::vec::Vec;

use crate::ast::{
    ArrowBody, AssignOp, AssignTarget, BinaryOp, Expr, Stmt, UnaryOp, UpdateOp, VarKind,
};
use crate::host::{CapabilityRequest, AlloyOsHost, VirtualHostEnv};
use crate::parser::parse_program;
use crate::value::{json_parse, json_stringify, FunctionBody, JsError, JsFunction, JsValue};

/// Default deterministic step limit per script execution.
pub const DEFAULT_MAX_STEPS: usize = 50_000;

const SHELL_FORBIDDEN_MSG: &str = "Shell execution is disabled in alloyOS";

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

#[derive(Debug, Clone, PartialEq, Eq)]
enum PromiseState {
    Pending,
    Fulfilled(JsValue),
    Rejected(JsValue),
}

#[derive(Debug, Clone)]
struct PromiseReaction {
    on_fulfilled: Option<JsFunction>,
    on_rejected: Option<JsFunction>,
    child_promise_id: usize,
}

#[derive(Debug, Clone)]
struct PromiseRecord {
    state: PromiseState,
    reactions: Vec<PromiseReaction>,
    handled: bool,
}

#[derive(Debug, Clone)]
struct Microtask {
    handler: Option<JsFunction>,
    is_rejection: bool,
    arg: JsValue,
    child_promise_id: usize,
}

/// Isolated JavaScript execution context (`JsIsolate` / `V8Isolate` / `JsRuntime`).
pub struct JsIsolate {
    scopes: Vec<BTreeMap<String, VarSlot>>,
    max_steps: usize,
    steps_executed: usize,
    allow_alloyos_bindings: bool,
    printed: Vec<String>,
    pending_exception: Option<JsValue>,
    promises: Vec<PromiseRecord>,
    microtasks: VecDeque<Microtask>,
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
            allow_alloyos_bindings: true,
            printed: Vec::new(),
            pending_exception: None,
            promises: Vec::new(),
            microtasks: VecDeque::new(),
        }
    }

    /// Creates a pure ECMAScript isolate for untrusted web/smoke scripts with `AlloyOS.*` disabled.
    pub fn new_isolated_web(max_steps: usize) -> Self {
        let mut isolate = Self::with_max_steps(max_steps);
        isolate.allow_alloyos_bindings = false;
        isolate
    }

    /// Enables or disables `AlloyOS.*` host namespace bindings in this isolate.
    pub fn set_allow_alloyos_bindings(&mut self, allow: bool) {
        self.allow_alloyos_bindings = allow;
    }

    /// Returns whether `AlloyOS.*` host namespace bindings are enabled in this isolate.
    pub fn allows_alloyos_bindings(&self) -> bool {
        self.allow_alloyos_bindings
    }

    /// Returns lines emitted via the global `print(...)` function.
    pub fn printed_output(&self) -> &[String] {
        &self.printed
    }

    /// Drains and returns all lines emitted via the global `print(...)` function.
    pub fn drain_printed_output(&mut self) -> Vec<String> {
        core::mem::take(&mut self.printed)
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

    /// Evaluates a JavaScript source string with capability-scoped `AlloyOS.*` bindings
    /// backed by `host`.
    pub fn eval_with_host(
        &mut self,
        source: &str,
        host: &mut dyn AlloyOsHost,
    ) -> Result<JsValue, JsError> {
        self.evaluate_source(source, host, true)
    }

    fn evaluate_source(
        &mut self,
        source: &str,
        host: &mut dyn AlloyOsHost,
        run_main: bool,
    ) -> Result<JsValue, JsError> {
        self.steps_executed = 0;
        self.pending_exception = None;
        let program = parse_program(source)?;

        // Hoist top-level function declarations before statement execution
        self.hoist_functions(&program);

        let mut last_val = JsValue::Undefined;
        for stmt in &program {
            match self.exec_stmt(stmt, host)? {
                ControlFlow::None(v) => last_val = v,
                ControlFlow::Return(v) => {
                    self.drain_microtasks(host)?;
                    return Ok(v);
                }
            }
        }
        if run_main
            && last_val == JsValue::Undefined
            && matches!(program.last(), Some(Stmt::FunctionDecl { name, .. }) if name == "main")
        {
            if let Ok(JsValue::Function(main_fn)) = self.lookup_var("main") {
                let res = self.invoke_js_function(main_fn, &[], host)?;
                self.drain_microtasks(host)?;
                return Ok(res);
            }
        }
        self.drain_microtasks(host)?;
        Ok(last_val)
    }

    /// Evaluates a JavaScript module source (hoisting its functions and top-level bindings)
    /// and invokes the named function `fn_name` with `args`.
    pub fn call_function(
        &mut self,
        source: &str,
        fn_name: &str,
        args: &[JsValue],
        host: &mut dyn AlloyOsHost,
    ) -> Result<JsValue, JsError> {
        self.evaluate_source(source, host, false)?;
        let func_val = self.lookup_var(fn_name)?;
        match func_val {
            JsValue::Function(func) => {
                let res = self.invoke_js_function(func, args, host)?;
                self.drain_microtasks(host)?;
                Ok(res)
            }
            other => Err(JsError::TypeError(format!(
                "'{}' is not a function (found {})",
                fn_name,
                other.type_of()
            ))),
        }
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
        if name == "AlloyOS" && !self.allow_alloyos_bindings {
            return Err(JsError::ReferenceError(
                "'AlloyOS' is not defined".to_string(),
            ));
        }
        for scope in self.scopes.iter().rev() {
            if let Some(slot) = scope.get(name) {
                return Ok(slot.value.clone());
            }
        }
        if is_builtin_constructor(name) {
            return Ok(JsValue::Function(JsFunction {
                name: Some(name.to_string()),
                params: Vec::new(),
                body: FunctionBody::Block(Vec::new()),
                captured_env: BTreeMap::new(),
            }));
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

    fn mutate_var(&mut self, name: &str, value: JsValue) -> Result<(), JsError> {
        if is_forbidden_shell_ident(name) {
            return Err(JsError::ForbiddenCapability(
                SHELL_FORBIDDEN_MSG.to_string(),
            ));
        }
        for scope in self.scopes.iter_mut().rev() {
            if let Some(slot) = scope.get_mut(name) {
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
        host: &mut dyn AlloyOsHost,
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
            Stmt::Throw(expr) => {
                let val = self.eval_expr(expr, host)?;
                let err = js_value_to_js_error(&val);
                self.pending_exception = Some(val);
                Err(err)
            }
            Stmt::Try {
                body,
                catch_param,
                catch_body,
                finally_body,
            } => {
                let body_res = self.exec_block(body, host);
                let outcome = match body_res {
                    Ok(cf) => Ok(cf),
                    Err(err) => {
                        if is_uncatchable_error(&err) {
                            return Err(err);
                        }
                        let caught_val = self
                            .pending_exception
                            .take()
                            .unwrap_or_else(|| js_error_to_js_value(&err));
                        if let Some(c_body) = catch_body {
                            self.push_scope();
                            self.hoist_functions(c_body);
                            if let Some(param) = catch_param {
                                self.define_var(param.clone(), caught_val, false);
                            }
                            let mut last = JsValue::Undefined;
                            let mut catch_res = None;
                            for s in c_body {
                                match self.exec_stmt(s, host) {
                                    Ok(ControlFlow::None(v)) => last = v,
                                    Ok(ControlFlow::Return(v)) => {
                                        catch_res = Some(Ok(ControlFlow::Return(v)));
                                        break;
                                    }
                                    Err(e) => {
                                        catch_res = Some(Err(e));
                                        break;
                                    }
                                }
                            }
                            self.pop_scope();
                            catch_res.unwrap_or(Ok(ControlFlow::None(last)))
                        } else {
                            self.pending_exception = Some(caught_val);
                            Err(err)
                        }
                    }
                };

                if let Some(f_body) = finally_body {
                    match self.exec_block(f_body, host) {
                        Ok(ControlFlow::None(_)) => outcome,
                        Ok(ControlFlow::Return(v)) => {
                            self.pending_exception = None;
                            Ok(ControlFlow::Return(v))
                        }
                        Err(f_err) => Err(f_err),
                    }
                } else {
                    outcome
                }
            }
        }
    }

    fn exec_block(
        &mut self,
        stmts: &[Stmt],
        host: &mut dyn AlloyOsHost,
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

    fn eval_expr(&mut self, expr: &Expr, host: &mut dyn AlloyOsHost) -> Result<JsValue, JsError> {
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
                // Check for forbidden AlloyOS shell properties or isolated web mode before evaluating object
                if let Expr::Ident(obj_name) = object.as_ref() {
                    if obj_name == "AlloyOS" {
                        if !self.allow_alloyos_bindings {
                            return Err(JsError::ReferenceError(
                                "'AlloyOS' is not defined".to_string(),
                            ));
                        }
                        if is_forbidden_shell_prop(property) {
                            return Err(JsError::ForbiddenCapability(
                                SHELL_FORBIDDEN_MSG.to_string(),
                            ));
                        }
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
            Expr::New { callee, args } => self.eval_new(callee, args, host),
        }
    }

    fn read_assign_target(
        &mut self,
        target: &AssignTarget,
        host: &mut dyn AlloyOsHost,
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
        host: &mut dyn AlloyOsHost,
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
                    self.mutate_var(var_name, container)
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
                        (JsValue::Object(map), JsValue::Number(i))
                            if map.get("__class__")
                                == Some(&JsValue::String("Uint8Array".to_string())) =>
                        {
                            if i >= 0 {
                                let byte_val =
                                    JsValue::Number(value.to_number()?.rem_euclid(256));
                                if let Some(JsValue::Array(bytes)) = map.get_mut("__bytes__") {
                                    let idx = i as usize;
                                    if idx < bytes.len() {
                                        bytes[idx] = byte_val;
                                    }
                                }
                            }
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
                    self.mutate_var(var_name, container)
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
            (JsValue::Object(map), JsValue::Number(i))
                if map.get("__class__") == Some(&JsValue::String("Uint8Array".to_string())) =>
            {
                if *i < 0 {
                    Ok(JsValue::Undefined)
                } else if let Some(JsValue::Array(bytes)) = map.get("__bytes__") {
                    Ok(bytes.get(*i as usize).cloned().unwrap_or(JsValue::Undefined))
                } else {
                    Ok(JsValue::Undefined)
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
        host: &mut dyn AlloyOsHost,
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
            BinaryOp::Instanceof => eval_instanceof(&l, &r),
            BinaryOp::And | BinaryOp::Or => unreachable!(),
        }
    }

    fn eval_call(
        &mut self,
        callee: &Expr,
        arg_exprs: &[Expr],
        host: &mut dyn AlloyOsHost,
    ) -> Result<JsValue, JsError> {
        // 1. Check direct identifier calls (including forbidden shell functions and built-ins)
        if let Expr::Ident(name) = callee {
            if is_forbidden_shell_ident(name) {
                return Err(JsError::ForbiddenCapability(
                    SHELL_FORBIDDEN_MSG.to_string(),
                ));
            }
            if name == "print" && !self.is_var_defined_in_scopes("print") {
                let mut parts = Vec::with_capacity(arg_exprs.len());
                for a in arg_exprs {
                    parts.push(self.eval_expr(a, host)?.to_js_string());
                }
                self.printed.push(parts.join(" "));
                return Ok(JsValue::Undefined);
            }
            if matches!(
                name.as_str(),
                "Error" | "SyntaxError" | "TypeError" | "ReferenceError"
            ) && !self.is_var_defined_in_scopes(name)
            {
                let mut args = Vec::with_capacity(arg_exprs.len());
                for a in arg_exprs {
                    args.push(self.eval_expr(a, host)?);
                }
                return self.construct_builtin(name, &args, host);
            }
            if name == "parseInt" {
                let mut args = Vec::with_capacity(arg_exprs.len());
                for a in arg_exprs {
                    args.push(self.eval_expr(a, host)?);
                }
                let first = args.first().unwrap_or(&JsValue::Undefined);
                return Ok(JsValue::Number(first.to_number()?));
            }
            if name == "encodeURIComponent" {
                let value = match arg_exprs.first() {
                    Some(expr) => self.eval_expr(expr, host)?.to_js_string(),
                    None => "undefined".to_string(),
                };
                let mut encoded = String::new();
                for byte in value.bytes() {
                    if byte.is_ascii_alphanumeric() || b"-_.!~*'()".contains(&byte) {
                        encoded.push(byte as char);
                    } else {
                        encoded.push_str(&format!("%{:02X}", byte));
                    }
                }
                return Ok(JsValue::String(encoded));
            }
            if name == "decodeURIComponent" {
                let value = match arg_exprs.first() {
                    Some(expr) => self.eval_expr(expr, host)?.to_js_string(),
                    None => "undefined".to_string(),
                };
                let mut decoded = Vec::new();
                let mut bytes = value.bytes();
                while let Some(byte) = bytes.next() {
                    if byte == b'%' {
                        let hi = bytes.next().and_then(|b| (b as char).to_digit(16));
                        let lo = bytes.next().and_then(|b| (b as char).to_digit(16));
                        match (hi, lo) {
                            (Some(hi), Some(lo)) => decoded.push((hi * 16 + lo) as u8),
                            _ => return Err(JsError::TypeError("Malformed URI component".into())),
                        }
                    } else { decoded.push(byte); }
                }
                return String::from_utf8(decoded).map(JsValue::String)
                    .map_err(|_| JsError::TypeError("Malformed URI component".into()));
            }
        }

        // 2. Check namespace / host calls (`AlloyOS.*`, `JSON.*`, `Object.*`, `console.log`)
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
            JsValue::Function(func) => {
                if func.body == FunctionBody::Block(Vec::new()) {
                    if let Some(ref ctor_name) = func.name {
                        if is_builtin_constructor(ctor_name) {
                            return self.construct_builtin(ctor_name, &args, host);
                        }
                    }
                }
                self.invoke_js_function(func, &args, host)
            }
            other => Err(JsError::TypeError(format!(
                "'{}' is not a function",
                other.to_js_string()
            ))),
        }
    }

    fn eval_new(
        &mut self,
        callee: &Expr,
        arg_exprs: &[Expr],
        host: &mut dyn AlloyOsHost,
    ) -> Result<JsValue, JsError> {
        let mut args = Vec::with_capacity(arg_exprs.len());
        for a in arg_exprs {
            args.push(self.eval_expr(a, host)?);
        }
        if let Expr::Ident(name) = callee {
            if is_builtin_constructor(name) && !self.is_var_defined_in_scopes(name) {
                return self.construct_builtin(name, &args, host);
            }
        }
        let callee_val = self.eval_expr(callee, host)?;
        match callee_val {
            JsValue::Function(func) => {
                if let Some(ref ctor_name) = func.name {
                    if is_builtin_constructor(ctor_name)
                        && func.body == FunctionBody::Block(Vec::new())
                    {
                        return self.construct_builtin(ctor_name, &args, host);
                    }
                }
                let res = self.invoke_js_function(func.clone(), &args, host)?;
                if matches!(res, JsValue::Object(_) | JsValue::Array(_)) {
                    Ok(res)
                } else {
                    let mut map = BTreeMap::new();
                    if let Some(name) = func.name {
                        map.insert("__class__".to_string(), JsValue::String(name));
                    }
                    Ok(JsValue::Object(map))
                }
            }
            other => Err(JsError::TypeError(format!(
                "'{}' is not a constructor",
                other.to_js_string()
            ))),
        }
    }

    fn construct_builtin(
        &mut self,
        ctor_name: &str,
        args: &[JsValue],
        host: &mut dyn AlloyOsHost,
    ) -> Result<JsValue, JsError> {
        match ctor_name {
            "Error" | "SyntaxError" | "TypeError" | "ReferenceError" => {
                let msg = args
                    .first()
                    .filter(|v| !matches!(v, JsValue::Undefined))
                    .map(|v| v.to_js_string())
                    .unwrap_or_default();
                Ok(make_error_object(ctor_name, msg))
            }
            "Map" => make_map_object(args.first()),
            "Uint8Array" => make_uint8_array_object(args.first()),
            "Array" => {
                if args.len() == 1 {
                    if let JsValue::Number(n) = args[0] {
                        if n < 0 {
                            return Err(JsError::TypeError("Invalid array length".to_string()));
                        }
                        return Ok(JsValue::Array(vec![JsValue::Undefined; n as usize]));
                    }
                }
                Ok(JsValue::Array(args.to_vec()))
            }
            "Object" => Ok(args
                .first()
                .cloned()
                .unwrap_or_else(|| JsValue::Object(BTreeMap::new()))),
            "Promise" => {
                let Some(JsValue::Function(_executor)) = args.first() else {
                    return Err(JsError::TypeError(
                        "Promise resolver is not a function".to_string(),
                    ));
                };
                let _ = host;
                Ok(self.promise_resolve(JsValue::Undefined))
            }
            _ => Err(JsError::TypeError(format!(
                "'{}' is not a constructor",
                ctor_name
            ))),
        }
    }

    fn is_var_defined_in_scopes(&self, name: &str) -> bool {
        self.scopes.iter().any(|s| s.contains_key(name))
    }

    fn try_eval_builtin_namespace_call(
        &mut self,
        callee: &Expr,
        arg_exprs: &[Expr],
        host: &mut dyn AlloyOsHost,
    ) -> Result<Option<JsValue>, JsError> {
        // Two-level member call: e.g. `AlloyOS.log(...)`, `JSON.stringify(...)`, `JSON.parse(...)`, `console.log(...)`
        if let Expr::Member { object, property } = callee {
            if let Expr::Ident(ns) = object.as_ref() {
                match ns.as_str() {
                    "AlloyOS" => {
                        if !self.allow_alloyos_bindings {
                            return Err(JsError::ReferenceError(
                                "'AlloyOS' is not defined".to_string(),
                            ));
                        }
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
                            "forge" => {
                                if args.is_empty() || args.len() > 2 {
                                    return Err(JsError::TypeError("AlloyOS.forge requires (prompt, options?)".to_string()));
                                }
                                let prompt = match &args[0] { JsValue::String(s) if !s.is_empty() => s, _ => return Err(JsError::TypeError("Forge prompt must be a nonempty string".to_string())) };
                                let mut model = None;
                                let mut max_tokens = 128;
                                if let Some(options) = args.get(1) {
                                    let fields = match options { JsValue::Object(fields) => fields, _ => return Err(JsError::TypeError("Forge options must be an object".to_string())) };
                                    for (key,value) in fields {
                                        match (key.as_str(),value) {
                                            ("model",JsValue::String(id)) if !id.is_empty() => model = Some(id.as_str()),
                                            ("max_tokens",JsValue::Number(n)) if *n > 0 && *n <= 256 => max_tokens = *n as u32,
                                            _ => return Err(JsError::TypeError("Invalid Forge option".to_string())),
                                        }
                                    }
                                }
                                host.check_capability(&CapabilityRequest::ForgeGenerate)
                                    .map_err(JsError::ForbiddenCapability)?;
                                host.forge(prompt, model, max_tokens).map(|s| Some(JsValue::String(s))).map_err(JsError::TypeError)
                            }
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
                                "Unknown AlloyOS method 'AlloyOS.{}'",
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
                            "tryParse" => {
                                let first = args.first().unwrap_or(&JsValue::Undefined);
                                let s = first.to_js_string();
                                match json_parse(&s) {
                                    Ok(val) => Ok(Some(val)),
                                    Err(_) => Ok(Some(JsValue::Null)),
                                }
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
                                    let keys = map
                                        .keys()
                                        .filter(|k| !k.starts_with("__"))
                                        .cloned()
                                        .map(JsValue::String)
                                        .collect();
                                    Ok(Some(JsValue::Array(keys)))
                                } else {
                                    Ok(Some(JsValue::Array(Vec::new())))
                                }
                            }
                            "values" => {
                                if let Some(JsValue::Object(map)) = args.first() {
                                    let vals = map
                                        .iter()
                                        .filter(|(k, _)| !k.starts_with("__"))
                                        .map(|(_, v)| v.clone())
                                        .collect();
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
                    "Promise" => {
                        let mut args = Vec::with_capacity(arg_exprs.len());
                        for a in arg_exprs {
                            args.push(self.eval_expr(a, host)?);
                        }
                        return match property.as_str() {
                            "resolve" => {
                                let first = args.first().cloned().unwrap_or(JsValue::Undefined);
                                Ok(Some(self.promise_resolve(first)))
                            }
                            "reject" => {
                                let first = args.first().cloned().unwrap_or(JsValue::Undefined);
                                Ok(Some(self.promise_reject(first)))
                            }
                            other => Err(JsError::TypeError(format!(
                                "Promise.{} is not a function",
                                other
                            ))),
                        };
                    }
                    _ => {}
                }
            }

            // Three-level member call: `AlloyOS.<subsystem>.<method>(...)`
            if let Expr::Member {
                object: root_obj,
                property: subsystem,
            } = object.as_ref()
            {
                if let Expr::Ident(root_name) = root_obj.as_ref() {
                    if root_name == "AlloyOS" {
                        if !self.allow_alloyos_bindings {
                            return Err(JsError::ReferenceError(
                                "'AlloyOS' is not defined".to_string(),
                            ));
                        }
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
                            self.eval_alloyos_host_call(subsystem, property, &args, host)?;
                        return Ok(Some(val));
                    }
                }
            }
        }
        Ok(None)
    }

    fn eval_alloyos_host_call(
        &mut self,
        subsystem: &str,
        method: &str,
        args: &[JsValue],
        host: &mut dyn AlloyOsHost,
    ) -> Result<JsValue, JsError> {
        match (subsystem, method) {
            ("fs", "read") => {
                let path = args
                    .first()
                    .ok_or_else(|| {
                        JsError::TypeError("AlloyOS.fs.read requires a path argument".to_string())
                    })?
                    .to_js_string();
                host.check_capability(&CapabilityRequest::FsRead(&path))
                    .map_err(JsError::ForbiddenCapability)?;
                match host.fs_read(&path) {
                    Some(content) => Ok(JsValue::String(content)),
                    None => Ok(JsValue::Null),
                }
            }
            ("fs", "write") => {
                if args.len() < 2 {
                    return Err(JsError::TypeError(
                        "AlloyOS.fs.write requires (path, content) arguments".to_string(),
                    ));
                }
                let path = args[0].to_js_string();
                let content = args[1].to_js_string();
                host.check_capability(&CapabilityRequest::FsWrite(&path))
                    .map_err(JsError::ForbiddenCapability)?;
                host.fs_write(&path, &content)
                    .map_err(JsError::HostError)?;
                Ok(JsValue::Bool(true))
            }
            ("fs", "delete") => {
                let path = args
                    .first()
                    .ok_or_else(|| {
                        JsError::TypeError(
                            "AlloyOS.fs.delete requires a path argument".to_string(),
                        )
                    })?
                    .to_js_string();
                host.check_capability(&CapabilityRequest::FsWrite(&path))
                    .map_err(JsError::ForbiddenCapability)?;
                let deleted = host.fs_delete(&path).map_err(JsError::HostError)?;
                Ok(JsValue::Bool(deleted))
            }
            ("fs", "list") => {
                let dir_path = args
                    .first()
                    .map(|v| v.to_js_string())
                    .unwrap_or_else(|| "/".to_string());
                host.check_capability(&CapabilityRequest::FsRead(&dir_path))
                    .map_err(JsError::ForbiddenCapability)?;
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
                        JsError::TypeError("AlloyOS.net.fetch requires a url argument".to_string())
                    })?
                    .to_js_string();
                host.check_capability(&CapabilityRequest::NetFetch(&url))
                    .map_err(JsError::ForbiddenCapability)?;
                let body = host.net_fetch(&url).map_err(JsError::HostError)?;
                Ok(JsValue::String(body))
            }
            ("net", "post") => {
                let url = args
                    .first()
                    .ok_or_else(|| {
                        JsError::TypeError("AlloyOS.net.post requires a url argument".to_string())
                    })?
                    .to_js_string();
                let body = args
                    .get(1)
                    .map(|v| v.to_js_string())
                    .unwrap_or_default();
                let mut headers = Vec::new();
                if let Some(h_val) = args.get(2) {
                    match h_val {
                        JsValue::Object(map) => {
                            for (k, v) in map {
                                headers.push((k.clone(), v.to_js_string()));
                            }
                        }
                        JsValue::Array(arr) => {
                            for item in arr {
                                if let JsValue::Array(pair) = item {
                                    if pair.len() == 2 {
                                        headers.push((pair[0].to_js_string(), pair[1].to_js_string()));
                                    }
                                }
                            }
                        }
                        _ => {}
                    }
                }
                host.check_capability(&CapabilityRequest::NetPost(&url))
                    .map_err(JsError::ForbiddenCapability)?;
                let resp = host.net_post(&url, &body, &headers).map_err(JsError::HostError)?;
                Ok(JsValue::String(resp))
            }
            // Pure conversion helpers: preferences are supplied by the calling app.
            // These bindings neither read nor persist system or app configuration.
            ("sys", "timeZones") => Ok(JsValue::Array(crate::timezone::SUPPORTED_TIMEZONES.iter()
                .map(|zone| JsValue::String((*zone).into())).collect())),
            ("sys", "localTime") => {
                if args.len() != 3 { return Err(JsError::TypeError("localTime requires (epochMs, timezone, clockFormat)".into())); }
                let ms = match args.first() {
                    Some(JsValue::Number(n)) if *n >= 0 => Some(*n as u64),
                    Some(JsValue::Null) => None,
                    _ => return Err(JsError::TypeError("epochMs must be a nonnegative integer or null".into())),
                };
                let settings = crate::timezone::ClockSettings {
                    timezone: args[1].to_js_string(), clock_format: args[2].to_js_string(),
                };
                Ok(crate::timezone::ClockDisplay::from_settings(Ok(settings), ms).to_value())
            }
            ("sys", "status") => {
                host.check_capability(&CapabilityRequest::SysStatus)
                    .map_err(JsError::ForbiddenCapability)?;
                let st = host.sys_status();
                let mut map = BTreeMap::new();
                map.insert("wallClockMs".to_string(), st.wall_clock_ms.map(|ms| JsValue::Number(ms as i64)).unwrap_or(JsValue::Null));
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
                            "AlloyOS.ui.notify requires a message argument".to_string(),
                        )
                    })?
                    .to_js_string();
                host.check_capability(&CapabilityRequest::UiNotify)
                    .map_err(JsError::ForbiddenCapability)?;
                host.ui_notify(&msg);
                Ok(JsValue::Bool(true))
            }
            ("scheduler", "list") => {
                host.check_capability(&CapabilityRequest::SchedulerAccess)
                    .map_err(JsError::ForbiddenCapability)?;
                host.scheduler_list().map_err(JsError::HostError)
            }
            ("scheduler", "create") => {
                if args.len() < 4 {
                    return Err(JsError::TypeError(
                        "AlloyOS.scheduler.create requires (id, scriptPath, trigger, source) arguments"
                            .to_string(),
                    ));
                }
                let id = args[0].to_js_string();
                let script_path = args[1].to_js_string();
                let trigger = args[2].to_js_string();
                let source = args[3].to_js_string();
                host.check_capability(&CapabilityRequest::SchedulerAccess)
                    .map_err(JsError::ForbiddenCapability)?;
                host.scheduler_create(&id, &script_path, &trigger, &source)
                    .map_err(JsError::HostError)
            }
            ("scheduler", "edit") => {
                if args.len() < 3 {
                    return Err(JsError::TypeError(
                        "AlloyOS.scheduler.edit requires (id, scriptPath, trigger, [source]) arguments"
                            .to_string(),
                    ));
                }
                let id = args[0].to_js_string();
                let script_path = args[1].to_js_string();
                let trigger = args[2].to_js_string();
                let source = args.get(3).map(|v| v.to_js_string()).unwrap_or_default();
                host.check_capability(&CapabilityRequest::SchedulerAccess)
                    .map_err(JsError::ForbiddenCapability)?;
                host.scheduler_edit(&id, &script_path, &trigger, &source)
                    .map_err(JsError::HostError)
            }
            ("scheduler", "delete") => {
                let id = args
                    .first()
                    .ok_or_else(|| {
                        JsError::TypeError(
                            "AlloyOS.scheduler.delete requires an id argument".to_string(),
                        )
                    })?
                    .to_js_string();
                host.check_capability(&CapabilityRequest::SchedulerAccess)
                    .map_err(JsError::ForbiddenCapability)?;
                host.scheduler_delete(&id).map_err(JsError::HostError)
            }
            _ => Err(JsError::TypeError(format!(
                "Unknown host capability 'AlloyOS.{}.{}'",
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
        host: &mut dyn AlloyOsHost,
    ) -> Result<JsValue, JsError> {
        match receiver_val {
            JsValue::Array(mut items) => match method {
                "push" => {
                    for arg in args {
                        items.push(arg.clone());
                    }
                    let new_len = items.len() as i64;
                    if let Expr::Ident(var_name) = receiver_expr {
                        self.mutate_var(var_name, JsValue::Array(items))?;
                    }
                    Ok(JsValue::Number(new_len))
                }
                "pop" => {
                    let popped = items.pop().unwrap_or(JsValue::Undefined);
                    if let Expr::Ident(var_name) = receiver_expr {
                        self.mutate_var(var_name, JsValue::Array(items))?;
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
                "reduce" => self.eval_array_reduce(&items, args, host),
                "forEach" => {
                    let Some(JsValue::Function(cb)) = args.first() else {
                        return Err(JsError::TypeError(
                            "Array.prototype.forEach requires a callback function".to_string(),
                        ));
                    };
                    for (idx, item) in items.iter().enumerate() {
                        self.invoke_js_function(
                            cb.clone(),
                            &[item.clone(), JsValue::Number(idx as i64)],
                            host,
                        )?;
                    }
                    Ok(JsValue::Undefined)
                }
                other => Err(JsError::TypeError(format!(
                    "Array.prototype.{} is not a function",
                    other
                ))),
            },
            JsValue::Object(mut map) => match map.get("__class__").cloned() {
                Some(JsValue::String(cls)) if cls == "Map" => {
                    let mut entries = match map.get("__entries__") {
                        Some(JsValue::Array(arr)) => arr.clone(),
                        _ => Vec::new(),
                    };
                    match method {
                        "get" => {
                            let key = args.first().unwrap_or(&JsValue::Undefined);
                            for entry in &entries {
                                if let JsValue::Array(pair) = entry {
                                    if pair.first().map(|k| k.strict_eq(key)).unwrap_or(false) {
                                        return Ok(pair
                                            .get(1)
                                            .cloned()
                                            .unwrap_or(JsValue::Undefined));
                                    }
                                }
                            }
                            Ok(JsValue::Undefined)
                        }
                        "has" => {
                            let key = args.first().unwrap_or(&JsValue::Undefined);
                            let found = entries.iter().any(|entry| {
                                matches!(entry, JsValue::Array(pair) if pair.first().map(|k| k.strict_eq(key)).unwrap_or(false))
                            });
                            Ok(JsValue::Bool(found))
                        }
                        "set" => {
                            let key = args.first().cloned().unwrap_or(JsValue::Undefined);
                            let val = args.get(1).cloned().unwrap_or(JsValue::Undefined);
                            map_set_entry(&mut entries, key, val);
                            map.insert("size".to_string(), JsValue::Number(entries.len() as i64));
                            map.insert("__entries__".to_string(), JsValue::Array(entries));
                            let updated = JsValue::Object(map);
                            if let Expr::Ident(var_name) = receiver_expr {
                                self.mutate_var(var_name, updated.clone())?;
                            }
                            Ok(updated)
                        }
                        "delete" => {
                            let key = args.first().unwrap_or(&JsValue::Undefined);
                            let orig_len = entries.len();
                            entries.retain(|entry| {
                                !matches!(entry, JsValue::Array(pair) if pair.first().map(|k| k.strict_eq(key)).unwrap_or(false))
                            });
                            let removed = entries.len() < orig_len;
                            map.insert("size".to_string(), JsValue::Number(entries.len() as i64));
                            map.insert("__entries__".to_string(), JsValue::Array(entries));
                            if let Expr::Ident(var_name) = receiver_expr {
                                self.mutate_var(var_name, JsValue::Object(map))?;
                            }
                            Ok(JsValue::Bool(removed))
                        }
                        "clear" => {
                            map.insert("size".to_string(), JsValue::Number(0));
                            map.insert("__entries__".to_string(), JsValue::Array(Vec::new()));
                            if let Expr::Ident(var_name) = receiver_expr {
                                self.mutate_var(var_name, JsValue::Object(map))?;
                            }
                            Ok(JsValue::Undefined)
                        }
                        other => Err(JsError::TypeError(format!(
                            "Map.prototype.{} is not a function",
                            other
                        ))),
                    }
                }
                Some(JsValue::String(cls)) if cls == "Uint8Array" => {
                    let bytes = match map.get("__bytes__") {
                        Some(JsValue::Array(arr)) => arr.clone(),
                        _ => Vec::new(),
                    };
                    match method {
                        "reduce" => self.eval_array_reduce(&bytes, args, host),
                        "map" => {
                            let Some(JsValue::Function(cb)) = args.first() else {
                                return Err(JsError::TypeError(
                                    "Uint8Array.prototype.map requires a callback function"
                                        .to_string(),
                                ));
                            };
                            let mut mapped = Vec::with_capacity(bytes.len());
                            for (idx, item) in bytes.iter().enumerate() {
                                let res = self.invoke_js_function(
                                    cb.clone(),
                                    &[item.clone(), JsValue::Number(idx as i64)],
                                    host,
                                )?;
                                mapped.push(JsValue::Number(res.to_number()?.rem_euclid(256)));
                            }
                            Ok(make_uint8_array_from_vec(mapped))
                        }
                        "slice" => {
                            let len = bytes.len() as i64;
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
                            Ok(make_uint8_array_from_vec(bytes[start..end].to_vec()))
                        }
                        other => Err(JsError::TypeError(format!(
                            "Uint8Array.prototype.{} is not a function",
                            other
                        ))),
                    }
                }
                Some(JsValue::String(cls)) if cls == "Promise" => {
                    let Some(JsValue::Number(id)) = map.get("__promise_id__") else {
                        return Err(JsError::TypeError("Invalid Promise object".to_string()));
                    };
                    let promise_id = *id as usize;
                    match method {
                        "then" => {
                            let on_fulfilled = match args.first() {
                                Some(JsValue::Function(f)) => Some(f.clone()),
                                _ => None,
                            };
                            let on_rejected = match args.get(1) {
                                Some(JsValue::Function(f)) => Some(f.clone()),
                                _ => None,
                            };
                            self.promise_then(promise_id, on_fulfilled, on_rejected)
                        }
                        "catch" => {
                            let on_rejected = match args.first() {
                                Some(JsValue::Function(f)) => Some(f.clone()),
                                _ => None,
                            };
                            self.promise_then(promise_id, None, on_rejected)
                        }
                        other => Err(JsError::TypeError(format!(
                            "Promise.prototype.{} is not a function",
                            other
                        ))),
                    }
                }
                _ => Err(JsError::TypeError(format!(
                    "Property '{}' is not callable on Object",
                    method
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
        host: &mut dyn AlloyOsHost,
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

    fn eval_array_reduce(
        &mut self,
        items: &[JsValue],
        args: &[JsValue],
        host: &mut dyn AlloyOsHost,
    ) -> Result<JsValue, JsError> {
        let Some(JsValue::Function(cb)) = args.first() else {
            return Err(JsError::TypeError(
                "Array.prototype.reduce requires a callback function".to_string(),
            ));
        };
        if args.len() >= 2 {
            let mut acc = args[1].clone();
            for (idx, item) in items.iter().enumerate() {
                acc = self.invoke_js_function(
                    cb.clone(),
                    &[acc, item.clone(), JsValue::Number(idx as i64)],
                    host,
                )?;
            }
            Ok(acc)
        } else {
            let Some(first) = items.first() else {
                return Err(JsError::TypeError(
                    "Reduce of empty array with no initial value".to_string(),
                ));
            };
            let mut acc = first.clone();
            for (idx, item) in items.iter().enumerate().skip(1) {
                acc = self.invoke_js_function(
                    cb.clone(),
                    &[acc, item.clone(), JsValue::Number(idx as i64)],
                    host,
                )?;
            }
            Ok(acc)
        }
    }

    fn alloc_promise(&mut self, state: PromiseState) -> (usize, JsValue) {
        let id = self.promises.len();
        self.promises.push(PromiseRecord {
            state,
            reactions: Vec::new(),
            handled: false,
        });
        let mut map = BTreeMap::new();
        map.insert("__class__".to_string(), JsValue::String("Promise".to_string()));
        map.insert("__promise_id__".to_string(), JsValue::Number(id as i64));
        (id, JsValue::Object(map))
    }

    fn promise_resolve(&mut self, value: JsValue) -> JsValue {
        if let JsValue::Object(ref map) = value {
            if map.get("__class__") == Some(&JsValue::String("Promise".to_string())) {
                return value;
            }
        }
        self.alloc_promise(PromiseState::Fulfilled(value)).1
    }

    fn promise_reject(&mut self, reason: JsValue) -> JsValue {
        self.alloc_promise(PromiseState::Rejected(reason)).1
    }

    fn promise_then(
        &mut self,
        promise_id: usize,
        on_fulfilled: Option<JsFunction>,
        on_rejected: Option<JsFunction>,
    ) -> Result<JsValue, JsError> {
        if promise_id >= self.promises.len() {
            return Err(JsError::TypeError("Invalid Promise reference".to_string()));
        }
        let (child_id, child_obj) = self.alloc_promise(PromiseState::Pending);
        let state = {
            let record = &mut self.promises[promise_id];
            record.handled = true;
            record.state.clone()
        };
        match state {
            PromiseState::Fulfilled(val) => {
                self.microtasks.push_back(Microtask {
                    handler: on_fulfilled,
                    is_rejection: false,
                    arg: val,
                    child_promise_id: child_id,
                });
            }
            PromiseState::Rejected(reason) => {
                self.microtasks.push_back(Microtask {
                    handler: on_rejected,
                    is_rejection: true,
                    arg: reason,
                    child_promise_id: child_id,
                });
            }
            PromiseState::Pending => {
                self.promises[promise_id].reactions.push(PromiseReaction {
                    on_fulfilled,
                    on_rejected,
                    child_promise_id: child_id,
                });
            }
        }
        Ok(child_obj)
    }

    fn settle_promise(&mut self, promise_id: usize, new_state: PromiseState) {
        if promise_id >= self.promises.len() {
            return;
        }
        if !matches!(self.promises[promise_id].state, PromiseState::Pending) {
            return;
        }
        // If fulfilling with another Promise, adopt its state
        let adopted_state = match &new_state {
            PromiseState::Fulfilled(JsValue::Object(map))
                if map.get("__class__") == Some(&JsValue::String("Promise".to_string())) =>
            {
                if let Some(JsValue::Number(other_id)) = map.get("__promise_id__") {
                    let oid = *other_id as usize;
                    if let Some(other_rec) = self.promises.get_mut(oid) {
                        other_rec.handled = true;
                        other_rec.state.clone()
                    } else {
                        new_state
                    }
                } else {
                    new_state
                }
            }
            _ => new_state,
        };

        let reactions = core::mem::take(&mut self.promises[promise_id].reactions);
        if !reactions.is_empty() {
            self.promises[promise_id].handled = true;
        }
        self.promises[promise_id].state = adopted_state.clone();

        for reaction in reactions {
            match &adopted_state {
                PromiseState::Fulfilled(val) => {
                    self.microtasks.push_back(Microtask {
                        handler: reaction.on_fulfilled,
                        is_rejection: false,
                        arg: val.clone(),
                        child_promise_id: reaction.child_promise_id,
                    });
                }
                PromiseState::Rejected(reason) => {
                    self.microtasks.push_back(Microtask {
                        handler: reaction.on_rejected,
                        is_rejection: true,
                        arg: reason.clone(),
                        child_promise_id: reaction.child_promise_id,
                    });
                }
                PromiseState::Pending => {}
            }
        }
    }

    fn drain_microtasks(&mut self, host: &mut dyn AlloyOsHost) -> Result<(), JsError> {
        while let Some(task) = self.microtasks.pop_front() {
            self.tick_step()?;
            match (task.handler, task.is_rejection) {
                (Some(func), _) => match self.invoke_js_function(func, &[task.arg], host) {
                    Ok(val) => {
                        self.settle_promise(task.child_promise_id, PromiseState::Fulfilled(val));
                    }
                    Err(err) => {
                        if is_uncatchable_error(&err) {
                            return Err(err);
                        }
                        let reason = self
                            .pending_exception
                            .take()
                            .unwrap_or_else(|| js_error_to_js_value(&err));
                        self.settle_promise(task.child_promise_id, PromiseState::Rejected(reason));
                    }
                },
                (None, false) => {
                    self.settle_promise(task.child_promise_id, PromiseState::Fulfilled(task.arg));
                }
                (None, true) => {
                    self.settle_promise(task.child_promise_id, PromiseState::Rejected(task.arg));
                }
            }
        }
        for rec in &mut self.promises {
            if !rec.handled {
                if let PromiseState::Rejected(ref reason) = rec.state {
                    rec.handled = true;
                    return Err(js_value_to_js_error(reason));
                }
            }
        }
        Ok(())
    }
}

fn is_builtin_constructor(name: &str) -> bool {
    matches!(
        name,
        "Error"
            | "SyntaxError"
            | "TypeError"
            | "ReferenceError"
            | "Map"
            | "Uint8Array"
            | "Promise"
            | "Array"
            | "Object"
    )
}

fn is_uncatchable_error(err: &JsError) -> bool {
    matches!(
        err,
        JsError::ExecutionLimitExceeded { .. }
            | JsError::OutOfFuel
            | JsError::ForbiddenCapability(_)
    )
}

fn make_error_object(class_name: &str, message: String) -> JsValue {
    let mut map = BTreeMap::new();
    map.insert(
        "__class__".to_string(),
        JsValue::String(class_name.to_string()),
    );
    map.insert("name".to_string(), JsValue::String(class_name.to_string()));
    map.insert("message".to_string(), JsValue::String(message));
    JsValue::Object(map)
}

fn js_error_to_js_value(err: &JsError) -> JsValue {
    match err {
        JsError::SyntaxError(msg) => make_error_object("SyntaxError", msg.clone()),
        JsError::ReferenceError(msg) => make_error_object("ReferenceError", msg.clone()),
        JsError::TypeError(msg) => make_error_object("TypeError", msg.clone()),
        JsError::RuntimeError(msg) | JsError::HostError(msg) => {
            make_error_object("Error", msg.clone())
        }
        JsError::ForbiddenCapability(msg) => make_error_object("Error", msg.clone()),
        JsError::ExecutionLimitExceeded { max_steps } => {
            make_error_object("Error", format!("exceeded {} steps", max_steps))
        }
        JsError::OutOfFuel => make_error_object("Error", "out of fuel".to_string()),
    }
}

fn js_value_to_js_error(val: &JsValue) -> JsError {
    if let JsValue::Object(map) = val {
        let cls = map
            .get("__class__")
            .or_else(|| map.get("name"))
            .and_then(|v| match v {
                JsValue::String(s) => Some(s.as_str()),
                _ => None,
            })
            .unwrap_or("Error");
        let msg = map
            .get("message")
            .map(|v| v.to_js_string())
            .unwrap_or_else(|| val.to_js_string());
        return match cls {
            "SyntaxError" => JsError::SyntaxError(msg),
            "ReferenceError" => JsError::ReferenceError(msg),
            "TypeError" => JsError::TypeError(msg),
            _ => JsError::RuntimeError(msg),
        };
    }
    JsError::RuntimeError(val.to_js_string())
}

fn eval_instanceof(left: &JsValue, right: &JsValue) -> Result<JsValue, JsError> {
    let JsValue::Function(func) = right else {
        return Err(JsError::TypeError(
            "Right-hand side of 'instanceof' is not callable".to_string(),
        ));
    };
    let Some(ctor_name) = func.name.as_deref() else {
        return Ok(JsValue::Bool(false));
    };
    match (left, ctor_name) {
        (JsValue::Array(_), "Array" | "Object") => Ok(JsValue::Bool(true)),
        (JsValue::Function(_), "Function" | "Object") => Ok(JsValue::Bool(true)),
        (JsValue::Object(map), _) => {
            let cls = map
                .get("__class__")
                .and_then(|v| match v {
                    JsValue::String(s) => Some(s.as_str()),
                    _ => None,
                })
                .unwrap_or("Object");
            if ctor_name == "Object" {
                Ok(JsValue::Bool(true))
            } else if ctor_name == "Error" {
                Ok(JsValue::Bool(matches!(
                    cls,
                    "Error" | "SyntaxError" | "TypeError" | "ReferenceError"
                )))
            } else {
                Ok(JsValue::Bool(cls == ctor_name))
            }
        }
        _ => Ok(JsValue::Bool(false)),
    }
}

fn map_set_entry(entries: &mut Vec<JsValue>, key: JsValue, val: JsValue) {
    for entry in entries.iter_mut() {
        if let JsValue::Array(pair) = entry {
            if pair.first().map(|k| k.strict_eq(&key)).unwrap_or(false) {
                if pair.len() >= 2 {
                    pair[1] = val;
                } else {
                    pair.push(val);
                }
                return;
            }
        }
    }
    entries.push(JsValue::Array(vec![key, val]));
}

fn make_map_object(iterable: Option<&JsValue>) -> Result<JsValue, JsError> {
    let mut entries = Vec::new();
    match iterable {
        None | Some(JsValue::Undefined | JsValue::Null) => {}
        Some(JsValue::Array(items)) => {
            for item in items {
                let JsValue::Array(pair) = item else {
                    return Err(JsError::TypeError(
                        "Iterator value is not an entry object".to_string(),
                    ));
                };
                let k = pair.first().cloned().unwrap_or(JsValue::Undefined);
                let v = pair.get(1).cloned().unwrap_or(JsValue::Undefined);
                map_set_entry(&mut entries, k, v);
            }
        }
        Some(_) => {
            return Err(JsError::TypeError(
                "Map constructor argument is not iterable".to_string(),
            ));
        }
    }
    let mut map = BTreeMap::new();
    map.insert("__class__".to_string(), JsValue::String("Map".to_string()));
    map.insert("size".to_string(), JsValue::Number(entries.len() as i64));
    map.insert("__entries__".to_string(), JsValue::Array(entries));
    Ok(JsValue::Object(map))
}

fn make_uint8_array_from_vec(bytes: Vec<JsValue>) -> JsValue {
    let mut map = BTreeMap::new();
    map.insert(
        "__class__".to_string(),
        JsValue::String("Uint8Array".to_string()),
    );
    map.insert("length".to_string(), JsValue::Number(bytes.len() as i64));
    map.insert("__bytes__".to_string(), JsValue::Array(bytes));
    JsValue::Object(map)
}

fn make_uint8_array_object(arg: Option<&JsValue>) -> Result<JsValue, JsError> {
    let bytes = match arg {
        None | Some(JsValue::Undefined) => Vec::new(),
        Some(JsValue::Number(n)) => {
            if *n < 0 {
                return Err(JsError::TypeError("Invalid typed array length".to_string()));
            }
            vec![JsValue::Number(0); *n as usize]
        }
        Some(JsValue::Array(items)) => {
            let mut out = Vec::with_capacity(items.len());
            for item in items {
                let n = item.to_number().unwrap_or(0);
                out.push(JsValue::Number(n.rem_euclid(256)));
            }
            out
        }
        Some(JsValue::Object(map))
            if map.get("__class__") == Some(&JsValue::String("Uint8Array".to_string())) =>
        {
            match map.get("__bytes__") {
                Some(JsValue::Array(arr)) => arr.clone(),
                _ => Vec::new(),
            }
        }
        Some(_) => Vec::new(),
    };
    Ok(make_uint8_array_from_vec(bytes))
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
