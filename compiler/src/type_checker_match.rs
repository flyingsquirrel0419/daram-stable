use crate::{
    diagnostics::Diagnostic,
    hir::{
        DefId, HirArm, HirBinOp, HirExpr, HirExprKind, HirLit, HirPattern, HirPatternKind,
        HirUnaryOp, HirVariant, Ty,
    },
};
use std::collections::{HashMap, HashSet};

#[derive(Default, Clone)]
struct MatchCoverage {
    catch_all: bool,
    bool_true: bool,
    bool_false: bool,
    unit: bool,
    enum_variants: HashSet<DefId>,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
enum FiniteSpace {
    Bool(bool),
    Int(i128),
    Unit,
    Tuple(Vec<FiniteSpace>),
    Array(Vec<FiniteSpace>),
    Struct {
        def: DefId,
        fields: Vec<(String, FiniteSpace)>,
    },
    Variant {
        def: DefId,
        args: Vec<FiniteSpace>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum GuardValue {
    Bool(bool),
    Unit,
    Int(i128),
}

pub(crate) struct MatchContext<'a> {
    pub def_names: &'a HashMap<DefId, String>,
    pub struct_fields: &'a HashMap<DefId, HashMap<String, Ty>>,
    pub enum_variants: &'a HashMap<DefId, Vec<HirVariant>>,
    pub resolve_ty: &'a dyn Fn(&Ty) -> Ty,
}

impl MatchContext<'_> {
    fn coverage_for_pattern(&self, pattern: &HirPattern) -> MatchCoverage {
        match &pattern.kind {
            HirPatternKind::Wildcard | HirPatternKind::Binding { .. } => MatchCoverage {
                catch_all: true,
                ..MatchCoverage::default()
            },
            HirPatternKind::Lit(HirLit::Bool(value)) => MatchCoverage {
                bool_true: *value,
                bool_false: !value,
                ..MatchCoverage::default()
            },
            HirPatternKind::Lit(HirLit::Unit) => MatchCoverage {
                unit: true,
                ..MatchCoverage::default()
            },
            HirPatternKind::Variant { def, .. } => {
                let mut coverage = MatchCoverage::default();
                coverage.enum_variants.insert(*def);
                coverage
            }
            HirPatternKind::Or(alternatives) => {
                let mut coverage = MatchCoverage::default();
                for alternative in alternatives {
                    let next = self.coverage_for_pattern(alternative);
                    coverage.catch_all |= next.catch_all;
                    coverage.bool_true |= next.bool_true;
                    coverage.bool_false |= next.bool_false;
                    coverage.unit |= next.unit;
                    coverage.enum_variants.extend(next.enum_variants);
                }
                coverage
            }
            HirPatternKind::Ref { inner, .. } => self.coverage_for_pattern(inner),
            HirPatternKind::Lit(_)
            | HirPatternKind::Tuple(_)
            | HirPatternKind::Struct { .. }
            | HirPatternKind::Range { .. }
            | HirPatternKind::Slice { .. } => MatchCoverage::default(),
        }
    }

    fn int_domain_bounds(&self, ty: &Ty) -> Option<(i128, i128)> {
        match (self.resolve_ty)(ty) {
            Ty::Int(size) => Some(match size {
                crate::hir::IntSize::I8 => (i8::MIN as i128, i8::MAX as i128),
                crate::hir::IntSize::I16 => (i16::MIN as i128, i16::MAX as i128),
                crate::hir::IntSize::I32 => (i32::MIN as i128, i32::MAX as i128),
                crate::hir::IntSize::I64 => (i64::MIN as i128, i64::MAX as i128),
                crate::hir::IntSize::I128 => (i128::MIN, i128::MAX),
                crate::hir::IntSize::ISize => (isize::MIN as i128, isize::MAX as i128),
            }),
            Ty::Uint(size) => Some(match size {
                crate::hir::UintSize::U8 => (0, u8::MAX as i128),
                crate::hir::UintSize::U16 => (0, u16::MAX as i128),
                crate::hir::UintSize::U32 => (0, u32::MAX as i128),
                crate::hir::UintSize::U64 => (0, u64::MAX as i128),
                crate::hir::UintSize::U128 => (0, i128::MAX),
                crate::hir::UintSize::USize => (0, usize::MAX as i128),
            }),
            _ => None,
        }
    }

