//! Type checker for the Daram language.
//!
//! Performs:
//! - Type inference via a simplified Hindley-Milner constraint solver
//! - Ability checking (Copy, Hash, Eq, PartialEq, Send, Sync, etc.)
//! - Return type checking
//! - Exhaustiveness checking for `match`
//! - Basic ownership / move checking

use crate::{
    builtin_catalog::{self, BuiltinReturnTy},
    diagnostics::Diagnostic,
    hir::{
        DefId, HirAssocItem, HirExpr, HirImpl, HirImplItem, HirModule, HirPattern, HirPatternKind,
        HirVariant, Ty,
    },
    source::FileId,
    type_checker_abilities::{self, AbilityContext, AbilityKind},
    type_checker_borrow::{self, BorrowPath, BorrowState},
    type_checker_match::{self, MatchContext},
    type_checker_places, type_checker_prepare,
};
use std::collections::{HashMap, HashSet};

#[path = "type_checker_collect.rs"]
mod collect_impl;
#[path = "type_checker_infer_calls.rs"]
mod infer_calls_impl;
#[path = "type_checker_infer.rs"]
mod infer_impl;
#[path = "type_checker_methods.rs"]
mod methods_impl;

// ─── Type unification ─────────────────────────────────────────────────────────

/// Unification table (union-find) for type inference.
struct UnionFind {
    parent: Vec<Option<Ty>>,
}

impl UnionFind {
    fn new() -> Self {
        Self { parent: Vec::new() }
    }

    fn ensure_var(&mut self, id: u32) {
        let required = id as usize + 1;
        if self.parent.len() < required {
            self.parent.resize(required, None);
        }
    }

    fn fresh_var(&mut self) -> u32 {
        let id = self.parent.len() as u32;
        self.parent.push(None);
        id
    }

    fn resolve(&self, ty: &Ty) -> Ty {
        match ty {
            Ty::Var(id) => {
                if let Some(bound) = self.parent.get(*id as usize).and_then(|v| v.as_ref()) {
                    self.resolve(bound)
                } else {
                    ty.clone()
                }
            }
            _ => ty.clone(),
        }
    }

    fn unify(&mut self, a: Ty, b: Ty) -> Result<(), (Ty, Ty)> {
        let a = self.resolve(&a);
        let b = self.resolve(&b);
        match (&a, &b) {
            _ if a == b => Ok(()),
            (Ty::Var(id), _) => {
                self.ensure_var(*id);
                let idx = *id as usize;
                self.parent[idx] = Some(b);
                Ok(())
            }
            (_, Ty::Var(id)) => {
                self.ensure_var(*id);
                let idx = *id as usize;
                self.parent[idx] = Some(a);
                Ok(())
            }
            (
                Ty::Ref {
                    mutable: m1,
                    inner: i1,
                },
                Ty::Ref {
                    mutable: m2,
                    inner: i2,
                },
            ) if m1 == m2 => self.unify(*i1.clone(), *i2.clone()),
            (
                Ty::RawPtr {
                    mutable: m1,
                    inner: i1,
                },
                Ty::RawPtr {
                    mutable: m2,
                    inner: i2,
                },
            ) if m1 == m2 => self.unify(*i1.clone(), *i2.clone()),
            (Ty::Slice(lhs), Ty::Slice(rhs)) => self.unify(*lhs.clone(), *rhs.clone()),
            (
                Ty::Array {
                    elem: l_elem,
                    len: l_len,
                },
                Ty::Array {
                    elem: r_elem,
                    len: r_len,
                },
            ) if l_len == r_len => self.unify(*l_elem.clone(), *r_elem.clone()),
            (Ty::Tuple(ts1), Ty::Tuple(ts2)) if ts1.len() == ts2.len() => {
                for (t1, t2) in ts1.clone().into_iter().zip(ts2.clone()) {
                    self.unify(t1, t2)?;
                }
                Ok(())
            }
            (
                Ty::Named {
                    def: left_def,
                    args: left_args,
                },
                Ty::Named {
                    def: right_def,
                    args: right_args,
                },
            ) if left_def == right_def && left_args.len() == right_args.len() => {
                for (left, right) in left_args.iter().cloned().zip(right_args.iter().cloned()) {
                    self.unify(left, right)?;
                }
                Ok(())
            }
            (
                Ty::FnPtr {
                    params: left_params,
                    ret: left_ret,
                },
                Ty::FnPtr {
                    params: right_params,
                    ret: right_ret,
                },
            ) if left_params.len() == right_params.len() => {
                for (left, right) in left_params
                    .iter()
                    .cloned()
                    .zip(right_params.iter().cloned())
                {
                    self.unify(left, right)?;
                }
                self.unify(*left_ret.clone(), *right_ret.clone())
            }
            // Allow concrete types to coerce to `dyn Ability` (one-way subtyping).
            (_, Ty::DynTrait(_)) | (Ty::DynTrait(_), _) => Ok(()),
            _ => Err((a, b)),
        }
    }
}

// ─── Type checker context ─────────────────────────────────────────────────────

struct TypeChecker<'hir> {
    hir: &'hir HirModule,
    uf: UnionFind,
    errors: Vec<Diagnostic>,
    def_names: HashMap<DefId, String>,
    /// Maps `DefId` → resolved function return type.
    fn_ret_tys: HashMap<DefId, Ty>,
    fn_param_tys: HashMap<DefId, Vec<Ty>>,
    fn_generic_vars: HashMap<DefId, Vec<u32>>,
    /// Maps value-like items (`const`, `static`) to their types.
    item_tys: HashMap<DefId, Ty>,
    abilities: HashMap<DefId, AbilityKind>,
    ability_impls: HashMap<DefId, HashSet<AbilityKind>>,
    method_defs: HashMap<(DefId, String), DefId>,
    resolved_methods: HashMap<crate::hir::HirId, DefId>,
    /// Maps struct defs to their field types.
    struct_fields: HashMap<DefId, HashMap<String, Ty>>,
    struct_generic_vars: HashMap<DefId, Vec<u32>>,
    enum_variants: HashMap<DefId, Vec<HirVariant>>,
    enum_generic_vars: HashMap<DefId, Vec<u32>>,
    variant_parents: HashMap<DefId, DefId>,
    locals: HashMap<crate::hir::HirId, Ty>,
    local_mutability: HashMap<crate::hir::HirId, bool>,
    local_borrow_aliases: HashMap<crate::hir::HirId, BorrowPath>,
    local_use_counts: HashMap<crate::hir::HirId, usize>,
    moved_locals: HashMap<crate::hir::HirId, crate::source::Span>,
    borrowed_locals: HashMap<BorrowPath, BorrowState>,
    current_return_ty: Option<Ty>,
}

struct RetainedMoveScopeSnapshot {
    locals: HashMap<crate::hir::HirId, Ty>,
    local_mutability: HashMap<crate::hir::HirId, bool>,
    local_borrow_aliases: HashMap<crate::hir::HirId, BorrowPath>,
    borrowed_locals: HashMap<BorrowPath, BorrowState>,
    outer_bindings: HashSet<crate::hir::HirId>,
}

struct BranchScopeSnapshot {
    locals: HashMap<crate::hir::HirId, Ty>,
    local_mutability: HashMap<crate::hir::HirId, bool>,
    local_borrow_aliases: HashMap<crate::hir::HirId, BorrowPath>,
    moved_locals: HashMap<crate::hir::HirId, crate::source::Span>,
    borrowed_locals: HashMap<BorrowPath, BorrowState>,
}

struct ClosureScopeSnapshot {
    locals: HashMap<crate::hir::HirId, Ty>,
    local_mutability: HashMap<crate::hir::HirId, bool>,
    local_borrow_aliases: HashMap<crate::hir::HirId, BorrowPath>,
    local_use_counts: HashMap<crate::hir::HirId, usize>,
    moved_locals: HashMap<crate::hir::HirId, crate::source::Span>,
    borrowed_locals: HashMap<BorrowPath, BorrowState>,
}

enum TryCarrierTy {
    Result { ok: Ty, err: Ty },
    Option { some: Ty },
}

impl<'hir> TypeChecker<'hir> {
    fn builtin_return_ty(name: &str) -> Option<Ty> {
        match builtin_catalog::known_return(name)? {
            BuiltinReturnTy::Unit => Some(Ty::Unit),
            BuiltinReturnTy::Never => Some(Ty::Never),
            BuiltinReturnTy::String => Some(Ty::String),
        }
    }

