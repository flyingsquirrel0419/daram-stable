use crate::{
    diagnostics::Diagnostic,
    hir::{DefId, HirAbility, HirAssocItem, HirImpl, HirModule, HirVariant, Ty},
    source::Span,
};
use std::collections::{HashMap, HashSet};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum AbilityKind {
    Copy,
    Clone,
    Debug,
    Default,
    Drop,
    PartialEq,
    Eq,
    Hash,
    Send,
    Sync,
}

pub(crate) struct AbilityContext<'a> {
    pub def_names: &'a HashMap<DefId, String>,
    pub abilities: &'a HashMap<DefId, AbilityKind>,
    pub ability_impls: &'a HashMap<DefId, HashSet<AbilityKind>>,
    pub struct_fields: &'a HashMap<DefId, HashMap<String, Ty>>,
    pub enum_variants: &'a HashMap<DefId, Vec<HirVariant>>,
    pub string_def: Option<DefId>,
}

impl AbilityContext<'_> {
    fn type_name(&self, def: DefId) -> String {
        self.def_names
            .get(&def)
            .cloned()
            .unwrap_or_else(|| format!("{def:?}"))
    }

    fn has_ability_impl(&self, def: DefId, ability: AbilityKind) -> bool {
        self.ability_impls
            .get(&def)
            .is_some_and(|abilities| abilities.contains(&ability))
    }
}

pub(crate) fn ability_kind_for_name(name: &str) -> Option<AbilityKind> {
    match name.rsplit("::").next().unwrap_or(name) {
        "Copy" => Some(AbilityKind::Copy),
        "Clone" => Some(AbilityKind::Clone),
        "Debug" => Some(AbilityKind::Debug),
        "Default" => Some(AbilityKind::Default),
        "Drop" => Some(AbilityKind::Drop),
        "PartialEq" => Some(AbilityKind::PartialEq),
        "Eq" => Some(AbilityKind::Eq),
        "Hash" => Some(AbilityKind::Hash),
        "Send" => Some(AbilityKind::Send),
        "Sync" => Some(AbilityKind::Sync),
        _ => None,
    }
}

pub(crate) fn ability_name(kind: AbilityKind) -> &'static str {
    match kind {
        AbilityKind::Copy => "Copy",
        AbilityKind::Clone => "Clone",
        AbilityKind::Debug => "Debug",
        AbilityKind::Default => "Default",
        AbilityKind::Drop => "Drop",
        AbilityKind::PartialEq => "PartialEq",
        AbilityKind::Eq => "Eq",
        AbilityKind::Hash => "Hash",
        AbilityKind::Send => "Send",
        AbilityKind::Sync => "Sync",
    }
}

pub(crate) fn collect_derived_abilities(
    module: &HirModule,
    ability_impls: &mut HashMap<DefId, HashSet<AbilityKind>>,
) {
    for strukt in &module.structs {
        collect_named_derives(strukt.def, &strukt.derives, ability_impls);
    }
    for enum_def in &module.enums {
        collect_named_derives(enum_def.def, &enum_def.derives, ability_impls);
    }
    for nested in &module.modules {
        if let Some(body) = &nested.body {
            collect_derived_abilities(body, ability_impls);
        }
    }
}

pub(crate) fn collect_abilities(
    abilities: &[HirAbility],
    def_names: &HashMap<DefId, String>,
    ability_kinds: &mut HashMap<DefId, AbilityKind>,
    method_defs: &mut HashMap<(DefId, String), DefId>,
) {
    for ability in abilities {
        if let Some(kind) = def_names
            .get(&ability.def)
            .and_then(|name| ability_kind_for_name(name))
        {
            ability_kinds.insert(ability.def, kind);
        }
        for item in &ability.items {
            if let HirAssocItem::Method(method) = item {
                let name = def_names
                    .get(&method.def)
                    .and_then(|q| q.rsplit("::").next())
                    .unwrap_or_default()
                    .to_string();
                method_defs.insert((ability.def, name), method.def);
            }
        }
    }
}