    fn int_pattern_intervals(&self, pattern: &HirPattern) -> Option<Vec<(i128, i128)>> {
        match &pattern.kind {
            HirPatternKind::Wildcard | HirPatternKind::Binding { .. } => {
                Some(vec![(i128::MIN, i128::MAX)])
            }
            HirPatternKind::Lit(HirLit::Integer(value)) => Some(vec![(*value, *value)]),
            HirPatternKind::Lit(HirLit::Uint(value)) => i128::try_from(*value)
                .ok()
                .map(|value| vec![(value, value)]),
            HirPatternKind::Range { lo, hi, inclusive } => {
                let lo = match &lo.kind {
                    HirPatternKind::Lit(HirLit::Integer(value)) => *value,
                    HirPatternKind::Lit(HirLit::Uint(value)) => i128::try_from(*value).ok()?,
                    _ => return None,
                };
                let hi = match &hi.kind {
                    HirPatternKind::Lit(HirLit::Integer(value)) => *value,
                    HirPatternKind::Lit(HirLit::Uint(value)) => i128::try_from(*value).ok()?,
                    _ => return None,
                };
                let end = if *inclusive { hi } else { hi.checked_sub(1)? };
                Some(vec![(lo, end)])
            }
            HirPatternKind::Or(patterns) => {
                let mut all = Vec::new();
                for pattern in patterns {
                    all.extend(self.int_pattern_intervals(pattern)?);
                }
                Some(all)
            }
            HirPatternKind::Ref { inner, .. } => self.int_pattern_intervals(inner),
            _ => None,
        }
    }

    fn normalize_intervals(
        &self,
        mut intervals: Vec<(i128, i128)>,
        domain: (i128, i128),
    ) -> Vec<(i128, i128)> {
        let (domain_lo, domain_hi) = domain;
        intervals = intervals
            .into_iter()
            .filter_map(|(lo, hi)| {
                let lo = lo.max(domain_lo);
                let hi = hi.min(domain_hi);
                (lo <= hi).then_some((lo, hi))
            })
            .collect();
        intervals.sort_by_key(|(lo, _)| *lo);
        let mut merged: Vec<(i128, i128)> = Vec::new();
        for (lo, hi) in intervals {
            if let Some((_, last_hi)) = merged.last_mut() {
                if lo <= last_hi.saturating_add(1) {
                    *last_hi = (*last_hi).max(hi);
                    continue;
                }
            }
            merged.push((lo, hi));
        }
        merged
    }

    fn intersect_intervals(
        &self,
        lhs: Vec<(i128, i128)>,
        rhs: Vec<(i128, i128)>,
        domain: (i128, i128),
    ) -> Vec<(i128, i128)> {
        let lhs = self.normalize_intervals(lhs, domain);
        let rhs = self.normalize_intervals(rhs, domain);
        let mut result = Vec::new();
        for (lhs_lo, lhs_hi) in &lhs {
            for (rhs_lo, rhs_hi) in &rhs {
                let lo = (*lhs_lo).max(*rhs_lo);
                let hi = (*lhs_hi).min(*rhs_hi);
                if lo <= hi {
                    result.push((lo, hi));
                }
            }
        }
        self.normalize_intervals(result, domain)
    }

