//! Recursive-descent parser for the Daram language.
//!
//! The parser converts a flat token stream (from the lexer) into an AST.
//! Error recovery is minimal but sufficient for useful diagnostics.

use crate::{
    ast::*,
    diagnostics::{Diagnostic, Label},
    lexer::{Token, TokenKind},
    source::{FileId, Span},
};

const MAX_PARSE_TOKENS: usize = 200_000;
const MAX_DELIMITER_NESTING: usize = 512;

#[path = "parser_recovery.rs"]
mod recovery_impl;

// ─── Parser state ─────────────────────────────────────────────────────────────

struct Parser<'a> {
    file: FileId,
    tokens: &'a [Token],
    pos: usize,
    errors: Vec<Diagnostic>,
}

impl<'a> Parser<'a> {
    fn new(file: FileId, tokens: &'a [Token]) -> Self {
        Self {
            file,
            tokens,
            pos: 0,
            errors: Vec::new(),
        }
    }

    // ── Token access ──────────────────────────────────────────────────────────

    fn peek(&self) -> &TokenKind {
        &self.tokens[self.pos.min(self.tokens.len() - 1)].kind
    }

    fn peek_tok(&self) -> &Token {
        &self.tokens[self.pos.min(self.tokens.len() - 1)]
    }

    fn advance(&mut self) -> &Token {
        let tok = &self.tokens[self.pos.min(self.tokens.len() - 1)];
        if self.pos < self.tokens.len() - 1 {
            self.pos += 1;
        }
        tok
    }

    fn span_of_current(&self) -> Span {
        let tok = self.peek_tok();
        Span::new(self.file, tok.start.0, tok.end.0)
    }

    fn at(&self, kind: &TokenKind) -> bool {
        self.peek() == kind
    }

    fn eat(&mut self, kind: &TokenKind) -> bool {
        if self.peek() == kind {
            self.advance();
            true
        } else {
            false
        }
    }

    fn is_at_end(&self) -> bool {
        matches!(self.peek(), TokenKind::Eof)
    }

    // ── Identifier parsing ────────────────────────────────────────────────────

    fn parse_ident(&mut self) -> Option<Ident> {
        let span = self.span_of_current();
        let name = match self.peek().clone() {
            TokenKind::Ident(name) => name,
            TokenKind::KwFrom => "from".to_string(),
            _ => {
                self.push_expected_diagnostic(
                    span,
                    "expected identifier",
                    Some("an identifier like `name`".to_string()),
                );
                return None;
            }
        };
        self.advance();
        Some(Ident::new(name, span))
    }

    fn parse_path_segment(&mut self) -> Option<Ident> {
        let span = self.span_of_current();
        let name = match self.peek().clone() {
            TokenKind::Ident(name) => name,
            TokenKind::KwFrom => "from".to_string(),
            TokenKind::KwSelf => "self".to_string(),
            TokenKind::KwSuper => "super".to_string(),
            TokenKind::KwCrate => "crate".to_string(),
            _ => {
                self.push_expected_diagnostic(
                    span,
                    "expected identifier",
                    Some("an identifier, `from`, `self`, `super`, or `crate`".to_string()),
                );
                return None;
            }
        };
        self.advance();
        Some(Ident::new(name, span))
    }

    fn parse_path(&mut self) -> Option<Path> {
        let start = self.span_of_current();
        let mut segments = Vec::new();
        let first = self.parse_path_segment()?;
        segments.push(first);
        while self.at(&TokenKind::DoubleColon)
            && matches!(
                self.tokens.get(self.pos + 1).map(|token| &token.kind),
                Some(
                    TokenKind::Ident(_)
                        | TokenKind::KwFrom
                        | TokenKind::KwSelf
                        | TokenKind::KwSuper
                        | TokenKind::KwCrate
                )
            )
        {
            self.advance();
            if let Some(seg) = self.parse_path_segment() {
                segments.push(seg);
            } else {
                break;
            }
        }
        let end = segments.last().map(|i| i.span).unwrap_or(start);
        Some(Path {
            span: start.merge(end),
            segments,
        })
    }

    // ── Visibility ────────────────────────────────────────────────────────────

    fn parse_visibility(&mut self) -> Visibility {
        if self.at(&TokenKind::KwPub) || self.at(&TokenKind::KwExport) {
            let s = self.peek_tok().start.0;
            let e = self.peek_tok().end.0;
            let file = self.file;
            self.advance();
            Visibility::public(Span::new(file, s, e))
        } else {
            Visibility::private()
        }
    }

    fn parse_derive_attrs(&mut self) -> Vec<Path> {
        let mut derives = Vec::new();
        while self.eat(&TokenKind::At) {
            let start = self.span_of_current();
            let Some(name) = self.parse_ident() else {
                break;
            };
            if name.name != "derive" {
                self.errors.push(
                    Diagnostic::error(format!("unsupported attribute `@{}`", name.name))
                        .with_span(name.span)
                        .with_note("only `@derive(...)` is supported right now"),
                );
                if self.eat(&TokenKind::LParen) {
                    self.skip_balanced_delimiters(&TokenKind::LParen, &TokenKind::RParen);
                }
                continue;
            }
            self.expect(&TokenKind::LParen, "expected '(' after @derive");
            while !self.at(&TokenKind::RParen) && !self.is_at_end() {
                if let Some(path) = self.parse_path() {
                    derives.push(path);
                } else {
                    break;
                }
                if !self.eat(&TokenKind::Comma) {
                    break;
                }
            }
            let end = self.span_of_current();
            self.expect(&TokenKind::RParen, "expected ')' after @derive arguments");
            if derives.is_empty() {
                self.errors.push(
                    Diagnostic::error("`@derive(...)` requires at least one ability name")
                        .with_span(start.merge(end)),
                );
            }
        }
        derives
    }

    // ── Generics ──────────────────────────────────────────────────────────────

    fn parse_generic_params(&mut self) -> GenericParams {
        if !self.at(&TokenKind::Lt) {
            return GenericParams::default();
        }
        let start = self.span_of_current();
        self.advance(); // `<`
        let mut params = Vec::new();
        while !self.at(&TokenKind::Gt) && !self.is_at_end() {
            let pspan = self.span_of_current();
            let name = match self.parse_ident() {
                Some(n) => n,
                None => break,
            };
            let mut bounds = Vec::new();
            if self.eat(&TokenKind::Colon) {
                loop {
                    if let Some(b) = self.parse_type_expr() {
                        bounds.push(b);
                    }
                    if !self.eat(&TokenKind::Plus) {
                        break;
                    }
                }
            }
            let default = if self.eat(&TokenKind::Eq) {
                self.parse_type_expr()
            } else {
                None
            };
            params.push(GenericParam {
                name,
                bounds,
                default,
                span: pspan,
            });
            if !self.eat(&TokenKind::Comma) {
                break;
            }
        }
        let end = self.span_of_current();
        self.expect(&TokenKind::Gt, "expected '>' to close generic params");
        GenericParams {
            params,
            span: Some(start.merge(end)),
        }
    }

    fn parse_where_clause(&mut self) -> Vec<WherePredicate> {
        if !self.at(&TokenKind::KwWhere) {
            return Vec::new();
        }
        self.advance();
        let mut preds = Vec::new();
        loop {
            let span = self.span_of_current();
            let ty = match self.parse_type_expr() {
                Some(t) => t,
                None => break,
            };
            let mut bounds = Vec::new();
            if self.eat(&TokenKind::Colon) {
                loop {
                    if let Some(b) = self.parse_type_expr() {
                        bounds.push(b);
                    }
                    if !self.eat(&TokenKind::Plus) {
                        break;
                    }
                }
            }
            preds.push(WherePredicate { ty, bounds, span });
            if !self.eat(&TokenKind::Comma) {
                break;
            }
            // Allow trailing comma before `{`
            if self.at(&TokenKind::LBrace) {
                break;
            }
        }
        preds
    }

    // ── Type expressions ──────────────────────────────────────────────────────

    fn parse_type_expr(&mut self) -> Option<TypeExpr> {
        let span = self.span_of_current();
        match self.peek() {
            TokenKind::Ampersand => {
                self.advance();
                let mutable = self.eat(&TokenKind::KwMut);
                let inner = Box::new(self.parse_type_expr()?);
                let end = inner.span();
                Some(TypeExpr::Ref {
                    mutable,
                    inner,
                    span: span.merge(end),
                })
            }
            TokenKind::LParen => {
                self.advance();
                if self.eat(&TokenKind::RParen) {
                    return Some(TypeExpr::Tuple {
                        elems: Vec::new(),
                        span,
                    });
                }
                let mut elems = Vec::new();
                loop {
                    if let Some(t) = self.parse_type_expr() {
                        elems.push(t);
                    }
                    if !self.eat(&TokenKind::Comma) {
                        break;
                    }
                    if self.at(&TokenKind::RParen) {
                        break;
                    }
                }
                let end = self.span_of_current();
                self.expect(&TokenKind::RParen, "expected ')'");
                Some(TypeExpr::Tuple {
                    elems,
                    span: span.merge(end),
                })
            }
            TokenKind::LBracket => {
                self.advance();
                let elem = Box::new(self.parse_type_expr()?);
                if self.eat(&TokenKind::Semi) {
                    let len = Box::new(self.parse_expr()?);
                    let end = self.span_of_current();
                    self.expect(&TokenKind::RBracket, "expected ']'");
                    Some(TypeExpr::Array {
                        elem,
                        len,
                        span: span.merge(end),
                    })
                } else {
                    let end = self.span_of_current();
                    self.expect(&TokenKind::RBracket, "expected ']'");
                    Some(TypeExpr::Slice {
                        elem,
                        span: span.merge(end),
                    })
                }
            }
            TokenKind::Bang => {
                self.advance();
                Some(TypeExpr::Never { span })
            }
            TokenKind::KwFn | TokenKind::KwFun => {
                self.advance();
                self.expect(&TokenKind::LParen, "expected '(' in fn type");
                let mut params = Vec::new();
                while !self.at(&TokenKind::RParen) && !self.is_at_end() {
                    if let Some(t) = self.parse_type_expr() {
                        params.push(t);
                    }
                    if !self.eat(&TokenKind::Comma) {
                        break;
                    }
                }
                self.expect(&TokenKind::RParen, "expected ')'");
                let ret = if self.eat(&TokenKind::Colon) || self.eat(&TokenKind::Arrow) {
                    self.parse_type_expr().map(Box::new)
                } else {
                    None
                };
                let end = self.span_of_current();
                Some(TypeExpr::FnPtr {
                    params,
                    ret,
                    span: span.merge(end),
                })
            }
            TokenKind::KwDyn => {
                self.advance();
                if let Some(ability) = self.parse_path() {
                    Some(TypeExpr::DynTrait { ability, span })
                } else {
                    self.errors.push(
                        Diagnostic::error("expected ability name after `dyn`").with_span(span),
                    );
                    Some(TypeExpr::Infer { span })
                }
            }
            TokenKind::KwSelf => {
                self.advance();
                Some(TypeExpr::SelfType { span })
            }
            TokenKind::Underscore => {
                self.advance();
                Some(TypeExpr::Infer { span })
            }
            TokenKind::Ident(_) | TokenKind::KwCrate | TokenKind::KwSuper => {
                let path = self.parse_path()?;
                let mut generics = Vec::new();
                if self.at(&TokenKind::Lt) {
                    self.advance();
                    while !self.at(&TokenKind::Gt) && !self.is_at_end() {
                        if let Some(t) = self.parse_type_expr() {
                            generics.push(t);
                        }
                        if !self.eat(&TokenKind::Comma) {
                            break;
                        }
                    }
                    self.expect(&TokenKind::Gt, "expected '>'");
                }
                let end = self.span_of_current();
                Some(TypeExpr::Named {
                    path,
                    generics,
                    span: span.merge(end),
                })
            }
            _ => {
                self.push_expected_diagnostic(
                    span,
                    "expected type",
                    Some("a type like `i32`, `Foo`, `&T`, or `(A, B)`".to_string()),
                );
                None
            }
        }
    }

