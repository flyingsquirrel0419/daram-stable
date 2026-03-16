use crate::{
    hir::{
        DefId, HirArm, HirAssocItem, HirEnum, HirExpr, HirFn, HirId, HirIdAlloc, HirImpl,
        HirImplItem, HirLit, HirModule, HirPattern, HirPatternKind, HirStmt, HirStruct, Ty,
    },
    source::{FileId, Span},
};
use std::collections::{HashMap, HashSet};

pub(crate) fn prepare_hir(hir: &mut HirModule) {
    let mut hir_id_alloc = next_hir_id_alloc(hir);
    synthesize_derived_impls(hir, &mut hir_id_alloc);
    synthesize_ability_default_methods(hir, &mut hir_id_alloc);
}

pub(crate) fn apply_resolved_methods(
    hir: &mut HirModule,
    resolved_methods: &HashMap<HirId, DefId>,
) {
    for function in &mut hir.functions {
        if let Some(body) = &mut function.body {
            apply_expr_methods(body, resolved_methods);
        }
    }
    for imp in &mut hir.impls {
        for item in &mut imp.items {
            if let HirImplItem::Method(method) = item {
                if let Some(body) = &mut method.body {
                    apply_expr_methods(body, resolved_methods);
                }
            }
        }
    }
    for nested in &mut hir.modules {
        if let Some(body) = &mut nested.body {
            apply_resolved_methods(body, resolved_methods);
        }
    }
}

fn synthesize_ability_default_methods(hir: &mut HirModule, hir_id_alloc: &mut HirIdAlloc) {
    let ability_methods = collect_ability_default_methods(hir);
    let mut next_def_indices = collect_next_def_indices(hir);
    synthesize_ability_defaults_in_module(
        hir,
        &ability_methods,
        &mut next_def_indices,
        hir_id_alloc,
    );
}

#[derive(Clone)]
struct DeriveSynthesisContext {
    def_names: HashMap<DefId, String>,
    structs: HashMap<DefId, HirStruct>,
    enums: HashMap<DefId, HirEnum>,
    explicit_default_methods: HashMap<DefId, DefId>,
    clone_proto: Option<HirFn>,
    partial_eq_proto: Option<HirFn>,
    debug_proto: Option<HirFn>,
    default_proto: Option<HirFn>,
    format_def: Option<DefId>,
    string_new_def: Option<DefId>,
}

#[derive(Clone, Copy)]
enum DerivedBodyKind {
    Clone,
    PartialEq,
    Debug,
    Default,
}

fn synthesize_derived_impls(hir: &mut HirModule, hir_id_alloc: &mut HirIdAlloc) {
    let ctx = collect_derive_synthesis_context(hir);
    let mut next_def_indices = collect_next_def_indices(hir);
    let mut default_methods = ctx.explicit_default_methods.clone();
    synthesize_derived_impls_in_module(
        hir,
        &ctx,
        &mut default_methods,
        &mut next_def_indices,
        hir_id_alloc,
    );
}

fn collect_derive_synthesis_context(module: &HirModule) -> DeriveSynthesisContext {
    let mut ctx = DeriveSynthesisContext {
        def_names: HashMap::new(),
        structs: HashMap::new(),
        enums: HashMap::new(),
        explicit_default_methods: HashMap::new(),
        clone_proto: None,
        partial_eq_proto: None,
        debug_proto: None,
        default_proto: None,
        format_def: None,
        string_new_def: None,
    };
    collect_derive_synthesis_context_in_module(module, &mut ctx);
    ctx
}

fn collect_derive_synthesis_context_in_module(
    module: &HirModule,
    ctx: &mut DeriveSynthesisContext,
) {
    ctx.def_names.extend(module.def_names.clone());
    for strukt in &module.structs {
        ctx.structs.insert(strukt.def, strukt.clone());
    }
    for item in &module.enums {
        ctx.enums.insert(item.def, item.clone());
    }
    for ability in &module.abilities {
        let ability_name = module
            .def_names
            .get(&ability.def)
            .and_then(|name| name.rsplit("::").next())
            .unwrap_or_default();
        for item in &ability.items {
            let HirAssocItem::Method(method) = item else {
                continue;
            };
            let Some(method_name) = module
                .def_names
                .get(&method.def)
                .and_then(|name| name.rsplit("::").next())
            else {
                continue;
            };
            match (ability_name, method_name) {
                ("Clone", "clone") => ctx.clone_proto = Some(method.clone()),
                ("PartialEq", "eq") => ctx.partial_eq_proto = Some(method.clone()),
                ("Debug", "fmt") => ctx.debug_proto = Some(method.clone()),
                ("Default", "default") => ctx.default_proto = Some(method.clone()),
                _ => {}
            }
        }
    }
    for imp in &module.impls {
        let Some(Ty::Named {
            def: ability_def, ..
        }) = &imp.trait_ref
        else {
            continue;
        };
        let Some(ability_name) = module
            .def_names
            .get(ability_def)
            .and_then(|name| name.rsplit("::").next())
        else {
            continue;
        };
        if ability_name != "Default" {
            continue;
        }
        let Some(owner_def) = named_def(&imp.self_ty) else {
            continue;
        };
        for item in &imp.items {
            let HirImplItem::Method(method) = item else {
                continue;
            };
            if module
                .def_names
                .get(&method.def)
                .and_then(|name| name.rsplit("::").next())
                .is_some_and(|name| name == "default")
            {
                ctx.explicit_default_methods.insert(owner_def, method.def);
            }
        }
    }
    for (def, name) in &module.def_names {
        match name.as_str() {
            "std::fmt::format" | "format" if ctx.format_def.is_none() => {
                ctx.format_def = Some(*def)
            }
            "std::core::String::new" | "String::new" if ctx.string_new_def.is_none() => {
                ctx.string_new_def = Some(*def)
            }
            _ => {}
        }
    }
    for nested in &module.modules {
        if let Some(body) = &nested.body {
            collect_derive_synthesis_context_in_module(body, ctx);
        }
    }
}