    fn new(hir: &'hir HirModule) -> Self {
        let mut checker = Self {
            hir,
            uf: UnionFind::new(),
            errors: Vec::new(),
            def_names: HashMap::new(),
            fn_ret_tys: HashMap::new(),
            fn_param_tys: HashMap::new(),
            fn_generic_vars: HashMap::new(),
            item_tys: HashMap::new(),
            abilities: HashMap::new(),
            ability_impls: HashMap::new(),
            method_defs: HashMap::new(),
            resolved_methods: HashMap::new(),
            struct_fields: HashMap::new(),
            struct_generic_vars: HashMap::new(),
            enum_variants: HashMap::new(),
            enum_generic_vars: HashMap::new(),
            variant_parents: HashMap::new(),
            locals: HashMap::new(),
            local_mutability: HashMap::new(),
            local_borrow_aliases: HashMap::new(),
            local_use_counts: HashMap::new(),
            moved_locals: HashMap::new(),
            borrowed_locals: HashMap::new(),
            current_return_ty: None,
        };
        checker.collect_module_items(hir);
        type_checker_abilities::collect_derived_abilities(hir, &mut checker.ability_impls);
        checker
    }

    fn snapshot_retained_move_scope(&self) -> RetainedMoveScopeSnapshot {
        RetainedMoveScopeSnapshot {
            outer_bindings: self.locals.keys().copied().collect(),
            locals: self.locals.clone(),
            local_mutability: self.local_mutability.clone(),
            local_borrow_aliases: self.local_borrow_aliases.clone(),
            borrowed_locals: self.borrowed_locals.clone(),
        }
    }

    fn restore_retained_move_scope(&mut self, snapshot: RetainedMoveScopeSnapshot) {
        self.locals = snapshot.locals;
        self.local_mutability = snapshot.local_mutability;
        self.local_borrow_aliases = snapshot.local_borrow_aliases;
        self.borrowed_locals = snapshot.borrowed_locals;
        self.moved_locals
            .retain(|id, _| snapshot.outer_bindings.contains(id));
    }

    fn snapshot_branch_scope(&self) -> BranchScopeSnapshot {
        BranchScopeSnapshot {
            locals: self.locals.clone(),
            local_mutability: self.local_mutability.clone(),
            local_borrow_aliases: self.local_borrow_aliases.clone(),
            moved_locals: self.moved_locals.clone(),
            borrowed_locals: self.borrowed_locals.clone(),
        }
    }

    fn restore_branch_scope(&mut self, snapshot: BranchScopeSnapshot) {
        self.locals = snapshot.locals;
        self.local_mutability = snapshot.local_mutability;
        self.local_borrow_aliases = snapshot.local_borrow_aliases;
        self.moved_locals = snapshot.moved_locals;
        self.borrowed_locals = snapshot.borrowed_locals;
    }

    fn snapshot_closure_scope(&self) -> ClosureScopeSnapshot {
        ClosureScopeSnapshot {
            locals: self.locals.clone(),
            local_mutability: self.local_mutability.clone(),
            local_borrow_aliases: self.local_borrow_aliases.clone(),
            local_use_counts: self.local_use_counts.clone(),
            moved_locals: self.moved_locals.clone(),
            borrowed_locals: self.borrowed_locals.clone(),
        }
    }

    fn restore_closure_scope(&mut self, snapshot: ClosureScopeSnapshot) {
        self.locals = snapshot.locals;
        self.local_mutability = snapshot.local_mutability;
        self.local_borrow_aliases = snapshot.local_borrow_aliases;
        self.local_use_counts = snapshot.local_use_counts;
        self.moved_locals = snapshot.moved_locals;
        self.borrowed_locals = snapshot.borrowed_locals;
    }

    fn string_def(&self) -> Option<DefId> {
        self.def_names.iter().find_map(|(def, name)| {
            (name == "std::core::String" || name == "String").then_some(*def)
        })
    }

    fn for_iter_item_ty(&mut self, iter_ty: &Ty) -> Option<Ty> {
        match self.uf.resolve(iter_ty) {
            Ty::Named { def, args }
                if matches!(
                    self.type_name(def).as_str(),
                    "std::collections::Vec" | "Vec"
                ) =>
            {
                args.first().cloned().map(|elem| Ty::Ref {
                    mutable: false,
                    inner: Box::new(elem),
                })
            }
            Ty::Named { def, args }
                if matches!(
                    self.type_name(def).as_str(),
                    "std::collections::HashMap" | "HashMap"
                ) =>
            {
                match args.as_slice() {
                    [key, value] => Some(Ty::Tuple(vec![
                        Ty::Ref {
                            mutable: false,
                            inner: Box::new(key.clone()),
                        },
                        Ty::Ref {
                            mutable: false,
                            inner: Box::new(value.clone()),
                        },
                    ])),
                    _ => None,
                }
            }
            Ty::Array { elem, .. } | Ty::Slice(elem) => Some(Ty::Ref {
                mutable: false,
                inner: elem,
            }),
            _ => None,
        }
    }

    fn is_variadic_builtin_name(name: &str) -> bool {
        builtin_catalog::is_variadic(name)
    }

    fn is_variadic_builtin_def(&self, def: DefId) -> bool {
        self.def_names
            .get(&def)
            .is_some_and(|name| Self::is_variadic_builtin_name(name))
    }

    fn int_ty() -> Ty {
        Ty::Int(crate::hir::IntSize::I32)
    }

    fn push_type_mismatch(
        &mut self,
        span: crate::source::Span,
        context: &str,
        found: Ty,
        expected: Ty,
    ) {
        let resolved_expected = self.uf.resolve(&expected);
        let resolved_found = self.uf.resolve(&found);
        let mut diagnostic = Diagnostic::error(format!(
            "{context}: expected `{:?}`, found `{:?}`",
            resolved_expected, resolved_found,
        ))
        .with_span(span);

        if let Some(note) = self.mismatch_help(context, &resolved_found, &resolved_expected) {
            diagnostic = diagnostic.with_note(note);
        }

        self.errors.push(diagnostic);
    }