    // ── Patterns ──────────────────────────────────────────────────────────────

    fn parse_pattern(&mut self) -> Option<Pattern> {
        let span = self.span_of_current();
        let mut pat = self.parse_pattern_inner()?;
        if self.at(&TokenKind::DotDot) || self.at(&TokenKind::DotDotEq) {
            let inclusive = self.eat(&TokenKind::DotDotEq);
            if !inclusive {
                self.expect(&TokenKind::DotDot, "expected '..' in range pattern");
            }
            let hi = self.parse_pattern_inner()?;
            let end = hi.span();
            pat = Pattern::Range {
                lo: Box::new(pat),
                hi: Box::new(hi),
                inclusive,
                span: span.merge(end),
            };
        }
        // Or-pattern
        if self.at(&TokenKind::Pipe) {
            let mut alts = vec![pat];
            while self.eat(&TokenKind::Pipe) {
                if let Some(p) = self.parse_pattern_inner() {
                    alts.push(p);
                }
            }
            let end = alts.last().map(|p| p.span()).unwrap_or(span);
            return Some(Pattern::Or {
                alternatives: alts,
                span: span.merge(end),
            });
        }
        Some(pat)
    }

    fn parse_pattern_inner(&mut self) -> Option<Pattern> {
        let span = self.span_of_current();
        match self.peek().clone() {
            TokenKind::Underscore => {
                self.advance();
                Some(Pattern::Wildcard { span })
            }
            TokenKind::KwSelf => {
                let name = self.parse_path_segment()?;
                Some(Pattern::Ident {
                    mutable: false,
                    name,
                })
            }
            TokenKind::KwMut => {
                self.advance();
                let name = self.parse_path_segment()?;
                Some(Pattern::Ident {
                    mutable: true,
                    name,
                })
            }
            TokenKind::Bool(b) => {
                self.advance();
                Some(Pattern::Literal {
                    lit: Literal::Bool(b),
                    span,
                })
            }
            TokenKind::Integer(n) => {
                self.advance();
                Some(Pattern::Literal {
                    lit: Literal::Integer(n),
                    span,
                })
            }
            TokenKind::Float(f) => {
                self.advance();
                Some(Pattern::Literal {
                    lit: Literal::Float(f),
                    span,
                })
            }
            TokenKind::StringLit(s) => {
                self.advance();
                Some(Pattern::Literal {
                    lit: Literal::String(s),
                    span,
                })
            }
            TokenKind::CharLit(c) => {
                self.advance();
                Some(Pattern::Literal {
                    lit: Literal::Char(c),
                    span,
                })
            }
            TokenKind::Ampersand => {
                self.advance();
                let mutable = self.eat(&TokenKind::KwMut);
                let inner = Box::new(self.parse_pattern()?);
                let end = inner.span();
                Some(Pattern::Ref {
                    mutable,
                    inner,
                    span: span.merge(end),
                })
            }
            TokenKind::LParen => {
                self.advance();
                if self.eat(&TokenKind::RParen) {
                    return Some(Pattern::Literal {
                        lit: Literal::Unit,
                        span,
                    });
                }
                let mut elems = Vec::new();
                loop {
                    if let Some(p) = self.parse_pattern() {
                        elems.push(p);
                    }
                    if !self.eat(&TokenKind::Comma) {
                        break;
                    }
                    if self.at(&TokenKind::RParen) {
                        break;
                    }
                }
                let end = self.span_of_current();
                self.expect(&TokenKind::RParen, "expected ')'");
                Some(Pattern::Tuple {
                    elems,
                    span: span.merge(end),
                })
            }
            TokenKind::LBracket => {
                self.advance();
                let mut elems = Vec::new();
                let mut rest_index = None;
                while !self.at(&TokenKind::RBracket) && !self.is_at_end() {
                    if self.eat(&TokenKind::DotDot) {
                        if rest_index.is_some() {
                            self.errors.push(
                                Diagnostic::error("slice pattern can contain at most one `..`")
                                    .with_span(span),
                            );
                        } else {
                            rest_index = Some(elems.len());
                        }
                    } else if let Some(pattern) = self.parse_pattern() {
                        elems.push(pattern);
                    } else {
                        break;
                    }
                    if !self.eat(&TokenKind::Comma) {
                        break;
                    }
                }
                let end = self.span_of_current();
                self.expect(&TokenKind::RBracket, "expected ']'");
                Some(Pattern::Slice {
                    elems,
                    rest_index,
                    span: span.merge(end),
                })
            }
            TokenKind::Ident(_) => {
                let path = self.parse_path()?;
                // Struct pattern?
                if self.at(&TokenKind::LBrace) {
                    self.advance();
                    let mut fields = Vec::new();
                    let mut rest = false;
                    while !self.at(&TokenKind::RBrace) && !self.is_at_end() {
                        let fspan = self.span_of_current();
                        if self.eat(&TokenKind::DotDot) {
                            rest = true;
                            break;
                        }
                        let name = match self.parse_ident() {
                            Some(n) => n,
                            None => break,
                        };
                        let pattern = if self.eat(&TokenKind::Colon) {
                            self.parse_pattern()
                        } else {
                            None
                        };
                        fields.push(FieldPattern {
                            name,
                            pattern,
                            span: fspan,
                        });
                        if !self.eat(&TokenKind::Comma) {
                            break;
                        }
                    }
                    let end = self.span_of_current();
                    self.expect(&TokenKind::RBrace, "expected '}'");
                    return Some(Pattern::Struct {
                        path,
                        fields,
                        rest,
                        span: span.merge(end),
                    });
                }
                // Tuple variant?
                if self.at(&TokenKind::LParen) {
                    self.advance();
                    let mut args = Vec::new();
                    while !self.at(&TokenKind::RParen) && !self.is_at_end() {
                        if let Some(p) = self.parse_pattern() {
                            args.push(p);
                        }
                        if !self.eat(&TokenKind::Comma) {
                            break;
                        }
                    }
                    let end = self.span_of_current();
                    self.expect(&TokenKind::RParen, "expected ')'");
                    return Some(Pattern::Variant {
                        path,
                        args,
                        span: span.merge(end),
                    });
                }
                // Simple path / ident pattern
                if path.segments.len() == 1 {
                    Some(Pattern::Ident {
                        mutable: false,
                        name: path.segments.into_iter().next().unwrap(),
                    })
                } else {
                    Some(Pattern::Variant {
                        path,
                        args: Vec::new(),
                        span,
                    })
                }
            }
            _ => {
                self.push_expected_diagnostic(
                    span,
                    "expected pattern",
                    Some("a binding, literal, tuple, struct, or variant pattern".to_string()),
                );
                None
            }
        }
    }

    fn set_pattern_mutability(pattern: &mut Pattern, mutable: bool) {
        match pattern {
            Pattern::Ident { mutable: slot, .. } => *slot = mutable,
            Pattern::Tuple { elems, .. }
            | Pattern::Variant { args: elems, .. }
            | Pattern::Slice { elems, .. }
            | Pattern::Or {
                alternatives: elems,
                ..
            } => {
                for elem in elems {
                    Self::set_pattern_mutability(elem, mutable);
                }
            }
            Pattern::Struct { fields, .. } => {
                for field in fields {
                    if let Some(pattern) = &mut field.pattern {
                        Self::set_pattern_mutability(pattern, mutable);
                    }
                }
            }
            Pattern::Range { lo, hi, .. } => {
                Self::set_pattern_mutability(lo, mutable);
                Self::set_pattern_mutability(hi, mutable);
            }
            Pattern::Ref { inner, .. } => Self::set_pattern_mutability(inner, mutable),
            Pattern::Wildcard { .. } | Pattern::Literal { .. } => {}
        }
    }

    fn parse_import_source_path(&mut self) -> Option<Path> {
        let span = self.span_of_current();
        match self.peek().clone() {
            TokenKind::StringLit(value) => {
                self.advance();

                let normalized = value.replace("::", "/");
                let segments = normalized
                    .split('/')
                    .filter(|segment| !segment.is_empty())
                    .map(|segment| Ident::new(segment, span))
                    .collect::<Vec<_>>();
                if segments.is_empty() {
                    self.errors.push(
                        Diagnostic::error("module path string cannot be empty").with_span(span),
                    );
                    return None;
                }
                Some(Path { segments, span })
            }
            TokenKind::Ident(_) | TokenKind::KwCrate | TokenKind::KwSuper | TokenKind::KwSelf => {
                self.parse_path()
            }
            _ => {
                self.push_expected_diagnostic(
                    span,
                    "expected import source",
                    Some(
                        "a string like \"std/fs\" or a package path like `json_extra`".to_string(),
                    ),
                );
                None
            }
        }
    }

