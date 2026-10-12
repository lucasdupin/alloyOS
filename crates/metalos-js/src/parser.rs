use alloc::boxed::Box;
use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec;
use alloc::vec::Vec;

use crate::ast::{
    ArrowBody, AssignOp, AssignTarget, BinaryOp, Expr, Stmt, UnaryOp, UpdateOp, VarKind,
};
use crate::lexer::{tokenize, Token};
use crate::value::JsError;

/// Parses JavaScript source code into a list of top-level AST statements.
pub fn parse_program(src: &str) -> Result<Vec<Stmt>, JsError> {
    let tokens = tokenize(src)?;
    let mut parser = Parser { tokens, pos: 0 };
    parser.parse_program()
}

struct Parser {
    tokens: Vec<Token>,
    pos: usize,
}

impl Parser {
    fn peek(&self) -> &Token {
        self.tokens.get(self.pos).unwrap_or(&Token::Eof)
    }

    fn peek_at(&self, offset: usize) -> &Token {
        self.tokens.get(self.pos + offset).unwrap_or(&Token::Eof)
    }

    fn advance(&mut self) -> Token {
        let tok = self.peek().clone();
        if self.pos < self.tokens.len() {
            self.pos += 1;
        }
        tok
    }

    fn check(&self, expected: &Token) -> bool {
        self.peek() == expected
    }

    fn consume(&mut self, expected: &Token, msg: &str) -> Result<(), JsError> {
        if self.check(expected) {
            self.advance();
            Ok(())
        } else {
            Err(JsError::SyntaxError(format!(
                "{}: found {:?}",
                msg,
                self.peek()
            )))
        }
    }

    fn consume_optional_semicolon(&mut self) {
        while self.check(&Token::Semicolon) {
            self.advance();
        }
    }

    fn expect_ident(&mut self, msg: &str) -> Result<String, JsError> {
        match self.advance() {
            Token::Ident(name) => Ok(name),
            other => Err(JsError::SyntaxError(format!(
                "{}: expected identifier, found {:?}",
                msg, other
            ))),
        }
    }

    fn parse_program(&mut self) -> Result<Vec<Stmt>, JsError> {
        let mut stmts = Vec::new();
        self.consume_optional_semicolon();
        while !self.check(&Token::Eof) {
            stmts.push(self.parse_stmt()?);
            self.consume_optional_semicolon();
        }
        Ok(stmts)
    }

    fn parse_stmt(&mut self) -> Result<Stmt, JsError> {
        if matches!(self.peek(), Token::Ident(s) if s == "export") {
            self.advance();
            if matches!(self.peek(), Token::Ident(s) if s == "default") {
                self.advance();
            }
        }
        match self.peek() {
            Token::Let => self.parse_var_decl(VarKind::Let),
            Token::Const => self.parse_var_decl(VarKind::Const),
            Token::Var => self.parse_var_decl(VarKind::Var),
            Token::If => self.parse_if_stmt(),
            Token::While => self.parse_while_stmt(),
            Token::For => self.parse_for_stmt(),
            Token::Function => self.parse_func_decl(),
            Token::Return => self.parse_return_stmt(),
            Token::LBrace => {
                let stmts = self.parse_block()?;
                Ok(Stmt::Block(stmts))
            }
            _ => {
                let expr = self.parse_expr()?;
                self.consume_optional_semicolon();
                Ok(Stmt::ExprStmt(expr))
            }
        }
    }

    fn parse_block(&mut self) -> Result<Vec<Stmt>, JsError> {
        self.consume(&Token::LBrace, "Expected '{' to start block")?;
        let mut stmts = Vec::new();
        self.consume_optional_semicolon();
        while !self.check(&Token::RBrace) && !self.check(&Token::Eof) {
            stmts.push(self.parse_stmt()?);
            self.consume_optional_semicolon();
        }
        self.consume(&Token::RBrace, "Expected '}' to close block")?;
        Ok(stmts)
    }

    fn parse_stmt_or_block(&mut self) -> Result<Vec<Stmt>, JsError> {
        if self.check(&Token::LBrace) {
            self.parse_block()
        } else {
            Ok(vec![self.parse_stmt()?])
        }
    }

    fn parse_var_decl(&mut self, kind: VarKind) -> Result<Stmt, JsError> {
        self.advance(); // consume let/const/var
        let name = self.expect_ident("Variable declaration requires a name")?;
        let init = if self.check(&Token::Assign) {
            self.advance();
            Some(self.parse_expr()?)
        } else {
            None
        };
        self.consume_optional_semicolon();
        Ok(Stmt::VarDecl { kind, name, init })
    }