    fn single_binding_in_pattern(&self, pattern: &HirPattern) -> Option<crate::hir::HirId> {
        fn collect(
            pattern: &HirPattern,
            seen: &mut Option<crate::hir::HirId>,
            ambiguous: &mut bool,
        ) {
            match &pattern.kind {
                HirPatternKind::Binding { id, .. } => match seen {
                    Some(existing) if *existing != *id => *ambiguous = true,
                    Some(_) => {}
                    None => *seen = Some(*id),
                },
                HirPatternKind::Tuple(patterns) | HirPatternKind::Or(patterns) => {
                    for pattern in patterns {
                        collect(pattern, seen, ambiguous);
                    }
                }
                HirPatternKind::Struct { fields, .. } => {
                    for (_, pattern) in fields {
                        collect(pattern, seen, ambiguous);
                    }
                }
                HirPatternKind::Variant { args, .. }
                | HirPatternKind::Slice { elems: args, .. } => {
                    for pattern in args {
                        collect(pattern, seen, ambiguous);
                    }
                }
                HirPatternKind::Range { lo, hi, .. } => {
                    collect(lo, seen, ambiguous);
                    collect(hi, seen, ambiguous);
                }
                HirPatternKind::Ref { inner, .. } => collect(inner, seen, ambiguous),
                HirPatternKind::Wildcard | HirPatternKind::Lit(_) => {}
            }
        }

        let mut seen = None;
        let mut ambiguous = false;
        collect(pattern, &mut seen, &mut ambiguous);
        (!ambiguous).then_some(seen).flatten()
    }

    fn int_constant_expr(&self, expr: &HirExpr) -> Option<i128> {
        match self.eval_guard_value(expr, &HashMap::new())? {
            GuardValue::Int(value) => Some(value),
            _ => None,
        }
    }

    fn guard_var_id(&self, expr: &HirExpr) -> Option<crate::hir::HirId> {
        match &expr.kind {
            HirExprKind::Var(id) => Some(*id),
            HirExprKind::Cast { expr, .. } => self.guard_var_id(expr),
            _ => None,
        }
    }

    fn swap_comparison(op: HirBinOp) -> HirBinOp {
        use HirBinOp::*;
        match op {
            Lt => Gt,
            Le => Ge,
            Gt => Lt,
            Ge => Le,
            other => other,
        }
    }

    fn comparison_intervals(
        &self,
        op: HirBinOp,
        value: i128,
        domain: (i128, i128),
    ) -> Vec<(i128, i128)> {
        use HirBinOp::*;
        let (domain_lo, domain_hi) = domain;
        let intervals = match op {
            Eq => vec![(value, value)],
            Ne => vec![
                (domain_lo, value.saturating_sub(1)),
                (value.saturating_add(1), domain_hi),
            ],
            Lt => vec![(domain_lo, value.saturating_sub(1))],
            Le => vec![(domain_lo, value)],
            Gt => vec![(value.saturating_add(1), domain_hi)],
            Ge => vec![(value, domain_hi)],
            _ => return Vec::new(),
        };
        self.normalize_intervals(intervals, domain)
    }

    fn guard_intervals_for_binding(
        &self,
        expr: &HirExpr,
        binding: crate::hir::HirId,
        domain: (i128, i128),
    ) -> Option<Vec<(i128, i128)>> {
        match &expr.kind {
            HirExprKind::UnaryOp {
                op: HirUnaryOp::Not,
                ..
            } => None,
            HirExprKind::BinOp { op, lhs, rhs } => match op {
                HirBinOp::And => Some(self.intersect_intervals(
                    self.guard_intervals_for_binding(lhs, binding, domain)?,
                    self.guard_intervals_for_binding(rhs, binding, domain)?,
                    domain,
                )),
                HirBinOp::Or => {
                    let mut intervals = self.guard_intervals_for_binding(lhs, binding, domain)?;
                    intervals.extend(self.guard_intervals_for_binding(rhs, binding, domain)?);
                    Some(self.normalize_intervals(intervals, domain))
                }
                HirBinOp::Eq
                | HirBinOp::Ne
                | HirBinOp::Lt
                | HirBinOp::Le
                | HirBinOp::Gt
                | HirBinOp::Ge => {
                    if self.guard_var_id(lhs) == Some(binding) {
                        Some(self.comparison_intervals(*op, self.int_constant_expr(rhs)?, domain))
                    } else if self.guard_var_id(rhs) == Some(binding) {
                        Some(self.comparison_intervals(
                            Self::swap_comparison(*op),
                            self.int_constant_expr(lhs)?,
                            domain,
                        ))
                    } else {
                        None
                    }
                }
                _ => None,
            },
            _ => None,
        }
    }

