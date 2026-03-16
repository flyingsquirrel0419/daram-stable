use super::*;

pub(super) fn token_kind_description(kind: &TokenKind) -> String {
    match kind {
        TokenKind::Integer(value) => format!("integer literal `{value}`"),
        TokenKind::Float(value) => format!("float literal `{value}`"),
        TokenKind::StringLit(value) => format!("string literal \"{}\"", value),
        TokenKind::CharLit(value) => format!("char literal `{value}`"),
        TokenKind::Bool(value) => format!("boolean literal `{value}`"),
        TokenKind::Ident(name) => format!("identifier `{name}`"),
        TokenKind::KwLet => "`let`".to_string(),
        TokenKind::KwMut => "`mut`".to_string(),
        TokenKind::KwFn => "`fn`".to_string(),
        TokenKind::KwFun => "`fun`".to_string(),
        TokenKind::KwReturn => "`return`".to_string(),
        TokenKind::KwIf => "`if`".to_string(),
        TokenKind::KwElse => "`else`".to_string(),
        TokenKind::KwFor => "`for`".to_string(),
        TokenKind::KwIn => "`in`".to_string(),
        TokenKind::KwWhile => "`while`".to_string(),
        TokenKind::KwLoop => "`loop`".to_string(),
        TokenKind::KwBreak => "`break`".to_string(),
        TokenKind::KwContinue => "`continue`".to_string(),
        TokenKind::KwStruct => "`struct`".to_string(),
        TokenKind::KwEnum => "`enum`".to_string(),
        TokenKind::KwConst => "`const`".to_string(),
        TokenKind::KwStatic => "`static`".to_string(),
        TokenKind::KwImpl => "`impl`".to_string(),
        TokenKind::KwExtend => "`extend`".to_string(),
        TokenKind::KwTrait => "`trait`".to_string(),
        TokenKind::KwInterface => "`interface`".to_string(),
        TokenKind::KwImplements => "`implements`".to_string(),
        TokenKind::KwMatch => "`match`".to_string(),
        TokenKind::KwAs => "`as`".to_string(),
        TokenKind::KwUse => "`use`".to_string(),
        TokenKind::KwImport => "`import`".to_string(),
        TokenKind::KwExport => "`export`".to_string(),
        TokenKind::KwFrom => "`from`".to_string(),
        TokenKind::KwMod => "`mod`".to_string(),
        TokenKind::KwPub => "`pub`".to_string(),
        TokenKind::KwSelf => "`self`".to_string(),
        TokenKind::KwSuper => "`super`".to_string(),
        TokenKind::KwCrate => "`crate`".to_string(),
        TokenKind::KwType => "`type`".to_string(),
        TokenKind::KwAsync => "`async`".to_string(),
        TokenKind::KwAwait => "`await`".to_string(),
        TokenKind::KwUnsafe => "`unsafe`".to_string(),
        TokenKind::KwErrdefer => "`errdefer`".to_string(),
        TokenKind::KwDefer => "`defer`".to_string(),
        TokenKind::KwWhere => "`where`".to_string(),
        TokenKind::KwAbility => "`ability`".to_string(),
        TokenKind::KwCapability => "`capability`".to_string(),
        TokenKind::KwMove => "`move`".to_string(),
        TokenKind::Plus => "`+`".to_string(),
        TokenKind::Minus => "`-`".to_string(),
        TokenKind::Star => "`*`".to_string(),
        TokenKind::Slash => "`/`".to_string(),
        TokenKind::Percent => "`%`".to_string(),
        TokenKind::Caret => "`^`".to_string(),
        TokenKind::Ampersand => "`&`".to_string(),
        TokenKind::Pipe => "`|`".to_string(),
        TokenKind::Tilde => "`~`".to_string(),
        TokenKind::Shl => "`<<`".to_string(),
        TokenKind::Shr => "`>>`".to_string(),
        TokenKind::Eq => "`=`".to_string(),
        TokenKind::EqEq => "`==`".to_string(),
        TokenKind::Ne => "`!=`".to_string(),
        TokenKind::Lt => "`<`".to_string(),
        TokenKind::Le => "`<=`".to_string(),
        TokenKind::Gt => "`>`".to_string(),
        TokenKind::Ge => "`>=`".to_string(),
        TokenKind::And => "`&&`".to_string(),
        TokenKind::Or => "`||`".to_string(),
        TokenKind::Bang => "`!`".to_string(),
        TokenKind::Question => "`?`".to_string(),
        TokenKind::Arrow => "`->`".to_string(),
        TokenKind::FatArrow => "`=>`".to_string(),
        TokenKind::DotDot => "`..`".to_string(),
        TokenKind::DotDotEq => "`..=`".to_string(),
        TokenKind::Dot => "`.`".to_string(),
        TokenKind::DoubleColon => "`::`".to_string(),
        TokenKind::PlusEq => "`+=`".to_string(),
        TokenKind::MinusEq => "`-=`".to_string(),
        TokenKind::StarEq => "`*=`".to_string(),
        TokenKind::SlashEq => "`/=`".to_string(),
        TokenKind::PercentEq => "`%=`".to_string(),
        TokenKind::AmpersandEq => "`&=`".to_string(),
        TokenKind::PipeEq => "`|=`".to_string(),
        TokenKind::CaretEq => "`^=`".to_string(),
        TokenKind::ShlEq => "`<<=`".to_string(),
        TokenKind::ShrEq => "`>>=`".to_string(),
        TokenKind::LParen => "`(`".to_string(),
        TokenKind::RParen => "`)`".to_string(),
        TokenKind::LBrace => "`{`".to_string(),
        TokenKind::RBrace => "`}`".to_string(),
        TokenKind::LBracket => "`[`".to_string(),
        TokenKind::RBracket => "`]`".to_string(),
        TokenKind::Comma => "`,`".to_string(),
        TokenKind::Semi => "`;`".to_string(),
        TokenKind::Colon => "`:`".to_string(),
        TokenKind::At => "`@`".to_string(),
        TokenKind::Hash => "`#`".to_string(),
        TokenKind::Underscore => "`_`".to_string(),
        TokenKind::Eof => "end of file".to_string(),
        TokenKind::KwExtern => "`extern`".to_string(),
        TokenKind::KwDyn => "`dyn`".to_string(),
    }
}

