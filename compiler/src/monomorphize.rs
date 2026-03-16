use crate::{
    hir::{DefId, HirModule, Ty},
    mir::{
        AggregateKind, MirConst, MirConstItem, MirFn, MirModule, Operand, Place, Projection,
        Rvalue, StatementKind, TerminatorKind,
    },
};
use std::collections::{HashMap, HashSet, VecDeque};

#[derive(Clone)]
struct FunctionTemplate {
    function: MirFn,
    generic_vars: Vec<u32>,
    param_tys: Vec<Ty>,
    ret_ty: Ty,
}

pub fn monomorphize(mir: &MirModule, _hir: &HirModule) -> MirModule {
    let templates = mir
        .functions
        .iter()
        .filter_map(|function| {
            let template = FunctionTemplate::from_fn(function.clone());
            (!template.generic_vars.is_empty()).then_some((function.def, template))
        })
        .collect::<HashMap<_, _>>();
    if templates.is_empty() {
        return mir.clone();
    }

    let mut functions = mir.functions.clone();
    let mut def_names = mir.def_names.clone();
    let mut specializations = HashMap::new();
    let mut specialized_defs = HashSet::new();
    let mut next_index = functions
        .iter()
        .map(|function| function.def.index)
        .max()
        .unwrap_or(0)
        .saturating_add(1);
    let mut queue = (0..functions.len()).collect::<VecDeque<_>>();

    while let Some(function_idx) = queue.pop_front() {
        let mut pending_specializations = Vec::new();
        {
            let function = &functions[function_idx];
            for (block_idx, block) in function.basic_blocks.iter().enumerate() {
                let Some(terminator) = &block.terminator else {
                    continue;
                };
                let TerminatorKind::Call {
                    callee: Operand::Def(def),
                    args,
                    destination,
                    ..
                } = &terminator.kind
                else {
                    continue;
                };
                let Some(template) = templates.get(def) else {
                    continue;
                };
                let Some(substitution) =
                    infer_call_substitution(function, template, args, destination)
                else {
                    continue;
                };
                pending_specializations.push((block_idx, *def, template.clone(), substitution));
            }
        }

        if pending_specializations.is_empty() {
            continue;
        }

        let mut pending_rewrites = Vec::new();
        for (block_idx, def, template, substitution) in pending_specializations {
            let specialization_key = specialization_key(&template, &substitution);
            let specialization_def =
                if let Some(existing) = specializations.get(&(def, specialization_key.clone())) {
                    *existing
                } else {
                    let specialized_def = DefId {
                        file: def.file,
                        index: next_index,
                    };
                    next_index = next_index.saturating_add(1);
                    let specialized_fn =
                        specialize_function(&template.function, specialized_def, &substitution);
                    let base_name = def_names
                        .get(&def)
                        .cloned()
                        .unwrap_or_else(|| format!("def_{}", def.index));
                    def_names.insert(
                        specialized_def,
                        format!("{base_name}__mono_{}", specialized_def.index),
                    );
                    specialized_defs.insert(specialized_def);
                    functions.push(specialized_fn);
                    queue.push_back(functions.len() - 1);
                    specializations.insert((def, specialization_key), specialized_def);
                    specialized_def
                };
            // Look up the specialized function's return type before taking
            // the mutable borrow of `functions[function_idx]`.
            let specialized_ret_ty = functions
                .iter()
                .find(|f| f.def == specialization_def)
                .and_then(|f| f.locals.first())
                .map(|l| l.ty.clone());
            pending_rewrites.push((block_idx, specialization_def, specialized_ret_ty));
        }

        let function = &mut functions[function_idx];
        for (block_idx, specialization_def, specialized_ret_ty) in pending_rewrites {
            if let Some(terminator) = &mut function.basic_blocks[block_idx].terminator {
                if let TerminatorKind::Call {
                    callee: Operand::Def(def),
                    destination,
                    ..
                } = &mut terminator.kind
                {
                    *def = specialization_def;
                    // Update the destination local's type to the specialized
                    // return type.  This is required so that downstream calls
                    // using the result can infer the concrete specialization.
                    if let Some(ret_ty) = specialized_ret_ty {
                        if let Some(local_decl) =
                            function.locals.get_mut(destination.local.0 as usize)
                        {
                            local_decl.ty = ret_ty;
                        }
                    }
                }
            }
        }
        // Propagate updated local types through Move/Copy assignments so
        // that `_w = Move(_temp)` gets `_w`'s type updated when `_temp`'s
        // type was just concretised.
        let propagated = propagate_local_types_through_moves(function);
        // If types changed, re-queue this function so that any downstream
        // calls (e.g. `unwrap(w)` after `w` is concretised from `wrap(42)`)
        // get a chance to be specialised.
        if propagated {
            queue.push_back(function_idx);
        }
    }

    let (reachable_fns, reachable_consts) =
        reachable_defs(&functions, &mir.consts, &def_names, &["main", "__main"]);
    let template_defs = templates.keys().copied().collect::<HashSet<_>>();

    MirModule {
        consts: mir
            .consts
            .iter()
            .filter(|item| reachable_consts.contains(&item.def))
            .cloned()
            .collect(),
        functions: functions
            .into_iter()
            .filter(|function| {
                let should_prune = (template_defs.contains(&function.def)
                    || specialized_defs.contains(&function.def))
                    && !reachable_fns.contains(&function.def);
                !should_prune
            })
            .collect(),
        enum_variant_indices: mir.enum_variant_indices.clone(),
        enum_variant_names: mir.enum_variant_names.clone(),
        struct_field_names: mir.struct_field_names.clone(),
        display_impls: mir.display_impls.clone(),
        def_names,
    }
}