    fn mismatch_help(&self, context: &str, found: &Ty, expected: &Ty) -> Option<&'static str> {
        match context {
            "mismatched assignment types" => {
                Some("assign a value of the binding's declared type, or change the binding type")
            }
            "mismatched index type" => {
                Some("array and slice indices currently require an `i32` value")
            }
            "mismatched match pattern type" => {
                Some("pattern literals and destructuring must match the scrutinee type")
            }
            "mismatched match guard type" => Some("match guards must evaluate to `bool`"),
            _ if matches!(expected, Ty::Ref { .. }) && !matches!(found, Ty::Ref { .. }) => {
                Some("consider borrowing the value with `&` if a reference is expected")
            }
            _ if matches!(found, Ty::Ref { .. }) && !matches!(expected, Ty::Ref { .. }) => Some(
                "consider dereferencing the value with `*` or returning an owned value instead",
            ),
            _ => None,
        }
    }

    fn ability_kind_for_name(&self, name: &str) -> Option<AbilityKind> {
        type_checker_abilities::ability_kind_for_name(name)
    }

    fn ability_kind_for_ty(&self, ty: &Ty) -> Option<AbilityKind> {
        let ctx = AbilityContext {
            def_names: &self.def_names,
            abilities: &self.abilities,
            ability_impls: &self.ability_impls,
            struct_fields: &self.struct_fields,
            enum_variants: &self.enum_variants,
            string_def: self.string_def(),
        };
        type_checker_abilities::ability_kind_for_ty(&ctx, &|ty| self.uf.resolve(ty), ty)
    }

    fn named_def(&self, ty: &Ty) -> Option<DefId> {
        let Ty::Named { def, .. } = self.uf.resolve(ty) else {
            return None;
        };
        Some(def)
    }

    fn type_name(&self, def: DefId) -> String {
        self.def_names
            .get(&def)
            .cloned()
            .unwrap_or_else(|| format!("{def:?}"))
    }

    fn try_carrier_ty(&self, ty: &Ty) -> Option<TryCarrierTy> {
        let Ty::Named { def, args } = self.uf.resolve(ty) else {
            return None;
        };
        let short_name = self
            .type_name(def)
            .rsplit("::")
            .next()
            .unwrap_or_default()
            .to_string();
        match (short_name.as_str(), args.as_slice()) {
            ("Result", [ok, err]) => Some(TryCarrierTy::Result {
                ok: ok.clone(),
                err: err.clone(),
            }),
            ("Option", [some]) => Some(TryCarrierTy::Option { some: some.clone() }),
            _ => None,
        }
    }

    fn is_capability_token_def(&self, def: DefId) -> bool {
        self.type_name(def)
            .rsplit("::")
            .next()
            .is_some_and(|name| name.ends_with("Cap"))
    }

    fn contains_capability_ty(&self, ty: &Ty) -> bool {
        match self.uf.resolve(ty) {
            Ty::Ref { inner, .. } | Ty::RawPtr { inner, .. } | Ty::Slice(inner) => {
                self.contains_capability_ty(&inner)
            }
            Ty::Array { elem, .. } => self.contains_capability_ty(&elem),
            Ty::Tuple(elems) => elems.iter().any(|elem| self.contains_capability_ty(elem)),
            Ty::Named { def, args } => {
                self.is_capability_token_def(def)
                    || args.iter().any(|arg| self.contains_capability_ty(arg))
            }
            Ty::FnPtr { params, ret } => {
                params
                    .iter()
                    .any(|param| self.contains_capability_ty(param))
                    || self.contains_capability_ty(&ret)
            }
            Ty::Bool
            | Ty::Char
            | Ty::Int(_)
            | Ty::Uint(_)
            | Ty::Float(_)
            | Ty::Unit
            | Ty::Never
            | Ty::Var(_)
            | Ty::ImplTrait(_)
            | Ty::DynTrait(_)
            | Ty::Str
            | Ty::String => false,
        }
    }

    fn is_direct_capability_token(&self, ty: &Ty) -> bool {
        matches!(self.uf.resolve(ty), Ty::Named { def, .. } if self.is_capability_token_def(def))
    }

    fn check_capability_surface(&mut self, ty: &Ty, span: crate::source::Span, context: &str) {
        if self.contains_capability_ty(ty) {
            self.errors.push(
                Diagnostic::error(format!(
                    "capability tokens are only supported as direct function parameters in v1, not in {context}"
                ))
                .with_span(span)
                .with_note(
                    "pass capability values as explicit `*Cap` parameters instead of storing or returning them",
                ),
            );
        }
    }

    fn is_copy_ty(&self, ty: &Ty) -> bool {
        let ctx = AbilityContext {
            def_names: &self.def_names,
            abilities: &self.abilities,
            ability_impls: &self.ability_impls,
            struct_fields: &self.struct_fields,
            enum_variants: &self.enum_variants,
            string_def: self.string_def(),
        };
        type_checker_abilities::is_copy_ty(&ctx, &|ty| self.uf.resolve(ty), ty)
    }

    fn check_ability_impl(&mut self, imp: &HirImpl, ability: AbilityKind) {
        let string_def = self.string_def();
        let ctx = AbilityContext {
            def_names: &self.def_names,
            abilities: &self.abilities,
            ability_impls: &self.ability_impls,
            struct_fields: &self.struct_fields,
            enum_variants: &self.enum_variants,
            string_def,
        };
        let self_def = self
            .named_def(&imp.self_ty)
            .or_else(|| self.method_owner_def(&imp.self_ty));
        type_checker_abilities::check_ability_impl(
            &ctx,
            &|ty| self.uf.resolve(ty),
            &mut self.errors,
            imp,
            ability,
            self_def,
        );
    }

    fn check_derived_ability(
        &mut self,
        def: DefId,
        span: crate::source::Span,
        ability: AbilityKind,
    ) {
        let ctx = AbilityContext {
            def_names: &self.def_names,
            abilities: &self.abilities,
            ability_impls: &self.ability_impls,
            struct_fields: &self.struct_fields,
            enum_variants: &self.enum_variants,
            string_def: self.string_def(),
        };
        type_checker_abilities::check_derived_ability(
            &ctx,
            &|ty| self.uf.resolve(ty),
            &mut self.errors,
            def,
            span,
            ability,
        );
    }

    fn instantiate_enum_ty(&mut self, def: DefId) -> Ty {
        let args = self
            .enum_generic_vars
            .get(&def)
            .cloned()
            .unwrap_or_default()
            .into_iter()
            .map(|_| Ty::Var(self.uf.fresh_var()))
            .collect();
        Ty::Named { def, args }
    }

    fn instantiate_struct_ty(&mut self, def: DefId) -> Ty {
        let args = self
            .struct_generic_vars
            .get(&def)
            .cloned()
            .unwrap_or_default()
            .into_iter()
            .map(|_| Ty::Var(self.uf.fresh_var()))
            .collect();
        Ty::Named { def, args }
    }

    fn substitute_enum_ty(&mut self, ty: &Ty, mapping: &HashMap<u32, Ty>) -> Ty {
        if let Ty::Var(id) = ty {
            if let Some(mapped) = mapping.get(id) {
                return mapped.clone();
            }
        }
        match self.uf.resolve(ty) {
            Ty::Var(id) => mapping.get(&id).cloned().unwrap_or(Ty::Var(id)),
            Ty::Ref { mutable, inner } => Ty::Ref {
                mutable,
                inner: Box::new(self.substitute_enum_ty(&inner, mapping)),
            },
            Ty::RawPtr { mutable, inner } => Ty::RawPtr {
                mutable,
                inner: Box::new(self.substitute_enum_ty(&inner, mapping)),
            },
            Ty::Array { elem, len } => Ty::Array {
                elem: Box::new(self.substitute_enum_ty(&elem, mapping)),
                len,
            },
            Ty::Slice(elem) => Ty::Slice(Box::new(self.substitute_enum_ty(&elem, mapping))),
            Ty::Tuple(elems) => Ty::Tuple(
                elems
                    .iter()
                    .map(|elem| self.substitute_enum_ty(elem, mapping))
                    .collect(),
            ),
            Ty::Named { def, args } => Ty::Named {
                def,
                args: args
                    .iter()
                    .map(|arg| self.substitute_enum_ty(arg, mapping))
                    .collect(),
            },
            Ty::FnPtr { params, ret } => Ty::FnPtr {
                params: params
                    .iter()
                    .map(|param| self.substitute_enum_ty(param, mapping))
                    .collect(),
                ret: Box::new(self.substitute_enum_ty(&ret, mapping)),
            },
            other => other,
        }
    }

    fn substitute_fn_ty(&mut self, ty: &Ty, mapping: &HashMap<u32, Ty>) -> Ty {
        if let Ty::Var(id) = ty {
            if let Some(mapped) = mapping.get(id) {
                return mapped.clone();
            }
        }
        match self.uf.resolve(ty) {
            Ty::Var(id) => mapping.get(&id).cloned().unwrap_or(Ty::Var(id)),
            Ty::Ref { mutable, inner } => Ty::Ref {
                mutable,
                inner: Box::new(self.substitute_fn_ty(&inner, mapping)),
            },
            Ty::RawPtr { mutable, inner } => Ty::RawPtr {
                mutable,
                inner: Box::new(self.substitute_fn_ty(&inner, mapping)),
            },
            Ty::Array { elem, len } => Ty::Array {
                elem: Box::new(self.substitute_fn_ty(&elem, mapping)),
                len,
            },
            Ty::Slice(elem) => Ty::Slice(Box::new(self.substitute_fn_ty(&elem, mapping))),
            Ty::Tuple(elems) => Ty::Tuple(
                elems
                    .iter()
                    .map(|elem| self.substitute_fn_ty(elem, mapping))
                    .collect(),
            ),
            Ty::Named { def, args } => Ty::Named {
                def,
                args: args
                    .iter()
                    .map(|arg| self.substitute_fn_ty(arg, mapping))
                    .collect(),
            },
            Ty::FnPtr { params, ret } => Ty::FnPtr {
                params: params
                    .iter()
                    .map(|param| self.substitute_fn_ty(param, mapping))
                    .collect(),
                ret: Box::new(self.substitute_fn_ty(&ret, mapping)),
            },
            other => other,
        }
    }

    fn instantiate_fn_signature(&mut self, def: DefId) -> Option<(Vec<Ty>, Ty)> {
        let params = self.fn_param_tys.get(&def)?.clone();
        let ret = self.fn_ret_tys.get(&def).cloned().unwrap_or(Ty::Unit);
        let generic_vars = self.fn_generic_vars.get(&def).cloned().unwrap_or_default();
        if generic_vars.is_empty() {
            return Some((params, ret));
        }

        let mapping = generic_vars
            .into_iter()
            .map(|id| (id, Ty::Var(self.uf.fresh_var())))
            .collect::<HashMap<_, _>>();
        Some((
            params
                .iter()
                .map(|param| self.substitute_fn_ty(param, &mapping))
                .collect(),
            self.substitute_fn_ty(&ret, &mapping),
        ))
    }

    fn resolve_nested_ty(&self, ty: &Ty) -> Ty {
        match self.uf.resolve(ty) {
            Ty::Ref { mutable, inner } => Ty::Ref {
                mutable,
                inner: Box::new(self.resolve_nested_ty(&inner)),
            },
            Ty::RawPtr { mutable, inner } => Ty::RawPtr {
                mutable,
                inner: Box::new(self.resolve_nested_ty(&inner)),
            },
            Ty::Array { elem, len } => Ty::Array {
                elem: Box::new(self.resolve_nested_ty(&elem)),
                len,
            },
            Ty::Slice(elem) => Ty::Slice(Box::new(self.resolve_nested_ty(&elem))),
            Ty::Tuple(elems) => Ty::Tuple(
                elems
                    .iter()
                    .map(|elem| self.resolve_nested_ty(elem))
                    .collect(),
            ),
            Ty::Named { def, args } => Ty::Named {
                def,
                args: args.iter().map(|arg| self.resolve_nested_ty(arg)).collect(),
            },
            Ty::FnPtr { params, ret } => Ty::FnPtr {
                params: params
                    .iter()
                    .map(|param| self.resolve_nested_ty(param))
                    .collect(),
                ret: Box::new(self.resolve_nested_ty(&ret)),
            },
            other => other,
        }
    }

    fn unify_call_ty(&mut self, actual: Ty, expected: Ty) -> Result<(), (Ty, Ty)> {
        let resolved_actual = self.resolve_nested_ty(&actual);
        let resolved_expected = self.resolve_nested_ty(&expected);
        if let Ty::Ref {
            mutable: false,
            inner,
        } = &resolved_expected
        {
            if resolved_actual == **inner {
                return Ok(());
            }
            if matches!(resolved_actual, Ty::String) && **inner == Ty::Str {
                return Ok(());
            }
        }
        if matches!(resolved_actual, Ty::Int(_) | Ty::Uint(_))
            && matches!(resolved_expected, Ty::Int(_) | Ty::Uint(_))
        {
            return Ok(());
        }
        self.uf.unify(actual, expected).map(|_| ())
    }

    fn enum_type_mapping(&mut self, def: DefId, ty: &Ty) -> HashMap<u32, Ty> {
        let resolved = self.uf.resolve(ty);
        let args = match resolved {
            Ty::Named {
                def: named_def,
                args,
            } if named_def == def => args,
            _ => match self.instantiate_enum_ty(def) {
                Ty::Named { args, .. } => args,
                _ => Vec::new(),
            },
        };
        self.enum_generic_vars
            .get(&def)
            .cloned()
            .unwrap_or_default()
            .into_iter()
            .zip(args)
            .collect()
    }

    fn struct_type_mapping(&mut self, def: DefId, ty: &Ty) -> HashMap<u32, Ty> {
        let resolved = self.uf.resolve(ty);
        let args = match resolved {
            Ty::Named {
                def: named_def,
                args,
            } if named_def == def => args,
            _ => match self.instantiate_struct_ty(def) {
                Ty::Named { args, .. } => args,
                _ => Vec::new(),
            },
        };
        self.struct_generic_vars
            .get(&def)
            .cloned()
            .unwrap_or_default()
            .into_iter()
            .zip(args)
            .collect()
    }

    fn record_use_after_move(&mut self, id: crate::hir::HirId, use_span: crate::source::Span) {
        self.errors.push(type_checker_borrow::record_use_after_move(
            &self.moved_locals,
            id,
            use_span,
        ));
    }

    fn local_root_id(&self, expr: &HirExpr) -> Option<crate::hir::HirId> {
        type_checker_borrow::local_root_id(expr)
    }

    fn borrow_path(&self, expr: &HirExpr) -> Option<BorrowPath> {
        type_checker_borrow::borrow_path(expr)
    }

    fn borrow_alias_path(&self, expr: &HirExpr) -> Option<BorrowPath> {
        type_checker_borrow::borrow_alias_path(expr, &self.local_borrow_aliases)
    }

    fn note_local_use(&mut self, id: crate::hir::HirId) {
        type_checker_borrow::note_local_use(
            &mut self.local_use_counts,
            &mut self.borrowed_locals,
            id,
        );
    }

    fn clear_borrows_for_root(&mut self, root: crate::hir::HirId) {
        type_checker_borrow::clear_borrows_for_root(&mut self.borrowed_locals, root);
    }

    fn clear_temporary_borrows(&mut self) {
        type_checker_borrow::clear_temporary_borrows(&mut self.borrowed_locals);
    }

    fn release_loans_owned_by(&mut self, owner: crate::hir::HirId) {
        type_checker_borrow::release_loans_owned_by(&mut self.borrowed_locals, owner);
    }

    fn local_ty_without_use(&mut self, id: crate::hir::HirId) -> Ty {
        self.locals
            .get(&id)
            .cloned()
            .unwrap_or_else(|| Ty::Var(self.uf.fresh_var()))
    }

    fn conflicting_borrow(
        &self,
        path: &BorrowPath,
        require_mutable: bool,
        ignored_owner: Option<crate::hir::HirId>,
    ) -> Option<crate::source::Span> {
        type_checker_borrow::conflicting_borrow(
            &self.borrowed_locals,
            path,
            require_mutable,
            ignored_owner,
        )
    }

    fn loan_owner_for_expr(&self, expr: &HirExpr) -> Option<crate::hir::HirId> {
        type_checker_borrow::loan_owner_for_expr(expr, &self.local_borrow_aliases)
    }

    fn check_borrow_conflict_on_use(
        &mut self,
        id: crate::hir::HirId,
        span: crate::source::Span,
        consume: bool,
    ) {
        let path = BorrowPath::root(id);
        if let Some(borrow_span) = self.conflicting_borrow(&path, false, None) {
            self.errors.push(
                Diagnostic::error("cannot use value while it is mutably borrowed")
                    .with_span(span)
                    .with_label(crate::diagnostics::Label::new(
                        borrow_span,
                        "mutable borrow occurs here",
                    ))
                    .with_note(
                        "use the value before taking the mutable borrow, or end the borrow first",
                    ),
            );
        }
        if consume {
            if let Some(borrow_span) = self.conflicting_borrow(&path, true, None) {
                self.errors.push(
                    Diagnostic::error("cannot move value while it is borrowed")
                        .with_span(span)
                        .with_label(crate::diagnostics::Label::new(
                            borrow_span,
                            "borrow occurs here",
                        ))
                        .with_note(
                            "move the value after the borrow ends, or pass it by reference instead",
                        ),
                );
            }
        }
    }

    fn check_path_read_use(&mut self, expr: &HirExpr, span: crate::source::Span) {
        let Some(path) = self.place_path(expr) else {
            return;
        };
        self.check_borrowed_path_read_use_with_owner(&path, span, self.loan_owner_for_expr(expr));
    }

    fn check_borrowed_path_read_use_with_owner(
        &mut self,
        path: &BorrowPath,
        span: crate::source::Span,
        ignored_owner: Option<crate::hir::HirId>,
    ) {
        if self.moved_locals.contains_key(&path.root) {
            self.record_use_after_move(path.root, span);
        }
        if let Some(borrow_span) = self.conflicting_borrow(&path, false, ignored_owner) {
            self.errors.push(
                Diagnostic::error("cannot use value while it is mutably borrowed")
                    .with_span(span)
                    .with_label(crate::diagnostics::Label::new(
                        borrow_span,
                        "mutable borrow occurs here",
                    ))
                    .with_note(
                        "use the value before taking the mutable borrow, or end the borrow first",
                    ),
            );
        }
    }

    fn place_path(&self, expr: &HirExpr) -> Option<BorrowPath> {
        type_checker_places::place_path(expr, &self.local_borrow_aliases)
    }

    fn note_if_path_moved(&mut self, path: &BorrowPath, span: crate::source::Span) {
        if let Some(diagnostic) =
            type_checker_places::moved_path_diagnostic(&self.moved_locals, path, span)
        {
            self.errors.push(diagnostic);
        }
    }

    fn report_write_borrow_conflict(
        &mut self,
        path: &BorrowPath,
        span: crate::source::Span,
        ignored_owner: Option<crate::hir::HirId>,
    ) {
        if let Some(diagnostic) = type_checker_places::write_borrow_conflict_diagnostic(
            self.conflicting_borrow(path, true, ignored_owner),
            span,
        ) {
            self.errors.push(diagnostic);
        }
    }

    fn report_mutable_borrow_conflict(
        &mut self,
        path: &BorrowPath,
        span: crate::source::Span,
        ignored_owner: Option<crate::hir::HirId>,
    ) {
        if let Some(diagnostic) = type_checker_places::mutable_borrow_conflict_diagnostic(
            self.conflicting_borrow(path, true, ignored_owner),
            span,
        ) {
            self.errors.push(diagnostic);
        }
    }

    fn record_shared_borrow(
        &mut self,
        path: BorrowPath,
        span: crate::source::Span,
        owner: Option<crate::hir::HirId>,
        ignored_owner: Option<crate::hir::HirId>,
    ) {
        if let Some(diagnostic) = type_checker_borrow::record_shared_borrow(
            &mut self.borrowed_locals,
            path,
            span,
            owner,
            ignored_owner,
        ) {
            self.errors.push(diagnostic);
        }
    }

    fn record_mutable_borrow(
        &mut self,
        path: BorrowPath,
        span: crate::source::Span,
        owner: Option<crate::hir::HirId>,
        ignored_owner: Option<crate::hir::HirId>,
    ) {
        if let Some(diagnostic) = type_checker_borrow::record_mutable_borrow(
            &mut self.borrowed_locals,
            path,
            span,
            owner,
            ignored_owner,
        ) {
            self.errors.push(diagnostic);
        }
    }

    fn local_ty_for_use(
        &mut self,
        id: crate::hir::HirId,
        span: crate::source::Span,
        consume: bool,
        allow_reinit: bool,
    ) -> Ty {
        let ty = self
            .locals
            .get(&id)
            .cloned()
            .unwrap_or_else(|| Ty::Var(self.uf.fresh_var()));

        if self.moved_locals.contains_key(&id) && !allow_reinit {
            self.record_use_after_move(id, span);
        }

        self.check_borrow_conflict_on_use(id, span, consume);

        if consume && !self.is_copy_ty(&ty) {
            self.moved_locals.insert(id, span);
        }

        self.note_local_use(id);

        ty
    }

    fn infer_place_expr_ty(&mut self, expr: &HirExpr, allow_reinit: bool) -> Ty {
        self.infer_place_expr_ty_inner(expr, allow_reinit, true)
    }

    fn infer_place_expr_ty_inner(
        &mut self,
        expr: &HirExpr,
        allow_reinit: bool,
        check_access: bool,
    ) -> Ty {
        use crate::hir::HirExprKind::*;

        match &expr.kind {
            Var(id) => {
                if check_access {
                    self.local_ty_for_use(*id, expr.span, false, allow_reinit)
                } else {
                    self.local_ty_without_use(*id)
                }
            }
            Field { base, field, .. } => {
                if check_access {
                    self.check_path_read_use(expr, expr.span);
                }
                let inferred_base_ty = self.infer_place_expr_ty_inner(base, false, false);
                let base_ty = self.uf.resolve(&inferred_base_ty);
                match base_ty {
                    Ty::Named { def, args } => {
                        let owner_ty = Ty::Named {
                            def,
                            args: args.clone(),
                        };
                        let substitution = self.struct_type_mapping(def, &owner_ty);
                        self.struct_fields
                            .get(&def)
                            .and_then(|fields| fields.get(field).cloned())
                            .map(|field_ty| self.substitute_enum_ty(&field_ty, &substitution))
                            .unwrap_or_else(|| {
                                self.errors.push(
                                    Diagnostic::error(format!("unknown field `{field}`"))
                                        .with_span(expr.span),
                                );
                                Ty::Var(self.uf.fresh_var())
                            })
                    }
                    Ty::Tuple(elems) => field
                        .parse::<usize>()
                        .ok()
                        .and_then(|idx| elems.get(idx).cloned())
                        .unwrap_or_else(|| {
                            self.errors.push(
                                Diagnostic::error(format!("invalid tuple field `{field}`"))
                                    .with_span(expr.span),
                            );
                            Ty::Var(self.uf.fresh_var())
                        }),
                    Ty::Var(id) => Ty::Var(id),
                    _ => Ty::Var(self.uf.fresh_var()),
                }
            }
            Index { base, index } => {
                let index_ty = self.infer_expr_ty(index);
                let expected_index_ty = Self::int_ty();
                if let Err((found, expected)) = self.uf.unify(index_ty, expected_index_ty) {
                    self.push_type_mismatch(index.span, "mismatched index type", found, expected);
                }
                if check_access {
                    self.check_path_read_use(expr, expr.span);
                }
                let inferred_base_ty = self.infer_place_expr_ty_inner(base, false, false);
                let base_ty = self.uf.resolve(&inferred_base_ty);
                match base_ty {
                    Ty::Array { elem, .. } | Ty::Slice(elem) => *elem,
                    Ty::Ref { inner, .. } => match *inner {
                        Ty::Array { elem, .. } | Ty::Slice(elem) => *elem,
                        other => {
                            self.errors.push(
                                Diagnostic::error(format!(
                                    "cannot index into value of type `{:?}`",
                                    other
                                ))
                                .with_span(base.span),
                            );
                            Ty::Var(self.uf.fresh_var())
                        }
                    },
                    Ty::Var(id) => Ty::Var(id),
                    other => {
                        self.errors.push(
                            Diagnostic::error(format!(
                                "cannot index into value of type `{:?}`",
                                other
                            ))
                            .with_span(base.span),
                        );
                        Ty::Var(self.uf.fresh_var())
                    }
                }
            }
            Deref(inner) => {
                if check_access {
                    if let Some(path) = self.borrow_alias_path(inner) {
                        self.check_borrowed_path_read_use_with_owner(
                            &path,
                            expr.span,
                            self.loan_owner_for_expr(inner),
                        );
                    }
                }
                let inner_ty = self.infer_expr_ty(inner);
                match self.uf.resolve(&inner_ty) {
                    Ty::Ref { inner, .. } | Ty::RawPtr { inner, .. } => *inner,
                    _ => Ty::Var(self.uf.fresh_var()),
                }
            }
            _ => self.infer_expr_ty(expr),
        }
    }

    fn check_place_write_access(&mut self, expr: &HirExpr, span: crate::source::Span) {
        use crate::hir::HirExprKind::*;

        match &expr.kind {
            Var(_) | Field { .. } | Index { .. } => {
                let place_ctx = type_checker_places::PlaceContext {
                    moved_locals: &self.moved_locals,
                    local_mutability: &self.local_mutability,
                    local_borrow_aliases: &self.local_borrow_aliases,
                };
                let diagnostics = type_checker_places::write_place_path_diagnostics(
                    &place_ctx,
                    expr,
                    span,
                    &|path, is_mutation, ignored_owner| {
                        self.conflicting_borrow(path, is_mutation, ignored_owner)
                    },
                    &|expr| self.loan_owner_for_expr(expr),
                );
                self.errors.extend(diagnostics);
            }
            Deref(inner) => {
                if let Some(path) = self.borrow_alias_path(inner) {
                    self.note_if_path_moved(&path, span);
                    self.report_write_borrow_conflict(&path, span, self.loan_owner_for_expr(inner));
                }
                let inner_ty = self.infer_expr_ty(inner);
                match self.uf.resolve(&inner_ty) {
                    Ty::Ref { mutable: true, .. } | Ty::RawPtr { mutable: true, .. } => {}
                    other => {
                        self.errors.push(
                            Diagnostic::error(format!(
                                "cannot mutate through non-mutable reference of type `{:?}`",
                                other
                            ))
                            .with_span(span),
                        );
                    }
                }
            }
            _ => {}
        }
    }

    fn check_assignment_target(&mut self, target: &HirExpr) -> Ty {
        use crate::hir::HirExprKind::*;

        match &target.kind {
            Var(id) => {
                if !self.local_mutability.get(id).copied().unwrap_or(false) {
                    self.errors.push(
                        Diagnostic::error("cannot assign to immutable binding")
                            .with_span(target.span)
                            .with_note("declare the binding as `let` if reassignment is intended"),
                    );
                }
                self.release_loans_owned_by(*id);
                self.moved_locals.remove(id);
                self.clear_borrows_for_root(*id);
                self.local_ty_for_use(*id, target.span, false, true)
            }
            Field { .. } | Index { .. } => {
                self.check_place_write_access(target, target.span);
                self.infer_place_expr_ty(target, false)
            }
            Deref(inner) => {
                self.check_place_write_access(target, target.span);
                let inner_ty = self.infer_expr_ty(inner);
                match self.uf.resolve(&inner_ty) {
                    Ty::Ref {
                        mutable: true,
                        inner,
                    }
                    | Ty::RawPtr {
                        mutable: true,
                        inner,
                    } => *inner,
                    other => {
                        self.errors.push(
                            Diagnostic::error(format!(
                                "cannot assign through non-mutable reference of type `{:?}`",
                                other
                            ))
                            .with_span(target.span),
                        );
                        Ty::Var(self.uf.fresh_var())
                    }
                }
            }
            _ => {
                self.errors
                    .push(Diagnostic::error("invalid assignment target").with_span(target.span));
                self.infer_expr_ty(target)
            }
        }
    }

    fn check_mutable_borrow_target(&mut self, expr: &HirExpr) {
        use crate::hir::HirExprKind::*;

        match &expr.kind {
            Var(_) | Field { .. } | Index { .. } => {
                let place_ctx = type_checker_places::PlaceContext {
                    moved_locals: &self.moved_locals,
                    local_mutability: &self.local_mutability,
                    local_borrow_aliases: &self.local_borrow_aliases,
                };
                let diagnostics = type_checker_places::mutable_borrow_place_path_diagnostics(
                    &place_ctx,
                    expr,
                    expr.span,
                    &|path, is_mutation, ignored_owner| {
                        self.conflicting_borrow(path, is_mutation, ignored_owner)
                    },
                    &|expr| self.loan_owner_for_expr(expr),
                );
                self.errors.extend(diagnostics);
            }
            Deref(inner) => {
                if let Some(path) = self.borrow_alias_path(inner) {
                    self.note_if_path_moved(&path, expr.span);
                    self.report_mutable_borrow_conflict(
                        &path,
                        expr.span,
                        self.loan_owner_for_expr(inner),
                    );
                }
                let inner_ty = self.infer_expr_ty(inner);
                match self.uf.resolve(&inner_ty) {
                    Ty::Ref { mutable: true, .. } | Ty::RawPtr { mutable: true, .. } => {}
                    other => {
                        self.errors.push(
                            Diagnostic::error(format!(
                                "cannot take mutable borrow through non-mutable reference of type `{:?}`",
                                other
                            ))
                            .with_span(expr.span),
                        );
                    }
                }
            }
            _ => {}
        }
    }

    fn bind_pattern(&mut self, pattern: &HirPattern, expected_ty: &Ty) {
        match &pattern.kind {
            HirPatternKind::Wildcard => {}
            HirPatternKind::Binding { id, .. } => {
                self.locals.insert(*id, expected_ty.clone());
                self.local_mutability.insert(*id, false);
                self.local_borrow_aliases.remove(id);
                self.moved_locals.remove(id);
            }
            HirPatternKind::Lit(lit) => {
                let lit_ty = self.lit_ty(lit);
                if let Err((found, expected)) = self.uf.unify(lit_ty, expected_ty.clone()) {
                    self.push_type_mismatch(
                        pattern.span,
                        "mismatched match pattern type",
                        found,
                        expected,
                    );
                }
            }
            HirPatternKind::Tuple(elems) => match self.uf.resolve(expected_ty) {
                Ty::Tuple(expected_elems) if expected_elems.len() == elems.len() => {
                    for (elem, expected_elem_ty) in elems.iter().zip(expected_elems.iter()) {
                        self.bind_pattern(elem, expected_elem_ty);
                    }
                }
                Ty::Tuple(_) => {
                    self.errors.push(
                        Diagnostic::error("mismatched tuple pattern arity").with_span(pattern.span),
                    );
                }
                found => {
                    let expected = Ty::Tuple(
                        (0..elems.len())
                            .map(|_| Ty::Var(self.uf.fresh_var()))
                            .collect(),
                    );
                    self.push_type_mismatch(
                        pattern.span,
                        "mismatched tuple pattern type",
                        found,
                        expected,
                    );
                }
            },
            HirPatternKind::Struct { def, fields, .. } => {
                let expected_struct_ty = match self.uf.resolve(expected_ty) {
                    Ty::Named {
                        def: found_def,
                        args,
                    } if found_def == *def => Ty::Named {
                        def: found_def,
                        args,
                    },
                    _ => self.instantiate_struct_ty(*def),
                };
                if let Err((found, expected)) = self
                    .uf
                    .unify(expected_ty.clone(), expected_struct_ty.clone())
                {
                    self.push_type_mismatch(
                        pattern.span,
                        "mismatched struct pattern type",
                        found,
                        expected,
                    );
                }
                if let Some(field_tys) = self.struct_fields.get(def).cloned() {
                    let substitution = self.struct_type_mapping(*def, &expected_struct_ty);
                    for (field_name, field_pattern) in fields {
                        if let Some(field_ty) = field_tys.get(field_name) {
                            let field_ty = self.substitute_enum_ty(field_ty, &substitution);
                            self.bind_pattern(field_pattern, &field_ty);
                        } else {
                            self.errors.push(
                                Diagnostic::error(format!(
                                    "unknown struct pattern field `{field_name}`"
                                ))
                                .with_span(field_pattern.span),
                            );
                        }
                    }
                }
            }
            HirPatternKind::Variant { def, args } => {
                if let Some(enum_def) = self.variant_parents.get(def).copied() {
                    let expected_enum_ty = match self.uf.resolve(expected_ty) {
                        Ty::Named { def, args } if def == enum_def => Ty::Named { def, args },
                        _ => self.instantiate_enum_ty(enum_def),
                    };
                    if let Err((found, expected)) =
                        self.uf.unify(expected_ty.clone(), expected_enum_ty.clone())
                    {
                        self.push_type_mismatch(
                            pattern.span,
                            "mismatched enum pattern type",
                            found,
                            expected,
                        );
                    }
                    if let Some(variant) = self
                        .enum_variants
                        .get(&enum_def)
                        .and_then(|variants| variants.iter().find(|variant| variant.def == *def))
                        .cloned()
                    {
                        let substitution = self.enum_type_mapping(enum_def, &expected_enum_ty);
                        if variant.fields.len() != args.len() {
                            self.errors.push(
                                Diagnostic::error("mismatched enum pattern arity")
                                    .with_span(pattern.span),
                            );
                        }
                        for (arg, field_ty) in args.iter().zip(variant.fields.iter()) {
                            let field_ty = self.substitute_enum_ty(field_ty, &substitution);
                            self.bind_pattern(arg, &field_ty);
                        }
                    }
                } else {
                    for arg in args {
                        let expected = Ty::Var(self.uf.fresh_var());
                        self.bind_pattern(arg, &expected);
                    }
                }
            }
            HirPatternKind::Range { lo, hi, .. } => {
                self.bind_pattern(lo, expected_ty);
                self.bind_pattern(hi, expected_ty);
            }
            HirPatternKind::Or(alternatives) => {
                for alternative in alternatives {
                    self.bind_pattern(alternative, expected_ty);
                }
            }
            HirPatternKind::Ref { inner, .. } => match self.uf.resolve(expected_ty) {
                Ty::Ref {
                    inner: expected_inner,
                    ..
                } => self.bind_pattern(inner, &expected_inner),
                found => {
                    let expected = Ty::Ref {
                        mutable: false,
                        inner: Box::new(Ty::Var(self.uf.fresh_var())),
                    };
                    self.push_type_mismatch(
                        pattern.span,
                        "mismatched reference pattern type",
                        found,
                        expected,
                    );
                }
            },
            HirPatternKind::Slice { elems, rest_index } => match self.uf.resolve(expected_ty) {
                Ty::Array { elem, len } => {
                    if (rest_index.is_none() && len != elems.len())
                        || (rest_index.is_some() && len < elems.len())
                    {
                        self.errors.push(
                            Diagnostic::error("mismatched slice pattern arity")
                                .with_span(pattern.span),
                        );
                    }
                    if let Some(rest_index) = *rest_index {
                        let prefix_len = rest_index.min(elems.len());
                        let suffix_start = prefix_len;
                        for elem_pattern in &elems[..prefix_len] {
                            self.bind_pattern(elem_pattern, &elem);
                        }
                        for elem_pattern in elems.iter().skip(suffix_start) {
                            self.bind_pattern(elem_pattern, &elem);
                        }
                    } else {
                        for elem_pattern in elems {
                            self.bind_pattern(elem_pattern, &elem);
                        }
                    }
                }
                Ty::Slice(elem) => {
                    for elem_pattern in elems {
                        self.bind_pattern(elem_pattern, &elem);
                    }
                }
                found => {
                    let expected = Ty::Slice(Box::new(Ty::Var(self.uf.fresh_var())));
                    self.push_type_mismatch(
                        pattern.span,
                        "mismatched slice pattern type",
                        found,
                        expected,
                    );
                }
            },
        }
    }
}

