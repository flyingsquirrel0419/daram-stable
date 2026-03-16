//! Daram lexer — converts source text into a flat stream of tokens.
//!
//! # Token design
//! - Whitespace and comments are skipped (not emitted) to keep the parser simple.
//! - String literals support `\n`, `\t`, `\\`, `\"`, `\r`, `\0`, and
//!   `\u{XXXX}` unicode escapes.
//! - Integer literals support decimal, `0x` hex, `0o` octal, and `0b` binary,
//!   with optional `_` separators.
//! - Float literals support `_` separators and an optional exponent.

use crate::source::ByteOffset;

const MAX_SOURCE_BYTES: usize = 2 * 1024 * 1024;
const MAX_TOKEN_COUNT: usize = 200_000;
const MAX_COMMENT_NESTING: usize = 256;

// ─── Token kinds ──────────────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq)]
pub enum TokenKind {
    // ── Literals ────────────────────────────────────────
    Integer(u128),
    Float(f64),
    StringLit(String),
    CharLit(char),
    /// `true` / `false`
    Bool(bool),

    // ── Identifiers / keywords ──────────────────────────
    Ident(String),

    // ── Keywords ────────────────────────────────────────
    KwLet,
    KwMut,
    KwFn,
    KwFun,
    KwReturn,
    KwIf,
    KwElse,
    KwFor,
    KwIn,
    KwWhile,
    KwLoop,
    KwBreak,
    KwContinue,
    KwStruct,
    KwEnum,
    KwConst,
    KwStatic,
    KwImpl,
    KwExtend,
    KwTrait,
    KwInterface,
    KwImplements,
    KwMatch,
    KwAs,
    KwUse,
    KwImport,
    KwExport,
    KwFrom,
    KwMod,
    KwPub,
    KwSelf,
    KwSuper,
    KwCrate,
    KwType,
    KwAsync,
    KwAwait,
    KwUnsafe,
    KwErrdefer,
    KwDefer,
    KwWhere,
    KwAbility,
    KwCapability,
    KwMove,
    KwExtern,
    KwDyn,

    // ── Operators ────────────────────────────────────────
    Plus,
    Minus,
    Star,
    Slash,
    Percent,
    Caret,
    Ampersand,
    Pipe,
    Tilde,
    Shl,         // <<
    Shr,         // >>
    Eq,          // =
    EqEq,        // ==
    Ne,          // !=
    Lt,          // <
    Le,          // <=
    Gt,          // >
    Ge,          // >=
    And,         // &&
    Or,          // ||
    Bang,        // !
    Question,    // ?
    Arrow,       // ->
    FatArrow,    // =>
    DotDot,      // ..
    DotDotEq,    // ..=
    Dot,         // .
    DoubleColon, // ::
    PlusEq,      // +=
    MinusEq,     // -=
    StarEq,      // *=
    SlashEq,     // /=
    PercentEq,   // %=
    AmpersandEq, // &=
    PipeEq,      // |=
    CaretEq,     // ^=
    ShlEq,       // <<=
    ShrEq,       // >>=

    // ── Delimiters ───────────────────────────────────────
    LParen,     // (
    RParen,     // )
    LBrace,     // {
    RBrace,     // }
    LBracket,   // [
    RBracket,   // ]
    Comma,      // ,
    Semi,       // ;
    Colon,      // :
    At,         // @
    Hash,       // #
    Underscore, // _

    // ── Special ──────────────────────────────────────────
    Eof,
}

