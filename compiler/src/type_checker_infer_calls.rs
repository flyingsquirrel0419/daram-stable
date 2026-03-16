use super::*;

impl<'hir> TypeChecker<'hir> {
    pub(super) fn infer_call_expr(&mut self, callee: &HirExpr, args: &[HirExpr]) -> Ty {
        match &callee.kind {
            crate::hir::HirExprKind::DefRef(def) if self.variant_parents.contains_key(def) => {
                let arg_tys = args
                    .iter()
                    .map(|arg| self.infer_expr_ty(arg))
                    .collect::<Vec<_>>();
                if let Some(enum_def) = self.variant_parents.get(def).copied() {
                    let enum_ty = self.instantiate_enum_ty(enum_def);
                    let substitution = self.enum_type_mapping(enum_def, &enum_ty);
                    if let Some(variant_fields) = self
                        .enum_variants
                        .get(&enum_def)
                        .and_then(|variants| variants.iter().find(|variant| variant.def == *def))
                        .map(|variant| variant.fields.clone())
                    {
                        if variant_fields.len() != arg_tys.len() {
                            self.errors.push(
                                Diagnostic::error(format!(
                                    "enum variant expects {} arguments, got {}",
                                    variant_fields.len(),
                                    arg_tys.len()
                                ))
                                .with_span(callee.span),
                            );
                        }
                        for (arg_ty, field_ty) in
                            arg_tys.into_iter().zip(variant_fields.into_iter())
                        {
                            let expected_field_ty =
                                self.substitute_enum_ty(&field_ty, &substitution);
                            if let Err((found, expected)) =
                                self.uf.unify(arg_ty, expected_field_ty.clone())
                            {
                                self.push_type_mismatch(
                                    callee.span,
                                    "mismatched enum variant field type",
                                    found,
                                    expected,
                                );
                            }
                        }
                    }
                    enum_ty
                } else {
                    Ty::Var(self.uf.fresh_var())
                }
            }
            crate::hir::HirExprKind::DefRef(def) => {
                let arg_tys = args
                    .iter()
                    .map(|arg| self.infer_expr_ty(arg))
                    .collect::<Vec<_>>();
                if let Some((expected_params, ret_ty)) = self.instantiate_fn_signature(*def) {
                    if expected_params.len() != args.len() {
                        self.errors.push(
                            Diagnostic::error(format!(
                                "function expects {} arguments, got {}",
                                expected_params.len(),
                                args.len()
                            ))
                            .with_span(callee.span)
                            .with_note(
                                "check the call arity and any trailing default parameters that can be omitted",
                            ),
                        );
                    } else {
                        for ((arg, arg_ty), expected_ty) in
                            args.iter().zip(arg_tys.iter()).zip(expected_params.iter())
                        {
                            if let Err((found, expected)) =
                                self.unify_call_ty(arg_ty.clone(), expected_ty.clone())
                            {
                                self.push_type_mismatch(
                                    arg.span,
                                    "mismatched function argument type",
                                    found,
                                    expected,
                                );
                            }
                        }
                    }
                    ret_ty
                } else {
                    self.fn_ret_tys
                        .get(def)
                        .cloned()
                        .unwrap_or_else(|| Ty::Var(self.uf.fresh_var()))
                }
            }
            _ => {
                for arg in args {
                    let _ = self.infer_expr_ty(arg);
                }
                let _ = self.infer_expr_ty(callee);
                Ty::Var(self.uf.fresh_var())
            }
        }
    }

    pub(super) fn infer_struct_expr(
        &mut self,
        expr: &HirExpr,
        def: DefId,
        fields: &[(String, HirExpr)],
        rest: &Option<Box<HirExpr>>,
    ) -> Ty {
        let struct_ty = self.instantiate_struct_ty(def);
        let substitution = self.struct_type_mapping(def, &struct_ty);
        if let Some(expected_fields) = self.struct_fields.get(&def).cloned() {
            let mut seen = HashMap::new();
            for (name, value) in fields {
                seen.insert(name.clone(), value.span);
                let value_ty = self.infer_expr_ty(value);
                if let Some(expected) = expected_fields.get(name) {
                    let expected = self.substitute_enum_ty(expected, &substitution);
                    if let Err((found, expected)) = self.uf.unify(value_ty, expected.clone()) {
                        self.push_type_mismatch(
                            value.span,
                            &format!("mismatched struct field `{name}` types"),
                            found,
                            expected,
                        );
                    }
                } else {
                    self.errors.push(
                        Diagnostic::error(format!("unknown struct field `{name}`"))
                            .with_span(value.span),
                    );
                }
            }
            if rest.is_none() {
                for field_name in expected_fields.keys() {
                    if !seen.contains_key(field_name) {
                        self.errors.push(
                            Diagnostic::error(format!("missing struct field `{field_name}`"))
                                .with_span(expr.span),
                        );
                    }
                }
            }
        } else {
            for (_, value) in fields {
                let _ = self.infer_expr_ty(value);
            }
        }
        if let Some(rest) = rest {
            let rest_ty = self.infer_expr_ty(rest);
            if let Err((found, expected)) = self.uf.unify(rest_ty, struct_ty.clone()) {
                self.push_type_mismatch(rest.span, "mismatched struct rest type", found, expected);
            }
        }
        struct_ty
    }