pub(crate) fn ability_kind_for_ty(
    ctx: &AbilityContext<'_>,
    resolve_ty: &dyn Fn(&Ty) -> Ty,
    ty: &Ty,
) -> Option<AbilityKind> {
    let Ty::Named { def, .. } = resolve_ty(ty) else {
        return None;
    };
    ctx.abilities.get(&def).copied()
}

pub(crate) fn is_copy_ty(
    ctx: &AbilityContext<'_>,
    resolve_ty: &dyn Fn(&Ty) -> Ty,
    ty: &Ty,
) -> bool {
    match resolve_ty(ty) {
        Ty::Bool
        | Ty::Char
        | Ty::Int(_)
        | Ty::Uint(_)
        | Ty::Float(_)
        | Ty::Unit
        | Ty::Never
        | Ty::Ref { .. }
        | Ty::RawPtr { .. }
        | Ty::FnPtr { .. } => true,
        Ty::Tuple(elems) => elems.iter().all(|elem| is_copy_ty(ctx, resolve_ty, elem)),
        Ty::Array { elem, .. } => is_copy_ty(ctx, resolve_ty, &elem),
        Ty::Var(_) => true,
        Ty::Named { def, .. } => ctx.has_ability_impl(def, AbilityKind::Copy),
        Ty::String => ctx
            .string_def
            .is_some_and(|def| ctx.has_ability_impl(def, AbilityKind::Copy)),
        Ty::Slice(_) | Ty::ImplTrait(_) | Ty::DynTrait(_) | Ty::Str => false,
    }
}

pub(crate) fn has_ability_ty(
    ctx: &AbilityContext<'_>,
    resolve_ty: &dyn Fn(&Ty) -> Ty,
    ty: &Ty,
    ability: AbilityKind,
) -> bool {
    if matches!(ability, AbilityKind::Clone | AbilityKind::Debug) {
        return !matches!(resolve_ty(ty), Ty::Slice(_) | Ty::ImplTrait(_) | Ty::Str);
    }
    match resolve_ty(ty) {
        Ty::Bool
        | Ty::Char
        | Ty::Int(_)
        | Ty::Uint(_)
        | Ty::Float(_)
        | Ty::Unit
        | Ty::Never
        | Ty::FnPtr { .. } => true,
        Ty::Ref { .. } | Ty::RawPtr { .. } => ability != AbilityKind::Drop,
        Ty::Tuple(elems) => elems
            .iter()
            .all(|elem| has_ability_ty(ctx, resolve_ty, elem, ability)),
        Ty::Array { elem, .. } => has_ability_ty(ctx, resolve_ty, &elem, ability),
        Ty::Var(_) => true,
        Ty::Named { def, .. } => ctx.has_ability_impl(def, ability),
        Ty::String => {
            matches!(
                ability,
                AbilityKind::Clone | AbilityKind::Debug | AbilityKind::Default
            ) || ctx
                .string_def
                .is_some_and(|def| ctx.has_ability_impl(def, ability))
        }
        Ty::Slice(_) | Ty::ImplTrait(_) | Ty::DynTrait(_) | Ty::Str => false,
    }
}

