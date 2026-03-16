use super::*;

impl<'hir> TypeChecker<'hir> {
    pub(super) fn infer_block_expr(
        &mut self,
        stmts: &[crate::hir::HirStmt],
        tail: &Option<Box<HirExpr>>,
    ) -> Ty {
        let snapshot = self.snapshot_retained_move_scope();
        for stmt in stmts {
            self.infer_stmt(stmt);
        }
        let ty = tail
            .as_ref()
            .map(|expr| self.infer_expr_ty(expr))
            .unwrap_or(Ty::Unit);
        self.restore_retained_move_scope(snapshot);
        ty
    }

    pub(super) fn infer_match_expr(
        &mut self,
        match_expr: &HirExpr,
        scrutinee: &HirExpr,
        arms: &[crate::hir::HirArm],
    ) -> Ty {
        let scrutinee_ty = self.infer_expr_ty(scrutinee);
        let mut arm_ty: Option<Ty> = None;
        for arm in arms {
            let snapshot = self.snapshot_branch_scope();
            self.bind_pattern(&arm.pattern, &scrutinee_ty);
            if let Some(guard) = &arm.guard {
                let guard_ty = self.infer_expr_ty(guard);
                if let Err((found, expected)) = self.uf.unify(guard_ty, Ty::Bool) {
                    self.push_type_mismatch(
                        guard.span,
                        "mismatched match guard type",
                        found,
                        expected,
                    );
                }
            }
            let body_ty = self.infer_expr_ty(&arm.body);
            if let Some(existing) = &arm_ty {
                if let Err((found, expected)) = self.uf.unify(existing.clone(), body_ty) {
                    self.push_type_mismatch(
                        arm.body.span,
                        "mismatched match arm type",
                        found,
                        expected,
                    );
                }
            } else {
                arm_ty = Some(body_ty);
            }
            self.restore_branch_scope(snapshot);
        }
        self.check_match_exhaustiveness(match_expr, &scrutinee_ty, arms);
        arm_ty.unwrap_or(Ty::Unit)
    }

    pub(super) fn infer_for_desugared_expr(
        &mut self,
        iter: &HirExpr,
        binding: crate::hir::HirId,
        body: &HirExpr,
    ) -> Ty {
        let iter_ty = self.infer_expr_ty(iter);
        let binding_ty = self.for_iter_item_ty(&iter_ty).unwrap_or_else(|| {
            self.errors.push(
                Diagnostic::error("for-loop iteration currently supports Vec and array-like values")
                    .with_span(iter.span)
                    .with_note(
                        "try iterating a `std::collections::Vec<T>`, `std::collections::HashMap<K, V>`, or `[T; N]` value",
                    ),
            );
            Ty::Var(self.uf.fresh_var())
        });
        let snapshot = self.snapshot_retained_move_scope();
        self.locals.insert(binding, binding_ty);
        self.local_mutability.insert(binding, false);
        self.local_borrow_aliases.remove(&binding);
        self.moved_locals.remove(&binding);
        let _ = self.infer_expr_ty(body);
        self.restore_retained_move_scope(snapshot);
        Ty::Unit
    }

    pub(super) fn infer_ref_expr(&mut self, mutable: bool, expr: &HirExpr) -> Ty {
        let inner = self.infer_place_expr_ty(expr, false);
        if mutable {
            self.check_mutable_borrow_target(expr);
            if let Some(path) = self
                .borrow_path(expr)
                .or_else(|| self.borrow_alias_path(expr))
            {
                self.record_mutable_borrow(path, expr.span, None, self.loan_owner_for_expr(expr));
            }
        } else if let Some(path) = self
            .borrow_path(expr)
            .or_else(|| self.borrow_alias_path(expr))
        {
            self.record_shared_borrow(path, expr.span, None, self.loan_owner_for_expr(expr));
        }
        Ty::Ref {
            mutable,
            inner: Box::new(inner),
        }
    }

    pub(super) fn infer_closure_expr(
        &mut self,
        params: &[crate::hir::HirParam],
        ret_ty: &Ty,
        body: &HirExpr,
    ) -> Ty {
        let snapshot = self.snapshot_closure_scope();
        self.local_use_counts = self.compute_use_counts(body);
        for param in params {
            self.locals.insert(param.binding, param.ty.clone());
            self.local_mutability.insert(param.binding, param.mutable);
            self.local_borrow_aliases.remove(&param.binding);
        }
        let body_ty = self.infer_expr_ty(body);
        if let Err((found, expected)) = self.uf.unify(body_ty, ret_ty.clone()) {
            self.push_type_mismatch(body.span, "mismatched closure return type", found, expected);
        }
        self.restore_closure_scope(snapshot);
        Ty::FnPtr {
            params: params.iter().map(|param| param.ty.clone()).collect(),
            ret: Box::new(ret_ty.clone()),
        }
    }
}