impl FunctionTemplate {
    fn from_fn(function: MirFn) -> Self {
        let ret_ty = function
            .locals
            .first()
            .map(|local| local.ty.clone())
            .unwrap_or(Ty::Unit);
        let param_tys = function
            .locals
            .iter()
            .skip(1)
            .take(function.argc)
            .map(|local| local.ty.clone())
            .collect::<Vec<_>>();
        let mut generic_vars = Vec::new();
        let mut seen = HashSet::new();
        for param_ty in &param_tys {
            collect_ty_vars(param_ty, &mut generic_vars, &mut seen);
        }
        collect_ty_vars(&ret_ty, &mut generic_vars, &mut seen);
        Self {
            function,
            generic_vars,
            param_tys,
            ret_ty,
        }
    }
}

fn infer_call_substitution(
    caller: &MirFn,
    template: &FunctionTemplate,
    args: &[Operand],
    destination: &Place,
) -> Option<HashMap<u32, Ty>> {
    if template.param_tys.len() != args.len() {
        return None;
    }

    let mut substitution = HashMap::new();
    for (expected_ty, arg) in template.param_tys.iter().zip(args.iter()) {
        let actual_ty = operand_ty(caller, arg)?;
        if !bind_ty(
            expected_ty,
            &actual_ty,
            &template.generic_vars,
            &mut substitution,
        ) {
            return None;
        }
    }

    let all_bound = template
        .generic_vars
        .iter()
        .all(|id| substitution.get(id).is_some_and(|ty| !contains_ty_var(ty)));
    let destination_ty = place_ty(caller, destination)?;
    if !contains_ty_var(&destination_ty)
        && !bind_ty(
            &template.ret_ty,
            &destination_ty,
            &template.generic_vars,
            &mut substitution,
        )
    {
        return None;
    }

    (all_bound
        || template
            .generic_vars
            .iter()
            .all(|id| substitution.get(id).is_some_and(|ty| !contains_ty_var(ty))))
    .then_some(substitution)
}