impl<'hir> TypeChecker<'hir> {
    fn check_match_exhaustiveness(
        &mut self,
        match_expr: &HirExpr,
        scrutinee_ty: &Ty,
        arms: &[crate::hir::HirArm],
    ) {
        let ctx = MatchContext {
            def_names: &self.def_names,
            struct_fields: &self.struct_fields,
            enum_variants: &self.enum_variants,
            resolve_ty: &|ty| self.uf.resolve(ty),
        };
        type_checker_match::check_match_exhaustiveness(
            &ctx,
            match_expr,
            scrutinee_ty,
            arms,
            &mut self.errors,
        );
    }

    fn compute_use_counts(&self, body: &crate::hir::HirExpr) -> HashMap<crate::hir::HirId, usize> {
        type_checker_borrow::compute_use_counts(body)
    }

    fn reset_analysis_state_for_value(&mut self, value: &crate::hir::HirExpr) {
        self.locals.clear();
        self.local_mutability.clear();
        self.local_borrow_aliases.clear();
        self.local_use_counts = self.compute_use_counts(value);
        self.moved_locals.clear();
        self.borrowed_locals.clear();
        self.current_return_ty = None;
    }

    fn reset_analysis_state_for_fn(&mut self, body: Option<&crate::hir::HirExpr>) {
        self.locals.clear();
        self.local_mutability.clear();
        self.local_borrow_aliases.clear();
        self.local_use_counts = body
            .map(|body| self.compute_use_counts(body))
            .unwrap_or_default();
        self.moved_locals.clear();
        self.borrowed_locals.clear();
        self.current_return_ty = None;
    }