    fn arm_int_intervals(&self, arm: &HirArm, domain: (i128, i128)) -> Option<Vec<(i128, i128)>> {
        let pattern_intervals =
            self.normalize_intervals(self.int_pattern_intervals(&arm.pattern)?, domain);
        match &arm.guard {
            None => Some(pattern_intervals),
            Some(guard) => {
                let binding = self.single_binding_in_pattern(&arm.pattern)?;
                let guard_intervals = self.guard_intervals_for_binding(guard, binding, domain)?;
                Some(self.intersect_intervals(pattern_intervals, guard_intervals, domain))
            }
        }
    }

    fn enumerate_finite_spaces(&self, ty: &Ty, budget: &mut usize) -> Option<Vec<FiniteSpace>> {
        if *budget == 0 {
            return None;
        }

        match (self.resolve_ty)(ty) {
            Ty::Bool => {
                *budget = budget.saturating_sub(2);
                Some(vec![FiniteSpace::Bool(true), FiniteSpace::Bool(false)])
            }
            Ty::Int(size) => {
                let values = match size {
                    crate::hir::IntSize::I8 => (i8::MIN as i128..=i8::MAX as i128)
                        .map(FiniteSpace::Int)
                        .collect::<Vec<_>>(),
                    _ => return None,
                };
                if values.len() > *budget {
                    return None;
                }
                *budget = budget.saturating_sub(values.len());
                Some(values)
            }
            Ty::Uint(size) => {
                let values = match size {
                    crate::hir::UintSize::U8 => (u8::MIN as i128..=u8::MAX as i128)
                        .map(FiniteSpace::Int)
                        .collect::<Vec<_>>(),
                    _ => return None,
                };
                if values.len() > *budget {
                    return None;
                }
                *budget = budget.saturating_sub(values.len());
                Some(values)
            }
            Ty::Unit => {
                *budget = budget.saturating_sub(1);
                Some(vec![FiniteSpace::Unit])
            }
            Ty::Tuple(elems) => {
                let mut spaces = vec![Vec::new()];
                for elem_ty in elems {
                    let elem_spaces = self.enumerate_finite_spaces(&elem_ty, budget)?;
                    spaces = self.cartesian_extend(spaces, &elem_spaces, budget)?;
                }
                Some(spaces.into_iter().map(FiniteSpace::Tuple).collect())
            }
            Ty::Array { elem, len } => {
                let mut spaces = vec![Vec::new()];
                for _ in 0..len {
                    let elem_spaces = self.enumerate_finite_spaces(&elem, budget)?;
                    spaces = self.cartesian_extend(spaces, &elem_spaces, budget)?;
                }
                Some(spaces.into_iter().map(FiniteSpace::Array).collect())
            }
            Ty::Named { def, .. } => {
                if let Some(fields) = self.struct_fields.get(&def) {
                    let mut ordered_fields = fields
                        .iter()
                        .map(|(name, ty)| (name.clone(), ty.clone()))
                        .collect::<Vec<_>>();
                    ordered_fields.sort_by(|left, right| left.0.cmp(&right.0));

                    let mut spaces = vec![Vec::new()];
                    for (name, field_ty) in ordered_fields {
                        let field_spaces = self.enumerate_finite_spaces(&field_ty, budget)?;
                        let mut next = Vec::new();
                        for prefix in spaces {
                            for field_space in &field_spaces {
                                if *budget == 0 {
                                    return None;
                                }
                                *budget = budget.saturating_sub(1);
                                let mut tuple = prefix.clone();
                                tuple.push((name.clone(), field_space.clone()));
                                next.push(tuple);
                            }
                        }
                        spaces = next;
                    }

                    return Some(
                        spaces
                            .into_iter()
                            .map(|fields| FiniteSpace::Struct { def, fields })
                            .collect(),
                    );
                }

                let variants = self.enum_variants.get(&def)?.clone();
                let mut spaces = Vec::new();
                for variant in variants {
                    let mut products = vec![Vec::new()];
                    for field_ty in variant.fields {
                        let field_spaces = self.enumerate_finite_spaces(&field_ty, budget)?;
                        products = self.cartesian_extend(products, &field_spaces, budget)?;
                    }
                    for args in products {
                        if *budget == 0 {
                            return None;
                        }
                        *budget = budget.saturating_sub(1);
                        spaces.push(FiniteSpace::Variant {
                            def: variant.def,
                            args,
                        });
                    }
                }
                Some(spaces)
            }
            _ => None,
        }
    }