fn synthesize_derived_impls_in_module(
    module: &mut HirModule,
    ctx: &DeriveSynthesisContext,
    default_methods: &mut HashMap<DefId, DefId>,
    next_def_indices: &mut HashMap<FileId, u32>,
    hir_id_alloc: &mut HirIdAlloc,
) {
    let mut pending = Vec::new();
    let mut synthesized = Vec::new();

    for strukt in &module.structs {
        let self_ty = derived_self_ty_for_struct(strukt);
        for derive in &strukt.derives {
            let Some((prototype, method_name, body_kind, ability_def)) =
                derive_method_template(ctx, derive)
            else {
                continue;
            };
            let mut method = prototype.clone();
            freshen_hir_fn(&mut method, hir_id_alloc);
            for param in &mut method.params {
                substitute_self_ty(&mut param.ty, &self_ty);
            }
            substitute_self_ty(&mut method.ret_ty, &self_ty);
            if let Some(first_param) = method.params.first_mut() {
                first_param.ty = self_ty.clone();
            }
            method.body = None;
            let qualified_name = format!(
                "{}::{method_name}",
                owner_name_for_def(&ctx.def_names, strukt.def)
            );
            method.def =
                derived_method_def(ctx, next_def_indices, strukt.span.file, &qualified_name);
            module.def_names.insert(method.def, qualified_name);
            if matches!(body_kind, DerivedBodyKind::Default) {
                default_methods.insert(strukt.def, method.def);
            }
            pending.push((synthesized.len(), body_kind, strukt.def));
            synthesized.push(HirImpl {
                id: hir_id_alloc.fresh(),
                type_params: strukt.type_params.clone(),
                trait_ref: Some(Ty::Named {
                    def: ability_def,
                    args: Vec::new(),
                }),
                self_ty: self_ty.clone(),
                items: vec![HirImplItem::Method(method)],
                span: strukt.span,
            });
        }
    }

    for item in &module.enums {
        let self_ty = derived_self_ty_for_enum(item);
        for derive in &item.derives {
            let Some((prototype, method_name, body_kind, ability_def)) =
                derive_method_template(ctx, derive)
            else {
                continue;
            };
            let mut method = prototype.clone();
            freshen_hir_fn(&mut method, hir_id_alloc);
            for param in &mut method.params {
                substitute_self_ty(&mut param.ty, &self_ty);
            }
            substitute_self_ty(&mut method.ret_ty, &self_ty);
            if let Some(first_param) = method.params.first_mut() {
                first_param.ty = self_ty.clone();
            }
            method.body = None;
            let qualified_name = format!(
                "{}::{method_name}",
                owner_name_for_def(&ctx.def_names, item.def)
            );
            method.def = derived_method_def(ctx, next_def_indices, item.span.file, &qualified_name);
            module.def_names.insert(method.def, qualified_name);
            if matches!(body_kind, DerivedBodyKind::Default) {
                default_methods.insert(item.def, method.def);
            }
            pending.push((synthesized.len(), body_kind, item.def));
            synthesized.push(HirImpl {
                id: hir_id_alloc.fresh(),
                type_params: item.type_params.clone(),
                trait_ref: Some(Ty::Named {
                    def: ability_def,
                    args: Vec::new(),
                }),
                self_ty: self_ty.clone(),
                items: vec![HirImplItem::Method(method)],
                span: item.span,
            });
        }
    }

    for (index, body_kind, owner_def) in pending {
        let HirImplItem::Method(method) = &mut synthesized[index].items[0] else {
            continue;
        };
        let body = if let Some(strukt) = ctx.structs.get(&owner_def) {
            build_derived_struct_method_body(
                ctx,
                default_methods,
                hir_id_alloc,
                method,
                strukt,
                body_kind,
            )
        } else if let Some(item) = ctx.enums.get(&owner_def) {
            build_derived_enum_method_body(
                ctx,
                default_methods,
                hir_id_alloc,
                method,
                item,
                body_kind,
            )
        } else {
            None
        };
        method.body = body;
    }

    module.impls.extend(synthesized);

    for nested in &mut module.modules {
        if let Some(body) = &mut nested.body {
            synthesize_derived_impls_in_module(
                body,
                ctx,
                default_methods,
                next_def_indices,
                hir_id_alloc,
            );
        }
    }
}

fn derive_method_template<'a>(
    ctx: &'a DeriveSynthesisContext,
    derive: &str,
) -> Option<(&'a HirFn, &'static str, DerivedBodyKind, DefId)> {
    match derive {
        "Clone" => Some((
            ctx.clone_proto.as_ref()?,
            "clone",
            DerivedBodyKind::Clone,
            find_ability_def(&ctx.def_names, "Clone")?,
        )),
        "PartialEq" => Some((
            ctx.partial_eq_proto.as_ref()?,
            "eq",
            DerivedBodyKind::PartialEq,
            find_ability_def(&ctx.def_names, "PartialEq")?,
        )),
        "Debug" => Some((
            ctx.debug_proto.as_ref()?,
            "fmt",
            DerivedBodyKind::Debug,
            find_ability_def(&ctx.def_names, "Debug")?,
        )),
        "Default" => Some((
            ctx.default_proto.as_ref()?,
            "default",
            DerivedBodyKind::Default,
            find_ability_def(&ctx.def_names, "Default")?,
        )),
        _ => None,
    }
}

fn derived_method_def(
    ctx: &DeriveSynthesisContext,
    next_def_indices: &mut HashMap<FileId, u32>,
    file: FileId,
    qualified_name: &str,
) -> DefId {
    if let Some(def) = ctx
        .def_names
        .iter()
        .find_map(|(def, name)| (name == qualified_name).then_some(*def))
    {
        return def;
    }
    let next_index = next_def_indices.entry(file).or_insert(0);
    let def = DefId {
        file,
        index: *next_index,
    };
    *next_index = next_index.saturating_add(1);
    def
}

fn substitute_self_ty(ty: &mut Ty, self_ty: &Ty) {
    match ty {
        Ty::Var(_) => *ty = self_ty.clone(),
        Ty::Ref { inner, .. } | Ty::RawPtr { inner, .. } | Ty::Slice(inner) => {
            substitute_self_ty(inner, self_ty);
        }
        Ty::Array { elem, .. } => substitute_self_ty(elem, self_ty),
        Ty::Tuple(items) | Ty::Named { args: items, .. } => {
            for item in items {
                substitute_self_ty(item, self_ty);
            }
        }
        Ty::FnPtr { params, ret } => {
            for param in params {
                substitute_self_ty(param, self_ty);
            }
            substitute_self_ty(ret, self_ty);
        }
        Ty::Bool
        | Ty::Char
        | Ty::Int(_)
        | Ty::Uint(_)
        | Ty::Float(_)
        | Ty::Unit
        | Ty::Never
        | Ty::ImplTrait(_)
        | Ty::DynTrait(_)
        | Ty::Str
        | Ty::String => {}
    }
}