    fn check_value_initializer(
        &mut self,
        value: &crate::hir::HirExpr,
        expected_ty: &Ty,
        span: crate::source::Span,
        context: &str,
    ) {
        self.reset_analysis_state_for_value(value);
        let value_ty = self.infer_expr_ty(value);
        if let Err((a, b)) = self.uf.unify(value_ty, expected_ty.clone()) {
            self.errors.push(
                Diagnostic::error(format!(
                    "mismatched {context} types: expected `{:?}`, found `{:?}`",
                    self.uf.resolve(&b),
                    self.uf.resolve(&a),
                ))
                .with_span(span),
            );
        }
    }

    fn check_const(&mut self, item: &crate::hir::HirConst) {
        self.check_capability_surface(&item.ty, item.span, "const items");
        self.check_value_initializer(&item.value, &item.ty, item.span, "const");
    }

    fn check_static(&mut self, item: &crate::hir::HirStatic) {
        self.check_capability_surface(&item.ty, item.span, "static items");
        self.check_value_initializer(&item.value, &item.ty, item.span, "static");
    }

    fn check_assoc_items(&mut self, items: &[HirAssocItem]) {
        for item in items {
            match item {
                HirAssocItem::Method(method) => self.check_fn(method),
                HirAssocItem::Const {
                    ty,
                    default: Some(value),
                    span,
                    ..
                } => {
                    self.check_capability_surface(ty, *span, "associated consts");
                    self.check_value_initializer(value, ty, *span, "associated const");
                }
                HirAssocItem::Const {
                    ty,
                    default: None,
                    span,
                    ..
                } => {
                    self.check_capability_surface(ty, *span, "associated consts");
                }
                HirAssocItem::TypeAssoc { default, span, .. } => {
                    if let Some(default) = default {
                        self.check_capability_surface(default, *span, "associated type defaults");
                    }
                }
            }
        }
    }

