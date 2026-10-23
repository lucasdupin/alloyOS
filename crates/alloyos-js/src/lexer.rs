use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec::Vec;

use crate::value::JsError;

/// Lexical token kinds for the embedded JavaScript parser.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Token {
    // Keywords
    Let,
    Const,
    Var,
    If,
    Else,
    While,
    For,
    Function,
    Return,
    True,
    False,
    Null,
    Undefined,
    Typeof,
    Try,
    Catch,
    Finally,
    Throw,
    New,
    Instanceof,

    // Identifiers and Literals
    Ident(String),
    Number(i64),
    StringLit(String),

    // Operators
    Plus,        // +
    Minus,       // -
    Star,        // *
    Slash,       // /
    Percent,     // %
    PlusPlus,    // ++
    MinusMinus,  // --
    Assign,      // =
    PlusAssign,  // +=
    MinusAssign, // -=
    EqEq,        // ==
    NotEq,       // !=
    StrictEq,    // ===
    StrictNotEq, // !==
    Lt,          // <
    Le,          // <=
    Gt,          // >
    Ge,          // >=
    AndAnd,      // &&
    OrOr,        // ||
    Bang,        // !
    FatArrow,    // =>

    // Delimiters
    LParen,    // (
    RParen,    // )
    LBrace,    // {
    RBrace,    // }
    LBracket,  // [
    RBracket,  // ]
    Comma,     // ,
    Dot,       // .
    Colon,     // :
    Semicolon, // ;

    Eof,
}