fn build_derived_struct_method_body(
    ctx: &DeriveSynthesisContext,
    default_methods: &HashMap<DefId, DefId>,
    hir_id_alloc: &mut HirIdAlloc,
    method: &HirFn,
    strukt: &HirStruct,
    body_kind: DerivedBodyKind,
) -> Option<HirExpr> {
    match body_kind {
        DerivedBodyKind::Clone => Some(expr_var(
            hir_id_alloc,
            method.params.first()?.binding,
            method.params.first()?.ty.clone(),
            method.span,
        )),
        DerivedBodyKind::PartialEq => build_partialeq_method_body(hir_id_alloc, method),
        DerivedBodyKind::Debug => build_debug_method_body(ctx, hir_id_alloc, method),
        DerivedBodyKind::Default => {
            let mut fields = Vec::with_capacity(strukt.fields.len());
            for field in &strukt.fields {
                fields.push((
                    field.name.clone(),
                    default_expr_for_ty(ctx, default_methods, hir_id_alloc, &field.ty, field.span)?,
                ));
            }
            Some(expr_struct(
                hir_id_alloc,
                strukt.def,
                fields,
                method.ret_ty.clone(),
                strukt.span,
            ))
        }
    }
}

fn build_derived_enum_method_body(
    ctx: &DeriveSynthesisContext,
    default_methods: &HashMap<DefId, DefId>,
    hir_id_alloc: &mut HirIdAlloc,
    method: &HirFn,
    item: &HirEnum,
    body_kind: DerivedBodyKind,
) -> Option<HirExpr> {
    match body_kind {
        DerivedBodyKind::Clone => Some(expr_var(
            hir_id_alloc,
            method.params.first()?.binding,
            method.params.first()?.ty.clone(),
            method.span,
        )),
        DerivedBodyKind::PartialEq => build_partialeq_method_body(hir_id_alloc, method),
        DerivedBodyKind::Debug => build_debug_method_body(ctx, hir_id_alloc, method),
        DerivedBodyKind::Default => {
            let variant = item.variants.first()?;
            let callee = expr_def_ref(
                hir_id_alloc,
                variant.def,
                method.ret_ty.clone(),
                variant.span,
            );
            let mut args = Vec::with_capacity(variant.fields.len());
            for ty in &variant.fields {
                args.push(default_expr_for_ty(
                    ctx,
                    default_methods,
                    hir_id_alloc,
                    ty,
                    variant.span,
                )?);
            }
            Some(expr_call(
                hir_id_alloc,
                callee,
                args,
                method.ret_ty.clone(),
                item.span,
            ))
        }
    }
}

fn build_debug_method_body(
    ctx: &DeriveSynthesisContext,
    hir_id_alloc: &mut HirIdAlloc,
    method: &HirFn,
) -> Option<HirExpr> {
    let self_param = method.params.first()?;
    let formatter_param = method.params.get(1)?;
    let rendered_binding = hir_id_alloc.fresh();
    let format_fn = expr_def_ref(hir_id_alloc, ctx.format_def?, Ty::String, method.span);
    let format_template = expr_str_lit(hir_id_alloc, "{}", method.span);
    let format_arg = expr_var(
        hir_id_alloc,
        self_param.binding,
        self_param.ty.clone(),
        method.span,
    );
    let rendered_expr = expr_call(
        hir_id_alloc,
        format_fn,
        vec![format_template, format_arg],
        Ty::String,
        method.span,
    );
    let rendered_var = expr_var(hir_id_alloc, rendered_binding, Ty::String, method.span);
    let formatter_var = expr_var(
        hir_id_alloc,
        formatter_param.binding,
        formatter_param.ty.clone(),
        method.span,
    );
    let formatter_expr = match &formatter_param.ty {
        Ty::Ref { inner, .. } => expr_deref(
            hir_id_alloc,
            formatter_var,
            inner.as_ref().clone(),
            method.span,
        ),
        _ => formatter_var,
    };
    let rendered_as_str = expr_method_call(
        hir_id_alloc,
        rendered_var,
        "as_str",
        Vec::new(),
        Ty::Ref {
            mutable: false,
            inner: Box::new(Ty::Str),
        },
        method.span,
    );
    let write_call = expr_method_call(
        hir_id_alloc,
        formatter_expr,
        "write_str",
        vec![rendered_as_str],
        method.ret_ty.clone(),
        method.span,
    );
    Some(HirExpr {
        id: hir_id_alloc.fresh(),
        span: method.span,
        ty: method.ret_ty.clone(),
        kind: crate::hir::HirExprKind::Block(
            vec![HirStmt {
                id: hir_id_alloc.fresh(),
                span: method.span,
                kind: crate::hir::HirStmtKind::Let {
                    binding: rendered_binding,
                    mutable: false,
                    ty: Ty::String,
                    init: Some(rendered_expr),
                },
            }],
            Some(Box::new(write_call)),
        ),
    })
}

fn build_partialeq_method_body(hir_id_alloc: &mut HirIdAlloc, method: &HirFn) -> Option<HirExpr> {
    let self_param = method.params.first()?;
    let other_param = method.params.get(1)?;
    let lhs = expr_var(
        hir_id_alloc,
        self_param.binding,
        self_param.ty.clone(),
        method.span,
    );
    let other = expr_var(
        hir_id_alloc,
        other_param.binding,
        other_param.ty.clone(),
        method.span,
    );
    let rhs = expr_deref(hir_id_alloc, other, self_param.ty.clone(), method.span);
    Some(expr_binop(
        hir_id_alloc,
        crate::hir::HirBinOp::Eq,
        lhs,
        rhs,
        Ty::Bool,
        method.span,
    ))
}

