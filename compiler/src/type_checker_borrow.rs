use crate::{
    diagnostics::{Diagnostic, Label},
    hir::{HirExpr, HirId},
    source::Span,
};
use std::collections::HashMap;

#[derive(Default, Clone)]
pub(crate) struct BorrowState {
    pub shared: Vec<BorrowLoan>,
    pub mutable: Option<BorrowLoan>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct BorrowLoan {
    pub span: Span,
    pub owner: Option<HirId>,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub(crate) enum BorrowSegment {
    Field(String),
    Index,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub(crate) struct BorrowPath {
    pub root: HirId,
    pub segments: Vec<BorrowSegment>,
}

impl BorrowPath {
    pub fn root(root: HirId) -> Self {
        Self {
            root,
            segments: Vec::new(),
        }
    }

    pub fn overlaps(&self, other: &Self) -> bool {
        if self.root != other.root {
            return false;
        }

        for (lhs, rhs) in self.segments.iter().zip(other.segments.iter()) {
            match (lhs, rhs) {
                (BorrowSegment::Index, _) | (_, BorrowSegment::Index) => return true,
                (BorrowSegment::Field(lhs), BorrowSegment::Field(rhs)) if lhs == rhs => {}
                (BorrowSegment::Field(_), BorrowSegment::Field(_)) => return false,
            }
        }

        true
    }
}

pub(crate) fn record_use_after_move(
    moved_locals: &HashMap<HirId, Span>,
    id: HirId,
    use_span: Span,
) -> Diagnostic {
    let mut diagnostic = Diagnostic::error("use of moved value")
        .with_span(use_span)
        .with_note("borrow the value with `&`, clone it, or avoid moving it before this use");
    if let Some(moved_span) = moved_locals.get(&id).copied() {
        diagnostic = diagnostic.with_label(Label::new(moved_span, "value moved here"));
    }
    diagnostic
}

pub(crate) fn local_root_id(expr: &HirExpr) -> Option<HirId> {
    use crate::hir::HirExprKind::*;
    match &expr.kind {
        Var(id) => Some(*id),
        Field { base, .. } | Index { base, .. } => local_root_id(base),
        Ref { expr, .. } => local_root_id(expr),
        _ => None,
    }
}

pub(crate) fn borrow_path(expr: &HirExpr) -> Option<BorrowPath> {
    use crate::hir::HirExprKind::*;

    match &expr.kind {
        Var(id) => Some(BorrowPath::root(*id)),
        Field { base, field, .. } => {
            let mut path = borrow_path(base)?;
            path.segments.push(BorrowSegment::Field(field.clone()));
            Some(path)
        }
        Index { base, .. } => {
            let mut path = borrow_path(base)?;
            path.segments.push(BorrowSegment::Index);
            Some(path)
        }
        Ref { expr, .. } => borrow_path(expr),
        _ => None,
    }
}

pub(crate) fn borrow_alias_path(
    expr: &HirExpr,
    local_borrow_aliases: &HashMap<HirId, BorrowPath>,
) -> Option<BorrowPath> {
    use crate::hir::HirExprKind::*;

    match &expr.kind {
        Var(id) => local_borrow_aliases.get(id).cloned(),
        Field { base, field, .. } => {
            let mut path =
                borrow_alias_path(base, local_borrow_aliases).or_else(|| borrow_path(base))?;
            path.segments.push(BorrowSegment::Field(field.clone()));
            Some(path)
        }
        Index { base, .. } => {
            let mut path =
                borrow_alias_path(base, local_borrow_aliases).or_else(|| borrow_path(base))?;
            path.segments.push(BorrowSegment::Index);
            Some(path)
        }
        Deref(inner) | Ref { expr: inner, .. } => {
            borrow_alias_path(inner, local_borrow_aliases).or_else(|| borrow_path(inner))
        }
        _ => None,
    }
}

pub(crate) fn loan_owner_for_expr(
    expr: &HirExpr,
    local_borrow_aliases: &HashMap<HirId, BorrowPath>,
) -> Option<HirId> {
    use crate::hir::HirExprKind::*;

    match &expr.kind {
        Var(id) => local_borrow_aliases.contains_key(id).then_some(*id),
        Field { base, .. } | Index { base, .. } | Deref(base) | Ref { expr: base, .. } => {
            loan_owner_for_expr(base, local_borrow_aliases)
        }
        _ => None,
    }
}

pub(crate) fn note_local_use(
    local_use_counts: &mut HashMap<HirId, usize>,
    borrowed_locals: &mut HashMap<BorrowPath, BorrowState>,
    id: HirId,
) {
    let should_release = if let Some(remaining) = local_use_counts.get_mut(&id) {
        if *remaining > 0 {
            *remaining -= 1;
        }
        *remaining == 0
    } else {
        false
    };

    if should_release {
        release_loans_owned_by(borrowed_locals, id);
    }
}

pub(crate) fn clear_borrows_for_root(
    borrowed_locals: &mut HashMap<BorrowPath, BorrowState>,
    root: HirId,
) {
    borrowed_locals.retain(|path, _| path.root != root);
}

pub(crate) fn clear_temporary_borrows(borrowed_locals: &mut HashMap<BorrowPath, BorrowState>) {
    retain_borrows(borrowed_locals, |loan| loan.owner.is_some());
}

pub(crate) fn release_loans_owned_by(
    borrowed_locals: &mut HashMap<BorrowPath, BorrowState>,
    owner: HirId,
) {
    retain_borrows(borrowed_locals, |loan| loan.owner != Some(owner));
}

pub(crate) fn retain_borrows(
    borrowed_locals: &mut HashMap<BorrowPath, BorrowState>,
    mut keep: impl FnMut(&BorrowLoan) -> bool,
) {
    borrowed_locals.retain(|_, state| {
        state.shared.retain(|loan| keep(loan));
        if state.mutable.as_ref().is_some_and(|loan| !keep(loan)) {
            state.mutable = None;
        }
        !state.shared.is_empty() || state.mutable.is_some()
    });
}

pub(crate) fn conflicting_borrow(
    borrowed_locals: &HashMap<BorrowPath, BorrowState>,
    path: &BorrowPath,
    require_mutable: bool,
    ignored_owner: Option<HirId>,
) -> Option<Span> {
    borrowed_locals.iter().find_map(|(borrowed_path, state)| {
        if !borrowed_path.overlaps(path) {
            return None;
        }
        if require_mutable {
            state
                .mutable
                .as_ref()
                .filter(|loan| loan.owner != ignored_owner)
                .map(|loan| loan.span)
                .or_else(|| {
                    state
                        .shared
                        .iter()
                        .find(|loan| loan.owner != ignored_owner)
                        .map(|loan| loan.span)
                })
        } else {
            state
                .mutable
                .as_ref()
                .filter(|loan| loan.owner != ignored_owner)
                .map(|loan| loan.span)
        }
    })
}

pub(crate) fn record_shared_borrow(
    borrowed_locals: &mut HashMap<BorrowPath, BorrowState>,
    path: BorrowPath,
    span: Span,
    owner: Option<HirId>,
    ignored_owner: Option<HirId>,
) -> Option<Diagnostic> {
    if let Some(existing) = conflicting_borrow(borrowed_locals, &path, false, ignored_owner) {
        return Some(
            Diagnostic::error("cannot immutably borrow value while it is mutably borrowed")
                .with_span(span)
                .with_label(Label::new(existing, "mutable borrow occurs here"))
                .with_note(
                    "use a shared borrow before taking `&mut`, or separate the borrow scopes",
                ),
        );
    }
    borrowed_locals
        .entry(path)
        .or_default()
        .shared
        .push(BorrowLoan { span, owner });
    None
}

pub(crate) fn record_mutable_borrow(
    borrowed_locals: &mut HashMap<BorrowPath, BorrowState>,
    path: BorrowPath,
    span: Span,
    owner: Option<HirId>,
    ignored_owner: Option<HirId>,
) -> Option<Diagnostic> {
    if let Some(existing) = conflicting_borrow(borrowed_locals, &path, true, ignored_owner) {
        return Some(
            Diagnostic::error("cannot mutably borrow value while it is already borrowed")
                .with_span(span)
                .with_label(Label::new(existing, "borrow occurs here"))
                .with_note("only one mutable borrow is allowed at a time"),
        );
    }
    borrowed_locals.entry(path).or_default().mutable = Some(BorrowLoan { span, owner });
    None
}

pub(crate) fn compute_use_counts(body: &HirExpr) -> HashMap<HirId, usize> {
    let mut uses = HashMap::new();
    collect_expr_uses(body, &mut uses);
    uses
}

fn collect_stmt_uses(stmt: &crate::hir::HirStmt, uses: &mut HashMap<HirId, usize>) {
    match &stmt.kind {
        crate::hir::HirStmtKind::Let { init, .. } => {
            if let Some(init) = init {
                collect_expr_uses(init, uses);
            }
        }
        crate::hir::HirStmtKind::Expr(expr)
        | crate::hir::HirStmtKind::Errdefer(expr)
        | crate::hir::HirStmtKind::Defer(expr) => collect_expr_uses(expr, uses),
        crate::hir::HirStmtKind::Use(_) => {}
    }
}

fn collect_expr_uses(expr: &HirExpr, uses: &mut HashMap<HirId, usize>) {
    use crate::hir::HirExprKind::*;

    match &expr.kind {
        Var(id) => *uses.entry(*id).or_default() += 1,
        Block(stmts, tail) => {
            for stmt in stmts {
                collect_stmt_uses(stmt, uses);
            }
            if let Some(tail) = tail {
                collect_expr_uses(tail, uses);
            }
        }
        Call { callee, args } => {
            collect_expr_uses(callee, uses);
            for arg in args {
                collect_expr_uses(arg, uses);
            }
        }
        MethodCall { receiver, args, .. } => {
            collect_expr_uses(receiver, uses);
            for arg in args {
                collect_expr_uses(arg, uses);
            }
        }
        Field { base, .. }
        | Deref(base)
        | Try(base)
        | Await(base)
        | Unsafe(base)
        | AsyncBlock(base)
        | Loop(base)
        | Errdefer(base)
        | Defer(base)
        | Ref { expr: base, .. }
        | Cast { expr: base, .. }
        | UnaryOp { operand: base, .. } => collect_expr_uses(base, uses),
        Index { base, index } => {
            collect_expr_uses(base, uses);
            collect_expr_uses(index, uses);
        }
        Tuple(elems) | Array(elems) => {
            for elem in elems {
                collect_expr_uses(elem, uses);
            }
        }
        Repeat { elem, .. } => collect_expr_uses(elem, uses),
        Struct { fields, rest, .. } => {
            for (_, value) in fields {
                collect_expr_uses(value, uses);
            }
            if let Some(rest) = rest {
                collect_expr_uses(rest, uses);
            }
        }
        If {
            condition,
            then_branch,
            else_branch,
        } => {
            collect_expr_uses(condition, uses);
            collect_expr_uses(then_branch, uses);
            if let Some(else_branch) = else_branch {
                collect_expr_uses(else_branch, uses);
            }
        }
        Match { scrutinee, arms } => {
            collect_expr_uses(scrutinee, uses);
            for arm in arms {
                if let Some(guard) = &arm.guard {
                    collect_expr_uses(guard, uses);
                }
                collect_expr_uses(&arm.body, uses);
            }
        }
        BinOp { lhs, rhs, .. }
        | Assign {
            target: lhs,
            value: rhs,
        } => {
            collect_expr_uses(lhs, uses);
            collect_expr_uses(rhs, uses);
        }
        Return(value) | Break(value) => {
            if let Some(value) = value {
                collect_expr_uses(value, uses);
            }
        }
        While { condition, body } => {
            collect_expr_uses(condition, uses);
            collect_expr_uses(body, uses);
        }
        ForDesugared { iter, body, .. } => {
            collect_expr_uses(iter, uses);
            collect_expr_uses(body, uses);
        }
        Closure { body, .. } => collect_expr_uses(body, uses),
        Range { lo, hi, .. } => {
            if let Some(lo) = lo {
                collect_expr_uses(lo, uses);
            }
            if let Some(hi) = hi {
                collect_expr_uses(hi, uses);
            }
        }
        Lit(_) | DefRef(_) | Continue => {}
    }
}