    fn parse_if_stmt(&mut self) -> Result<Stmt, JsError> {
        self.advance(); // consume 'if'
        self.consume(&Token::LParen, "Expected '(' after 'if'")?;
        let cond = self.parse_expr()?;
        self.consume(&Token::RParen, "Expected ')' after if condition")?;
        let then_branch = self.parse_stmt_or_block()?;
        let else_branch = if self.check(&Token::Else) {
            self.advance();
            Some(self.parse_stmt_or_block()?)
        } else {
            None
        };
        Ok(Stmt::If {
            cond,
            then_branch,
            else_branch,
        })
    }

    fn parse_while_stmt(&mut self) -> Result<Stmt, JsError> {
        self.advance(); // consume 'while'
        self.consume(&Token::LParen, "Expected '(' after 'while'")?;
        let cond = self.parse_expr()?;
        self.consume(&Token::RParen, "Expected ')' after while condition")?;
        let body = self.parse_stmt_or_block()?;
        Ok(Stmt::While { cond, body })
    }

    fn parse_for_stmt(&mut self) -> Result<Stmt, JsError> {
        self.advance(); // consume 'for'
        self.consume(&Token::LParen, "Expected '(' after 'for'")?;

        let init = if self.check(&Token::Semicolon) {
            self.advance();
            None
        } else if matches!(self.peek(), Token::Let | Token::Const | Token::Var) {
            let kind = match self.peek() {
                Token::Let => VarKind::Let,
                Token::Const => VarKind::Const,
                _ => VarKind::Var,
            };
            self.advance();
            let name = self.expect_ident("Expected identifier in for-loop initializer")?;
            let var_init = if self.check(&Token::Assign) {
                self.advance();
                Some(self.parse_expr()?)
            } else {
                None
            };
            self.consume(&Token::Semicolon, "Expected ';' after for-loop initializer")?;
            Some(Box::new(Stmt::VarDecl {
                kind,
                name,
                init: var_init,
            }))
        } else {
            let expr = self.parse_expr()?;
            self.consume(&Token::Semicolon, "Expected ';' after for-loop initializer")?;
            Some(Box::new(Stmt::ExprStmt(expr)))
        };

        let cond = if self.check(&Token::Semicolon) {
            self.advance();
            None
        } else {
            let c = self.parse_expr()?;
            self.consume(&Token::Semicolon, "Expected ';' after for-loop condition")?;
            Some(c)
        };

        let update = if self.check(&Token::RParen) {
            None
        } else {
            Some(self.parse_expr()?)
        };
        self.consume(&Token::RParen, "Expected ')' after for-loop clauses")?;

        let body = self.parse_stmt_or_block()?;
        Ok(Stmt::For {
            init,
            cond,
            update,
            body,
        })
    }

    fn parse_func_decl(&mut self) -> Result<Stmt, JsError> {
        self.advance(); // consume 'function'
        let name = self.expect_ident("Expected function name")?;
        let params = self.parse_param_list()?;
        let body = self.parse_block()?;
        Ok(Stmt::FunctionDecl { name, params, body })
    }

    fn parse_param_list(&mut self) -> Result<Vec<String>, JsError> {
        self.consume(&Token::LParen, "Expected '(' for parameter list")?;
        let mut params = Vec::new();
        if !self.check(&Token::RParen) {
            loop {
                params.push(self.expect_ident("Expected parameter name")?);
                if self.check(&Token::Comma) {
                    self.advance();
                    if self.check(&Token::RParen) {
                        break;
                    }
                } else {
                    break;
                }
            }
        }
        self.consume(&Token::RParen, "Expected ')' after parameter list")?;
        Ok(params)
    }

    fn parse_return_stmt(&mut self) -> Result<Stmt, JsError> {
        self.advance(); // consume 'return'
        if self.check(&Token::Semicolon) || self.check(&Token::RBrace) || self.check(&Token::Eof) {
            self.consume_optional_semicolon();
            return Ok(Stmt::Return(None));
        }
        let val = self.parse_expr()?;
        self.consume_optional_semicolon();
        Ok(Stmt::Return(Some(val)))
    }

    fn parse_expr(&mut self) -> Result<Expr, JsError> {
        self.parse_assignment()
    }