fn default_expr_for_ty(
    ctx: &DeriveSynthesisContext,
    default_methods: &HashMap<DefId, DefId>,
    hir_id_alloc: &mut HirIdAlloc,
    ty: &Ty,
    span: Span,
) -> Option<HirExpr> {
    match ty {
        Ty::Bool => Some(expr_bool_lit(hir_id_alloc, false, span)),
        Ty::Char => Some(expr_char_lit(hir_id_alloc, '\0', span)),
        Ty::Int(_) => Some(expr_int_lit(hir_id_alloc, 0, ty.clone(), span)),
        Ty::Uint(_) => Some(expr_uint_lit(hir_id_alloc, 0, ty.clone(), span)),
        Ty::Float(_) => Some(expr_float_lit(hir_id_alloc, 0.0, ty.clone(), span)),
        Ty::Unit | Ty::Never => Some(expr_unit_lit(hir_id_alloc, span)),
        Ty::String => {
            let callee = expr_def_ref(hir_id_alloc, ctx.string_new_def?, Ty::String, span);
            Some(expr_call(
                hir_id_alloc,
                callee,
                Vec::new(),
                Ty::String,
                span,
            ))
        }
        Ty::Tuple(items) => Some(HirExpr {
            id: hir_id_alloc.fresh(),
            span,
            ty: ty.clone(),
            kind: crate::hir::HirExprKind::Tuple(
                items
                    .iter()
                    .map(|item| default_expr_for_ty(ctx, default_methods, hir_id_alloc, item, span))
                    .collect::<Option<Vec<_>>>()?,
            ),
        }),
        Ty::Array { elem, len } => Some(HirExpr {
            id: hir_id_alloc.fresh(),
            span,
            ty: ty.clone(),
            kind: crate::hir::HirExprKind::Array(
                (0..*len)
                    .map(|_| default_expr_for_ty(ctx, default_methods, hir_id_alloc, elem, span))
                    .collect::<Option<Vec<_>>>()?,
            ),
        }),
        Ty::Named { def, .. } => {
            let callee = expr_def_ref(hir_id_alloc, *default_methods.get(def)?, ty.clone(), span);
            Some(expr_call(
                hir_id_alloc,
                callee,
                Vec::new(),
                ty.clone(),
                span,
            ))
        }
        _ => None,
    }
}

fn derived_self_ty_for_struct(strukt: &HirStruct) -> Ty {
    Ty::Named {
        def: strukt.def,
        args: collect_ty_vars_from_tys(strukt.fields.iter().map(|field| field.ty.clone())),
    }
}

fn derived_self_ty_for_enum(item: &HirEnum) -> Ty {
    Ty::Named {
        def: item.def,
        args: collect_ty_vars_from_tys(
            item.variants
                .iter()
                .flat_map(|variant| variant.fields.clone()),
        ),
    }
}

fn collect_ty_vars_from_tys(tys: impl IntoIterator<Item = Ty>) -> Vec<Ty> {
    fn visit(ty: &Ty, seen: &mut Vec<u32>) {
        match ty {
            Ty::Var(id) => {
                if !seen.contains(id) {
                    seen.push(*id);
                }
            }
            Ty::Ref { inner, .. } | Ty::RawPtr { inner, .. } | Ty::Slice(inner) => {
                visit(inner, seen)
            }
            Ty::Array { elem, .. } => visit(elem, seen),
            Ty::Tuple(items) | Ty::Named { args: items, .. } => {
                for item in items {
                    visit(item, seen);
                }
            }
            Ty::FnPtr { params, ret } => {
                for param in params {
                    visit(param, seen);
                }
                visit(ret, seen);
            }
            _ => {}
        }
    }

    let mut seen = Vec::new();
    for ty in tys {
        visit(&ty, &mut seen);
    }
    seen.into_iter().map(Ty::Var).collect()
}

fn owner_name_for_def(def_names: &HashMap<DefId, String>, def: DefId) -> String {
    def_names
        .get(&def)
        .cloned()
        .unwrap_or_else(|| format!("def_{}", def.index))
}

fn find_ability_def(def_names: &HashMap<DefId, String>, name: &str) -> Option<DefId> {
    def_names.iter().find_map(|(def, value)| {
        value
            .rsplit("::")
            .next()
            .is_some_and(|candidate| candidate == name)
            .then_some(*def)
    })
}

fn named_def(ty: &Ty) -> Option<DefId> {
    match ty {
        Ty::Named { def, .. } => Some(*def),
        Ty::Ref { inner, .. } => named_def(inner),
        _ => None,
    }
}

fn expr_var(alloc: &mut HirIdAlloc, binding: HirId, ty: Ty, span: Span) -> HirExpr {
    HirExpr {
        id: alloc.fresh(),
        span,
        ty,
        kind: crate::hir::HirExprKind::Var(binding),
    }
}

fn expr_def_ref(alloc: &mut HirIdAlloc, def: DefId, ty: Ty, span: Span) -> HirExpr {
    HirExpr {
        id: alloc.fresh(),
        span,
        ty,
        kind: crate::hir::HirExprKind::DefRef(def),
    }
}

fn expr_call(
    alloc: &mut HirIdAlloc,
    callee: HirExpr,
    args: Vec<HirExpr>,
    ty: Ty,
    span: Span,
) -> HirExpr {
    HirExpr {
        id: alloc.fresh(),
        span,
        ty,
        kind: crate::hir::HirExprKind::Call {
            callee: Box::new(callee),
            args,
        },
    }
}

fn expr_binop(
    alloc: &mut HirIdAlloc,
    op: crate::hir::HirBinOp,
    lhs: HirExpr,
    rhs: HirExpr,
    ty: Ty,
    span: Span,
) -> HirExpr {
    HirExpr {
        id: alloc.fresh(),
        span,
        ty,
        kind: crate::hir::HirExprKind::BinOp {
            op,
            lhs: Box::new(lhs),
            rhs: Box::new(rhs),
        },
    }
}

fn expr_method_call(
    alloc: &mut HirIdAlloc,
    receiver: HirExpr,
    method_name: &str,
    args: Vec<HirExpr>,
    ty: Ty,
    span: Span,
) -> HirExpr {
    HirExpr {
        id: alloc.fresh(),
        span,
        ty,
        kind: crate::hir::HirExprKind::MethodCall {
            receiver: Box::new(receiver),
            method_name: method_name.to_string(),
            method_id: DefId {
                file: span.file,
                index: u32::MAX,
            },
            args,
        },
    }
}

fn expr_deref(alloc: &mut HirIdAlloc, expr: HirExpr, ty: Ty, span: Span) -> HirExpr {
    HirExpr {
        id: alloc.fresh(),
        span,
        ty,
        kind: crate::hir::HirExprKind::Deref(Box::new(expr)),
    }
}

fn expr_struct(
    alloc: &mut HirIdAlloc,
    def: DefId,
    fields: Vec<(String, HirExpr)>,
    ty: Ty,
    span: Span,
) -> HirExpr {
    HirExpr {
        id: alloc.fresh(),
        span,
        ty,
        kind: crate::hir::HirExprKind::Struct {
            def,
            fields,
            rest: None,
        },
    }
}

fn expr_bool_lit(alloc: &mut HirIdAlloc, value: bool, span: Span) -> HirExpr {
    HirExpr {
        id: alloc.fresh(),
        span,
        ty: Ty::Bool,
        kind: crate::hir::HirExprKind::Lit(HirLit::Bool(value)),
    }
}