    fn check_impl_items(&mut self, items: &[HirImplItem]) {
        for item in items {
            match item {
                HirImplItem::Method(method) => self.check_fn(method),
                HirImplItem::Const {
                    ty, value, span, ..
                } => {
                    self.check_capability_surface(ty, *span, "impl consts");
                    self.check_value_initializer(value, ty, *span, "impl const");
                }
                HirImplItem::TypeAssoc { ty, span, .. } => {
                    self.check_capability_surface(ty, *span, "impl associated types");
                }
            }
        }
    }

    fn check_fn(&mut self, f: &crate::hir::HirFn) {
        self.reset_analysis_state_for_fn(f.body.as_ref());
        for param in &f.params {
            if self.contains_capability_ty(&param.ty) && !self.is_direct_capability_token(&param.ty)
            {
                self.errors.push(
                    Diagnostic::error(
                        "capability tokens are only supported as direct function parameters in v1",
                    )
                    .with_span(f.span)
                    .with_note(
                        "use a plain `*Cap` parameter instead of wrapping it in another type",
                    ),
                );
            }
            if self.is_direct_capability_token(&param.ty) && param.mutable {
                self.errors.push(
                    Diagnostic::error("capability parameters must be immutable in v1")
                        .with_span(f.span)
                        .with_note(
                            "pass capability tokens by value and keep the parameter immutable",
                        ),
                );
            }
        }
        self.check_capability_surface(&f.ret_ty, f.span, "return types");
        for param in &f.params {
            self.locals.insert(param.binding, param.ty.clone());
            self.local_mutability.insert(param.binding, param.mutable);
            self.local_borrow_aliases.remove(&param.binding);
        }
        self.current_return_ty = Some(f.ret_ty.clone());
        if let Some(body) = &f.body {
            let body_ty = self.infer_expr_ty(body);
            // Coerce unit return for void functions.
            let expected_ret = self.uf.resolve(&f.ret_ty);
            if let Err((a, b)) = self.uf.unify(body_ty, expected_ret.clone()) {
                let resolved_a = self.uf.resolve(&a);
                let resolved_b = self.uf.resolve(&b);
                self.errors.push(
                    Diagnostic::error(format!(
                        "mismatched types: expected `{:?}`, found `{:?}`",
                        resolved_b, resolved_a,
                    ))
                    .with_span(body.span),
                );
            }
        }
        self.current_return_ty = None;
    }