fn bind_ty(
    template: &Ty,
    actual: &Ty,
    generic_vars: &[u32],
    substitution: &mut HashMap<u32, Ty>,
) -> bool {
    match template {
        Ty::Var(id) if generic_vars.contains(id) => match substitution.get(id) {
            Some(existing) => existing == actual,
            None => {
                substitution.insert(*id, actual.clone());
                true
            }
        },
        Ty::Ref {
            mutable: expected_mutable,
            inner: expected_inner,
        } => match actual {
            Ty::Ref {
                mutable: actual_mutable,
                inner: actual_inner,
            } if expected_mutable == actual_mutable => {
                bind_ty(expected_inner, actual_inner, generic_vars, substitution)
            }
            _ => false,
        },
        Ty::RawPtr {
            mutable: expected_mutable,
            inner: expected_inner,
        } => match actual {
            Ty::RawPtr {
                mutable: actual_mutable,
                inner: actual_inner,
            } if expected_mutable == actual_mutable => {
                bind_ty(expected_inner, actual_inner, generic_vars, substitution)
            }
            _ => false,
        },
        Ty::Array {
            elem: expected_elem,
            len: expected_len,
        } => match actual {
            Ty::Array {
                elem: actual_elem,
                len: actual_len,
            } if expected_len == actual_len => {
                bind_ty(expected_elem, actual_elem, generic_vars, substitution)
            }
            _ => false,
        },
        Ty::Slice(expected_elem) => match actual {
            Ty::Slice(actual_elem) => {
                bind_ty(expected_elem, actual_elem, generic_vars, substitution)
            }
            _ => false,
        },
        Ty::Tuple(expected_elems) => match actual {
            Ty::Tuple(actual_elems) if expected_elems.len() == actual_elems.len() => expected_elems
                .iter()
                .zip(actual_elems.iter())
                .all(|(expected, actual)| bind_ty(expected, actual, generic_vars, substitution)),
            _ => false,
        },
        Ty::Named {
            def: expected_def,
            args: expected_args,
        } => match actual {
            Ty::Named {
                def: actual_def,
                args: actual_args,
            } if expected_def == actual_def && expected_args.len() == actual_args.len() => {
                expected_args
                    .iter()
                    .zip(actual_args.iter())
                    .all(|(expected, actual)| bind_ty(expected, actual, generic_vars, substitution))
            }
            _ => false,
        },
        Ty::FnPtr {
            params: expected_params,
            ret: expected_ret,
        } => match actual {
            Ty::FnPtr {
                params: actual_params,
                ret: actual_ret,
            } if expected_params.len() == actual_params.len() => {
                expected_params
                    .iter()
                    .zip(actual_params.iter())
                    .all(|(expected, actual)| bind_ty(expected, actual, generic_vars, substitution))
                    && bind_ty(expected_ret, actual_ret, generic_vars, substitution)
            }
            _ => false,
        },
        _ => template == actual,
    }
}

fn specialize_function(
    function: &MirFn,
    specialized_def: DefId,
    substitution: &HashMap<u32, Ty>,
) -> MirFn {
    let mut function = function.clone();
    function.def = specialized_def;

    // First pass: apply the primary substitution to all locals.
    for local in &mut function.locals {
        local.ty = substitute_ty(&local.ty, substitution);
    }

    // Second pass: some intermediate locals may carry fresh type-variable IDs
    // that are *aliases* of substituted vars (e.g. from `instantiate_struct_ty`
    // creating a fresh Var that gets unified with a generic param during type
    // checking). Extend the substitution with bindings inferred from pairs of
    // concrete and still-abstract locals that share structural shape.
    let extended = extend_substitution_from_locals(&function.locals, substitution);
    if !extended.is_empty() {
        for local in &mut function.locals {
            local.ty = substitute_ty(&local.ty, &extended);
        }
    }

    for block in &mut function.basic_blocks {
        for statement in &mut block.statements {
            if let StatementKind::Assign(_, rvalue) = &mut statement.kind {
                substitute_rvalue(rvalue, substitution);
            }
        }
    }
    function
}

/// Build additional substitutions for type variables that survived the primary
/// substitution because they are aliases of already-substituted vars.
///
/// Strategy: for every pair of locals (concrete, abstract) where:
/// - concrete has no remaining type variables, and
/// - abstract has the same structural "shape" (same Named def, same arity),
/// bind the abstract vars to the concrete args.
fn extend_substitution_from_locals(
    locals: &[crate::mir::LocalDecl],
    base: &HashMap<u32, Ty>,
) -> HashMap<u32, Ty> {
    let mut extra: HashMap<u32, Ty> = HashMap::new();
    let concrete: Vec<&Ty> = locals
        .iter()
        .map(|l| &l.ty)
        .filter(|ty| !contains_ty_var(ty))
        .collect();
    for abstract_ty in locals
        .iter()
        .map(|l| &l.ty)
        .filter(|ty| contains_ty_var(ty))
    {
        // Collect the unbound var IDs in this abstract type so bind_ty can
        // treat them as the "generic vars" eligible for binding.
        let mut vars = Vec::new();
        let mut seen_vars = HashSet::new();
        collect_ty_vars(abstract_ty, &mut vars, &mut seen_vars);
        for &conc_ty in &concrete {
            bind_ty(abstract_ty, conc_ty, &vars, &mut extra);
        }
    }
    // Remove entries already in the base substitution to avoid conflicts.
    extra.retain(|k, _| !base.contains_key(k));
    extra
}