    fn cartesian_extend(
        &self,
        prefixes: Vec<Vec<FiniteSpace>>,
        elems: &[FiniteSpace],
        budget: &mut usize,
    ) -> Option<Vec<Vec<FiniteSpace>>> {
        let mut next = Vec::new();
        for prefix in prefixes {
            for elem in elems {
                if *budget == 0 {
                    return None;
                }
                *budget = budget.saturating_sub(1);
                let mut tuple = prefix.clone();
                tuple.push(elem.clone());
                next.push(tuple);
            }
        }
        Some(next)
    }

    fn render_space(&self, space: &FiniteSpace) -> String {
        match space {
            FiniteSpace::Bool(value) => format!("`{value}`"),
            FiniteSpace::Int(value) => format!("`{value}`"),
            FiniteSpace::Unit => "`()`".to_string(),
            FiniteSpace::Tuple(values) => {
                let inner = values
                    .iter()
                    .map(|value| self.render_space(value).trim_matches('`').to_string())
                    .collect::<Vec<_>>()
                    .join(", ");
                format!("`({inner})`")
            }
            FiniteSpace::Array(values) => {
                let inner = values
                    .iter()
                    .map(|value| self.render_space(value).trim_matches('`').to_string())
                    .collect::<Vec<_>>()
                    .join(", ");
                format!("`[{inner}]`")
            }
            FiniteSpace::Struct { def, fields } => {
                let struct_name = self
                    .def_names
                    .get(def)
                    .cloned()
                    .unwrap_or_else(|| format!("struct#{}", def.index));
                let rendered = fields
                    .iter()
                    .map(|(name, value)| {
                        format!("{}: {}", name, self.render_space(value).trim_matches('`'))
                    })
                    .collect::<Vec<_>>()
                    .join(", ");
                format!("`{struct_name} {{ {rendered} }}`")
            }
            FiniteSpace::Variant { def, args } => {
                let variant_name = self
                    .enum_variants
                    .values()
                    .flat_map(|variants| variants.iter())
                    .find(|variant| variant.def == *def)
                    .map(|variant| variant.name.clone())
                    .unwrap_or_else(|| format!("variant#{}", def.index));
                if args.is_empty() {
                    format!("`{variant_name}`")
                } else {
                    let rendered = args
                        .iter()
                        .map(|value| self.render_space(value).trim_matches('`').to_string())
                        .collect::<Vec<_>>()
                        .join(", ");
                    format!("`{variant_name}({rendered})`")
                }
            }
        }
    }