impl<'a> Parser<'a> {
    pub(super) fn current_token_description(&self) -> String {
        token_kind_description(self.peek())
    }

    pub(super) fn push_expected_diagnostic(
        &mut self,
        span: Span,
        message: &str,
        expected: Option<String>,
    ) {
        let found = self.current_token_description();
        let mut diagnostic = Diagnostic::error(message)
            .with_span(span)
            .with_label(Label::new(span, format!("found {}", found)));
        if let Some(expected) = expected {
            diagnostic = diagnostic.with_note(format!("expected {}", expected));
        }
        self.errors.push(diagnostic);
    }

    pub(super) fn expect(&mut self, kind: &TokenKind, msg: &str) -> Option<Span> {
        if self.peek() == kind {
            let start = self.peek_tok().start.0;
            let end = self.peek_tok().end.0;
            let file = self.file;
            self.advance();
            Some(Span::new(file, start, end))
        } else {
            let span = self.span_of_current();
            self.push_expected_diagnostic(span, msg, Some(token_kind_description(kind)));
            None
        }
    }

    pub(super) fn skip_balanced_delimiters(&mut self, open: &TokenKind, close: &TokenKind) {
        let mut depth = 1usize;
        while depth > 0 && !self.is_at_end() {
            match self.peek() {
                kind if kind == open => {
                    depth += 1;
                    self.advance();
                }
                kind if kind == close => {
                    depth = depth.saturating_sub(1);
                    self.advance();
                }
                _ => {
                    self.advance();
                }
            }
        }
    }
}