    fn parse_import_tree(&mut self) -> Option<UseTree> {
        let start = self.span_of_current();
        if self.eat(&TokenKind::Star) {
            self.expect(&TokenKind::KwAs, "expected 'as' after '*' in import");
            let alias = self.parse_ident()?;
            self.expect(&TokenKind::KwFrom, "expected 'from' in import");
            let prefix = self.parse_import_source_path()?;
            return Some(UseTree {
                prefix,
                kind: UseTreeKind::Alias(alias),
                span: start,
            });
        }

        if self.eat(&TokenKind::LBrace) {
            let mut children = Vec::new();
            while !self.at(&TokenKind::RBrace) && !self.is_at_end() {
                let child_span = self.span_of_current();
                let name = self.parse_ident()?;
                let child_prefix = Path {
                    span: name.span,
                    segments: vec![name.clone()],
                };
                let kind = if self.eat(&TokenKind::KwAs) {
                    UseTreeKind::Alias(self.parse_ident()?)
                } else {
                    UseTreeKind::Simple
                };
                children.push(UseTree {
                    prefix: child_prefix,
                    kind,
                    span: child_span,
                });
                if !self.eat(&TokenKind::Comma) {
                    break;
                }
            }
            self.expect(&TokenKind::RBrace, "expected '}'");
            self.expect(&TokenKind::KwFrom, "expected 'from' in import");
            let prefix = self.parse_import_source_path()?;
            return Some(UseTree {
                prefix,
                kind: UseTreeKind::Nested(children),
                span: start,
            });
        }

        let alias = self.parse_ident()?;
        self.expect(&TokenKind::KwFrom, "expected 'from' in import");
        let prefix = self.parse_import_source_path()?;
        Some(UseTree {
            prefix,
            kind: UseTreeKind::Alias(alias),
            span: start,
        })
    }

    // ── Expressions ───────────────────────────────────────────────────────────

    fn parse_expr(&mut self) -> Option<Expr> {
        self.parse_assign_expr()
    }

    fn parse_expr_fragment(file: FileId, tokens: &[Token]) -> Option<Expr> {
        let eof_offset = tokens.last().map(|token| token.end).unwrap_or_default();
        let mut fragment = tokens.to_vec();
        fragment.push(Token {
            kind: TokenKind::Eof,
            start: eof_offset,
            end: eof_offset,
        });

        let mut parser = Parser::new(file, &fragment);
        let expr = parser.parse_expr()?;
        if !parser.errors.is_empty() || !parser.is_at_end() {
            return None;
        }
        Some(expr)
    }

    fn find_control_flow_block_start(&self) -> Option<usize> {
        let mut paren_depth = 0usize;
        let mut bracket_depth = 0usize;
        let mut best = None;

        for idx in self.pos..self.tokens.len() {
            match self.tokens[idx].kind {
                TokenKind::LParen => paren_depth += 1,
                TokenKind::RParen => paren_depth = paren_depth.saturating_sub(1),
                TokenKind::LBracket => bracket_depth += 1,
                TokenKind::RBracket => bracket_depth = bracket_depth.saturating_sub(1),
                TokenKind::LBrace if paren_depth == 0 && bracket_depth == 0 => {
                    if idx > self.pos
                        && Self::parse_expr_fragment(self.file, &self.tokens[self.pos..idx])
                            .is_some()
                    {
                        best = Some(idx);
                    }
                }
                _ => {}
            }
        }

        best
    }

    fn parse_expr_before_block(&mut self) -> Option<Expr> {
        let block_start = self.find_control_flow_block_start()?;
        let expr = Self::parse_expr_fragment(self.file, &self.tokens[self.pos..block_start])?;
        self.pos = block_start;
        Some(expr)
    }

    fn parse_assign_expr(&mut self) -> Option<Expr> {
        let lhs = self.parse_range_expr()?;
        let span = lhs.span();
        let compound_op = match self.peek() {
            TokenKind::PlusEq => Some(CompoundOp::Add),
            TokenKind::MinusEq => Some(CompoundOp::Sub),
            TokenKind::StarEq => Some(CompoundOp::Mul),
            TokenKind::SlashEq => Some(CompoundOp::Div),
            TokenKind::PercentEq => Some(CompoundOp::Rem),
            TokenKind::AmpersandEq => Some(CompoundOp::BitAnd),
            TokenKind::PipeEq => Some(CompoundOp::BitOr),
            TokenKind::CaretEq => Some(CompoundOp::BitXor),
            TokenKind::ShlEq => Some(CompoundOp::Shl),
            TokenKind::ShrEq => Some(CompoundOp::Shr),
            _ => None,
        };
        if let Some(op) = compound_op {
            self.advance();
            let rhs = self.parse_assign_expr()?;
            let end = rhs.span();
            return Some(Expr::CompoundAssign {
                op,
                target: Box::new(lhs),
                value: Box::new(rhs),
                span: span.merge(end),
            });
        }
        if self.eat(&TokenKind::Eq) {
            let rhs = self.parse_assign_expr()?;
            let end = rhs.span();
            return Some(Expr::Assign {
                target: Box::new(lhs),
                value: Box::new(rhs),
                span: span.merge(end),
            });
        }
        Some(lhs)
    }

    fn parse_range_expr(&mut self) -> Option<Expr> {
        let lo = self.parse_or_expr()?;
        let span = lo.span();
        match self.peek() {
            TokenKind::DotDot => {
                self.advance();
                let hi = self.parse_or_expr().map(Box::new);
                let end = hi.as_ref().map(|e| e.span()).unwrap_or(span);
                return Some(Expr::Range {
                    lo: Some(Box::new(lo)),
                    hi,
                    inclusive: false,
                    span: span.merge(end),
                });
            }
            TokenKind::DotDotEq => {
                self.advance();
                let hi = self.parse_or_expr().map(Box::new);
                let end = hi.as_ref().map(|e| e.span()).unwrap_or(span);
                return Some(Expr::Range {
                    lo: Some(Box::new(lo)),
                    hi,
                    inclusive: true,
                    span: span.merge(end),
                });
            }
            _ => {}
        }
        Some(lo)
    }

    fn parse_or_expr(&mut self) -> Option<Expr> {
        let mut lhs = self.parse_and_expr()?;
        while self.at(&TokenKind::Or) {
            self.advance();
            let rhs = self.parse_and_expr()?;
            let span = lhs.span().merge(rhs.span());
            lhs = Expr::BinOp {
                op: BinOp::Or,
                lhs: Box::new(lhs),
                rhs: Box::new(rhs),
                span,
            };
        }
        Some(lhs)
    }

    fn parse_and_expr(&mut self) -> Option<Expr> {
        let mut lhs = self.parse_cmp_expr()?;
        while self.at(&TokenKind::And) {
            self.advance();
            let rhs = self.parse_cmp_expr()?;
            let span = lhs.span().merge(rhs.span());
            lhs = Expr::BinOp {
                op: BinOp::And,
                lhs: Box::new(lhs),
                rhs: Box::new(rhs),
                span,
            };
        }
        Some(lhs)
    }

    fn parse_cmp_expr(&mut self) -> Option<Expr> {
        let lhs = self.parse_bitor_expr()?;
        let op = match self.peek() {
            TokenKind::EqEq => Some(BinOp::Eq),
            TokenKind::Ne => Some(BinOp::Ne),
            TokenKind::Lt => Some(BinOp::Lt),
            TokenKind::Le => Some(BinOp::Le),
            TokenKind::Gt => Some(BinOp::Gt),
            TokenKind::Ge => Some(BinOp::Ge),
            _ => None,
        };
        if let Some(op) = op {
            self.advance();
            let rhs = self.parse_bitor_expr()?;
            let span = lhs.span().merge(rhs.span());
            return Some(Expr::BinOp {
                op,
                lhs: Box::new(lhs),
                rhs: Box::new(rhs),
                span,
            });
        }
        Some(lhs)
    }

    fn parse_bitor_expr(&mut self) -> Option<Expr> {
        let mut lhs = self.parse_bitxor_expr()?;
        while self.at(&TokenKind::Pipe) {
            self.advance();
            let rhs = self.parse_bitxor_expr()?;
            let span = lhs.span().merge(rhs.span());
            lhs = Expr::BinOp {
                op: BinOp::BitOr,
                lhs: Box::new(lhs),
                rhs: Box::new(rhs),
                span,
            };
        }
        Some(lhs)
    }

    fn parse_bitxor_expr(&mut self) -> Option<Expr> {
        let mut lhs = self.parse_bitand_expr()?;
        while self.at(&TokenKind::Caret) {
            self.advance();
            let rhs = self.parse_bitand_expr()?;
            let span = lhs.span().merge(rhs.span());
            lhs = Expr::BinOp {
                op: BinOp::BitXor,
                lhs: Box::new(lhs),
                rhs: Box::new(rhs),
                span,
            };
        }
        Some(lhs)
    }

    fn parse_bitand_expr(&mut self) -> Option<Expr> {
        let mut lhs = self.parse_shift_expr()?;
        while self.at(&TokenKind::Ampersand) {
            self.advance();
            let rhs = self.parse_shift_expr()?;
            let span = lhs.span().merge(rhs.span());
            lhs = Expr::BinOp {
                op: BinOp::BitAnd,
                lhs: Box::new(lhs),
                rhs: Box::new(rhs),
                span,
            };
        }
        Some(lhs)
    }

    fn parse_shift_expr(&mut self) -> Option<Expr> {
        let mut lhs = self.parse_add_expr()?;
        loop {
            let op = match self.peek() {
                TokenKind::Shl => BinOp::Shl,
                TokenKind::Shr => BinOp::Shr,
                _ => break,
            };
            self.advance();
            let rhs = self.parse_add_expr()?;
            let span = lhs.span().merge(rhs.span());
            lhs = Expr::BinOp {
                op,
                lhs: Box::new(lhs),
                rhs: Box::new(rhs),
                span,
            };
        }
        Some(lhs)
    }