    pub(super) fn infer_method_call_expr(
        &mut self,
        expr: &HirExpr,
        receiver: &HirExpr,
        method_name: &str,
        args: &[HirExpr],
    ) -> Ty {
        let receiver_ty = self.infer_expr_ty(receiver);
        let arg_tys = args
            .iter()
            .map(|arg| self.infer_expr_ty(arg))
            .collect::<Vec<_>>();
        if let Some(method_def) = self.lookup_method_def(&receiver_ty, method_name) {
            self.resolved_methods.insert(expr.id, method_def);
            if let Some((expected_params, ret_ty)) = self.instantiate_fn_signature(method_def) {
                if expected_params.len() != args.len() + 1 {
                    self.errors.push(
                        Diagnostic::error(format!(
                            "method `{}` expects {} arguments, got {}",
                            method_name,
                            expected_params.len().saturating_sub(1),
                            args.len()
                        ))
                        .with_span(expr.span),
                    );
                } else {
                    if let Err((found, expected)) =
                        self.unify_call_ty(receiver_ty.clone(), expected_params[0].clone())
                    {
                        self.push_type_mismatch(
                            receiver.span,
                            "mismatched method receiver type",
                            found,
                            expected,
                        );
                    }
                    for ((arg, arg_ty), expected_ty) in args
                        .iter()
                        .zip(arg_tys.iter())
                        .zip(expected_params.iter().skip(1))
                    {
                        if let Err((found, expected)) =
                            self.unify_call_ty(arg_ty.clone(), expected_ty.clone())
                        {
                            self.push_type_mismatch(
                                arg.span,
                                "mismatched method argument type",
                                found,
                                expected,
                            );
                        }
                    }
                }
                ret_ty
            } else {
                self.fn_ret_tys
                    .get(&method_def)
                    .cloned()
                    .unwrap_or_else(|| Ty::Var(self.uf.fresh_var()))
            }
        } else {
            self.errors.push(
                Diagnostic::error(format!("no method named `{method_name}` for this receiver"))
                    .with_span(expr.span),
            );
            Ty::Var(self.uf.fresh_var())
        }
    }

    pub(super) fn infer_try_expr(&mut self, expr: &HirExpr) -> Ty {
        let source_ty = self.infer_expr_ty(expr);
        let Some(current_return_ty) = self.current_return_ty.clone() else {
            self.errors.push(
                Diagnostic::error("`?` can only be used inside a function body")
                    .with_span(expr.span),
            );
            return Ty::Var(self.uf.fresh_var());
        };

        match self.try_carrier_ty(&source_ty) {
            Some(TryCarrierTy::Result { ok, err }) => match self.try_carrier_ty(&current_return_ty)
            {
                Some(TryCarrierTy::Result {
                    err: expected_err, ..
                }) => {
                    if let Err((found, expected)) = self.uf.unify(err, expected_err.clone()) {
                        self.push_type_mismatch(
                            expr.span,
                            "mismatched `?` error type",
                            found,
                            expected,
                        );
                    }
                    ok
                }
                Some(TryCarrierTy::Option { .. }) | None => {
                    self.errors.push(
                        Diagnostic::error(
                            "`?` on `Result` requires the enclosing function to return `Result`",
                        )
                        .with_span(expr.span)
                        .with_note(
                            "change the function return type to `Result<..., E>` or handle the error explicitly",
                        ),
                    );
                    Ty::Var(self.uf.fresh_var())
                }
            },
            Some(TryCarrierTy::Option { some }) => match self.try_carrier_ty(&current_return_ty) {
                Some(TryCarrierTy::Option { .. }) => some,
                Some(TryCarrierTy::Result { .. }) | None => {
                    self.errors.push(
                        Diagnostic::error(
                            "`?` on `Option` requires the enclosing function to return `Option`",
                        )
                        .with_span(expr.span)
                        .with_note(
                            "change the function return type to `Option<T>` or handle the `None` case explicitly",
                        ),
                    );
                    Ty::Var(self.uf.fresh_var())
                }
            },
            None => {
                self.errors.push(
                    Diagnostic::error("`?` can only be applied to `Result` or `Option` values")
                        .with_span(expr.span),
                );
                Ty::Var(self.uf.fresh_var())
            }
        }
    }
}
