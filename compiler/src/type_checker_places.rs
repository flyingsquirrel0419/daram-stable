use crate::{
    diagnostics::{Diagnostic, Label},
    hir::{HirExpr, HirId},
    source::Span,
    type_checker_borrow::{self, BorrowPath},
};
use std::collections::HashMap;

pub(crate) struct PlaceContext<'a> {
    pub moved_locals: &'a HashMap<HirId, Span>,
    pub local_mutability: &'a HashMap<HirId, bool>,
    pub local_borrow_aliases: &'a HashMap<HirId, BorrowPath>,
}

pub(crate) fn place_path(
    expr: &HirExpr,
    local_borrow_aliases: &HashMap<HirId, BorrowPath>,
) -> Option<BorrowPath> {
    type_checker_borrow::borrow_path(expr)
        .or_else(|| type_checker_borrow::borrow_alias_path(expr, local_borrow_aliases))
}

pub(crate) fn moved_path_diagnostic(
    moved_locals: &HashMap<HirId, Span>,
    path: &BorrowPath,
    span: Span,
) -> Option<Diagnostic> {
    moved_locals
        .contains_key(&path.root)
        .then(|| type_checker_borrow::record_use_after_move(moved_locals, path.root, span))
}

pub(crate) fn write_borrow_conflict_diagnostic(
    borrow_span: Option<Span>,
    span: Span,
) -> Option<Diagnostic> {
    borrow_span.map(|borrow_span| {
        Diagnostic::error("cannot assign to value while it is borrowed")
            .with_span(span)
            .with_label(Label::new(borrow_span, "borrow occurs here"))
            .with_note("perform the assignment after the borrow ends")
    })
}

pub(crate) fn mutable_borrow_conflict_diagnostic(
    borrow_span: Option<Span>,
    span: Span,
) -> Option<Diagnostic> {
    borrow_span.map(|borrow_span| {
        Diagnostic::error("cannot mutably borrow value while it is already borrowed")
            .with_span(span)
            .with_label(Label::new(borrow_span, "borrow occurs here"))
    })
}

pub(crate) fn immutable_root_diagnostic(
    local_mutability: &HashMap<HirId, bool>,
    path: &BorrowPath,
    span: Span,
    message: &str,
    note: &str,
) -> Option<Diagnostic> {
    (!local_mutability.get(&path.root).copied().unwrap_or(false))
        .then(|| Diagnostic::error(message).with_span(span).with_note(note))
}

pub(crate) fn write_place_path_diagnostics(
    ctx: &PlaceContext<'_>,
    expr: &HirExpr,
    span: Span,
    conflicting_borrow: &dyn Fn(&BorrowPath, bool, Option<HirId>) -> Option<Span>,
    loan_owner_for_expr: &dyn Fn(&HirExpr) -> Option<HirId>,
) -> Vec<Diagnostic> {
    let Some(path) = place_path(expr, ctx.local_borrow_aliases) else {
        return Vec::new();
    };
    let mut diagnostics = Vec::new();
    if let Some(diagnostic) = moved_path_diagnostic(ctx.moved_locals, &path, span) {
        diagnostics.push(diagnostic);
    }
    if let Some(diagnostic) = write_borrow_conflict_diagnostic(
        conflicting_borrow(&path, true, loan_owner_for_expr(expr)),
        span,
    ) {
        diagnostics.push(diagnostic);
    }
    if let Some(diagnostic) = immutable_root_diagnostic(
        ctx.local_mutability,
        &path,
        span,
        "cannot mutate through immutable binding",
        "declare the original binding as `let` to allow mutation",
    ) {
        diagnostics.push(diagnostic);
    }
    diagnostics
}

pub(crate) fn mutable_borrow_place_path_diagnostics(
    ctx: &PlaceContext<'_>,
    expr: &HirExpr,
    span: Span,
    conflicting_borrow: &dyn Fn(&BorrowPath, bool, Option<HirId>) -> Option<Span>,
    loan_owner_for_expr: &dyn Fn(&HirExpr) -> Option<HirId>,
) -> Vec<Diagnostic> {
    let Some(path) = place_path(expr, ctx.local_borrow_aliases) else {
        return Vec::new();
    };
    let mut diagnostics = Vec::new();
    if let Some(diagnostic) = moved_path_diagnostic(ctx.moved_locals, &path, span) {
        diagnostics.push(diagnostic);
    }
    if let Some(diagnostic) = mutable_borrow_conflict_diagnostic(
        conflicting_borrow(&path, true, loan_owner_for_expr(expr)),
        span,
    ) {
        diagnostics.push(diagnostic);
    }
    if let Some(diagnostic) = immutable_root_diagnostic(
        ctx.local_mutability,
        &path,
        span,
        "cannot borrow immutable binding as mutable",
        "declare the binding as `let` before taking `&mut`",
    ) {
        diagnostics.push(diagnostic);
    }
    diagnostics
}