    fn parse_add_expr(&mut self) -> Option<Expr> {
        let mut lhs = self.parse_mul_expr()?;
        loop {
            let op = match self.peek() {
                TokenKind::Plus => BinOp::Add,
                TokenKind::Minus => BinOp::Sub,
                _ => break,
            };
            self.advance();
            let rhs = self.parse_mul_expr()?;
            let span = lhs.span().merge(rhs.span());
            lhs = Expr::BinOp {
                op,
                lhs: Box::new(lhs),
                rhs: Box::new(rhs),
                span,
            };
        }
        Some(lhs)
    }

    fn parse_mul_expr(&mut self) -> Option<Expr> {
        let mut lhs = self.parse_cast_expr()?;
        loop {
            let op = match self.peek() {
                TokenKind::Star => BinOp::Mul,
                TokenKind::Slash => BinOp::Div,
                TokenKind::Percent => BinOp::Rem,
                _ => break,
            };
            self.advance();
            let rhs = self.parse_cast_expr()?;
            let span = lhs.span().merge(rhs.span());
            lhs = Expr::BinOp {
                op,
                lhs: Box::new(lhs),
                rhs: Box::new(rhs),
                span,
            };
        }
        Some(lhs)
    }

    fn parse_cast_expr(&mut self) -> Option<Expr> {
        let mut lhs = self.parse_unary_expr()?;
        while self.eat(&TokenKind::KwAs) {
            let ty = self.parse_type_expr()?;
            let end = ty.span();
            let span = lhs.span().merge(end);
            lhs = Expr::Cast {
                expr: Box::new(lhs),
                ty,
                span,
            };
        }
        Some(lhs)
    }

    fn parse_unary_expr(&mut self) -> Option<Expr> {
        let span = self.span_of_current();
        match self.peek() {
            TokenKind::Minus => {
                self.advance();
                let operand = Box::new(self.parse_unary_expr()?);
                let end = operand.span();
                Some(Expr::UnaryOp {
                    op: UnaryOp::Neg,
                    operand,
                    span: span.merge(end),
                })
            }
            TokenKind::Bang => {
                self.advance();
                let operand = Box::new(self.parse_unary_expr()?);
                let end = operand.span();
                Some(Expr::UnaryOp {
                    op: UnaryOp::Not,
                    operand,
                    span: span.merge(end),
                })
            }
            TokenKind::Tilde => {
                self.advance();
                let operand = Box::new(self.parse_unary_expr()?);
                let end = operand.span();
                Some(Expr::UnaryOp {
                    op: UnaryOp::BitNot,
                    operand,
                    span: span.merge(end),
                })
            }
            TokenKind::Ampersand => {
                self.advance();
                let mutable = self.eat(&TokenKind::KwMut);
                let expr = Box::new(self.parse_unary_expr()?);
                let end = expr.span();
                Some(Expr::Ref {
                    mutable,
                    expr,
                    span: span.merge(end),
                })
            }
            TokenKind::Star => {
                self.advance();
                let expr = Box::new(self.parse_unary_expr()?);
                let end = expr.span();
                Some(Expr::Deref {
                    expr,
                    span: span.merge(end),
                })
            }
            _ => self.parse_postfix_expr(),
        }
    }

    fn parse_postfix_expr(&mut self) -> Option<Expr> {
        let mut base = self.parse_primary_expr()?;
        loop {
            let span = base.span();
            match self.peek() {
                TokenKind::Dot => {
                    self.advance();
                    if let TokenKind::Ident(name) = self.peek().clone() {
                        let field_span = self.span_of_current();
                        self.advance();
                        let field = Ident::new(name, field_span);
                        // Method call?
                        if self.at(&TokenKind::LParen) {
                            self.advance();
                            let args = self.parse_call_args()?;
                            let end = self.span_of_current();
                            self.expect(&TokenKind::RParen, "expected ')'");
                            base = Expr::MethodCall {
                                receiver: Box::new(base),
                                method: field,
                                args,
                                span: span.merge(end),
                            };
                        } else {
                            base = Expr::Field {
                                base: Box::new(base),
                                field,
                                span: span.merge(field_span),
                            };
                        }
                    } else if let TokenKind::Integer(n) = self.peek().clone() {
                        // Tuple field access: `foo.0`
                        let idx_span = self.span_of_current();
                        self.advance();
                        let field = Ident::new(n.to_string(), idx_span);
                        base = Expr::Field {
                            base: Box::new(base),
                            field,
                            span: span.merge(idx_span),
                        };
                    } else {
                        let s = self.span_of_current();
                        self.push_expected_diagnostic(
                            s,
                            "expected field name",
                            Some("an identifier or tuple index after `.`".to_string()),
                        );
                        break;
                    }
                }
                TokenKind::LParen => {
                    self.advance();
                    let args = self.parse_call_args()?;
                    let end = self.span_of_current();
                    self.expect(&TokenKind::RParen, "expected ')'");
                    base = Expr::Call {
                        callee: Box::new(base),
                        args,
                        span: span.merge(end),
                    };
                }
                TokenKind::LBracket => {
                    self.advance();
                    let index = Box::new(self.parse_expr()?);
                    let end = self.span_of_current();
                    self.expect(&TokenKind::RBracket, "expected ']'");
                    base = Expr::Index {
                        base: Box::new(base),
                        index,
                        span: span.merge(end),
                    };
                }
                TokenKind::Question => {
                    self.advance();
                    let end = self.span_of_current();
                    base = Expr::Try {
                        expr: Box::new(base),
                        span: span.merge(end),
                    };
                }
                TokenKind::KwAwait => {
                    self.advance();
                    let end = self.span_of_current();
                    base = Expr::Await {
                        expr: Box::new(base),
                        span: span.merge(end),
                    };
                }
                _ => break,
            }
        }
        Some(base)
    }

    fn parse_call_args(&mut self) -> Option<Vec<Expr>> {
        let mut args = Vec::new();
        while !self.at(&TokenKind::RParen) && !self.is_at_end() {
            if let Some(e) = self.parse_expr() {
                args.push(e);
            }
            if !self.eat(&TokenKind::Comma) {
                break;
            }
        }
        Some(args)
    }

    fn parse_primary_expr(&mut self) -> Option<Expr> {
        let span = self.span_of_current();
        match self.peek().clone() {
            // Literals
            TokenKind::Integer(n) => {
                self.advance();
                Some(Expr::Literal {
                    lit: Literal::Integer(n),
                    span,
                })
            }
            TokenKind::Float(f) => {
                self.advance();
                Some(Expr::Literal {
                    lit: Literal::Float(f),
                    span,
                })
            }
            TokenKind::StringLit(s) => {
                self.advance();
                Some(Expr::Literal {
                    lit: Literal::String(s),
                    span,
                })
            }
            TokenKind::CharLit(c) => {
                self.advance();
                Some(Expr::Literal {
                    lit: Literal::Char(c),
                    span,
                })
            }
            TokenKind::Bool(b) => {
                self.advance();
                Some(Expr::Literal {
                    lit: Literal::Bool(b),
                    span,
                })
            }
            // `if`
            TokenKind::KwIf => self.parse_if_expr(),
            // `match`
            TokenKind::KwMatch => self.parse_match_expr(),
            // `while`
            TokenKind::KwWhile => self.parse_while_expr(),
            // `loop`
            TokenKind::KwLoop => self.parse_loop_expr(),
            // `for`
            TokenKind::KwFor => self.parse_for_expr(),
            // inline `fn(...) { ... }`
            TokenKind::KwFn | TokenKind::KwFun => self.parse_closure_like_fn_expr(),
            // Block
            TokenKind::LBrace => self.parse_block_expr(),
            // Tuple / grouped
            TokenKind::LParen => {
                self.advance();
                if self.eat(&TokenKind::RParen) {
                    return Some(Expr::Literal {
                        lit: Literal::Unit,
                        span,
                    });
                }
                let first = self.parse_expr()?;
                if self.eat(&TokenKind::Comma) {
                    let mut elems = vec![first];
                    while !self.at(&TokenKind::RParen) && !self.is_at_end() {
                        if let Some(e) = self.parse_expr() {
                            elems.push(e);
                        }
                        if !self.eat(&TokenKind::Comma) {
                            break;
                        }
                    }
                    let end = self.span_of_current();
                    self.expect(&TokenKind::RParen, "expected ')'");
                    Some(Expr::Tuple {
                        elems,
                        span: span.merge(end),
                    })
                } else {
                    self.expect(&TokenKind::RParen, "expected ')'");
                    Some(first)
                }
            }
            // Array
            TokenKind::LBracket => {
                self.advance();
                if self.eat(&TokenKind::RBracket) {
                    return Some(Expr::Array {
                        elems: Vec::new(),
                        span,
                    });
                }
                let first = self.parse_expr()?;
                if self.eat(&TokenKind::Semi) {
                    let count = Box::new(self.parse_expr()?);
                    let end = self.span_of_current();
                    self.expect(&TokenKind::RBracket, "expected ']'");
                    return Some(Expr::Repeat {
                        elem: Box::new(first),
                        count,
                        span: span.merge(end),
                    });
                }
                let mut elems = vec![first];
                while self.eat(&TokenKind::Comma) {
                    if self.at(&TokenKind::RBracket) {
                        break;
                    }
                    if let Some(e) = self.parse_expr() {
                        elems.push(e);
                    }
                }
                let end = self.span_of_current();
                self.expect(&TokenKind::RBracket, "expected ']'");
                Some(Expr::Array {
                    elems,
                    span: span.merge(end),
                })
            }
            // return / break / continue
            TokenKind::KwReturn => {
                self.advance();
                let value = if !self.at(&TokenKind::Semi) && !self.at(&TokenKind::RBrace) {
                    self.parse_expr().map(Box::new)
                } else {
                    None
                };
                let end = value.as_ref().map(|e| e.span()).unwrap_or(span);
                Some(Expr::Return {
                    value,
                    span: span.merge(end),
                })
            }
            TokenKind::KwBreak => {
                self.advance();
                let value = if !self.at(&TokenKind::Semi) && !self.at(&TokenKind::RBrace) {
                    self.parse_expr().map(Box::new)
                } else {
                    None
                };
                let end = value.as_ref().map(|e| e.span()).unwrap_or(span);
                Some(Expr::Break {
                    value,
                    span: span.merge(end),
                })
            }
            TokenKind::KwContinue => {
                self.advance();
                Some(Expr::Continue { span })
            }
            // unsafe block
            TokenKind::KwUnsafe => {
                self.advance();
                let body = Box::new(self.parse_block_expr()?);
                let end = body.span();
                Some(Expr::Unsafe {
                    body,
                    span: span.merge(end),
                })
            }
            // async block
            TokenKind::KwAsync => {
                self.advance();
                // async block or async closure
                self.parse_block_expr()
            }
            // Path or struct literal
            TokenKind::Ident(_) | TokenKind::KwSelf | TokenKind::KwSuper | TokenKind::KwCrate => {
                let path = self.parse_path()?;
                // Struct literal?
                if self.at(&TokenKind::LBrace) {
                    return self.parse_struct_literal(path, span);
                }
                let end = path.span;
                Some(Expr::Path {
                    path,
                    span: span.merge(end),
                })
            }
            TokenKind::Eof => None,
            _ => {
                let s = self.span_of_current();
                self.push_expected_diagnostic(
                    s,
                    "expected expression",
                    Some("a literal, path, block, call, or control-flow expression".to_string()),
                );
                None
            }
        }
    }