impl TokenKind {
    /// True if this token kind is a keyword, false if it is an identifier.
    pub fn is_keyword(&self) -> bool {
        matches!(
            self,
            TokenKind::KwLet
                | TokenKind::KwMut
                | TokenKind::KwFn
                | TokenKind::KwFun
                | TokenKind::KwReturn
                | TokenKind::KwIf
                | TokenKind::KwElse
                | TokenKind::KwFor
                | TokenKind::KwIn
                | TokenKind::KwWhile
                | TokenKind::KwLoop
                | TokenKind::KwBreak
                | TokenKind::KwContinue
                | TokenKind::KwStruct
                | TokenKind::KwEnum
                | TokenKind::KwConst
                | TokenKind::KwStatic
                | TokenKind::KwImpl
                | TokenKind::KwExtend
                | TokenKind::KwTrait
                | TokenKind::KwInterface
                | TokenKind::KwImplements
                | TokenKind::KwMatch
                | TokenKind::KwAs
                | TokenKind::KwUse
                | TokenKind::KwImport
                | TokenKind::KwExport
                | TokenKind::KwFrom
                | TokenKind::KwMod
                | TokenKind::KwPub
                | TokenKind::KwSelf
                | TokenKind::KwSuper
                | TokenKind::KwCrate
                | TokenKind::KwType
                | TokenKind::KwAsync
                | TokenKind::KwAwait
                | TokenKind::KwUnsafe
                | TokenKind::KwErrdefer
                | TokenKind::KwDefer
                | TokenKind::KwWhere
                | TokenKind::KwAbility
                | TokenKind::KwCapability
                | TokenKind::KwMove
                | TokenKind::KwExtern
                | TokenKind::KwDyn
        )
    }
}

// ─── Token ────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct Token {
    pub kind: TokenKind,
    /// Byte offset of the first character of this token in the source.
    pub start: ByteOffset,
    /// Byte offset one past the last character of this token.
    pub end: ByteOffset,
}

impl Token {
    fn new(kind: TokenKind, start: u32, end: u32) -> Self {
        Self {
            kind,
            start: ByteOffset(start),
            end: ByteOffset(end),
        }
    }
}

// ─── Lexer ────────────────────────────────────────────────────────────────────

/// Keyword lookup table.
fn keyword(s: &str) -> Option<TokenKind> {
    match s {
        "let" => Some(TokenKind::KwLet),
        "mut" => Some(TokenKind::KwMut),
        "fn" => Some(TokenKind::KwFn),
        "fun" => Some(TokenKind::KwFun),
        "return" => Some(TokenKind::KwReturn),
        "if" => Some(TokenKind::KwIf),
        "else" => Some(TokenKind::KwElse),
        "for" => Some(TokenKind::KwFor),
        "in" => Some(TokenKind::KwIn),
        "while" => Some(TokenKind::KwWhile),
        "loop" => Some(TokenKind::KwLoop),
        "break" => Some(TokenKind::KwBreak),
        "continue" => Some(TokenKind::KwContinue),
        "struct" => Some(TokenKind::KwStruct),
        "enum" => Some(TokenKind::KwEnum),
        "const" => Some(TokenKind::KwConst),
        "static" => Some(TokenKind::KwStatic),
        "impl" => Some(TokenKind::KwImpl),
        "extend" => Some(TokenKind::KwExtend),
        "trait" => Some(TokenKind::KwTrait),
        "interface" => Some(TokenKind::KwInterface),
        "implements" => Some(TokenKind::KwImplements),
        "match" => Some(TokenKind::KwMatch),
        "as" => Some(TokenKind::KwAs),
        "use" => Some(TokenKind::KwUse),
        "import" => Some(TokenKind::KwImport),
        "export" => Some(TokenKind::KwExport),
        "from" => Some(TokenKind::KwFrom),
        "mod" => Some(TokenKind::KwMod),
        "pub" => Some(TokenKind::KwPub),
        "self" => Some(TokenKind::KwSelf),
        "super" => Some(TokenKind::KwSuper),
        "crate" => Some(TokenKind::KwCrate),
        "type" => Some(TokenKind::KwType),
        "async" => Some(TokenKind::KwAsync),
        "await" => Some(TokenKind::KwAwait),
        "unsafe" => Some(TokenKind::KwUnsafe),
        "errdefer" => Some(TokenKind::KwErrdefer),
        "defer" => Some(TokenKind::KwDefer),
        "where" => Some(TokenKind::KwWhere),
        "ability" => Some(TokenKind::KwAbility),
        "capability" => Some(TokenKind::KwCapability),
        "move" => Some(TokenKind::KwMove),
        "extern" => Some(TokenKind::KwExtern),
        "dyn" => Some(TokenKind::KwDyn),
        "true" => Some(TokenKind::Bool(true)),
        "false" => Some(TokenKind::Bool(false)),
        "_" => Some(TokenKind::Underscore),
        _ => None,
    }
}