pub(crate) fn check_ability_impl(
    ctx: &AbilityContext<'_>,
    resolve_ty: &dyn Fn(&Ty) -> Ty,
    errors: &mut Vec<Diagnostic>,
    imp: &HirImpl,
    ability: AbilityKind,
    self_def: Option<DefId>,
) {
    let Some(self_def) = self_def else {
        errors.push(
            Diagnostic::error(format!(
                "`{}` can only be implemented for named types in v1",
                ability_name(ability)
            ))
            .with_span(imp.span),
        );
        return;
    };

    if ability == AbilityKind::Copy && ctx.has_ability_impl(self_def, AbilityKind::Drop) {
        errors.push(
            Diagnostic::error("cannot implement both `Copy` and `Drop` for the same type")
                .with_span(imp.span)
                .with_note("remove one of the ability impls to keep move semantics coherent"),
        );
    }

    if ability == AbilityKind::Drop && ctx.has_ability_impl(self_def, AbilityKind::Copy) {
        errors.push(
            Diagnostic::error("cannot implement both `Copy` and `Drop` for the same type")
                .with_span(imp.span)
                .with_note("types with destructors must move, not copy, in v1"),
        );
    }

    if let Some(fields) = ctx.struct_fields.get(&self_def).cloned() {
        for (field_name, field_ty) in fields {
            if !has_ability_ty(ctx, resolve_ty, &field_ty, ability) {
                errors.push(
                    Diagnostic::error(format!(
                        "cannot implement `{}` for `{}` because field `{field_name}` does not satisfy `{}`",
                        ability_name(ability),
                        ctx.type_name(self_def),
                        ability_name(ability),
                    ))
                    .with_span(imp.span)
                    .with_note("remove the impl or change the field type to one with the same ability"),
                );
            }
        }
    }

    if let Some(variants) = ctx.enum_variants.get(&self_def).cloned() {
        for variant in variants {
            for (index, field_ty) in variant.fields.iter().enumerate() {
                if !has_ability_ty(ctx, resolve_ty, field_ty, ability) {
                    errors.push(
                        Diagnostic::error(format!(
                            "cannot implement `{}` for `{}` because variant `{}` field {} does not satisfy `{}`",
                            ability_name(ability),
                            ctx.type_name(self_def),
                            variant.name,
                            index,
                            ability_name(ability),
                        ))
                        .with_span(imp.span)
                        .with_note("remove the impl or change the variant field type to one with the same ability"),
                    );
                }
            }
        }
    }
}

pub(crate) fn check_derived_ability(
    ctx: &AbilityContext<'_>,
    resolve_ty: &dyn Fn(&Ty) -> Ty,
    errors: &mut Vec<Diagnostic>,
    def: DefId,
    span: Span,
    ability: AbilityKind,
) {
    if ability == AbilityKind::Copy && ctx.has_ability_impl(def, AbilityKind::Drop) {
        errors.push(
            Diagnostic::error("cannot combine `@derive(Copy)` with `Drop` on the same type")
                .with_span(span)
                .with_note(
                    "remove `@derive(Copy)` or the `Drop` impl to keep move semantics coherent",
                ),
        );
    }

    if ability == AbilityKind::Drop && ctx.has_ability_impl(def, AbilityKind::Copy) {
        errors.push(
            Diagnostic::error("cannot combine `@derive(Drop)` with `Copy` on the same type")
                .with_span(span)
                .with_note("types with destructors must move, not copy, in v1"),
        );
    }

    if let Some(fields) = ctx.struct_fields.get(&def).cloned() {
        for (field_name, field_ty) in fields {
            if !has_ability_ty(ctx, resolve_ty, &field_ty, ability) {
                errors.push(
                    Diagnostic::error(format!(
                        "cannot derive `{}` for `{}` because field `{field_name}` does not satisfy `{}`",
                        ability_name(ability),
                        ctx.type_name(def),
                        ability_name(ability),
                    ))
                    .with_span(span)
                    .with_note("remove the derive or change the field type to one with the same ability"),
                );
            }
        }
    }

    if let Some(variants) = ctx.enum_variants.get(&def).cloned() {
        for variant in variants {
            for (index, field_ty) in variant.fields.iter().enumerate() {
                if !has_ability_ty(ctx, resolve_ty, field_ty, ability) {
                    errors.push(
                        Diagnostic::error(format!(
                            "cannot derive `{}` for `{}` because variant `{}` field {} does not satisfy `{}`",
                            ability_name(ability),
                            ctx.type_name(def),
                            variant.name,
                            index,
                            ability_name(ability),
                        ))
                        .with_span(span)
                        .with_note("remove the derive or change the variant field type to one with the same ability"),
                    );
                }
            }
        }
    }
}

fn collect_named_derives(
    def: DefId,
    derives: &[String],
    ability_impls: &mut HashMap<DefId, HashSet<AbilityKind>>,
) {
    for derive in derives {
        let Some(ability) = ability_kind_for_name(derive) else {
            continue;
        };
        ability_impls.entry(def).or_default().insert(ability);
    }
}