/// After updating call-destination locals, propagate concrete types through
/// simple `dst = Move/Copy(src)` assignments.  This ensures that when `_temp`
/// was updated to a concrete type, `_w = Move(_temp)` gets the same type.
///
/// We run a fixed-point loop because assignments may be chained.
/// Returns true if any local type was updated.
fn propagate_local_types_through_moves(function: &mut MirFn) -> bool {
    let mut any_changed = false;
    loop {
        let mut changed = false;
        // Collect (dst_local, src_local) pairs for plain Move/Copy assigns.
        let pairs: Vec<(crate::mir::Local, crate::mir::Local)> = function
            .basic_blocks
            .iter()
            .flat_map(|bb| &bb.statements)
            .filter_map(|stmt| {
                if let StatementKind::Assign(place, rvalue) = &stmt.kind {
                    if place.projections.is_empty() {
                        match rvalue {
                            Rvalue::Use(Operand::Move(src)) | Rvalue::Use(Operand::Copy(src)) => {
                                return Some((place.local, *src));
                            }
                            _ => {}
                        }
                    }
                }
                None
            })
            .collect();

        for (dst, src) in pairs {
            let src_ty = function
                .locals
                .get(src.0 as usize)
                .map(|l| l.ty.clone())
                .unwrap_or(Ty::Unit);
            let dst_ty = function
                .locals
                .get(dst.0 as usize)
                .map(|l| l.ty.clone())
                .unwrap_or(Ty::Unit);
            if !contains_ty_var(&src_ty) && contains_ty_var(&dst_ty) {
                if let Some(local_decl) = function.locals.get_mut(dst.0 as usize) {
                    local_decl.ty = src_ty;
                    changed = true;
                    any_changed = true;
                }
            }
        }
        if !changed {
            break;
        }
    }
    any_changed
}

fn substitute_rvalue(rvalue: &mut Rvalue, substitution: &HashMap<u32, Ty>) {
    match rvalue {
        Rvalue::Cast { target_ty, .. } => {
            *target_ty = substitute_ty(target_ty, substitution);
        }
        Rvalue::Aggregate(kind, _) => {
            if let crate::mir::AggregateKind::Array(elem_ty) = kind {
                *elem_ty = substitute_ty(elem_ty, substitution);
            }
        }
        Rvalue::Use(_)
        | Rvalue::Read(_)
        | Rvalue::BinaryOp { .. }
        | Rvalue::UnaryOp { .. }
        | Rvalue::Ref { .. }
        | Rvalue::AddressOf { .. }
        | Rvalue::Discriminant(_)
        | Rvalue::Len(_) => {}
    }
}

fn substitute_ty(ty: &Ty, substitution: &HashMap<u32, Ty>) -> Ty {
    match ty {
        Ty::Var(id) => substitution.get(id).cloned().unwrap_or(Ty::Var(*id)),
        Ty::Ref { mutable, inner } => Ty::Ref {
            mutable: *mutable,
            inner: Box::new(substitute_ty(inner, substitution)),
        },
        Ty::RawPtr { mutable, inner } => Ty::RawPtr {
            mutable: *mutable,
            inner: Box::new(substitute_ty(inner, substitution)),
        },
        Ty::Array { elem, len } => Ty::Array {
            elem: Box::new(substitute_ty(elem, substitution)),
            len: *len,
        },
        Ty::Slice(elem) => Ty::Slice(Box::new(substitute_ty(elem, substitution))),
        Ty::Tuple(elems) => Ty::Tuple(
            elems
                .iter()
                .map(|elem| substitute_ty(elem, substitution))
                .collect(),
        ),
        Ty::Named { def, args } => Ty::Named {
            def: *def,
            args: args
                .iter()
                .map(|arg| substitute_ty(arg, substitution))
                .collect(),
        },
        Ty::FnPtr { params, ret } => Ty::FnPtr {
            params: params
                .iter()
                .map(|param| substitute_ty(param, substitution))
                .collect(),
            ret: Box::new(substitute_ty(ret, substitution)),
        },
        other => other.clone(),
    }
}

fn operand_ty(function: &MirFn, operand: &Operand) -> Option<Ty> {
    match operand {
        Operand::Copy(local) | Operand::Move(local) => function
            .locals
            .get(local.0 as usize)
            .map(|local| local.ty.clone()),
        Operand::Const(constant) => Some(const_ty(constant)),
        Operand::Def(_) => None,
    }
}