    fn is_arrow_function_ahead(&self) -> bool {
        // Case 1: `ident => ...`
        if matches!(self.peek(), Token::Ident(_)) && matches!(self.peek_at(1), Token::FatArrow) {
            return true;
        }
        // Case 2: `( ... ) => ...`
        if matches!(self.peek(), Token::LParen) {
            let mut depth = 0usize;
            let mut offset = 0usize;
            while let Some(tok) = self.tokens.get(self.pos + offset) {
                match tok {
                    Token::LParen => depth += 1,
                    Token::RParen => {
                        depth = depth.saturating_sub(1);
                        if depth == 0 {
                            return matches!(self.peek_at(offset + 1), Token::FatArrow);
                        }
                    }
                    Token::Eof => break,
                    _ => {}
                }
                offset += 1;
            }
        }
        false
    }

    fn parse_arrow_function(&mut self) -> Result<Expr, JsError> {
        let params = if matches!(self.peek(), Token::Ident(_)) {
            let p = self.expect_ident("Expected arrow parameter")?;
            vec![p]
        } else {
            self.parse_param_list()?
        };
        self.consume(&Token::FatArrow, "Expected '=>' in arrow function")?;
        let body = if self.check(&Token::LBrace) {
            ArrowBody::Block(self.parse_block()?)
        } else {
            ArrowBody::Expr(Box::new(self.parse_assignment()?))
        };
        Ok(Expr::ArrowFunction { params, body })
    }

    fn parse_assignment(&mut self) -> Result<Expr, JsError> {
        if self.is_arrow_function_ahead() {
            return self.parse_arrow_function();
        }

        let left = self.parse_logical_or()?;

        let assign_op = match self.peek() {
            Token::Assign => Some(AssignOp::Assign),
            Token::PlusAssign => Some(AssignOp::AddAssign),
            Token::MinusAssign => Some(AssignOp::SubAssign),
            _ => None,
        };

        if let Some(op) = assign_op {
            self.advance();
            let target = expr_to_assign_target(left)?;
            let value = self.parse_assignment()?;
            return Ok(Expr::Assign {
                op,
                target,
                value: Box::new(value),
            });
        }

        Ok(left)
    }

    fn parse_logical_or(&mut self) -> Result<Expr, JsError> {
        let mut expr = self.parse_logical_and()?;
        while self.check(&Token::OrOr) {
            self.advance();
            let right = self.parse_logical_and()?;
            expr = Expr::Binary {
                op: BinaryOp::Or,
                left: Box::new(expr),
                right: Box::new(right),
            };
        }
        Ok(expr)
    }

    fn parse_logical_and(&mut self) -> Result<Expr, JsError> {
        let mut expr = self.parse_equality()?;
        while self.check(&Token::AndAnd) {
            self.advance();
            let right = self.parse_equality()?;
            expr = Expr::Binary {
                op: BinaryOp::And,
                left: Box::new(expr),
                right: Box::new(right),
            };
        }
        Ok(expr)
    }

    fn parse_equality(&mut self) -> Result<Expr, JsError> {
        let mut expr = self.parse_comparison()?;
        loop {
            let op = match self.peek() {
                Token::EqEq => Some(BinaryOp::EqEq),
                Token::NotEq => Some(BinaryOp::NotEq),
                Token::StrictEq => Some(BinaryOp::StrictEq),
                Token::StrictNotEq => Some(BinaryOp::StrictNotEq),
                _ => None,
            };
            if let Some(op) = op {
                self.advance();
                let right = self.parse_comparison()?;
                expr = Expr::Binary {
                    op,
                    left: Box::new(expr),
                    right: Box::new(right),
                };
            } else {
                break;
            }
        }
        Ok(expr)
    }

    fn parse_comparison(&mut self) -> Result<Expr, JsError> {
        let mut expr = self.parse_additive()?;
        loop {
            let op = match self.peek() {
                Token::Lt => Some(BinaryOp::Lt),
                Token::Le => Some(BinaryOp::Le),
                Token::Gt => Some(BinaryOp::Gt),
                Token::Ge => Some(BinaryOp::Ge),
                _ => None,
            };
            if let Some(op) = op {
                self.advance();
                let right = self.parse_additive()?;
                expr = Expr::Binary {
                    op,
                    left: Box::new(expr),
                    right: Box::new(right),
                };
            } else {
                break;
            }
        }
        Ok(expr)
    }