struct Lexer<'src> {
    src: &'src [u8],
    pos: usize,
    tokens: Vec<Token>,
    errors: Vec<LexError>,
}

#[derive(Debug)]
pub struct LexError {
    pub message: String,
    pub offset: u32,
}

impl<'src> Lexer<'src> {
    fn new(src: &'src str) -> Self {
        Self {
            src: src.as_bytes(),
            pos: 0,
            tokens: Vec::new(),
            errors: Vec::new(),
        }
    }

    fn peek(&self) -> Option<u8> {
        self.src.get(self.pos).copied()
    }

    fn peek_at(&self, offset: usize) -> Option<u8> {
        self.src.get(self.pos + offset).copied()
    }

    fn advance(&mut self) -> Option<u8> {
        let c = self.src.get(self.pos).copied();
        if c.is_some() {
            self.pos += 1;
        }
        c
    }

    fn eat(&mut self, expected: u8) -> bool {
        if self.peek() == Some(expected) {
            self.pos += 1;
            true
        } else {
            false
        }
    }

    fn skip_whitespace_and_comments(&mut self) {
        loop {
            while matches!(self.peek(), Some(b' ' | b'\t' | b'\r' | b'\n')) {
                self.pos += 1;
            }
            if self.peek() == Some(b'/') {
                if self.peek_at(1) == Some(b'/') {
                    // Line comment
                    while matches!(self.peek(), Some(c) if c != b'\n') {
                        self.pos += 1;
                    }
                    continue;
                } else if self.peek_at(1) == Some(b'*') {
                    // Block comment (nested)
                    self.pos += 2;
                    let mut depth = 1usize;
                    while depth > 0 {
                        match (self.peek(), self.peek_at(1)) {
                            (Some(b'/'), Some(b'*')) => {
                                self.pos += 2;
                                depth += 1;
                                if depth > MAX_COMMENT_NESTING {
                                    self.error("block comment nesting limit exceeded");
                                    break;
                                }
                            }
                            (Some(b'*'), Some(b'/')) => {
                                self.pos += 2;
                                depth -= 1;
                            }
                            (None, _) => {
                                self.error("unterminated block comment");
                                break;
                            }
                            _ => {
                                self.pos += 1;
                            }
                        }
                    }
                    continue;
                }
            }
            break;
        }
    }

    fn error(&mut self, msg: &str) {
        self.errors.push(LexError {
            message: msg.to_string(),
            offset: self.pos as u32,
        });
    }