fn place_ty(function: &MirFn, place: &Place) -> Option<Ty> {
    let mut ty = function.locals.get(place.local.0 as usize)?.ty.clone();
    for projection in &place.projections {
        ty = match (projection, ty) {
            (Projection::Field(index), Ty::Tuple(elems)) => elems.get(*index).cloned()?,
            (Projection::Index(_), Ty::Array { elem, .. }) => *elem,
            (Projection::Deref, Ty::Ref { inner, .. }) => *inner,
            (Projection::Deref, Ty::RawPtr { inner, .. }) => *inner,
            (Projection::VariantField { field_idx, .. }, Ty::Named { args, .. }) => {
                args.get(*field_idx).cloned().unwrap_or(Ty::Unit)
            }
            _ => return None,
        };
    }
    Some(ty)
}

fn const_ty(constant: &crate::mir::MirConst) -> Ty {
    match constant {
        crate::mir::MirConst::Bool(_) => Ty::Bool,
        crate::mir::MirConst::Int(_) => Ty::Int(crate::hir::IntSize::I64),
        crate::mir::MirConst::Uint(_) => Ty::Uint(crate::hir::UintSize::U64),
        crate::mir::MirConst::Float(_) => Ty::Float(crate::hir::FloatSize::F64),
        crate::mir::MirConst::Char(_) => Ty::Char,
        crate::mir::MirConst::Str(_) => Ty::Ref {
            mutable: false,
            inner: Box::new(Ty::Str),
        },
        crate::mir::MirConst::Tuple(items) => Ty::Tuple(items.iter().map(const_ty).collect()),
        crate::mir::MirConst::Array(items) => {
            let elem_ty = items.first().map(const_ty).unwrap_or(Ty::Unit);
            Ty::Array {
                elem: Box::new(elem_ty),
                len: items.len(),
            }
        }
        crate::mir::MirConst::Struct { def, fields } => Ty::Named {
            def: *def,
            args: fields.iter().map(const_ty).collect(),
        },
        crate::mir::MirConst::Ref(inner) => Ty::Ref {
            mutable: false,
            inner: Box::new(const_ty(inner)),
        },
        crate::mir::MirConst::Unit => Ty::Unit,
        crate::mir::MirConst::Undef => Ty::Var(u32::MAX),
    }
}