fn expr_char_lit(alloc: &mut HirIdAlloc, value: char, span: Span) -> HirExpr {
    HirExpr {
        id: alloc.fresh(),
        span,
        ty: Ty::Char,
        kind: crate::hir::HirExprKind::Lit(HirLit::Char(value)),
    }
}

fn expr_int_lit(alloc: &mut HirIdAlloc, value: i128, ty: Ty, span: Span) -> HirExpr {
    HirExpr {
        id: alloc.fresh(),
        span,
        ty,
        kind: crate::hir::HirExprKind::Lit(HirLit::Integer(value)),
    }
}

fn expr_uint_lit(alloc: &mut HirIdAlloc, value: u128, ty: Ty, span: Span) -> HirExpr {
    HirExpr {
        id: alloc.fresh(),
        span,
        ty,
        kind: crate::hir::HirExprKind::Lit(HirLit::Uint(value)),
    }
}

fn expr_float_lit(alloc: &mut HirIdAlloc, value: f64, ty: Ty, span: Span) -> HirExpr {
    HirExpr {
        id: alloc.fresh(),
        span,
        ty,
        kind: crate::hir::HirExprKind::Lit(HirLit::Float(value)),
    }
}

fn expr_str_lit(alloc: &mut HirIdAlloc, value: &str, span: Span) -> HirExpr {
    HirExpr {
        id: alloc.fresh(),
        span,
        ty: Ty::Str,
        kind: crate::hir::HirExprKind::Lit(HirLit::String(value.to_string())),
    }
}

fn expr_unit_lit(alloc: &mut HirIdAlloc, span: Span) -> HirExpr {
    HirExpr {
        id: alloc.fresh(),
        span,
        ty: Ty::Unit,
        kind: crate::hir::HirExprKind::Lit(HirLit::Unit),
    }
}

fn collect_ability_default_methods(module: &HirModule) -> HashMap<DefId, Vec<(String, HirFn)>> {
    let mut methods = HashMap::new();
    collect_ability_default_methods_in_module(module, &mut methods);
    methods
}

fn collect_ability_default_methods_in_module(
    module: &HirModule,
    methods: &mut HashMap<DefId, Vec<(String, HirFn)>>,
) {
    for ability in &module.abilities {
        let defaults = ability
            .items
            .iter()
            .filter_map(|item| match item {
                HirAssocItem::Method(method) if method.body.is_some() => Some((
                    module
                        .def_names
                        .get(&method.def)
                        .and_then(|name| name.rsplit("::").next())
                        .unwrap_or_default()
                        .to_string(),
                    method.clone(),
                )),
                _ => None,
            })
            .collect::<Vec<_>>();
        if !defaults.is_empty() {
            methods.insert(ability.def, defaults);
        }
    }
    for nested in &module.modules {
        if let Some(body) = &nested.body {
            collect_ability_default_methods_in_module(body, methods);
        }
    }
}

fn collect_next_def_indices(module: &HirModule) -> HashMap<FileId, u32> {
    let mut next = HashMap::new();
    collect_next_def_indices_in_module(module, &mut next);
    next
}

fn collect_next_def_indices_in_module(module: &HirModule, next: &mut HashMap<FileId, u32>) {
    for def in module.def_names.keys() {
        let entry = next.entry(def.file).or_insert(0);
        *entry = (*entry).max(def.index.saturating_add(1));
    }
    for nested in &module.modules {
        if let Some(body) = &nested.body {
            collect_next_def_indices_in_module(body, next);
        }
    }
}

fn synthesize_ability_defaults_in_module(
    module: &mut HirModule,
    ability_methods: &HashMap<DefId, Vec<(String, HirFn)>>,
    next_def_indices: &mut HashMap<FileId, u32>,
    hir_id_alloc: &mut HirIdAlloc,
) {
    let def_names = module.def_names.clone();
    for imp in &mut module.impls {
        let Some(Ty::Named {
            def: ability_def, ..
        }) = imp.trait_ref.clone()
        else {
            continue;
        };
        let Some(defaults) = ability_methods.get(&ability_def) else {
            continue;
        };
        let Some(owner_name) = default_method_owner_name(&imp.self_ty, &def_names) else {
            continue;
        };
        let existing = imp
            .items
            .iter()
            .filter_map(|item| match item {
                HirImplItem::Method(method) => def_names
                    .get(&method.def)
                    .and_then(|name| name.rsplit("::").next())
                    .map(str::to_string),
                _ => None,
            })
            .collect::<HashSet<_>>();
        let mut synthesized = Vec::new();
        for (method_name, method) in defaults {
            if existing.contains(method_name) {
                continue;
            }
            let mut cloned = method.clone();
            freshen_hir_fn(&mut cloned, hir_id_alloc);
            if let Some(first_param) = cloned.params.first_mut() {
                first_param.ty = imp.self_ty.clone();
            }
            let next_index = next_def_indices.entry(imp.span.file).or_insert(0);
            cloned.def = DefId {
                file: imp.span.file,
                index: *next_index,
            };
            *next_index = next_index.saturating_add(1);
            module
                .def_names
                .insert(cloned.def, format!("{owner_name}::{method_name}"));
            synthesized.push(HirImplItem::Method(cloned));
        }
        imp.items.extend(synthesized);
    }
    for nested in &mut module.modules {
        if let Some(body) = &mut nested.body {
            synthesize_ability_defaults_in_module(
                body,
                ability_methods,
                next_def_indices,
                hir_id_alloc,
            );
        }
    }
}

fn default_method_owner_name(ty: &Ty, def_names: &HashMap<DefId, String>) -> Option<String> {
    match ty {
        Ty::Named { def, .. } => def_names.get(def).cloned(),
        Ty::Ref { inner, .. } => default_method_owner_name(inner, def_names),
        Ty::String => Some("std::core::String".to_string()),
        _ => None,
    }
}

fn next_hir_id_alloc(module: &HirModule) -> HirIdAlloc {
    let mut max_id = 0;
    collect_max_hir_id_in_module(module, &mut max_id);
    let mut alloc = HirIdAlloc::default();
    for _ in 0..=max_id {
        alloc.fresh();
    }
    alloc
}