/// Tokenizes JavaScript source code into a sequence of `Token`s ending with `Token::Eof`.
pub const MAX_SOURCE_BYTES: usize = 256 * 1024;
pub const MAX_TOKENS: usize = 8192;
pub fn tokenize(src: &str) -> Result<Vec<Token>, JsError> {
    if src.len() > MAX_SOURCE_BYTES {
        return Err(JsError::SyntaxError(
            "Source exceeds parser byte limit".into(),
        ));
    }
    let chars: Vec<char> = src.chars().collect();
    let mut pos = 0;
    let mut tokens = Vec::new();

    while pos < chars.len() {
        if tokens.len() >= MAX_TOKENS {
            return Err(JsError::SyntaxError(
                "Source exceeds parser token limit".into(),
            ));
        }
        let ch = chars[pos];

        // Skip whitespace
        if ch.is_whitespace() {
            pos += 1;
            continue;
        }

        // Single-line comment //
        if ch == '/' && pos + 1 < chars.len() && chars[pos + 1] == '/' {
            pos += 2;
            while pos < chars.len() && chars[pos] != '\n' {
                pos += 1;
            }
            continue;
        }

        // Multi-line comment /* ... */
        if ch == '/' && pos + 1 < chars.len() && chars[pos + 1] == '*' {
            pos += 2;
            let mut closed = false;
            while pos + 1 < chars.len() {
                if chars[pos] == '*' && chars[pos + 1] == '/' {
                    pos += 2;
                    closed = true;
                    break;
                }
                pos += 1;
            }
            if !closed {
                return Err(JsError::SyntaxError(
                    "Unterminated multi-line comment".to_string(),
                ));
            }
            continue;
        }

        // String literal ('...', "...", `...`)
        if ch == '"' || ch == '\'' || ch == '`' {
            let quote = ch;
            pos += 1;
            let mut s = String::new();
            let mut closed = false;
            while pos < chars.len() {
                let c = chars[pos];
                pos += 1;
                if c == quote {
                    closed = true;
                    break;
                }
                if c == '\\' {
                    if pos >= chars.len() {
                        return Err(JsError::SyntaxError(
                            "Unterminated string escape".to_string(),
                        ));
                    }
                    let esc = chars[pos];
                    pos += 1;
                    match esc {
                        'n' => s.push('\n'),
                        'r' => s.push('\r'),
                        't' => s.push('\t'),
                        '\\' => s.push('\\'),
                        '\'' => s.push('\''),
                        '"' => s.push('"'),
                        '`' => s.push('`'),
                        other => s.push(other),
                    }
                } else {
                    s.push(c);
                }
            }
            if !closed {
                return Err(JsError::SyntaxError(
                    "Unterminated string literal".to_string(),
                ));
            }
            tokens.push(Token::StringLit(s));
            continue;
        }

        // Numeric literal
        if ch.is_ascii_digit() {
            let mut num_str = String::new();
            while pos < chars.len() && chars[pos].is_ascii_digit() {
                num_str.push(chars[pos]);
                pos += 1;
            }
            // If followed by decimal point, consume fractional digits and truncate to i64
            if pos < chars.len()
                && chars[pos] == '.'
                && pos + 1 < chars.len()
                && chars[pos + 1].is_ascii_digit()
            {
                pos += 1;
                while pos < chars.len() && chars[pos].is_ascii_digit() {
                    pos += 1;
                }
            }
            let val = num_str.parse::<i64>().map_err(|_| {
                JsError::SyntaxError(format!("Invalid numeric literal '{}'", num_str))
            })?;
            tokens.push(Token::Number(val));
            continue;
        }

        // Identifiers & Keywords
        if is_ident_start(ch) {
            let mut ident = String::new();
            while pos < chars.len() && is_ident_continue(chars[pos]) {
                ident.push(chars[pos]);
                pos += 1;
            }
            let tok = match ident.as_str() {
                "let" => Token::Let,
                "const" => Token::Const,
                "var" => Token::Var,
                "if" => Token::If,
                "else" => Token::Else,
                "while" => Token::While,
                "for" => Token::For,
                "function" => Token::Function,
                "return" => Token::Return,
                "true" => Token::True,
                "false" => Token::False,
                "null" => Token::Null,
                "undefined" => Token::Undefined,
                "typeof" => Token::Typeof,
                "try" => Token::Try,
                "catch" => Token::Catch,
                "finally" => Token::Finally,
                "throw" => Token::Throw,
                "new" => Token::New,
                "instanceof" => Token::Instanceof,
                _ => Token::Ident(ident),
            };
            tokens.push(tok);
            continue;
        }

        // 3-char operators
        if pos + 2 < chars.len() {
            let s3: [char; 3] = [chars[pos], chars[pos + 1], chars[pos + 2]];
            match s3 {
                ['=', '=', '='] => {
                    tokens.push(Token::StrictEq);
                    pos += 3;
                    continue;
                }
                ['!', '=', '='] => {
                    tokens.push(Token::StrictNotEq);
                    pos += 3;
                    continue;
                }
                _ => {}
            }
        }

        // 2-char operators
        if pos + 1 < chars.len() {
            let s2: [char; 2] = [chars[pos], chars[pos + 1]];
            let matched = match s2 {
                ['+', '+'] => Some(Token::PlusPlus),
                ['-', '-'] => Some(Token::MinusMinus),
                ['+', '='] => Some(Token::PlusAssign),
                ['-', '='] => Some(Token::MinusAssign),
                ['=', '='] => Some(Token::EqEq),
                ['!', '='] => Some(Token::NotEq),
                ['<', '='] => Some(Token::Le),
                ['>', '='] => Some(Token::Ge),
                ['&', '&'] => Some(Token::AndAnd),
                ['|', '|'] => Some(Token::OrOr),
                ['=', '>'] => Some(Token::FatArrow),
                _ => None,
            };
            if let Some(tok) = matched {
                tokens.push(tok);
                pos += 2;
                continue;
            }
        }

        // 1-char operators and delimiters
        let single = match ch {
            '+' => Some(Token::Plus),
            '-' => Some(Token::Minus),
            '*' => Some(Token::Star),
            '/' => Some(Token::Slash),
            '%' => Some(Token::Percent),
            '=' => Some(Token::Assign),
            '<' => Some(Token::Lt),
            '>' => Some(Token::Gt),
            '!' => Some(Token::Bang),
            '(' => Some(Token::LParen),
            ')' => Some(Token::RParen),
            '{' => Some(Token::LBrace),
            '}' => Some(Token::RBrace),
            '[' => Some(Token::LBracket),
            ']' => Some(Token::RBracket),
            ',' => Some(Token::Comma),
            '.' => Some(Token::Dot),
            ':' => Some(Token::Colon),
            ';' => Some(Token::Semicolon),
            _ => None,
        };

        if let Some(tok) = single {
            tokens.push(tok);
            pos += 1;
        } else {
            return Err(JsError::SyntaxError(format!(
                "Unexpected character '{}' at index {}",
                ch, pos
            )));
        }
    }

    tokens.push(Token::Eof);
    Ok(tokens)
}

fn is_ident_start(ch: char) -> bool {
    ch.is_ascii_alphabetic() || ch == '_' || ch == '$'
}

fn is_ident_continue(ch: char) -> bool {
    ch.is_ascii_alphanumeric() || ch == '_' || ch == '$'
}