    fn parse_additive(&mut self) -> Result<Expr, JsError> {
        let mut expr = self.parse_multiplicative()?;
        loop {
            let op = match self.peek() {
                Token::Plus => Some(BinaryOp::Add),
                Token::Minus => Some(BinaryOp::Sub),
                _ => None,
            };
            if let Some(op) = op {
                self.advance();
                let right = self.parse_multiplicative()?;
                expr = Expr::Binary {
                    op,
                    left: Box::new(expr),
                    right: Box::new(right),
                };
            } else {
                break;
            }
        }
        Ok(expr)
    }

    fn parse_multiplicative(&mut self) -> Result<Expr, JsError> {
        let mut expr = self.parse_unary()?;
        loop {
            let op = match self.peek() {
                Token::Star => Some(BinaryOp::Mul),
                Token::Slash => Some(BinaryOp::Div),
                Token::Percent => Some(BinaryOp::Rem),
                _ => None,
            };
            if let Some(op) = op {
                self.advance();
                let right = self.parse_unary()?;
                expr = Expr::Binary {
                    op,
                    left: Box::new(expr),
                    right: Box::new(right),
                };
            } else {
                break;
            }
        }
        Ok(expr)
    }

    fn parse_unary(&mut self) -> Result<Expr, JsError> {
        match self.peek() {
            Token::Bang => {
                self.advance();
                let expr = self.parse_unary()?;
                Ok(Expr::Unary {
                    op: UnaryOp::Not,
                    expr: Box::new(expr),
                })
            }
            Token::Minus => {
                self.advance();
                let expr = self.parse_unary()?;
                Ok(Expr::Unary {
                    op: UnaryOp::Neg,
                    expr: Box::new(expr),
                })
            }
            Token::Plus => {
                self.advance();
                let expr = self.parse_unary()?;
                Ok(Expr::Unary {
                    op: UnaryOp::Pos,
                    expr: Box::new(expr),
                })
            }
            Token::Typeof => {
                self.advance();
                let expr = self.parse_unary()?;
                Ok(Expr::Unary {
                    op: UnaryOp::Typeof,
                    expr: Box::new(expr),
                })
            }
            Token::PlusPlus => {
                self.advance();
                let inner = self.parse_postfix()?;
                let target = expr_to_assign_target(inner)?;
                Ok(Expr::Update {
                    op: UpdateOp::Inc,
                    target,
                    prefix: true,
                })
            }
            Token::MinusMinus => {
                self.advance();
                let inner = self.parse_postfix()?;
                let target = expr_to_assign_target(inner)?;
                Ok(Expr::Update {
                    op: UpdateOp::Dec,
                    target,
                    prefix: true,
                })
            }
            _ => self.parse_postfix(),
        }
    }

    fn parse_postfix(&mut self) -> Result<Expr, JsError> {
        let mut expr = self.parse_primary()?;

        loop {
            match self.peek() {
                Token::Dot => {
                    self.advance();
                    let prop = match self.advance() {
                        Token::Ident(s) => s,
                        // Allow keywords as property names (e.g. obj.return or obj.typeof)
                        Token::Return => "return".to_string(),
                        Token::If => "if".to_string(),
                        Token::Else => "else".to_string(),
                        Token::For => "for".to_string(),
                        Token::While => "while".to_string(),
                        Token::Function => "function".to_string(),
                        Token::Let => "let".to_string(),
                        Token::Const => "const".to_string(),
                        Token::Var => "var".to_string(),
                        other => {
                            return Err(JsError::SyntaxError(format!(
                                "Expected property name after '.', found {:?}",
                                other
                            )))
                        }
                    };
                    expr = Expr::Member {
                        object: Box::new(expr),
                        property: prop,
                    };
                }
                Token::LBracket => {
                    self.advance();
                    let index = self.parse_expr()?;
                    self.consume(&Token::RBracket, "Expected ']' after index expression")?;
                    expr = Expr::Index {
                        object: Box::new(expr),
                        index: Box::new(index),
                    };
                }
                Token::LParen => {
                    self.advance();
                    let mut args = Vec::new();
                    if !self.check(&Token::RParen) {
                        loop {
                            args.push(self.parse_expr()?);
                            if self.check(&Token::Comma) {
                                self.advance();
                                if self.check(&Token::RParen) {
                                    break;
                                }
                            } else {
                                break;
                            }
                        }
                    }
                    self.consume(&Token::RParen, "Expected ')' after call arguments")?;
                    expr = Expr::Call {
                        callee: Box::new(expr),
                        args,
                    };
                }
                Token::PlusPlus => {
                    self.advance();
                    let target = expr_to_assign_target(expr)?;
                    expr = Expr::Update {
                        op: UpdateOp::Inc,
                        target,
                        prefix: false,
                    };
                }
                Token::MinusMinus => {
                    self.advance();
                    let target = expr_to_assign_target(expr)?;
                    expr = Expr::Update {
                        op: UpdateOp::Dec,
                        target,
                        prefix: false,
                    };
                }
                _ => break,
            }
        }

        Ok(expr)
    }