fn collect_max_hir_id_in_module(module: &HirModule, max_id: &mut u32) {
    for function in &module.functions {
        collect_max_hir_id_in_fn(function, max_id);
    }
    for item in &module.consts {
        *max_id = (*max_id).max(item.id.0);
        collect_max_hir_id_in_expr(&item.value, max_id);
    }
    for item in &module.statics {
        *max_id = (*max_id).max(item.id.0);
        collect_max_hir_id_in_expr(&item.value, max_id);
    }
    for item in &module.type_aliases {
        *max_id = (*max_id).max(item.id.0);
    }
    for item in &module.uses {
        *max_id = (*max_id).max(item.id.0);
    }
    for item in &module.traits {
        *max_id = (*max_id).max(item.id.0);
        collect_max_hir_id_in_assoc_items(&item.items, max_id);
    }
    for item in &module.interfaces {
        *max_id = (*max_id).max(item.id.0);
        collect_max_hir_id_in_assoc_items(&item.items, max_id);
    }
    for item in &module.abilities {
        *max_id = (*max_id).max(item.id.0);
        collect_max_hir_id_in_assoc_items(&item.items, max_id);
    }
    for item in &module.impls {
        *max_id = (*max_id).max(item.id.0);
        for impl_item in &item.items {
            match impl_item {
                HirImplItem::Method(method) => collect_max_hir_id_in_fn(method, max_id),
                HirImplItem::Const { value, .. } => collect_max_hir_id_in_expr(value, max_id),
                HirImplItem::TypeAssoc { .. } => {}
            }
        }
    }
    for item in &module.modules {
        *max_id = (*max_id).max(item.id.0);
        if let Some(body) = &item.body {
            collect_max_hir_id_in_module(body, max_id);
        }
    }
    for item in &module.structs {
        *max_id = (*max_id).max(item.id.0);
    }
    for item in &module.enums {
        *max_id = (*max_id).max(item.id.0);
    }
}

fn collect_max_hir_id_in_assoc_items(items: &[HirAssocItem], max_id: &mut u32) {
    for item in items {
        match item {
            HirAssocItem::Method(method) => collect_max_hir_id_in_fn(method, max_id),
            HirAssocItem::Const {
                default: Some(value),
                ..
            } => collect_max_hir_id_in_expr(value, max_id),
            HirAssocItem::Const { default: None, .. } | HirAssocItem::TypeAssoc { .. } => {}
        }
    }
}

fn collect_max_hir_id_in_fn(function: &HirFn, max_id: &mut u32) {
    *max_id = (*max_id).max(function.id.0);
    for param in &function.params {
        *max_id = (*max_id).max(param.id.0.max(param.binding.0));
    }
    if let Some(body) = &function.body {
        collect_max_hir_id_in_expr(body, max_id);
    }
}