    fn parse_if_expr(&mut self) -> Option<Expr> {
        let start = self.span_of_current();
        self.expect(&TokenKind::KwIf, "expected 'if'");
        let condition = Box::new(
            self.parse_expr_before_block()
                .or_else(|| self.parse_expr())?,
        );
        let then_branch = Box::new(self.parse_block_expr()?);
        let else_branch = if self.eat(&TokenKind::KwElse) {
            if self.at(&TokenKind::KwIf) {
                self.parse_if_expr().map(Box::new)
            } else {
                self.parse_block_expr().map(Box::new)
            }
        } else {
            None
        };
        let end = else_branch
            .as_ref()
            .map(|e| e.span())
            .unwrap_or_else(|| then_branch.span());
        Some(Expr::If {
            condition,
            then_branch,
            else_branch,
            span: start.merge(end),
        })
    }

    fn parse_match_expr(&mut self) -> Option<Expr> {
        let start = self.span_of_current();
        self.expect(&TokenKind::KwMatch, "expected 'match'");
        let scrutinee = Box::new(
            self.parse_expr_before_block()
                .or_else(|| self.parse_expr())?,
        );
        self.expect(&TokenKind::LBrace, "expected '{'");
        let mut arms = Vec::new();
        while !self.at(&TokenKind::RBrace) && !self.is_at_end() {
            let arm_span = self.span_of_current();
            let pattern = match self.parse_pattern() {
                Some(p) => p,
                None => break,
            };
            let guard = if self.eat(&TokenKind::KwIf) {
                self.parse_expr()
            } else {
                None
            };
            self.expect(&TokenKind::FatArrow, "expected '=>'");
            let body = match self.parse_expr() {
                Some(e) => e,
                None => break,
            };
            let has_block = matches!(
                body,
                Expr::Block { .. } | Expr::If { .. } | Expr::Match { .. }
            );
            let end = body.span();
            arms.push(MatchArm {
                pattern,
                guard,
                body,
                span: arm_span.merge(end),
            });
            if !has_block {
                self.eat(&TokenKind::Comma);
            } else {
                self.eat(&TokenKind::Comma);
            }
        }
        let end = self.span_of_current();
        self.expect(&TokenKind::RBrace, "expected '}'");
        Some(Expr::Match {
            scrutinee,
            arms,
            span: start.merge(end),
        })
    }

    fn parse_while_expr(&mut self) -> Option<Expr> {
        let start = self.span_of_current();
        self.expect(&TokenKind::KwWhile, "expected 'while'");
        if self.eat(&TokenKind::KwLet) {
            let pattern = self.parse_pattern()?;
            self.expect(&TokenKind::Eq, "expected '='");
            let scrutinee = Box::new(
                self.parse_expr_before_block()
                    .or_else(|| self.parse_expr())?,
            );
            let body = Box::new(self.parse_block_expr()?);
            let end = body.span();
            return Some(Expr::WhileLet {
                pattern,
                scrutinee,
                body,
                span: start.merge(end),
            });
        }
        let condition = Box::new(
            self.parse_expr_before_block()
                .or_else(|| self.parse_expr())?,
        );
        let body = Box::new(self.parse_block_expr()?);
        let end = body.span();
        Some(Expr::While {
            condition,
            body,
            span: start.merge(end),
        })
    }

    fn parse_for_expr(&mut self) -> Option<Expr> {
        let start = self.span_of_current();
        self.expect(&TokenKind::KwFor, "expected 'for'");
        let pattern = self.parse_pattern()?;
        self.expect(&TokenKind::KwIn, "expected 'in'");
        let iterable = Box::new(
            self.parse_expr_before_block()
                .or_else(|| self.parse_expr())?,
        );
        let body = Box::new(self.parse_block_expr()?);
        let end = body.span();
        Some(Expr::For {
            pattern,
            iterable,
            body,
            span: start.merge(end),
        })
    }

    fn parse_loop_expr(&mut self) -> Option<Expr> {
        let start = self.span_of_current();
        self.expect(&TokenKind::KwLoop, "expected 'loop'");
        let body = Box::new(self.parse_block_expr()?);
        let end = body.span();
        Some(Expr::Loop {
            body,
            span: start.merge(end),
        })
    }

    fn parse_closure_like_fn_expr(&mut self) -> Option<Expr> {
        // Inline closures: `fn(x: T) -> U { ... }` or `fun(x: T): U { ... }`
        let start = self.span_of_current();
        self.advance(); // eat `fn` / `fun`
        self.expect(&TokenKind::LParen, "expected '('");
        let mut params = Vec::new();
        while !self.at(&TokenKind::RParen) && !self.is_at_end() {
            let pspan = self.span_of_current();
            let pattern = match self.parse_pattern() {
                Some(p) => p,
                None => break,
            };
            let ty = if self.eat(&TokenKind::Colon) {
                self.parse_type_expr()
            } else {
                None
            };
            params.push(ClosureParam {
                pattern,
                ty,
                span: pspan,
            });
            if !self.eat(&TokenKind::Comma) {
                break;
            }
        }
        self.expect(&TokenKind::RParen, "expected ')'");
        let ret_ty = if self.eat(&TokenKind::Colon) || self.eat(&TokenKind::Arrow) {
            self.parse_type_expr()
        } else {
            None
        };
        let body = Box::new(self.parse_block_expr()?);
        let end = body.span();
        Some(Expr::Closure {
            params,
            ret_ty,
            body,
            span: start.merge(end),
        })
    }

    fn parse_block_expr(&mut self) -> Option<Expr> {
        let start = self.span_of_current();
        self.expect(&TokenKind::LBrace, "expected '{'");
        let mut stmts = Vec::new();
        let mut tail: Option<Box<Expr>> = None;
        while !self.at(&TokenKind::RBrace) && !self.is_at_end() {
            let s = self.parse_stmt();
            match s {
                Some(Stmt::Expr {
                    expr,
                    has_semi: false,
                }) if !self.at(&TokenKind::RBrace) => {
                    stmts.push(Stmt::Expr {
                        expr,
                        has_semi: false,
                    });
                }
                Some(Stmt::Expr {
                    expr,
                    has_semi: false,
                }) => {
                    tail = Some(Box::new(expr));
                }
                Some(stmt) => stmts.push(stmt),
                None => break,
            }
        }
        let end = self.span_of_current();
        self.expect(&TokenKind::RBrace, "expected '}'");
        Some(Expr::Block {
            stmts,
            tail,
            span: start.merge(end),
        })
    }

    fn parse_struct_literal(&mut self, path: Path, start: Span) -> Option<Expr> {
        self.expect(&TokenKind::LBrace, "expected '{'");
        let mut fields = Vec::new();
        let mut rest: Option<Box<Expr>> = None;
        while !self.at(&TokenKind::RBrace) && !self.is_at_end() {
            if self.eat(&TokenKind::DotDot) {
                rest = self.parse_expr().map(Box::new);
                break;
            }
            let fspan = self.span_of_current();
            let name = match self.parse_ident() {
                Some(n) => n,
                None => break,
            };
            let value = if self.eat(&TokenKind::Colon) {
                self.parse_expr()
            } else {
                None
            };
            fields.push(FieldInit {
                name,
                value,
                span: fspan,
            });
            if !self.eat(&TokenKind::Comma) {
                break;
            }
        }
        let end = self.span_of_current();
        self.expect(&TokenKind::RBrace, "expected '}'");
        Some(Expr::Struct {
            path,
            fields,
            rest,
            span: start.merge(end),
        })
    }

    // ── Statements ────────────────────────────────────────────────────────────