    fn parse_primary(&mut self) -> Result<Expr, JsError> {
        match self.peek().clone() {
            Token::Undefined => {
                self.advance();
                Ok(Expr::Undefined)
            }
            Token::Null => {
                self.advance();
                Ok(Expr::Null)
            }
            Token::True => {
                self.advance();
                Ok(Expr::Bool(true))
            }
            Token::False => {
                self.advance();
                Ok(Expr::Bool(false))
            }
            Token::Number(n) => {
                self.advance();
                Ok(Expr::Number(n))
            }
            Token::StringLit(s) => {
                self.advance();
                Ok(Expr::String(s))
            }
            Token::Ident(name) => {
                self.advance();
                Ok(Expr::Ident(name))
            }
            Token::Function => {
                self.advance();
                let name = if let Token::Ident(n) = self.peek().clone() {
                    self.advance();
                    Some(n)
                } else {
                    None
                };
                let params = self.parse_param_list()?;
                let body = self.parse_block()?;
                Ok(Expr::FunctionExpr { name, params, body })
            }
            Token::LParen => {
                self.advance();
                let expr = self.parse_expr()?;
                self.consume(&Token::RParen, "Expected ')' after parenthesized expression")?;
                Ok(expr)
            }
            Token::LBracket => self.parse_array_literal(),
            Token::LBrace => self.parse_object_literal(),
            other => Err(JsError::SyntaxError(format!(
                "Unexpected token in expression: {:?}",
                other
            ))),
        }
    }

    fn parse_array_literal(&mut self) -> Result<Expr, JsError> {
        self.consume(&Token::LBracket, "Expected '['")?;
        let mut items = Vec::new();
        while !self.check(&Token::RBracket) && !self.check(&Token::Eof) {
            items.push(self.parse_expr()?);
            if self.check(&Token::Comma) {
                self.advance();
            } else {
                break;
            }
        }
        self.consume(&Token::RBracket, "Expected ']' after array literal")?;
        Ok(Expr::ArrayLit(items))
    }

    fn parse_object_literal(&mut self) -> Result<Expr, JsError> {
        self.consume(&Token::LBrace, "Expected '{'")?;
        let mut entries = Vec::new();
        while !self.check(&Token::RBrace) && !self.check(&Token::Eof) {
            let (key, is_ident) = match self.advance() {
                Token::Ident(k) => (k, true),
                Token::StringLit(k) => (k, false),
                Token::Number(n) => (format!("{}", n), false),
                other => {
                    return Err(JsError::SyntaxError(format!(
                        "Expected object property key, found {:?}",
                        other
                    )))
                }
            };

            if self.check(&Token::Colon) {
                self.advance();
                let val = self.parse_expr()?;
                entries.push((key, val));
            } else if is_ident {
                // Shorthand property `{ a }` -> `{ a: a }`
                let val = Expr::Ident(key.clone());
                entries.push((key, val));
            } else {
                return Err(JsError::SyntaxError(
                    "Expected ':' after object property key".to_string(),
                ));
            }

            if self.check(&Token::Comma) {
                self.advance();
            } else {
                break;
            }
        }
        self.consume(&Token::RBrace, "Expected '}' after object literal")?;
        Ok(Expr::ObjectLit(entries))
    }
}

fn expr_to_assign_target(expr: Expr) -> Result<AssignTarget, JsError> {
    match expr {
        Expr::Ident(name) => Ok(AssignTarget::Ident(name)),
        Expr::Member { object, property } => Ok(AssignTarget::Member { object, property }),
        Expr::Index { object, index } => Ok(AssignTarget::Index { object, index }),
        _ => Err(JsError::SyntaxError(
            "Invalid left-hand side in assignment".to_string(),
        )),
    }
}