    fn bind_space(
        &self,
        pattern: &HirPattern,
        space: &FiniteSpace,
        bindings: &mut HashMap<crate::hir::HirId, GuardValue>,
    ) -> bool {
        match (&pattern.kind, space) {
            (HirPatternKind::Wildcard, _) => true,
            (HirPatternKind::Binding { id, .. }, value) => {
                if let Some(bound) = Self::guard_value_from_space(value) {
                    bindings.insert(*id, bound);
                }
                true
            }
            (HirPatternKind::Lit(HirLit::Bool(lhs)), FiniteSpace::Bool(rhs)) => lhs == rhs,
            (HirPatternKind::Lit(HirLit::Integer(lhs)), FiniteSpace::Int(rhs)) => lhs == rhs,
            (HirPatternKind::Lit(HirLit::Uint(lhs)), FiniteSpace::Int(rhs)) => {
                i128::try_from(*lhs).ok() == Some(*rhs)
            }
            (HirPatternKind::Lit(HirLit::Unit), FiniteSpace::Unit) => true,
            (HirPatternKind::Range { .. }, FiniteSpace::Int(value)) => {
                let Some(intervals) = self.int_pattern_intervals(pattern) else {
                    return false;
                };
                intervals
                    .into_iter()
                    .any(|(start, end)| *value >= start && *value <= end)
            }
            (HirPatternKind::Tuple(patterns), FiniteSpace::Tuple(values)) => {
                patterns.len() == values.len()
                    && patterns
                        .iter()
                        .zip(values)
                        .all(|(pattern, value)| self.bind_space(pattern, value, bindings))
            }
            (HirPatternKind::Slice { elems, rest_index }, FiniteSpace::Array(values)) => {
                if let Some(rest_index) = *rest_index {
                    if values.len() < elems.len() {
                        return false;
                    }
                    let suffix_len = elems.len().saturating_sub(rest_index);
                    elems[..rest_index.min(elems.len())]
                        .iter()
                        .zip(values.iter())
                        .all(|(pattern, value)| self.bind_space(pattern, value, bindings))
                        && elems
                            .iter()
                            .skip(rest_index)
                            .rev()
                            .zip(values.iter().rev())
                            .all(|(pattern, value)| self.bind_space(pattern, value, bindings))
                        && suffix_len <= values.len()
                } else {
                    elems.len() == values.len()
                        && elems
                            .iter()
                            .zip(values)
                            .all(|(pattern, value)| self.bind_space(pattern, value, bindings))
                }
            }
            (
                HirPatternKind::Struct { def, fields, rest },
                FiniteSpace::Struct {
                    def: space_def,
                    fields: space_fields,
                },
            ) => {
                if def != space_def {
                    return false;
                }
                if !*rest && fields.len() != space_fields.len() {
                    return false;
                }
                fields.iter().all(|(field_name, pattern)| {
                    space_fields
                        .iter()
                        .find(|(name, _)| name == field_name)
                        .map(|(_, value)| self.bind_space(pattern, value, bindings))
                        .unwrap_or(false)
                })
            }
            (
                HirPatternKind::Variant { def, args },
                FiniteSpace::Variant {
                    def: space_def,
                    args: space_args,
                },
            ) => {
                def == space_def
                    && args.len() == space_args.len()
                    && args
                        .iter()
                        .zip(space_args)
                        .all(|(pattern, value)| self.bind_space(pattern, value, bindings))
            }
            (HirPatternKind::Or(patterns), _) => patterns.iter().any(|pattern| {
                let mut nested = bindings.clone();
                let matched = self.bind_space(pattern, space, &mut nested);
                if matched {
                    *bindings = nested;
                }
                matched
            }),
            (HirPatternKind::Ref { inner, .. }, _) => self.bind_space(inner, space, bindings),
            _ => false,
        }
    }

    fn guard_value_from_space(space: &FiniteSpace) -> Option<GuardValue> {
        match space {
            FiniteSpace::Bool(value) => Some(GuardValue::Bool(*value)),
            FiniteSpace::Int(value) => Some(GuardValue::Int(*value)),
            FiniteSpace::Unit => Some(GuardValue::Unit),
            FiniteSpace::Tuple(_)
            | FiniteSpace::Array(_)
            | FiniteSpace::Struct { .. }
            | FiniteSpace::Variant { .. } => None,
        }
    }