    fn lex_string(&mut self, start: usize) -> Token {
        let mut value = String::new();
        loop {
            match self.peek() {
                None | Some(b'\n') => {
                    self.error("unterminated string literal");
                    break;
                }
                Some(b'"') => {
                    self.advance();
                    break;
                }
                Some(b'\\') => {
                    self.advance();
                    match self.advance() {
                        Some(b'n') => value.push('\n'),
                        Some(b't') => value.push('\t'),
                        Some(b'r') => value.push('\r'),
                        Some(b'\\') => value.push('\\'),
                        Some(b'"') => value.push('"'),
                        Some(b'0') => value.push('\0'),
                        Some(b'u') => {
                            if self.eat(b'{') {
                                let hex_start = self.pos;
                                while matches!(self.peek(), Some(c) if c.is_ascii_hexdigit()) {
                                    self.pos += 1;
                                }
                                let hex = std::str::from_utf8(&self.src[hex_start..self.pos])
                                    .unwrap_or("");
                                if self.eat(b'}') {
                                    if let Ok(n) = u32::from_str_radix(hex, 16) {
                                        if let Some(ch) = char::from_u32(n) {
                                            value.push(ch);
                                        } else {
                                            self.error("invalid unicode code point");
                                        }
                                    } else {
                                        self.error("invalid unicode escape");
                                    }
                                } else {
                                    self.error("expected '}' in unicode escape");
                                }
                            } else {
                                self.error("expected '{' in unicode escape");
                            }
                        }
                        _ => self.error("unknown escape sequence"),
                    }
                }
                Some(b) => {
                    self.advance();
                    value.push(b as char);
                }
            }
        }
        Token::new(TokenKind::StringLit(value), start as u32, self.pos as u32)
    }

    fn lex_char(&mut self, start: usize) -> Token {
        let ch = match self.peek() {
            Some(b'\\') => {
                self.advance();
                match self.advance() {
                    Some(b'n') => '\n',
                    Some(b't') => '\t',
                    Some(b'r') => '\r',
                    Some(b'\\') => '\\',
                    Some(b'\'') => '\'',
                    Some(b'0') => '\0',
                    _ => {
                        self.error("unknown char escape");
                        '?'
                    }
                }
            }
            Some(c) => {
                self.advance();
                c as char
            }
            None => {
                self.error("unterminated char literal");
                '?'
            }
        };
        if !self.eat(b'\'') {
            self.error("unterminated char literal — expected closing \"'\"");
        }
        Token::new(TokenKind::CharLit(ch), start as u32, self.pos as u32)
    }

    fn lex_number(&mut self, first: u8, start: usize) -> Token {
        if first == b'0' {
            match self.peek() {
                Some(b'x') | Some(b'X') => {
                    self.advance();
                    return self.lex_integer(start, 16);
                }
                Some(b'o') | Some(b'O') => {
                    self.advance();
                    return self.lex_integer(start, 8);
                }
                Some(b'b') | Some(b'B') => {
                    self.advance();
                    return self.lex_integer(start, 2);
                }
                _ => {}
            }
        }

        // Decimal integer or float
        let num_start = start;
        // Already consumed `first`
        while matches!(self.peek(), Some(c) if c.is_ascii_digit() || c == b'_') {
            self.pos += 1;
        }

        let is_float = self.peek() == Some(b'.')
            && matches!(self.peek_at(1), Some(c) if c.is_ascii_digit())
            || self.peek() == Some(b'e')
            || self.peek() == Some(b'E');

        if is_float {
            if self.peek() == Some(b'.') {
                self.pos += 1;
                while matches!(self.peek(), Some(c) if c.is_ascii_digit() || c == b'_') {
                    self.pos += 1;
                }
            }
            if matches!(self.peek(), Some(b'e') | Some(b'E')) {
                self.pos += 1;
                if matches!(self.peek(), Some(b'+') | Some(b'-')) {
                    self.pos += 1;
                }
                while matches!(self.peek(), Some(c) if c.is_ascii_digit() || c == b'_') {
                    self.pos += 1;
                }
            }
            let raw = std::str::from_utf8(&self.src[num_start..self.pos]).unwrap_or("0");
            let clean: String = raw.chars().filter(|&c| c != '_').collect();
            let val: f64 = clean.parse().unwrap_or(0.0);
            Token::new(TokenKind::Float(val), num_start as u32, self.pos as u32)
        } else {
            let raw = std::str::from_utf8(&self.src[num_start..self.pos]).unwrap_or("0");
            let clean: String = raw.chars().filter(|&c| c != '_').collect();
            let val: u128 = clean.parse().unwrap_or(0);
            Token::new(TokenKind::Integer(val), num_start as u32, self.pos as u32)
        }
    }