    fn infer_stmt(&mut self, stmt: &crate::hir::HirStmt) {
        match &stmt.kind {
            crate::hir::HirStmtKind::Let {
                binding,
                mutable,
                ty,
                init,
                ..
            } => {
                if let Some(init) = init {
                    let init_ty = self.infer_expr_ty(init);
                    if let Err((a, b)) = self.uf.unify(init_ty, ty.clone()) {
                        self.errors.push(
                            Diagnostic::error(format!(
                                "mismatched let binding types: expected `{:?}`, found `{:?}`",
                                self.uf.resolve(&b),
                                self.uf.resolve(&a),
                            ))
                            .with_span(stmt.span),
                        );
                    }
                }
                self.locals.insert(*binding, ty.clone());
                self.local_mutability.insert(*binding, *mutable);
                if let Some(init) = init {
                    if let Some(path) = self.borrow_alias_path(init) {
                        self.local_borrow_aliases.insert(*binding, path);
                    } else {
                        self.local_borrow_aliases.remove(binding);
                    }
                } else {
                    self.local_borrow_aliases.remove(binding);
                }
                self.release_loans_owned_by(*binding);
                self.clear_temporary_borrows();
                if let Some(path) = self.local_borrow_aliases.get(binding).cloned() {
                    let ignored_owner = init
                        .as_ref()
                        .and_then(|expr| self.loan_owner_for_expr(expr));
                    if let Ty::Ref { mutable, .. } = self.uf.resolve(ty) {
                        if mutable {
                            self.record_mutable_borrow(
                                path,
                                stmt.span,
                                Some(*binding),
                                ignored_owner,
                            );
                        } else {
                            self.record_shared_borrow(
                                path,
                                stmt.span,
                                Some(*binding),
                                ignored_owner,
                            );
                        }
                    }
                }
                self.moved_locals.remove(binding);
                self.clear_borrows_for_root(*binding);
            }
            crate::hir::HirStmtKind::Expr(expr)
            | crate::hir::HirStmtKind::Errdefer(expr)
            | crate::hir::HirStmtKind::Defer(expr) => {
                let _ = self.infer_expr_ty(expr);
                self.clear_temporary_borrows();
            }
            crate::hir::HirStmtKind::Use(_) => {
                self.clear_temporary_borrows();
            }
        }
    }