    fn eval_guard_value(
        &self,
        expr: &HirExpr,
        bindings: &HashMap<crate::hir::HirId, GuardValue>,
    ) -> Option<GuardValue> {
        match &expr.kind {
            HirExprKind::Lit(HirLit::Bool(value)) => Some(GuardValue::Bool(*value)),
            HirExprKind::Lit(HirLit::Unit) => Some(GuardValue::Unit),
            HirExprKind::Lit(HirLit::Integer(value)) => Some(GuardValue::Int(*value)),
            HirExprKind::Lit(HirLit::Uint(value)) => {
                i128::try_from(*value).ok().map(GuardValue::Int)
            }
            HirExprKind::Var(id) => bindings.get(id).cloned(),
            HirExprKind::UnaryOp {
                op: HirUnaryOp::Not,
                operand,
            } => match self.eval_guard_value(operand, bindings)? {
                GuardValue::Bool(value) => Some(GuardValue::Bool(!value)),
                _ => None,
            },
            HirExprKind::Cast { expr, target_ty } => {
                let value = self.eval_guard_value(expr, bindings)?;
                match (value, (self.resolve_ty)(target_ty)) {
                    (GuardValue::Bool(value), Ty::Bool) => Some(GuardValue::Bool(value)),
                    (GuardValue::Unit, Ty::Unit) => Some(GuardValue::Unit),
                    (GuardValue::Int(value), Ty::Int(_))
                    | (GuardValue::Int(value), Ty::Uint(_))
                    | (GuardValue::Int(value), Ty::Char) => Some(GuardValue::Int(value)),
                    _ => None,
                }
            }
            HirExprKind::BinOp { op, lhs, rhs } => {
                let lhs = self.eval_guard_value(lhs, bindings)?;
                let rhs = self.eval_guard_value(rhs, bindings)?;
                match op {
                    HirBinOp::And => match (lhs, rhs) {
                        (GuardValue::Bool(lhs), GuardValue::Bool(rhs)) => {
                            Some(GuardValue::Bool(lhs && rhs))
                        }
                        _ => None,
                    },
                    HirBinOp::Or => match (lhs, rhs) {
                        (GuardValue::Bool(lhs), GuardValue::Bool(rhs)) => {
                            Some(GuardValue::Bool(lhs || rhs))
                        }
                        _ => None,
                    },
                    HirBinOp::Eq => Some(GuardValue::Bool(lhs == rhs)),
                    HirBinOp::Ne => Some(GuardValue::Bool(lhs != rhs)),
                    HirBinOp::Lt => match (lhs, rhs) {
                        (GuardValue::Int(lhs), GuardValue::Int(rhs)) => {
                            Some(GuardValue::Bool(lhs < rhs))
                        }
                        _ => None,
                    },
                    HirBinOp::Le => match (lhs, rhs) {
                        (GuardValue::Int(lhs), GuardValue::Int(rhs)) => {
                            Some(GuardValue::Bool(lhs <= rhs))
                        }
                        _ => None,
                    },
                    HirBinOp::Gt => match (lhs, rhs) {
                        (GuardValue::Int(lhs), GuardValue::Int(rhs)) => {
                            Some(GuardValue::Bool(lhs > rhs))
                        }
                        _ => None,
                    },
                    HirBinOp::Ge => match (lhs, rhs) {
                        (GuardValue::Int(lhs), GuardValue::Int(rhs)) => {
                            Some(GuardValue::Bool(lhs >= rhs))
                        }
                        _ => None,
                    },
                    _ => None,
                }
            }
            _ => None,
        }
    }