    fn parse_stmt(&mut self) -> Option<Stmt> {
        let span = self.span_of_current();
        match self.peek() {
            TokenKind::KwLet => {
                self.advance();
                let pattern = if self.eat(&TokenKind::KwMut) {
                    let mut pattern = self.parse_pattern()?;
                    Self::set_pattern_mutability(&mut pattern, true);
                    pattern
                } else {
                    let mut pattern = self.parse_pattern()?;
                    Self::set_pattern_mutability(&mut pattern, true);
                    pattern
                };
                let ty = if self.eat(&TokenKind::Colon) {
                    self.parse_type_expr()
                } else {
                    None
                };
                let init = if self.eat(&TokenKind::Eq) {
                    self.parse_expr()
                } else {
                    None
                };
                self.expect(&TokenKind::Semi, "expected ';' after let binding");
                Some(Stmt::Let {
                    pattern,
                    ty,
                    init,
                    span,
                })
            }
            TokenKind::KwConst => {
                self.advance();
                let mut pattern = self.parse_pattern()?;
                Self::set_pattern_mutability(&mut pattern, false);
                let ty = if self.eat(&TokenKind::Colon) {
                    self.parse_type_expr()
                } else {
                    None
                };
                let init = if self.eat(&TokenKind::Eq) {
                    self.parse_expr()
                } else {
                    None
                };
                self.expect(&TokenKind::Semi, "expected ';' after const binding");
                Some(Stmt::Let {
                    pattern,
                    ty,
                    init,
                    span,
                })
            }
            TokenKind::KwErrdefer => {
                self.advance();
                let body = self.parse_block_expr()?;
                Some(Stmt::Errdefer { body, span })
            }
            TokenKind::KwDefer => {
                self.advance();
                let body = self.parse_block_expr()?;
                Some(Stmt::Defer { body, span })
            }
            TokenKind::KwUse => {
                self.advance();
                let tree = self.parse_use_tree()?;
                self.expect(&TokenKind::Semi, "expected ';' after use");
                Some(Stmt::Use { tree, span })
            }
            TokenKind::KwImport => {
                self.advance();
                let tree = self.parse_import_tree()?;
                self.expect(&TokenKind::Semi, "expected ';' after import");
                Some(Stmt::Use { tree, span })
            }
            _ => {
                let expr = self.parse_expr()?;
                let has_semi = self.eat(&TokenKind::Semi);
                Some(Stmt::Expr { expr, has_semi })
            }
        }
    }

    // ── Use declarations ──────────────────────────────────────────────────────

    fn parse_use_tree(&mut self) -> Option<UseTree> {
        let span = self.span_of_current();
        let prefix = self.parse_path()?;
        let kind = if self.at(&TokenKind::DoubleColon)
            && matches!(
                self.tokens.get(self.pos + 1).map(|t| &t.kind),
                Some(TokenKind::Star)
            ) {
            self.advance();
            self.advance();
            UseTreeKind::Glob
        } else if self.at(&TokenKind::DoubleColon)
            && matches!(
                self.tokens.get(self.pos + 1).map(|t| &t.kind),
                Some(TokenKind::LBrace)
            )
        {
            self.advance();
            self.advance();
            let mut children = Vec::new();
            while !self.at(&TokenKind::RBrace) && !self.is_at_end() {
                if let Some(child) = self.parse_use_tree() {
                    children.push(child);
                }
                if !self.eat(&TokenKind::Comma) {
                    break;
                }
            }
            self.expect(&TokenKind::RBrace, "expected '}'");
            UseTreeKind::Nested(children)
        } else if self.eat(&TokenKind::KwAs) {
            let alias = self.parse_ident()?;
            UseTreeKind::Alias(alias)
        } else {
            UseTreeKind::Simple
        };
        Some(UseTree { prefix, kind, span })
    }

    // ── Items ─────────────────────────────────────────────────────────────────

    fn parse_item(&mut self) -> Option<Item> {
        let derives = self.parse_derive_attrs();
        let vis = self.parse_visibility();
        let span = self.span_of_current();
        match self.peek() {
            TokenKind::KwFn | TokenKind::KwFun | TokenKind::KwAsync | TokenKind::KwUnsafe => {
                let is_async = self.eat(&TokenKind::KwAsync);
                let is_unsafe = self.eat(&TokenKind::KwUnsafe);
                if !(self.eat(&TokenKind::KwFn) || self.eat(&TokenKind::KwFun)) {
                    self.push_expected_diagnostic(
                        self.span_of_current(),
                        "expected function declaration",
                        Some("`fun` or `fn`".to_string()),
                    );
                    return None;
                }
                Some(Item::Function(
                    self.parse_fn_def(vis, is_async, is_unsafe, span)?,
                ))
            }
            TokenKind::KwStruct => {
                self.advance();
                Some(Item::Struct(self.parse_struct_def(derives, vis, span)?))
            }
            TokenKind::KwEnum => {
                self.advance();
                Some(Item::Enum(self.parse_enum_def(derives, vis, span)?))
            }
            TokenKind::KwConst => {
                if !derives.is_empty() {
                    self.errors.push(
                        Diagnostic::error(
                            "`@derive(...)` is only supported on `struct` and `enum` items",
                        )
                        .with_span(span),
                    );
                }
                self.advance();
                Some(Item::Const(self.parse_const_def(vis, span)?))
            }
            TokenKind::KwStatic => {
                if !derives.is_empty() {
                    self.errors.push(
                        Diagnostic::error(
                            "`@derive(...)` is only supported on `struct` and `enum` items",
                        )
                        .with_span(span),
                    );
                }
                self.advance();
                Some(Item::Static(self.parse_static_def(vis, span)?))
            }
            TokenKind::KwTrait => {
                if !derives.is_empty() {
                    self.errors.push(
                        Diagnostic::error(
                            "`@derive(...)` is only supported on `struct` and `enum` items",
                        )
                        .with_span(span),
                    );
                }
                self.advance();
                Some(Item::Trait(self.parse_trait_def(vis, span)?))
            }
            TokenKind::KwInterface => {
                if !derives.is_empty() {
                    self.errors.push(
                        Diagnostic::error(
                            "`@derive(...)` is only supported on `struct` and `enum` items",
                        )
                        .with_span(span),
                    );
                }
                self.advance();
                Some(Item::Interface(self.parse_interface_def(vis, span)?))
            }
            TokenKind::KwImpl => {
                if !derives.is_empty() {
                    self.errors.push(
                        Diagnostic::error(
                            "`@derive(...)` is only supported on `struct` and `enum` items",
                        )
                        .with_span(span),
                    );
                }
                self.advance();
                Some(Item::Impl(self.parse_impl_block(span)?))
            }
            TokenKind::KwExtend => {
                if !derives.is_empty() {
                    self.errors.push(
                        Diagnostic::error(
                            "`@derive(...)` is only supported on `struct` and `enum` items",
                        )
                        .with_span(span),
                    );
                }
                self.advance();
                Some(Item::Impl(self.parse_impl_block(span)?))
            }
            TokenKind::KwType => {
                if !derives.is_empty() {
                    self.errors.push(
                        Diagnostic::error(
                            "`@derive(...)` is only supported on `struct` and `enum` items",
                        )
                        .with_span(span),
                    );
                }
                self.advance();
                Some(Item::TypeAlias(self.parse_type_alias(vis, span)?))
            }
            TokenKind::KwAbility => {
                if !derives.is_empty() {
                    self.errors.push(
                        Diagnostic::error(
                            "`@derive(...)` is only supported on `struct` and `enum` items",
                        )
                        .with_span(span),
                    );
                }
                self.advance();
                Some(Item::Ability(self.parse_ability_def(vis, span)?))
            }
            TokenKind::KwUse => {
                if !derives.is_empty() {
                    self.errors.push(
                        Diagnostic::error(
                            "`@derive(...)` is only supported on `struct` and `enum` items",
                        )
                        .with_span(span),
                    );
                }
                self.advance();
                let tree = self.parse_use_tree()?;
                self.expect(&TokenKind::Semi, "expected ';'");
                Some(Item::Use(tree, span))
            }
            TokenKind::KwImport => {
                if !derives.is_empty() {
                    self.errors.push(
                        Diagnostic::error(
                            "`@derive(...)` is only supported on `struct` and `enum` items",
                        )
                        .with_span(span),
                    );
                }
                self.advance();
                let tree = self.parse_import_tree()?;
                self.expect(&TokenKind::Semi, "expected ';'");
                Some(Item::Use(tree, span))
            }
            TokenKind::KwExtern => {
                if !derives.is_empty() {
                    self.errors.push(
                        Diagnostic::error(
                            "`@derive(...)` is only supported on `struct` and `enum` items",
                        )
                        .with_span(span),
                    );
                }
                self.advance();
                Some(Item::ExternBlock(self.parse_extern_block(span)?))
            }
            TokenKind::KwMod => {
                if !derives.is_empty() {
                    self.errors.push(
                        Diagnostic::error(
                            "`@derive(...)` is only supported on `struct` and `enum` items",
                        )
                        .with_span(span),
                    );
                }
                self.advance();
                self.errors.push(
                    Diagnostic::error("inline `mod` blocks are no longer supported")
                        .with_span(span)
                        .with_note(
                            "use file=module layout instead (`src/foo.dr`, `src/foo/mod.dr`)",
                        ),
                );
                if self.eat(&TokenKind::LBrace) {
                    self.skip_balanced_delimiters(&TokenKind::LBrace, &TokenKind::RBrace);
                }
                None
            }
            _ => {
                if !derives.is_empty() {
                    self.errors.push(
                        Diagnostic::error(
                            "`@derive(...)` must be followed by a `struct` or `enum` item",
                        )
                        .with_span(span),
                    );
                }
                let s = self.span_of_current();
                self.push_expected_diagnostic(
                    s,
                    "expected item",
                    Some(
                        "an item like `fun`, `struct`, `enum`, `const`, `import`, or `use`"
                            .to_string(),
                    ),
                );
                // Skip one token to avoid infinite loop
                self.advance();
                None
            }
        }
    }

    fn parse_fn_def(
        &mut self,
        vis: Visibility,
        is_async: bool,
        is_unsafe: bool,
        start: Span,
    ) -> Option<FnDef> {
        let name = self.parse_ident()?;
        let generics = self.parse_generic_params();
        self.expect(&TokenKind::LParen, "expected '('");
        let mut params = Vec::new();
        while !self.at(&TokenKind::RParen) && !self.is_at_end() {
            let pspan = self.span_of_current();
            let pattern = match self.parse_pattern() {
                Some(p) => p,
                None => break,
            };
            let ty = if self.eat(&TokenKind::Colon) {
                match self.parse_type_expr() {
                    Some(t) => t,
                    None => break,
                }
            } else if matches!(
                &pattern,
                Pattern::Ident { name, .. } if name.name == "self"
            ) {
                TypeExpr::SelfType { span: pspan }
            } else {
                self.push_expected_diagnostic(
                    self.span_of_current(),
                    "expected ':' in parameter",
                    Some("a parameter type annotation like `name: Type`".to_string()),
                );
                break;
            };
            let default = if self.eat(&TokenKind::Eq) {
                self.parse_expr()
            } else {
                None
            };
            params.push(FnParam {
                pattern,
                ty,
                default,
                span: pspan,
            });
            if !self.eat(&TokenKind::Comma) {
                break;
            }
        }
        self.expect(&TokenKind::RParen, "expected ')'");
        let ret_ty = if self.eat(&TokenKind::Colon) || self.eat(&TokenKind::Arrow) {
            self.parse_type_expr()
        } else {
            None
        };
        let where_clause = self.parse_where_clause();
        let body = if self.at(&TokenKind::LBrace) {
            self.parse_block_expr()
        } else {
            self.expect(&TokenKind::Semi, "expected ';' or body block");
            None
        };
        let end = body.as_ref().map(|b| b.span()).unwrap_or(start);
        Some(FnDef {
            visibility: vis,
            is_async,
            is_unsafe,
            name,
            generics,
            params,
            ret_ty,
            where_clause,
            body,
            span: start.merge(end),
        })
    }