fn collect_ty_vars(ty: &Ty, ordered: &mut Vec<u32>, seen: &mut HashSet<u32>) {
    match ty {
        Ty::Var(id) => {
            if seen.insert(*id) {
                ordered.push(*id);
            }
        }
        Ty::Ref { inner, .. } | Ty::RawPtr { inner, .. } | Ty::Slice(inner) => {
            collect_ty_vars(inner, ordered, seen);
        }
        Ty::Array { elem, .. } => collect_ty_vars(elem, ordered, seen),
        Ty::Tuple(elems) => {
            for elem in elems {
                collect_ty_vars(elem, ordered, seen);
            }
        }
        Ty::Named { args, .. } => {
            for arg in args {
                collect_ty_vars(arg, ordered, seen);
            }
        }
        Ty::FnPtr { params, ret } => {
            for param in params {
                collect_ty_vars(param, ordered, seen);
            }
            collect_ty_vars(ret, ordered, seen);
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

fn contains_ty_var(ty: &Ty) -> bool {
    match ty {
        Ty::Var(_) => true,
        Ty::Ref { inner, .. } | Ty::RawPtr { inner, .. } | Ty::Slice(inner) => {
            contains_ty_var(inner)
        }
        Ty::Array { elem, .. } => contains_ty_var(elem),
        Ty::Tuple(elems) => elems.iter().any(contains_ty_var),
        Ty::Named { args, .. } => args.iter().any(contains_ty_var),
        Ty::FnPtr { params, ret } => params.iter().any(contains_ty_var) || contains_ty_var(ret),
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
        | Ty::String => false,
    }
}

fn specialization_key(template: &FunctionTemplate, substitution: &HashMap<u32, Ty>) -> String {
    template
        .generic_vars
        .iter()
        .map(|id| {
            substitution
                .get(id)
                .map(|ty| format!("{ty:?}"))
                .unwrap_or_else(|| "_".to_string())
        })
        .collect::<Vec<_>>()
        .join("|")
}

fn reachable_defs(
    functions: &[MirFn],
    consts: &[MirConstItem],
    def_names: &HashMap<DefId, String>,
    entry_candidates: &[&str],
) -> (HashSet<DefId>, HashSet<DefId>) {
    let function_map = functions
        .iter()
        .map(|function| (function.def, function))
        .collect::<HashMap<_, _>>();
    let const_map = consts
        .iter()
        .map(|item| (item.def, item))
        .collect::<HashMap<_, _>>();

    let entry_defs = def_names
        .iter()
        .filter_map(|(def, name)| {
            entry_candidates
                .iter()
                .any(|candidate| name == candidate || name.ends_with(&format!("::{candidate}")))
                .then_some(*def)
        })
        .collect::<Vec<_>>();

    let mut reachable_fns = HashSet::new();
    let mut reachable_consts = HashSet::new();
    let mut queue = entry_defs.into_iter().collect::<VecDeque<_>>();

    while let Some(def) = queue.pop_front() {
        if let Some(function) = function_map.get(&def) {
            if !reachable_fns.insert(def) {
                continue;
            }
            for nested in function_defs(function) {
                if function_map.contains_key(&nested) && !reachable_fns.contains(&nested) {
                    queue.push_back(nested);
                }
                if const_map.contains_key(&nested) && !reachable_consts.contains(&nested) {
                    queue.push_back(nested);
                }
            }
        } else if let Some(item) = const_map.get(&def) {
            if !reachable_consts.insert(def) {
                continue;
            }
            for nested in const_defs(item) {
                if function_map.contains_key(&nested) && !reachable_fns.contains(&nested) {
                    queue.push_back(nested);
                }
                if const_map.contains_key(&nested) && !reachable_consts.contains(&nested) {
                    queue.push_back(nested);
                }
            }
        }
    }

    (reachable_fns, reachable_consts)
}

fn function_defs(function: &MirFn) -> Vec<DefId> {
    let mut defs = Vec::new();
    for block in &function.basic_blocks {
        for statement in &block.statements {
            if let StatementKind::Assign(_, rvalue) = &statement.kind {
                rvalue_defs(rvalue, &mut defs);
            }
        }
        if let Some(terminator) = &block.terminator {
            match &terminator.kind {
                TerminatorKind::Call { callee, args, .. } => {
                    operand_defs(callee, &mut defs);
                    for arg in args {
                        operand_defs(arg, &mut defs);
                    }
                }
                TerminatorKind::SwitchInt { discriminant, .. } => {
                    operand_defs(discriminant, &mut defs);
                }
                TerminatorKind::Assert { cond, .. } => operand_defs(cond, &mut defs),
                TerminatorKind::Goto(_)
                | TerminatorKind::Return
                | TerminatorKind::Drop { .. }
                | TerminatorKind::Unreachable
                | TerminatorKind::ErrdeferUnwind(_) => {}
            }
        }
    }
    defs
}

fn rvalue_defs(rvalue: &Rvalue, out: &mut Vec<DefId>) {
    match rvalue {
        Rvalue::Use(operand) | Rvalue::Cast { operand, .. } => operand_defs(operand, out),
        Rvalue::BinaryOp { lhs, rhs, .. } => {
            operand_defs(lhs, out);
            operand_defs(rhs, out);
        }
        Rvalue::UnaryOp { operand, .. } => operand_defs(operand, out),
        Rvalue::Aggregate(kind, operands) => {
            if let AggregateKind::Closure(def) = kind {
                out.push(*def);
            }
            for operand in operands {
                operand_defs(operand, out);
            }
        }
        Rvalue::Read(_)
        | Rvalue::Ref { .. }
        | Rvalue::AddressOf { .. }
        | Rvalue::Discriminant(_)
        | Rvalue::Len(_) => {}
    }
}

fn operand_defs(operand: &Operand, out: &mut Vec<DefId>) {
    if let Operand::Def(def) = operand {
        out.push(*def);
    }
}

fn const_defs(item: &MirConstItem) -> Vec<DefId> {
    let mut defs = Vec::new();
    const_value_defs(&item.value, &mut defs);
    defs
}

fn const_value_defs(value: &MirConst, out: &mut Vec<DefId>) {
    match value {
        MirConst::Tuple(items) | MirConst::Array(items) => {
            for item in items {
                const_value_defs(item, out);
            }
        }
        MirConst::Struct { def, fields } => {
            out.push(*def);
            for field in fields {
                const_value_defs(field, out);
            }
        }
        MirConst::Ref(inner) => const_value_defs(inner, out),
        MirConst::Bool(_)
        | MirConst::Int(_)
        | MirConst::Uint(_)
        | MirConst::Float(_)
        | MirConst::Char(_)
        | MirConst::Str(_)
        | MirConst::Unit
        | MirConst::Undef => {}
    }
}