    fn infer_expr_ty(&mut self, expr: &crate::hir::HirExpr) -> Ty {
        use crate::hir::HirExprKind::*;
        match &expr.kind {
            Lit(lit) => self.lit_ty(lit),
            Var(id) => self.local_ty_for_use(*id, expr.span, true, false),
            DefRef(def) if self.variant_parents.contains_key(def) => self
                .variant_parents
                .get(def)
                .copied()
                .map(|enum_def| self.instantiate_enum_ty(enum_def))
                .unwrap_or_else(|| Ty::Var(self.uf.fresh_var())),
            DefRef(def) => self
                .item_tys
                .get(def)
                .cloned()
                .unwrap_or_else(|| Ty::Var(self.uf.fresh_var())),
            Block(stmts, tail) => self.infer_block_expr(stmts, tail),
            Call { callee, args } => self.infer_call_expr(callee, args),
            Tuple(elems) => {
                let tys = elems.iter().map(|e| self.infer_expr_ty(e)).collect();
                Ty::Tuple(tys)
            }
            Array(elems) => {
                let elem_ty = elems
                    .first()
                    .map(|expr| self.infer_expr_ty(expr))
                    .unwrap_or_else(|| Ty::Var(self.uf.fresh_var()));
                for elem in elems.iter().skip(1) {
                    let next_ty = self.infer_expr_ty(elem);
                    let _ = self.uf.unify(elem_ty.clone(), next_ty);
                }
                Ty::Array {
                    elem: Box::new(elem_ty),
                    len: elems.len(),
                }
            }
            BinOp { op, lhs, rhs } => {
                let lt = self.infer_expr_ty(lhs);
                let rt = self.infer_expr_ty(rhs);
                let _ = self.uf.unify(lt.clone(), rt);
                use crate::hir::HirBinOp::*;
                match op {
                    Eq | Ne | Lt | Le | Gt | Ge | And | Or => Ty::Bool,
                    _ => lt,
                }
            }
            UnaryOp {
                op: crate::hir::HirUnaryOp::Not,
                operand,
            } => {
                let t = self.infer_expr_ty(operand);
                let _ = self.uf.unify(t, Ty::Bool);
                Ty::Bool
            }
            UnaryOp { operand, .. } => self.infer_expr_ty(operand),
            If {
                condition,
                then_branch,
                else_branch,
            } => {
                let cond_ty = self.infer_expr_ty(condition);
                let _ = self.uf.unify(cond_ty, Ty::Bool);
                let then_ty = self.infer_expr_ty(then_branch);
                if let Some(else_br) = else_branch {
                    let else_ty = self.infer_expr_ty(else_br);
                    let _ = self.uf.unify(then_ty.clone(), else_ty);
                }
                then_ty
            }
            Match { scrutinee, arms } => self.infer_match_expr(expr, scrutinee, arms),
            Assign { target, value } => {
                let target_ty = self.check_assignment_target(target);
                let value_ty = self.infer_expr_ty(value);
                if let Err((found, expected)) = self.uf.unify(target_ty, value_ty) {
                    self.push_type_mismatch(
                        value.span,
                        "mismatched assignment types",
                        found,
                        expected,
                    );
                }
                Ty::Unit
            }
            Return(val) => {
                if let Some(v) = val {
                    if matches!(
                        self.current_return_ty
                            .as_ref()
                            .map(|ty| self.uf.resolve(ty)),
                        Some(Ty::Ref { .. })
                    ) && self.local_root_id(v).is_some()
                    {
                        self.errors.push(
                            Diagnostic::error("cannot return reference to local binding")
                                .with_span(v.span)
                                .with_note("return an owned value instead of a reference to stack-local data"),
                        );
                    }
                    let value_ty = self.infer_expr_ty(v);
                    if let Some(expected) = &self.current_return_ty {
                        let _ = self.uf.unify(value_ty, expected.clone());
                    }
                }
                Ty::Never
            }
            Break(_) | Continue => Ty::Never,
            While { condition, body } => {
                let cond_ty = self.infer_expr_ty(condition);
                let _ = self.uf.unify(cond_ty, Ty::Bool);
                let _ = self.infer_expr_ty(body);
                Ty::Unit
            }
            ForDesugared {
                iter,
                binding,
                body,
            } => self.infer_for_desugared_expr(iter, *binding, body),
            Loop(body) | Errdefer(body) | Defer(body) | AsyncBlock(body) | Unsafe(body) => {
                let _ = self.infer_expr_ty(body);
                Ty::Unit
            }
            Ref { mutable, expr } => self.infer_ref_expr(*mutable, expr),
            Deref(expr) => {
                let inner_ty = self.infer_expr_ty(expr);
                match self.uf.resolve(&inner_ty) {
                    Ty::Ref { inner, .. } => *inner,
                    _ => Ty::Var(self.uf.fresh_var()),
                }
            }
            Cast { target_ty, .. } => target_ty.clone(),
            Await(expr) => self.infer_expr_ty(expr),
            Struct { def, fields, rest } => self.infer_struct_expr(expr, *def, fields, rest),
            MethodCall {
                receiver,
                method_name,
                method_id: _,
                args,
            } => self.infer_method_call_expr(expr, receiver, method_name, args),
            Field { base, field, .. } => {
                let _ = base;
                let _ = field;
                self.infer_place_expr_ty(expr, false)
            }
            Index { base, index } => {
                let _ = base;
                let _ = index;
                self.infer_place_expr_ty(expr, false)
            }
            Repeat { elem, count } => {
                let elem_ty = self.infer_expr_ty(elem);
                Ty::Array {
                    elem: Box::new(elem_ty),
                    len: *count,
                }
            }
            Try(expr) => self.infer_try_expr(expr),
            Closure {
                params,
                ret_ty,
                body,
                ..
            } => self.infer_closure_expr(params, ret_ty, body),
            Range { .. } => Ty::Var(self.uf.fresh_var()),
        }
    }

    fn lit_ty(&self, lit: &crate::hir::HirLit) -> Ty {
        use crate::hir::HirLit::*;
        match lit {
            Integer(_) => Ty::Int(crate::hir::IntSize::I32),
            Uint(_) => Ty::Uint(crate::hir::UintSize::U64),
            Float(_) => Ty::Float(crate::hir::FloatSize::F64),
            String(_) => Ty::String,
            Char(_) => Ty::Char,
            Bool(_) => Ty::Bool,
            Unit => Ty::Unit,
        }
    }

    fn run(&mut self) {
        self.check_module(self.hir);
    }

    fn check_module(&mut self, module: &HirModule) {
        for item in &module.consts {
            self.check_const(item);
        }
        for item in &module.statics {
            self.check_static(item);
        }
        for item in &module.type_aliases {
            self.check_capability_surface(&item.ty, item.span, "type aliases");
        }
        for item in &module.structs {
            for field in &item.fields {
                self.check_capability_surface(&field.ty, field.span, "struct fields");
            }
            for derive in &item.derives {
                if let Some(ability) = self.ability_kind_for_name(derive) {
                    self.check_derived_ability(item.def, item.span, ability);
                }
            }
        }
        for item in &module.enums {
            for derive in &item.derives {
                if let Some(ability) = self.ability_kind_for_name(derive) {
                    self.check_derived_ability(item.def, item.span, ability);
                }
            }
        }
        for f in &module.functions {
            self.check_fn(f);
        }
        for trait_def in &module.traits {
            self.check_assoc_items(&trait_def.items);
        }
        for interface_def in &module.interfaces {
            self.check_assoc_items(&interface_def.items);
        }
        for imp in &module.impls {
            if let Some(trait_ref) = &imp.trait_ref {
                if let Some(ability) = self.ability_kind_for_ty(trait_ref) {
                    self.check_ability_impl(imp, ability);
                }
            }
            self.check_impl_items(&imp.items);
        }
        for nested in &module.modules {
            if let Some(body) = &nested.body {
                self.check_module(body);
            }
        }
    }
}

/// Run the type checker on a HIR module, returning any type errors.
pub fn check(file: FileId, hir: &HirModule) -> Vec<Diagnostic> {
    let _ = file;
    let mut checker = TypeChecker::new(hir);
    checker.run();
    checker.errors
}

pub fn check_and_prepare(file: FileId, hir: &mut HirModule) -> Vec<Diagnostic> {
    let _ = file;
    type_checker_prepare::prepare_hir(hir);
    let (resolved_methods, errors) = {
        let mut checker = TypeChecker::new(hir);
        checker.run();
        (checker.resolved_methods.clone(), checker.errors)
    };
    type_checker_prepare::apply_resolved_methods(hir, &resolved_methods);
    errors
}