    fn lex_integer(&mut self, start: usize, radix: u32) -> Token {
        let digit_start = self.pos;
        while matches!(self.peek(), Some(c) if (c as char).is_digit(radix) || c == b'_') {
            self.pos += 1;
        }
        let raw = std::str::from_utf8(&self.src[digit_start..self.pos]).unwrap_or("0");
        let clean: String = raw.chars().filter(|&c| c != '_').collect();
        let val: u128 = u128::from_str_radix(&clean, radix).unwrap_or(0);
        Token::new(TokenKind::Integer(val), start as u32, self.pos as u32)
    }

    fn next_token(&mut self) -> Option<Token> {
        self.skip_whitespace_and_comments();
        let start = self.pos;
        let b = self.advance()?;

        let tok = match b {
            // String literal
            b'"' => self.lex_string(start),
            // Char literal
            b'\'' => self.lex_char(start),
            // Numbers
            b'0'..=b'9' => self.lex_number(b, start),
            // Identifiers / keywords
            b'a'..=b'z' | b'A'..=b'Z' | b'_' => {
                while matches!(self.peek(), Some(c) if c.is_ascii_alphanumeric() || c == b'_') {
                    self.pos += 1;
                }
                let raw = std::str::from_utf8(&self.src[start..self.pos]).unwrap_or("");
                let kind = keyword(raw).unwrap_or_else(|| TokenKind::Ident(raw.to_string()));
                Token::new(kind, start as u32, self.pos as u32)
            }
            // Operators and punctuation
            b'+' => {
                if self.eat(b'=') {
                    Token::new(TokenKind::PlusEq, start as u32, self.pos as u32)
                } else {
                    Token::new(TokenKind::Plus, start as u32, self.pos as u32)
                }
            }
            b'-' => {
                if self.eat(b'>') {
                    Token::new(TokenKind::Arrow, start as u32, self.pos as u32)
                } else if self.eat(b'=') {
                    Token::new(TokenKind::MinusEq, start as u32, self.pos as u32)
                } else {
                    Token::new(TokenKind::Minus, start as u32, self.pos as u32)
                }
            }
            b'*' => {
                if self.eat(b'=') {
                    Token::new(TokenKind::StarEq, start as u32, self.pos as u32)
                } else {
                    Token::new(TokenKind::Star, start as u32, self.pos as u32)
                }
            }
            b'/' => {
                if self.eat(b'=') {
                    Token::new(TokenKind::SlashEq, start as u32, self.pos as u32)
                } else {
                    Token::new(TokenKind::Slash, start as u32, self.pos as u32)
                }
            }
            b'%' => {
                if self.eat(b'=') {
                    Token::new(TokenKind::PercentEq, start as u32, self.pos as u32)
                } else {
                    Token::new(TokenKind::Percent, start as u32, self.pos as u32)
                }
            }
            b'^' => {
                if self.eat(b'=') {
                    Token::new(TokenKind::CaretEq, start as u32, self.pos as u32)
                } else {
                    Token::new(TokenKind::Caret, start as u32, self.pos as u32)
                }
            }
            b'&' => {
                if self.eat(b'&') {
                    Token::new(TokenKind::And, start as u32, self.pos as u32)
                } else if self.eat(b'=') {
                    Token::new(TokenKind::AmpersandEq, start as u32, self.pos as u32)
                } else {
                    Token::new(TokenKind::Ampersand, start as u32, self.pos as u32)
                }
            }
            b'|' => {
                if self.eat(b'|') {
                    Token::new(TokenKind::Or, start as u32, self.pos as u32)
                } else if self.eat(b'=') {
                    Token::new(TokenKind::PipeEq, start as u32, self.pos as u32)
                } else {
                    Token::new(TokenKind::Pipe, start as u32, self.pos as u32)
                }
            }
            b'~' => Token::new(TokenKind::Tilde, start as u32, self.pos as u32),
            b'!' => {
                if self.eat(b'=') {
                    Token::new(TokenKind::Ne, start as u32, self.pos as u32)
                } else {
                    Token::new(TokenKind::Bang, start as u32, self.pos as u32)
                }
            }
            b'=' => {
                if self.eat(b'=') {
                    Token::new(TokenKind::EqEq, start as u32, self.pos as u32)
                } else if self.eat(b'>') {
                    Token::new(TokenKind::FatArrow, start as u32, self.pos as u32)
                } else {
                    Token::new(TokenKind::Eq, start as u32, self.pos as u32)
                }
            }
            b'<' => {
                if self.eat(b'<') {
                    if self.eat(b'=') {
                        Token::new(TokenKind::ShlEq, start as u32, self.pos as u32)
                    } else {
                        Token::new(TokenKind::Shl, start as u32, self.pos as u32)
                    }
                } else if self.eat(b'=') {
                    Token::new(TokenKind::Le, start as u32, self.pos as u32)
                } else {
                    Token::new(TokenKind::Lt, start as u32, self.pos as u32)
                }
            }
            b'>' => {
                if self.eat(b'>') {
                    if self.eat(b'=') {
                        Token::new(TokenKind::ShrEq, start as u32, self.pos as u32)
                    } else {
                        Token::new(TokenKind::Shr, start as u32, self.pos as u32)
                    }
                } else if self.eat(b'=') {
                    Token::new(TokenKind::Ge, start as u32, self.pos as u32)
                } else {
                    Token::new(TokenKind::Gt, start as u32, self.pos as u32)
                }
            }
            b'.' => {
                if self.eat(b'.') {
                    if self.eat(b'=') {
                        Token::new(TokenKind::DotDotEq, start as u32, self.pos as u32)
                    } else {
                        Token::new(TokenKind::DotDot, start as u32, self.pos as u32)
                    }
                } else {
                    Token::new(TokenKind::Dot, start as u32, self.pos as u32)
                }
            }
            b':' => {
                if self.eat(b':') {
                    Token::new(TokenKind::DoubleColon, start as u32, self.pos as u32)
                } else {
                    Token::new(TokenKind::Colon, start as u32, self.pos as u32)
                }
            }
            b'?' => Token::new(TokenKind::Question, start as u32, self.pos as u32),
            b'(' => Token::new(TokenKind::LParen, start as u32, self.pos as u32),
            b')' => Token::new(TokenKind::RParen, start as u32, self.pos as u32),
            b'{' => Token::new(TokenKind::LBrace, start as u32, self.pos as u32),
            b'}' => Token::new(TokenKind::RBrace, start as u32, self.pos as u32),
            b'[' => Token::new(TokenKind::LBracket, start as u32, self.pos as u32),
            b']' => Token::new(TokenKind::RBracket, start as u32, self.pos as u32),
            b',' => Token::new(TokenKind::Comma, start as u32, self.pos as u32),
            b';' => Token::new(TokenKind::Semi, start as u32, self.pos as u32),
            b'@' => Token::new(TokenKind::At, start as u32, self.pos as u32),
            b'#' => Token::new(TokenKind::Hash, start as u32, self.pos as u32),
            _ => {
                self.error(&format!("unexpected character: '{}'", b as char));
                return self.next_token();
            }
        };

        Some(tok)
    }