fn collect_max_hir_id_in_expr(expr: &HirExpr, max_id: &mut u32) {
    use crate::hir::HirExprKind::*;

    *max_id = (*max_id).max(expr.id.0);
    match &expr.kind {
        Block(stmts, tail) => {
            for stmt in stmts {
                collect_max_hir_id_in_stmt(stmt, max_id);
            }
            if let Some(tail) = tail {
                collect_max_hir_id_in_expr(tail, max_id);
            }
        }
        Call { callee, args } => {
            collect_max_hir_id_in_expr(callee, max_id);
            for arg in args {
                collect_max_hir_id_in_expr(arg, max_id);
            }
        }
        MethodCall { receiver, args, .. } => {
            collect_max_hir_id_in_expr(receiver, max_id);
            for arg in args {
                collect_max_hir_id_in_expr(arg, max_id);
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
        | Defer(base) => collect_max_hir_id_in_expr(base, max_id),
        Index { base, index } => {
            collect_max_hir_id_in_expr(base, max_id);
            collect_max_hir_id_in_expr(index, max_id);
        }
        Tuple(elems) | Array(elems) => {
            for elem in elems {
                collect_max_hir_id_in_expr(elem, max_id);
            }
        }
        Repeat { elem, .. } => collect_max_hir_id_in_expr(elem, max_id),
        Struct { fields, rest, .. } => {
            for (_, value) in fields {
                collect_max_hir_id_in_expr(value, max_id);
            }
            if let Some(rest) = rest {
                collect_max_hir_id_in_expr(rest, max_id);
            }
        }
        If {
            condition,
            then_branch,
            else_branch,
        } => {
            collect_max_hir_id_in_expr(condition, max_id);
            collect_max_hir_id_in_expr(then_branch, max_id);
            if let Some(else_branch) = else_branch {
                collect_max_hir_id_in_expr(else_branch, max_id);
            }
        }
        Match { scrutinee, arms } => {
            collect_max_hir_id_in_expr(scrutinee, max_id);
            for arm in arms {
                collect_max_hir_id_in_arm(arm, max_id);
            }
        }
        BinOp { lhs, rhs, .. } => {
            collect_max_hir_id_in_expr(lhs, max_id);
            collect_max_hir_id_in_expr(rhs, max_id);
        }
        UnaryOp { operand, .. } | Cast { expr: operand, .. } | Ref { expr: operand, .. } => {
            collect_max_hir_id_in_expr(operand, max_id);
        }
        Assign { target, value } => {
            collect_max_hir_id_in_expr(target, max_id);
            collect_max_hir_id_in_expr(value, max_id);
        }
        Return(Some(value)) | Break(Some(value)) => collect_max_hir_id_in_expr(value, max_id),
        Return(None) | Break(None) => {}
        ForDesugared {
            iter,
            binding,
            body,
        } => {
            *max_id = (*max_id).max(binding.0);
            collect_max_hir_id_in_expr(iter, max_id);
            collect_max_hir_id_in_expr(body, max_id);
        }
        While { condition, body } => {
            collect_max_hir_id_in_expr(condition, max_id);
            collect_max_hir_id_in_expr(body, max_id);
        }
        Closure {
            params,
            body,
            captures,
            ..
        } => {
            for param in params {
                *max_id = (*max_id).max(param.id.0.max(param.binding.0));
            }
            for capture in captures {
                *max_id = (*max_id).max(capture.0);
            }
            collect_max_hir_id_in_expr(body, max_id);
        }
        Range { lo, hi, .. } => {
            if let Some(lo) = lo {
                collect_max_hir_id_in_expr(lo, max_id);
            }
            if let Some(hi) = hi {
                collect_max_hir_id_in_expr(hi, max_id);
            }
        }
        Lit(_) | Var(_) | DefRef(_) | Continue => {}
    }
}

fn collect_max_hir_id_in_stmt(stmt: &HirStmt, max_id: &mut u32) {
    *max_id = (*max_id).max(stmt.id.0);
    match &stmt.kind {
        crate::hir::HirStmtKind::Let { binding, init, .. } => {
            *max_id = (*max_id).max(binding.0);
            if let Some(init) = init {
                collect_max_hir_id_in_expr(init, max_id);
            }
        }
        crate::hir::HirStmtKind::Expr(expr)
        | crate::hir::HirStmtKind::Errdefer(expr)
        | crate::hir::HirStmtKind::Defer(expr) => collect_max_hir_id_in_expr(expr, max_id),
        crate::hir::HirStmtKind::Use(_) => {}
    }
}

fn collect_max_hir_id_in_arm(arm: &HirArm, max_id: &mut u32) {
    *max_id = (*max_id).max(arm.id.0);
    collect_max_hir_id_in_pattern(&arm.pattern, max_id);
    if let Some(guard) = &arm.guard {
        collect_max_hir_id_in_expr(guard, max_id);
    }
    collect_max_hir_id_in_expr(&arm.body, max_id);
}

fn collect_max_hir_id_in_pattern(pattern: &HirPattern, max_id: &mut u32) {
    *max_id = (*max_id).max(pattern.id.0);
    match &pattern.kind {
        HirPatternKind::Binding { id, .. } => {
            *max_id = (*max_id).max(id.0);
        }
        HirPatternKind::Tuple(elems) | HirPatternKind::Or(elems) => {
            for elem in elems {
                collect_max_hir_id_in_pattern(elem, max_id);
            }
        }
        HirPatternKind::Struct { fields, .. } => {
            for (_, field) in fields {
                collect_max_hir_id_in_pattern(field, max_id);
            }
        }
        HirPatternKind::Variant { args, .. } | HirPatternKind::Slice { elems: args, .. } => {
            for arg in args {
                collect_max_hir_id_in_pattern(arg, max_id);
            }
        }
        HirPatternKind::Range { lo, hi, .. } => {
            collect_max_hir_id_in_pattern(lo, max_id);
            collect_max_hir_id_in_pattern(hi, max_id);
        }
        HirPatternKind::Ref { inner, .. } => collect_max_hir_id_in_pattern(inner, max_id),
        HirPatternKind::Wildcard | HirPatternKind::Lit(_) => {}
    }
}

fn freshen_hir_fn(function: &mut HirFn, alloc: &mut HirIdAlloc) {
    let mut remap = HashMap::new();
    function.id = remap_hir_id(function.id, alloc, &mut remap);
    for param in &mut function.params {
        param.id = remap_hir_id(param.id, alloc, &mut remap);
        param.binding = remap_hir_id(param.binding, alloc, &mut remap);
    }
    if let Some(body) = &mut function.body {
        freshen_hir_expr(body, alloc, &mut remap);
    }
}

fn remap_hir_id(id: HirId, alloc: &mut HirIdAlloc, remap: &mut HashMap<HirId, HirId>) -> HirId {
    *remap.entry(id).or_insert_with(|| alloc.fresh())
}

fn freshen_hir_expr(expr: &mut HirExpr, alloc: &mut HirIdAlloc, remap: &mut HashMap<HirId, HirId>) {
    use crate::hir::HirExprKind::*;

    expr.id = remap_hir_id(expr.id, alloc, remap);
    match &mut expr.kind {
        Var(id) => {
            *id = remap_hir_id(*id, alloc, remap);
        }
        Block(stmts, tail) => {
            for stmt in stmts {
                freshen_hir_stmt(stmt, alloc, remap);
            }
            if let Some(tail) = tail {
                freshen_hir_expr(tail, alloc, remap);
            }
        }
        Call { callee, args } => {
            freshen_hir_expr(callee, alloc, remap);
            for arg in args {
                freshen_hir_expr(arg, alloc, remap);
            }
        }
        MethodCall { receiver, args, .. } => {
            freshen_hir_expr(receiver, alloc, remap);
            for arg in args {
                freshen_hir_expr(arg, alloc, remap);
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
        | Defer(base) => freshen_hir_expr(base, alloc, remap),
        Index { base, index } => {
            freshen_hir_expr(base, alloc, remap);
            freshen_hir_expr(index, alloc, remap);
        }
        Tuple(elems) | Array(elems) => {
            for elem in elems {
                freshen_hir_expr(elem, alloc, remap);
            }
        }
        Repeat { elem, .. } => freshen_hir_expr(elem, alloc, remap),
        Struct { fields, rest, .. } => {
            for (_, value) in fields {
                freshen_hir_expr(value, alloc, remap);
            }
            if let Some(rest) = rest {
                freshen_hir_expr(rest, alloc, remap);
            }
        }
        If {
            condition,
            then_branch,
            else_branch,
        } => {
            freshen_hir_expr(condition, alloc, remap);
            freshen_hir_expr(then_branch, alloc, remap);
            if let Some(else_branch) = else_branch {
                freshen_hir_expr(else_branch, alloc, remap);
            }
        }
        Match { scrutinee, arms } => {
            freshen_hir_expr(scrutinee, alloc, remap);
            for arm in arms {
                freshen_hir_arm(arm, alloc, remap);
            }
        }
        BinOp { lhs, rhs, .. } => {
            freshen_hir_expr(lhs, alloc, remap);
            freshen_hir_expr(rhs, alloc, remap);
        }
        UnaryOp { operand, .. } | Cast { expr: operand, .. } | Ref { expr: operand, .. } => {
            freshen_hir_expr(operand, alloc, remap);
        }
        Assign { target, value } => {
            freshen_hir_expr(target, alloc, remap);
            freshen_hir_expr(value, alloc, remap);
        }
        Return(Some(value)) | Break(Some(value)) => freshen_hir_expr(value, alloc, remap),
        Return(None) | Break(None) => {}
        ForDesugared {
            iter,
            binding,
            body,
        } => {
            *binding = remap_hir_id(*binding, alloc, remap);
            freshen_hir_expr(iter, alloc, remap);
            freshen_hir_expr(body, alloc, remap);
        }
        While { condition, body } => {
            freshen_hir_expr(condition, alloc, remap);
            freshen_hir_expr(body, alloc, remap);
        }
        Closure {
            params,
            body,
            captures,
            ..
        } => {
            for param in params {
                param.id = remap_hir_id(param.id, alloc, remap);
                param.binding = remap_hir_id(param.binding, alloc, remap);
            }
            for capture in captures {
                *capture = remap_hir_id(*capture, alloc, remap);
            }
            freshen_hir_expr(body, alloc, remap);
        }
        Range { lo, hi, .. } => {
            if let Some(lo) = lo {
                freshen_hir_expr(lo, alloc, remap);
            }
            if let Some(hi) = hi {
                freshen_hir_expr(hi, alloc, remap);
            }
        }
        Lit(_) | DefRef(_) | Continue => {}
    }
}

fn freshen_hir_stmt(stmt: &mut HirStmt, alloc: &mut HirIdAlloc, remap: &mut HashMap<HirId, HirId>) {
    stmt.id = remap_hir_id(stmt.id, alloc, remap);
    match &mut stmt.kind {
        crate::hir::HirStmtKind::Let { binding, init, .. } => {
            *binding = remap_hir_id(*binding, alloc, remap);
            if let Some(init) = init {
                freshen_hir_expr(init, alloc, remap);
            }
        }
        crate::hir::HirStmtKind::Expr(expr)
        | crate::hir::HirStmtKind::Errdefer(expr)
        | crate::hir::HirStmtKind::Defer(expr) => freshen_hir_expr(expr, alloc, remap),
        crate::hir::HirStmtKind::Use(_) => {}
    }
}

fn freshen_hir_arm(arm: &mut HirArm, alloc: &mut HirIdAlloc, remap: &mut HashMap<HirId, HirId>) {
    arm.id = remap_hir_id(arm.id, alloc, remap);
    freshen_hir_pattern(&mut arm.pattern, alloc, remap);
    if let Some(guard) = &mut arm.guard {
        freshen_hir_expr(guard, alloc, remap);
    }
    freshen_hir_expr(&mut arm.body, alloc, remap);
}

fn freshen_hir_pattern(
    pattern: &mut HirPattern,
    alloc: &mut HirIdAlloc,
    remap: &mut HashMap<HirId, HirId>,
) {
    pattern.id = remap_hir_id(pattern.id, alloc, remap);
    match &mut pattern.kind {
        HirPatternKind::Binding { id, .. } => {
            *id = remap_hir_id(*id, alloc, remap);
        }
        HirPatternKind::Tuple(elems) | HirPatternKind::Or(elems) => {
            for elem in elems {
                freshen_hir_pattern(elem, alloc, remap);
            }
        }
        HirPatternKind::Struct { fields, .. } => {
            for (_, field) in fields {
                freshen_hir_pattern(field, alloc, remap);
            }
        }
        HirPatternKind::Variant { args, .. } | HirPatternKind::Slice { elems: args, .. } => {
            for arg in args {
                freshen_hir_pattern(arg, alloc, remap);
            }
        }
        HirPatternKind::Range { lo, hi, .. } => {
            freshen_hir_pattern(lo, alloc, remap);
            freshen_hir_pattern(hi, alloc, remap);
        }
        HirPatternKind::Ref { inner, .. } => freshen_hir_pattern(inner, alloc, remap),
        HirPatternKind::Wildcard | HirPatternKind::Lit(_) => {}
    }
}

fn apply_stmt_methods(stmt: &mut HirStmt, resolved_methods: &HashMap<HirId, DefId>) {
    match &mut stmt.kind {
        crate::hir::HirStmtKind::Let { init, .. } => {
            if let Some(init) = init {
                apply_expr_methods(init, resolved_methods);
            }
        }
        crate::hir::HirStmtKind::Expr(expr)
        | crate::hir::HirStmtKind::Errdefer(expr)
        | crate::hir::HirStmtKind::Defer(expr) => apply_expr_methods(expr, resolved_methods),
        crate::hir::HirStmtKind::Use(_) => {}
    }
}

fn apply_expr_methods(expr: &mut HirExpr, resolved_methods: &HashMap<HirId, DefId>) {
    use crate::hir::HirExprKind::*;

    match &mut expr.kind {
        MethodCall {
            receiver,
            method_id,
            args,
            ..
        } => {
            if let Some(def) = resolved_methods.get(&expr.id) {
                *method_id = *def;
            }
            apply_expr_methods(receiver, resolved_methods);
            for arg in args {
                apply_expr_methods(arg, resolved_methods);
            }
        }
        Block(stmts, tail) => {
            for stmt in stmts {
                apply_stmt_methods(stmt, resolved_methods);
            }
            if let Some(tail) = tail {
                apply_expr_methods(tail, resolved_methods);
            }
        }
        Call { callee, args } => {
            apply_expr_methods(callee, resolved_methods);
            for arg in args {
                apply_expr_methods(arg, resolved_methods);
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
        | Defer(base) => apply_expr_methods(base, resolved_methods),
        Index { base, index } => {
            apply_expr_methods(base, resolved_methods);
            apply_expr_methods(index, resolved_methods);
        }
        BinOp { lhs, rhs, .. } => {
            apply_expr_methods(lhs, resolved_methods);
            apply_expr_methods(rhs, resolved_methods);
        }
        Tuple(elems) | Array(elems) => {
            for elem in elems {
                apply_expr_methods(elem, resolved_methods);
            }
        }
        Repeat { elem, .. } => apply_expr_methods(elem, resolved_methods),
        Struct { fields, rest, .. } => {
            for (_, value) in fields {
                apply_expr_methods(value, resolved_methods);
            }
            if let Some(rest) = rest {
                apply_expr_methods(rest, resolved_methods);
            }
        }
        If {
            condition,
            then_branch,
            else_branch,
        } => {
            apply_expr_methods(condition, resolved_methods);
            apply_expr_methods(then_branch, resolved_methods);
            if let Some(else_branch) = else_branch {
                apply_expr_methods(else_branch, resolved_methods);
            }
        }
        Match { scrutinee, arms } => {
            apply_expr_methods(scrutinee, resolved_methods);
            for arm in arms {
                if let Some(guard) = &mut arm.guard {
                    apply_expr_methods(guard, resolved_methods);
                }
                apply_expr_methods(&mut arm.body, resolved_methods);
            }
        }
        While { condition, body } => {
            apply_expr_methods(condition, resolved_methods);
            apply_expr_methods(body, resolved_methods);
        }
        ForDesugared { iter, body, .. } => {
            apply_expr_methods(iter, resolved_methods);
            apply_expr_methods(body, resolved_methods);
        }
        UnaryOp { operand, .. } | Cast { expr: operand, .. } | Ref { expr: operand, .. } => {
            apply_expr_methods(operand, resolved_methods);
        }
        Assign { target, value } => {
            apply_expr_methods(target, resolved_methods);
            apply_expr_methods(value, resolved_methods);
        }
        Return(value) | Break(value) => {
            if let Some(value) = value {
                apply_expr_methods(value, resolved_methods);
            }
        }
        Range { lo, hi, .. } => {
            if let Some(lo) = lo {
                apply_expr_methods(lo, resolved_methods);
            }
            if let Some(hi) = hi {
                apply_expr_methods(hi, resolved_methods);
            }
        }
        Closure { body, .. } => apply_expr_methods(body, resolved_methods),
        Lit(_) | Var(_) | DefRef(_) | Continue => {}
    }
}