    fn guard_matches_space(&self, arm: &HirArm, space: &FiniteSpace) -> bool {
        let mut bindings = HashMap::new();
        if !self.bind_space(&arm.pattern, space, &mut bindings) {
            return false;
        }
        match &arm.guard {
            Some(guard) => {
                matches!(
                    self.eval_guard_value(guard, &bindings),
                    Some(GuardValue::Bool(true))
                )
            }
            None => true,
        }
    }
}

pub(crate) fn check_match_exhaustiveness(
    ctx: &MatchContext<'_>,
    match_expr: &HirExpr,
    scrutinee_ty: &Ty,
    arms: &[HirArm],
    errors: &mut Vec<Diagnostic>,
) {
    let resolved_scrutinee_ty = (ctx.resolve_ty)(scrutinee_ty);
    let mut finite_budget = 256;
    if let Some(spaces) = ctx.enumerate_finite_spaces(&resolved_scrutinee_ty, &mut finite_budget) {
        let missing = spaces
            .iter()
            .filter(|space| !arms.iter().any(|arm| ctx.guard_matches_space(arm, space)))
            .map(|space| ctx.render_space(space))
            .collect::<Vec<_>>();

        if !missing.is_empty() {
            errors.push(
                Diagnostic::error(format!(
                    "non-exhaustive match: missing {}",
                    missing.join(", ")
                ))
                .with_span(match_expr.span),
            );
        }
        return;
    }

    if let Some((domain_lo, domain_hi)) = ctx.int_domain_bounds(&resolved_scrutinee_ty) {
        let domain = (domain_lo, domain_hi);
        let mut intervals = Vec::new();
        let mut has_catch_all = false;
        let mut unsupported_guard = false;
        for arm in arms {
            let Some(arm_intervals) = ctx.arm_int_intervals(arm, domain) else {
                if arm.guard.is_some() {
                    unsupported_guard = true;
                }
                continue;
            };
            if arm_intervals
                .iter()
                .any(|(lo, hi)| *lo <= domain_lo && *hi >= domain_hi)
            {
                has_catch_all = true;
                break;
            }
            intervals.extend(arm_intervals);
        }
        if has_catch_all || unsupported_guard {
            return;
        }
        if !intervals.is_empty() {
            intervals = ctx.normalize_intervals(intervals, domain);
            let mut cursor = domain_lo;
            for (lo, hi) in intervals {
                if hi < cursor {
                    continue;
                }
                if lo > cursor {
                    errors.push(
                        Diagnostic::error(format!(
                            "non-exhaustive match: missing values like `{cursor}`"
                        ))
                        .with_span(match_expr.span),
                    );
                    return;
                }
                cursor = hi.saturating_add(1);
                if cursor > domain_hi {
                    return;
                }
            }
            if cursor <= domain_hi {
                errors.push(
                    Diagnostic::error(format!(
                        "non-exhaustive match: missing values like `{cursor}`"
                    ))
                    .with_span(match_expr.span),
                );
                return;
            }
        }
    }

    let mut coverage = MatchCoverage::default();
    for arm in arms {
        if arm.guard.is_some() {
            continue;
        }
        let arm_coverage = ctx.coverage_for_pattern(&arm.pattern);
        coverage.catch_all |= arm_coverage.catch_all;
        coverage.bool_true |= arm_coverage.bool_true;
        coverage.bool_false |= arm_coverage.bool_false;
        coverage.unit |= arm_coverage.unit;
        coverage.enum_variants.extend(arm_coverage.enum_variants);
    }

    if coverage.catch_all {
        return;
    }

    let missing = match resolved_scrutinee_ty {
        Ty::Bool if !(coverage.bool_true && coverage.bool_false) => {
            let mut parts = Vec::new();
            if !coverage.bool_true {
                parts.push("`true`");
            }
            if !coverage.bool_false {
                parts.push("`false`");
            }
            Some(parts.join(" and "))
        }
        Ty::Unit if !coverage.unit => Some("`()".to_string()),
        Ty::Named { def, .. } => ctx.enum_variants.get(&def).and_then(|variants| {
            let missing = variants
                .iter()
                .filter(|variant| !coverage.enum_variants.contains(&variant.def))
                .map(|variant| format!("`{}`", variant.name))
                .collect::<Vec<_>>();
            (!missing.is_empty()).then(|| missing.join(", "))
        }),
        _ => None,
    };

    if let Some(missing) = missing {
        errors.push(
            Diagnostic::error(format!("non-exhaustive match: missing {missing}"))
                .with_span(match_expr.span),
        );
    }
}