    fn parse_struct_def(
        &mut self,
        derives: Vec<Path>,
        vis: Visibility,
        start: Span,
    ) -> Option<StructDef> {
        let name = self.parse_ident()?;
        let generics = self.parse_generic_params();
        let where_clause = self.parse_where_clause();
        let kind = if self.eat(&TokenKind::Semi)
            || matches!(
                self.peek(),
                TokenKind::KwFn
                    | TokenKind::KwFun
                    | TokenKind::KwAsync
                    | TokenKind::KwUnsafe
                    | TokenKind::KwStruct
                    | TokenKind::KwEnum
                    | TokenKind::KwConst
                    | TokenKind::KwStatic
                    | TokenKind::KwTrait
                    | TokenKind::KwInterface
                    | TokenKind::KwImpl
                    | TokenKind::KwExtend
                    | TokenKind::KwType
                    | TokenKind::KwAbility
                    | TokenKind::KwUse
                    | TokenKind::KwImport
                    | TokenKind::KwPub
                    | TokenKind::KwExport
                    | TokenKind::RBrace
                    | TokenKind::Eof
            ) {
            StructKind::Unit
        } else if self.at(&TokenKind::LParen) {
            self.advance();
            let mut fields = Vec::new();
            while !self.at(&TokenKind::RParen) && !self.is_at_end() {
                let fspan = self.span_of_current();
                let fvis = self.parse_visibility();
                let ty = match self.parse_type_expr() {
                    Some(t) => t,
                    None => break,
                };
                fields.push(TupleField {
                    visibility: fvis,
                    ty,
                    span: fspan,
                });
                if !self.eat(&TokenKind::Comma) {
                    break;
                }
            }
            self.expect(&TokenKind::RParen, "expected ')'");
            self.eat(&TokenKind::Semi);
            StructKind::Tuple(fields)
        } else {
            self.expect(&TokenKind::LBrace, "expected '{'");
            let mut fields = Vec::new();
            while !self.at(&TokenKind::RBrace) && !self.is_at_end() {
                let fspan = self.span_of_current();
                let fvis = self.parse_visibility();
                let fname = match self.parse_ident() {
                    Some(n) => n,
                    None => break,
                };
                self.expect(&TokenKind::Colon, "expected ':'");
                let ty = match self.parse_type_expr() {
                    Some(t) => t,
                    None => break,
                };
                fields.push(StructField {
                    visibility: fvis,
                    name: fname,
                    ty,
                    span: fspan,
                });
                if self.eat(&TokenKind::Comma) {
                    continue;
                }
                if matches!(
                    self.peek(),
                    TokenKind::KwPub | TokenKind::KwExport | TokenKind::Ident(_)
                ) {
                    continue;
                }
                if !self.at(&TokenKind::RBrace) {
                    break;
                }
            }
            self.expect(&TokenKind::RBrace, "expected '}'");
            StructKind::Fields(fields)
        };
        Some(StructDef {
            derives,
            visibility: vis,
            name,
            generics,
            where_clause,
            kind,
            span: start,
        })
    }

    fn parse_enum_def(
        &mut self,
        derives: Vec<Path>,
        vis: Visibility,
        start: Span,
    ) -> Option<EnumDef> {
        let name = self.parse_ident()?;
        let generics = self.parse_generic_params();
        let where_clause = self.parse_where_clause();
        self.expect(&TokenKind::LBrace, "expected '{'");
        let mut variants = Vec::new();
        while !self.at(&TokenKind::RBrace) && !self.is_at_end() {
            let vspan = self.span_of_current();
            let vname = match self.parse_ident() {
                Some(n) => n,
                None => break,
            };
            let kind = if self.at(&TokenKind::LParen) {
                self.advance();
                let mut elems = Vec::new();
                while !self.at(&TokenKind::RParen) && !self.is_at_end() {
                    if let Some(t) = self.parse_type_expr() {
                        elems.push(t);
                    }
                    if !self.eat(&TokenKind::Comma) {
                        break;
                    }
                }
                self.expect(&TokenKind::RParen, "expected ')'");
                VariantKind::Tuple(elems)
            } else if self.at(&TokenKind::LBrace) {
                self.advance();
                let mut fields = Vec::new();
                while !self.at(&TokenKind::RBrace) && !self.is_at_end() {
                    let fspan = self.span_of_current();
                    let fname = match self.parse_ident() {
                        Some(n) => n,
                        None => break,
                    };
                    self.expect(&TokenKind::Colon, "expected ':'");
                    let ty = match self.parse_type_expr() {
                        Some(t) => t,
                        None => break,
                    };
                    fields.push(StructField {
                        visibility: Visibility::private(),
                        name: fname,
                        ty,
                        span: fspan,
                    });
                    if self.eat(&TokenKind::Comma) {
                        continue;
                    }
                    if matches!(self.peek(), TokenKind::Ident(_)) {
                        continue;
                    }
                    if !self.at(&TokenKind::RBrace) {
                        break;
                    }
                }
                self.expect(&TokenKind::RBrace, "expected '}'");
                VariantKind::Struct(fields)
            } else {
                VariantKind::Unit
            };
            variants.push(EnumVariant {
                name: vname,
                kind,
                span: vspan,
            });
            if self.eat(&TokenKind::Comma) {
                continue;
            }
            if matches!(self.peek(), TokenKind::Ident(_)) {
                continue;
            }
            if !self.at(&TokenKind::RBrace) {
                break;
            }
        }
        self.expect(&TokenKind::RBrace, "expected '}'");
        Some(EnumDef {
            derives,
            visibility: vis,
            name,
            generics,
            where_clause,
            variants,
            span: start,
        })
    }

    fn parse_trait_def(&mut self, vis: Visibility, start: Span) -> Option<TraitDef> {
        let name = self.parse_ident()?;
        let generics = self.parse_generic_params();
        let mut super_traits = Vec::new();
        if self.eat(&TokenKind::Colon) {
            loop {
                if let Some(t) = self.parse_type_expr() {
                    super_traits.push(t);
                }
                if !self.eat(&TokenKind::Plus) {
                    break;
                }
            }
        }
        let where_clause = self.parse_where_clause();
        self.expect(&TokenKind::LBrace, "expected '{'");
        let mut items = Vec::new();
        while !self.at(&TokenKind::RBrace) && !self.is_at_end() {
            if let Some(item) = self.parse_trait_item() {
                items.push(item);
            }
        }
        self.expect(&TokenKind::RBrace, "expected '}'");
        Some(TraitDef {
            visibility: vis,
            name,
            generics,
            super_traits,
            where_clause,
            items,
            span: start,
        })
    }

    fn parse_interface_def(&mut self, vis: Visibility, start: Span) -> Option<InterfaceDef> {
        let name = self.parse_ident()?;
        let generics = self.parse_generic_params();
        let mut super_traits = Vec::new();
        if self.eat(&TokenKind::Colon) {
            loop {
                if let Some(t) = self.parse_type_expr() {
                    super_traits.push(t);
                }
                if !self.eat(&TokenKind::Plus) {
                    break;
                }
            }
        }
        let where_clause = self.parse_where_clause();
        self.expect(&TokenKind::LBrace, "expected '{'");
        let mut items = Vec::new();
        while !self.at(&TokenKind::RBrace) && !self.is_at_end() {
            if let Some(item) = self.parse_trait_item() {
                items.push(item);
            }
        }
        self.expect(&TokenKind::RBrace, "expected '}'");
        Some(InterfaceDef {
            visibility: vis,
            name,
            generics,
            super_traits,
            where_clause,
            items,
            span: start,
        })
    }

    fn parse_trait_item(&mut self) -> Option<TraitItem> {
        let span = self.span_of_current();
        let vis = self.parse_visibility();
        match self.peek() {
            TokenKind::KwFn | TokenKind::KwFun | TokenKind::KwAsync | TokenKind::KwUnsafe => {
                let is_async = self.eat(&TokenKind::KwAsync);
                let is_unsafe = self.eat(&TokenKind::KwUnsafe);
                if !(self.eat(&TokenKind::KwFn) || self.eat(&TokenKind::KwFun)) {
                    self.push_expected_diagnostic(
                        self.span_of_current(),
                        "expected function declaration",
                        Some("`fun` or `fn`".to_string()),
                    );
                    return None;
                }
                let def = self.parse_fn_def(vis, is_async, is_unsafe, span)?;
                Some(TraitItem::Method(def))
            }
            TokenKind::KwType => {
                self.advance();
                let name = self.parse_ident()?;
                let mut bounds = Vec::new();
                if self.eat(&TokenKind::Colon) {
                    loop {
                        if let Some(b) = self.parse_type_expr() {
                            bounds.push(b);
                        }
                        if !self.eat(&TokenKind::Plus) {
                            break;
                        }
                    }
                }
                let default = if self.eat(&TokenKind::Eq) {
                    self.parse_type_expr()
                } else {
                    None
                };
                self.expect(&TokenKind::Semi, "expected ';'");
                Some(TraitItem::TypeAssoc {
                    name,
                    bounds,
                    default,
                    span,
                })
            }
            TokenKind::KwConst => {
                self.advance();
                let _ = vis;
                let name = self.parse_ident()?;
                self.expect(&TokenKind::Colon, "expected ':'");
                let ty = self.parse_type_expr()?;
                let default = if self.eat(&TokenKind::Eq) {
                    self.parse_expr()
                } else {
                    None
                };
                self.expect(&TokenKind::Semi, "expected ';'");
                Some(TraitItem::Const {
                    name,
                    ty,
                    default,
                    span,
                })
            }
            _ => {
                self.advance(); // skip to prevent infinite loop
                None
            }
        }
    }