    fn run(mut self) -> (Vec<Token>, Vec<LexError>) {
        if self.src.len() > MAX_SOURCE_BYTES {
            self.error("source file exceeds maximum size");
            let eof_pos = self.pos as u32;
            self.tokens
                .push(Token::new(TokenKind::Eof, eof_pos, eof_pos));
            return (self.tokens, self.errors);
        }

        while let Some(tok) = self.next_token() {
            self.tokens.push(tok);
            if self.tokens.len() >= MAX_TOKEN_COUNT {
                self.error("token limit exceeded");
                break;
            }
        }
        let eof_pos = self.pos as u32;
        self.tokens
            .push(Token::new(TokenKind::Eof, eof_pos, eof_pos));
        (self.tokens, self.errors)
    }
}

/// Lex a source string into tokens.
/// Lex errors are returned separately; the token stream always ends with `Eof`.
pub fn lex(src: &str) -> Vec<Token> {
    let (tokens, _errors) = lex_with_errors(src);
    tokens
}

/// Lex a source string, returning both tokens and any lexing errors.
pub fn lex_with_errors(src: &str) -> (Vec<Token>, Vec<LexError>) {
    Lexer::new(src).run()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn kinds(src: &str) -> Vec<TokenKind> {
        lex(src).into_iter().map(|t| t.kind).collect()
    }

    #[test]
    fn test_keywords() {
        let result =
            kinds("let mut fn fun return if else for in while loop break continue const static implements");
        assert_eq!(result[0], TokenKind::KwLet);
        assert_eq!(result[1], TokenKind::KwMut);
        assert_eq!(result[2], TokenKind::KwFn);
        assert_eq!(result[3], TokenKind::KwFun);
        assert_eq!(result[10], TokenKind::KwLoop);
        assert_eq!(result[11], TokenKind::KwBreak);
        assert_eq!(result[12], TokenKind::KwContinue);
        assert_eq!(result[13], TokenKind::KwConst);
        assert_eq!(result[14], TokenKind::KwStatic);
        assert_eq!(result[15], TokenKind::KwImplements);
    }

    #[test]
    fn test_integers() {
        assert_eq!(kinds("42"), vec![TokenKind::Integer(42), TokenKind::Eof]);
        assert_eq!(kinds("0xff"), vec![TokenKind::Integer(255), TokenKind::Eof]);
        assert_eq!(
            kinds("0b1010"),
            vec![TokenKind::Integer(10), TokenKind::Eof]
        );
        assert_eq!(kinds("0o17"), vec![TokenKind::Integer(15), TokenKind::Eof]);
        assert_eq!(
            kinds("1_000_000"),
            vec![TokenKind::Integer(1_000_000), TokenKind::Eof]
        );
    }

    #[test]
    fn test_floats() {
        assert!(matches!(kinds("3.14")[0], TokenKind::Float(_)));
        assert!(matches!(kinds("1e10")[0], TokenKind::Float(_)));
    }

    #[test]
    fn test_string() {
        let toks = lex(r#""hello\nworld""#);
        assert_eq!(toks[0].kind, TokenKind::StringLit("hello\nworld".into()));
    }

    #[test]
    fn test_operators() {
        assert_eq!(kinds("->"), vec![TokenKind::Arrow, TokenKind::Eof]);
        assert_eq!(kinds("=>"), vec![TokenKind::FatArrow, TokenKind::Eof]);
        assert_eq!(kinds("..="), vec![TokenKind::DotDotEq, TokenKind::Eof]);
        assert_eq!(kinds("::"), vec![TokenKind::DoubleColon, TokenKind::Eof]);
    }

    #[test]
    fn test_comments_skipped() {
        assert_eq!(
            kinds("// comment\n42"),
            vec![TokenKind::Integer(42), TokenKind::Eof]
        );
        assert_eq!(
            kinds("/* block */ 42"),
            vec![TokenKind::Integer(42), TokenKind::Eof]
        );
    }

    #[test]
    fn test_errdefer_keyword() {
        assert_eq!(
            kinds("errdefer"),
            vec![TokenKind::KwErrdefer, TokenKind::Eof]
        );
    }

    #[test]
    fn test_source_size_limit() {
        let src = "a".repeat(MAX_SOURCE_BYTES + 1);
        let (_tokens, errors) = lex_with_errors(&src);
        assert!(errors
            .iter()
            .any(|error| error.message.contains("maximum size")));
    }
}