    fn parse_impl_block(&mut self, start: Span) -> Option<ImplBlock> {
        let generics = self.parse_generic_params();
        let first_ty = self.parse_type_expr()?;
        let (trait_ref, self_ty) = if self.eat(&TokenKind::KwImplements) {
            let trait_ref = self.parse_type_expr()?;
            (Some(trait_ref), first_ty)
        } else if self.eat(&TokenKind::KwFor) {
            let st = self.parse_type_expr()?;
            (Some(first_ty), st)
        } else {
            (None, first_ty)
        };
        let where_clause = self.parse_where_clause();
        self.expect(&TokenKind::LBrace, "expected '{'");
        let mut items = Vec::new();
        while !self.at(&TokenKind::RBrace) && !self.is_at_end() {
            let ispan = self.span_of_current();
            let vis = self.parse_visibility();
            match self.peek() {
                TokenKind::KwFn | TokenKind::KwFun | TokenKind::KwAsync | TokenKind::KwUnsafe => {
                    let is_async = self.eat(&TokenKind::KwAsync);
                    let is_unsafe = self.eat(&TokenKind::KwUnsafe);
                    if !(self.eat(&TokenKind::KwFn) || self.eat(&TokenKind::KwFun)) {
                        self.push_expected_diagnostic(
                            self.span_of_current(),
                            "expected function declaration",
                            Some("`fun` or `fn`".to_string()),
                        );
                        break;
                    }
                    if let Some(def) = self.parse_fn_def(vis, is_async, is_unsafe, ispan) {
                        items.push(ImplItem::Method(def));
                    }
                }
                TokenKind::KwType => {
                    self.advance();
                    let name = match self.parse_ident() {
                        Some(n) => n,
                        None => continue,
                    };
                    self.expect(&TokenKind::Eq, "expected '='");
                    let ty = match self.parse_type_expr() {
                        Some(t) => t,
                        None => continue,
                    };
                    self.expect(&TokenKind::Semi, "expected ';'");
                    items.push(ImplItem::TypeAssoc {
                        name,
                        ty,
                        span: ispan,
                    });
                }
                TokenKind::KwConst => {
                    self.advance();
                    if let Some(def) = self.parse_const_def(vis, ispan) {
                        items.push(ImplItem::Const(def));
                    }
                }
                _ => {
                    self.advance();
                }
            }
        }
        self.expect(&TokenKind::RBrace, "expected '}'");
        Some(ImplBlock {
            generics,
            trait_ref,
            self_ty,
            where_clause,
            items,
            span: start,
        })
    }

    fn parse_type_alias(&mut self, vis: Visibility, start: Span) -> Option<TypeAlias> {
        let name = self.parse_ident()?;
        let generics = self.parse_generic_params();
        self.expect(&TokenKind::Eq, "expected '='");
        let ty = self.parse_type_expr()?;
        self.expect(&TokenKind::Semi, "expected ';'");
        Some(TypeAlias {
            visibility: vis,
            name,
            generics,
            ty,
            span: start,
        })
    }

    fn parse_ability_def(&mut self, vis: Visibility, start: Span) -> Option<AbilityDef> {
        let name = self.parse_ident()?;
        let mut super_abilities = Vec::new();
        if self.eat(&TokenKind::Colon) {
            loop {
                if let Some(p) = self.parse_path() {
                    super_abilities.push(p);
                }
                if !self.eat(&TokenKind::Plus) {
                    break;
                }
            }
        }
        let mut items = Vec::new();
        if self.at(&TokenKind::LBrace) {
            self.advance();
            while !self.at(&TokenKind::RBrace) && !self.is_at_end() {
                if let Some(item) = self.parse_trait_item() {
                    items.push(item);
                }
            }
            self.expect(&TokenKind::RBrace, "expected '}'");
        } else {
            self.expect(&TokenKind::Semi, "expected ';'");
        }
        Some(AbilityDef {
            visibility: vis,
            name,
            super_abilities,
            items,
            span: start,
        })
    }

    fn parse_const_def(&mut self, vis: Visibility, start: Span) -> Option<ConstDef> {
        let name = self.parse_ident()?;
        self.expect(&TokenKind::Colon, "expected ':'");
        let ty = self.parse_type_expr()?;
        self.expect(&TokenKind::Eq, "expected '='");
        let value = self.parse_expr()?;
        let end = value.span();
        self.expect(&TokenKind::Semi, "expected ';'");
        Some(ConstDef {
            visibility: vis,
            name,
            ty,
            value,
            span: start.merge(end),
        })
    }

    fn parse_static_def(&mut self, vis: Visibility, start: Span) -> Option<StaticDef> {
        let mutable = self.eat(&TokenKind::KwMut);
        let name = self.parse_ident()?;
        self.expect(&TokenKind::Colon, "expected ':'");
        let ty = self.parse_type_expr()?;
        self.expect(&TokenKind::Eq, "expected '='");
        let value = self.parse_expr()?;
        let end = value.span();
        self.expect(&TokenKind::Semi, "expected ';'");
        Some(StaticDef {
            visibility: vis,
            mutable,
            name,
            ty,
            value,
            span: start.merge(end),
        })
    }

    fn parse_extern_block(&mut self, start: Span) -> Option<ExternBlock> {
        let abi = if let TokenKind::StringLit(s) = self.peek().clone() {
            self.advance();
            s
        } else {
            "C".to_string()
        };
        self.expect(&TokenKind::LBrace, "expected '{' after `extern` ABI string");
        let mut functions = Vec::new();
        while !matches!(self.peek(), TokenKind::RBrace | TokenKind::Eof) {
            let fn_span = self.span_of_current();
            let vis = self.parse_visibility();
            if !(self.eat(&TokenKind::KwFn) || self.eat(&TokenKind::KwFun)) {
                self.push_expected_diagnostic(
                    fn_span,
                    "expected function declaration in extern block",
                    Some("`fun` or `fn`".to_string()),
                );
                // skip to next `;` or `}`
                while !matches!(
                    self.peek(),
                    TokenKind::Semi | TokenKind::RBrace | TokenKind::Eof
                ) {
                    self.advance();
                }
                self.eat(&TokenKind::Semi);
                continue;
            }
            let name = match self.parse_ident() {
                Some(n) => n,
                None => {
                    self.eat(&TokenKind::Semi);
                    continue;
                }
            };
            self.expect(&TokenKind::LParen, "expected '('");
            let mut params = Vec::new();
            while !matches!(self.peek(), TokenKind::RParen | TokenKind::Eof) {
                let p_span = self.span_of_current();
                let pattern = self.parse_pattern()?;
                self.expect(&TokenKind::Colon, "expected ':'");
                let ty = self.parse_type_expr()?;
                let p_end = ty.span();
                params.push(FnParam {
                    pattern,
                    ty,
                    default: None,
                    span: p_span.merge(p_end),
                });
                if !self.eat(&TokenKind::Comma) {
                    break;
                }
            }
            self.expect(&TokenKind::RParen, "expected ')'");
            let ret_ty = if self.eat(&TokenKind::Colon) {
                self.parse_type_expr()
            } else if self.eat(&TokenKind::Arrow) {
                self.parse_type_expr()
            } else {
                None
            };
            let fn_end = self.span_of_current();
            self.expect(&TokenKind::Semi, "expected ';' after extern fn declaration");
            functions.push(ExternFn {
                visibility: vis,
                name,
                params,
                ret_ty,
                span: fn_span.merge(fn_end),
            });
        }
        let end = self.span_of_current();
        self.expect(&TokenKind::RBrace, "expected '}'");
        Some(ExternBlock {
            abi,
            functions,
            span: start.merge(end),
        })
    }

    // ── Top-level ─────────────────────────────────────────────────────────────

    fn parse_module(&mut self) -> Module {
        let mut items = Vec::new();
        while !self.is_at_end() {
            if let Some(item) = self.parse_item() {
                items.push(item);
            }
        }
        let file = self.file;
        let span = if let (Some(first), Some(last)) = (items.first(), items.last()) {
            let start = match first {
                _ => Span::new(file, 0, 0),
            };
            let end = match last {
                _ => Span::new(file, 0, 0),
            };
            start.merge(end)
        } else {
            Span::new(file, 0, 0)
        };
        Module { file, items, span }
    }
}

/// Parse a token stream into an AST module.
/// Returns the module and any parse diagnostics.
pub fn parse(file: FileId, tokens: &[Token]) -> (Module, Vec<Diagnostic>) {
    if tokens.len() > MAX_PARSE_TOKENS {
        let span = tokens
            .first()
            .map(|tok| Span::new(file, tok.start.0, tok.end.0))
            .unwrap_or_else(|| Span::new(file, 0, 0));
        return (
            Module {
                file,
                items: Vec::new(),
                span,
            },
            vec![Diagnostic::error("token stream exceeds parser limit").with_span(span)],
        );
    }

    let mut nesting = 0usize;
    for tok in tokens {
        match tok.kind {
            TokenKind::LParen | TokenKind::LBrace | TokenKind::LBracket => {
                nesting += 1;
                if nesting > MAX_DELIMITER_NESTING {
                    let span = Span::new(file, tok.start.0, tok.end.0);
                    return (
                        Module {
                            file,
                            items: Vec::new(),
                            span,
                        },
                        vec![
                            Diagnostic::error("nesting depth exceeds parser limit").with_span(span)
                        ],
                    );
                }
            }
            TokenKind::RParen | TokenKind::RBrace | TokenKind::RBracket => {
                nesting = nesting.saturating_sub(1);
            }
            _ => {}
        }
    }

    let mut parser = Parser::new(file, tokens);
    let module = parser.parse_module();
    (module, parser.errors)
}
