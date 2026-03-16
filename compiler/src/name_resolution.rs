//! Name resolution pass.
//!
//! This pass takes the AST and resolves all names to their definition sites,
//! producing a HIR module. It handles:
//! - Variable scoping
//! - Function / struct / enum / trait definitions
//! - `use` declarations
//! - Module paths
//! - Generics (basic)
//! - `self` inside `impl` blocks

use crate::{
    ast::{self, Expr, Item, Module, Pattern, Stmt},
    builtin_catalog,
    diagnostics::Diagnostic,
    hir::{
        DefId, HirAbility, HirArm, HirAssocItem, HirBinOp, HirConst, HirEnum, HirExpr, HirExprKind,
        HirExternFn, HirField, HirFn, HirGenericParam, HirId, HirIdAlloc, HirImpl, HirImplItem,
        HirInterface, HirLit, HirMod, HirModule, HirParam, HirPattern, HirPatternKind, HirStatic,
        HirStmt, HirStmtKind, HirStruct, HirTrait, HirTypeAlias, HirUnaryOp, HirUse, HirUseKind,
        HirUseTree, HirVariant, Ty,
    },
    source::{FileId, Span},
};
use std::collections::{HashMap, HashSet};

// ─── Resolver ─────────────────────────────────────────────────────────────────

struct Resolver {
    file: FileId,
    alloc: HirIdAlloc,
    errors: Vec<Diagnostic>,
    /// Global definition table for this file: name → DefId.
    defs: HashMap<String, DefId>,
    def_names: HashMap<DefId, String>,
    const_exprs: HashMap<String, ast::Expr>,
    variant_parents: HashMap<DefId, DefId>,
    variant_defs: HashSet<DefId>,
    default_fn_wrappers: HashMap<(DefId, usize), DefId>,
    module_path: Vec<String>,
    type_scopes: Vec<HashMap<String, Ty>>,
    def_counter: u32,
}

impl Resolver {
    fn new(file: FileId) -> Self {
        let mut resolver = Self {
            file,
            alloc: HirIdAlloc::default(),
            errors: Vec::new(),
            defs: HashMap::new(),
            def_names: HashMap::new(),
            const_exprs: HashMap::new(),
            variant_parents: HashMap::new(),
            variant_defs: HashSet::new(),
            default_fn_wrappers: HashMap::new(),
            module_path: Vec::new(),
            type_scopes: Vec::new(),
            def_counter: 0,
        };
        for builtin in builtin_catalog::all_builtin_names() {
            resolver.fresh_def(builtin);
        }
        resolver
    }

    fn fresh_id(&mut self) -> HirId {
        self.alloc.fresh()
    }

    fn fresh_def(&mut self, name: &str) -> DefId {
        let id = DefId {
            file: self.file,
            index: self.def_counter,
        };
        self.def_counter += 1;
        self.defs.insert(name.to_string(), id);
        self.def_names.entry(id).or_insert_with(|| name.to_string());
        id
    }

    fn lookup_or_define(&mut self, name: &str) -> DefId {
        self.defs
            .get(name)
            .copied()
            .unwrap_or_else(|| self.fresh_def(name))
    }

    fn fresh_ty_var(&mut self) -> Ty {
        Ty::Var(self.fresh_id().0)
    }

    fn qualify_name(&self, name: &str) -> String {
        if self.module_path.is_empty() {
            name.to_string()
        } else {
            format!("{}::{name}", self.module_path.join("::"))
        }
    }

    fn define_in_current_module(&mut self, name: &str) -> DefId {
        let qualified = self.qualify_name(name);
        let def = self
            .defs
            .get(&qualified)
            .copied()
            .unwrap_or_else(|| self.fresh_def(&qualified));
        self.bind_name_to_def(&qualified, def);
        self.bind_name_to_def(name, def);
        if !self.module_path.is_empty() {
            let relative = self
                .module_path
                .last()
                .map(|segment| format!("{segment}::{name}"));
            if let Some(relative) = relative {
                self.bind_name_to_def(&relative, def);
            }
        }
        def
    }

    fn predeclare_name(&mut self, name: &str, bind_local: bool) -> DefId {
        let qualified = self.qualify_name(name);
        let def = self
            .defs
            .get(&qualified)
            .copied()
            .unwrap_or_else(|| self.fresh_def(&qualified));
        self.bind_name_to_def(&qualified, def);
        if bind_local {
            self.bind_name_to_def(name, def);
            if !self.module_path.is_empty() {
                let relative = self
                    .module_path
                    .last()
                    .map(|segment| format!("{segment}::{name}"));
                if let Some(relative) = relative {
                    self.bind_name_to_def(&relative, def);
                }
            }
        }
        def
    }

    fn predeclare_items(&mut self, items: &[Item], bind_local: bool) {
        for item in items {
            match item {
                Item::Function(f) => {
                    self.predeclare_name(&f.name.name, bind_local);
                    self.register_default_fn_wrappers(f);
                }
                Item::Struct(s) => {
                    self.predeclare_name(&s.name.name, bind_local);
                    self.predeclare_derived_assoc_methods(&s.name.name, &s.derives);
                }
                Item::Enum(e) => {
                    let enum_def = self.predeclare_name(&e.name.name, bind_local);
                    self.predeclare_derived_assoc_methods(&e.name.name, &e.derives);
                    for variant in &e.variants {
                        let qualified =
                            format!("{}::{}", self.qualify_name(&e.name.name), variant.name.name);
                        let variant_def = self.fresh_def(&qualified);
                        self.variant_parents.insert(variant_def, enum_def);
                        self.variant_defs.insert(variant_def);
                        if bind_local && !self.defs.contains_key(&variant.name.name) {
                            self.bind_name_to_def(&variant.name.name, variant_def);
                        }
                        self.bind_name_to_def(&qualified, variant_def);
                    }
                }
                Item::Trait(t) => {
                    self.predeclare_name(&t.name.name, bind_local);
                }
                Item::Interface(i) => {
                    self.predeclare_name(&i.name.name, bind_local);
                }
                Item::TypeAlias(a) => {
                    self.predeclare_name(&a.name.name, bind_local);
                }
                Item::Ability(a) => {
                    self.predeclare_name(&a.name.name, bind_local);
                }
                Item::Const(c) => {
                    self.predeclare_name(&c.name.name, bind_local);
                    self.const_exprs
                        .insert(self.qualify_name(&c.name.name), c.value.clone());
                    if bind_local {
                        self.const_exprs
                            .insert(c.name.name.clone(), c.value.clone());
                    }
                }
                Item::Static(s) => {
                    self.predeclare_name(&s.name.name, bind_local);
                }
                Item::Module(m) => {
                    self.predeclare_name(&m.name.name, bind_local);
                    if let Some(items) = &m.body {
                        self.module_path.push(m.name.name.clone());
                        self.predeclare_items(items, false);
                        self.module_path.pop();
                    }
                }
                Item::ExternBlock(ext) => {
                    for f in &ext.functions {
                        self.predeclare_name(&f.name.name, bind_local);
                    }
                }
                Item::Use(_, _) | Item::Impl(_) => {}
            }
        }
    }

    fn predeclare_derived_assoc_methods(&mut self, owner_name: &str, derives: &[ast::Path]) {
        let owner_qualified = self.qualify_name(owner_name);
        for derive in derives {
            let Some(name) = derive.segments.last().map(|segment| segment.name.as_str()) else {
                continue;
            };
            let method_name = match name {
                "Clone" => Some("clone"),
                "PartialEq" => Some("eq"),
                "Debug" => Some("fmt"),
                "Default" => Some("default"),
                "Hash" => Some("hash"),
                _ => None,
            };
            let Some(method_name) = method_name else {
                continue;
            };
            self.lookup_or_define(&format!("{owner_qualified}::{method_name}"));
        }
    }

    fn has_trailing_default_params(params: &[ast::FnParam]) -> bool {
        let mut saw_default = false;
        for param in params {
            if param.default.is_some() {
                saw_default = true;
            } else if saw_default {
                return false;
            }
        }
        true
    }

    fn first_default_param_index(params: &[ast::FnParam]) -> Option<usize> {
        params.iter().position(|param| param.default.is_some())
    }

    fn can_synthesize_default_wrappers(params: &[ast::FnParam]) -> bool {
        Self::has_trailing_default_params(params)
            && params
                .iter()
                .all(|param| matches!(param.pattern, ast::Pattern::Ident { .. }))
    }

    fn register_default_fn_wrappers(&mut self, f: &ast::FnDef) {
        if !Self::can_synthesize_default_wrappers(&f.params) {
            return;
        }
        let Some(first_default) = Self::first_default_param_index(&f.params) else {
            return;
        };
        let base_def = self.define_in_current_module(&f.name.name);
        let qualified = self.qualify_name(&f.name.name);
        for arg_count in first_default..f.params.len() {
            let wrapper_name = format!("{qualified}::__default$arity{arg_count}");
            let wrapper_def = self.fresh_def(&wrapper_name);
            self.default_fn_wrappers
                .insert((base_def, arg_count), wrapper_def);
        }
    }

    fn default_wrapper_for_call(&self, def: DefId, arg_count: usize) -> Option<DefId> {
        self.default_fn_wrappers.get(&(def, arg_count)).copied()
    }

    fn push_type_scope(&mut self, names: impl IntoIterator<Item = String>) {
        let mut scope = HashMap::new();
        for name in names {
            scope.insert(name, self.fresh_ty_var());
        }
        self.type_scopes.push(scope);
    }

    fn pop_type_scope(&mut self) {
        self.type_scopes.pop();
    }

    fn push_generics(&mut self, generics: &ast::GenericParams) {
        self.push_type_scope(generics.params.iter().map(|param| param.name.name.clone()));
    }

    fn resolve_generic_params(&mut self, generics: &ast::GenericParams) -> Vec<HirGenericParam> {
        generics
            .params
            .iter()
            .map(|param| HirGenericParam {
                name: param.name.name.clone(),
                bounds: param
                    .bounds
                    .iter()
                    .map(|bound| self.resolve_type(bound))
                    .collect(),
                default: param
                    .default
                    .as_ref()
                    .map(|default| self.resolve_type(default)),
            })
            .collect()
    }

    fn lookup_type_scope(&self, name: &str) -> Option<Ty> {
        self.type_scopes
            .iter()
            .rev()
            .find_map(|scope| scope.get(name).cloned())
    }

    fn bind_name_to_def(&mut self, name: &str, def: DefId) {
        self.defs.insert(name.to_string(), def);
    }

    fn assoc_owner_name(&self, ty: &Ty) -> Option<String> {
        match ty {
            Ty::Named { def, .. } => self.def_names.get(def).cloned(),
            Ty::String => Some("std::core::String".to_string()),
            _ => None,
        }
    }

    fn import_def(
        &mut self,
        alias: String,
        def: DefId,
        changes: Option<&mut Vec<(String, Option<DefId>)>>,
    ) {
        let previous = self.defs.insert(alias.clone(), def);
        if let Some(changes) = changes {
            changes.push((alias, previous));
        }
    }

    fn restore_imports(&mut self, mut changes: Vec<(String, Option<DefId>)>) {
        while let Some((alias, previous)) = changes.pop() {
            if let Some(def) = previous {
                self.defs.insert(alias, def);
            } else {
                self.defs.remove(&alias);
            }
        }
    }

    fn lookup_def(&self, key: &str) -> Option<DefId> {
        self.defs
            .get(key)
            .copied()
            .or_else(|| {
                ["crate::", "self::", "super::"]
                    .iter()
                    .find_map(|prefix| key.strip_prefix(prefix))
                    .and_then(|stripped| self.defs.get(stripped).copied())
            })
            .or_else(|| {
                (!self.module_path.is_empty())
                    .then(|| format!("{}::{key}", self.module_path.join("::")))
                    .and_then(|qualified| self.defs.get(&qualified).copied())
            })
    }

    fn eval_const_int_expr(&mut self, expr: &ast::Expr, stack: &mut Vec<String>) -> Option<i128> {
        match expr {
            ast::Expr::Literal {
                lit: ast::Literal::Integer(value),
                ..
            } => i128::try_from(*value).ok(),
            ast::Expr::UnaryOp { op, operand, .. } => {
                let operand = self.eval_const_int_expr(operand, stack)?;
                match op {
                    ast::UnaryOp::Neg => operand.checked_neg(),
                    ast::UnaryOp::BitNot => Some(!operand),
                    ast::UnaryOp::Not => Some((operand == 0) as i128),
                }
            }
            ast::Expr::BinOp { op, lhs, rhs, span } => {
                let lhs = self.eval_const_int_expr(lhs, stack)?;
                let rhs = self.eval_const_int_expr(rhs, stack)?;
                match op {
                    ast::BinOp::Add => lhs.checked_add(rhs),
                    ast::BinOp::Sub => lhs.checked_sub(rhs),
                    ast::BinOp::Mul => lhs.checked_mul(rhs),
                    ast::BinOp::Div => {
                        if rhs == 0 {
                            self.errors.push(
                                Diagnostic::error("division by zero in const expression")
                                    .with_span(*span),
                            );
                            None
                        } else {
                            lhs.checked_div(rhs)
                        }
                    }
                    ast::BinOp::Rem => {
                        if rhs == 0 {
                            self.errors.push(
                                Diagnostic::error("remainder by zero in const expression")
                                    .with_span(*span),
                            );
                            None
                        } else {
                            lhs.checked_rem(rhs)
                        }
                    }
                    ast::BinOp::BitAnd => Some(lhs & rhs),
                    ast::BinOp::BitOr => Some(lhs | rhs),
                    ast::BinOp::BitXor => Some(lhs ^ rhs),
                    ast::BinOp::Shl => u32::try_from(rhs).ok().and_then(|rhs| lhs.checked_shl(rhs)),
                    ast::BinOp::Shr => u32::try_from(rhs).ok().and_then(|rhs| lhs.checked_shr(rhs)),
                    ast::BinOp::Eq => Some((lhs == rhs) as i128),
                    ast::BinOp::Ne => Some((lhs != rhs) as i128),
                    ast::BinOp::Lt => Some((lhs < rhs) as i128),
                    ast::BinOp::Le => Some((lhs <= rhs) as i128),
                    ast::BinOp::Gt => Some((lhs > rhs) as i128),
                    ast::BinOp::Ge => Some((lhs >= rhs) as i128),
                    ast::BinOp::And => Some(((lhs != 0) && (rhs != 0)) as i128),
                    ast::BinOp::Or => Some(((lhs != 0) || (rhs != 0)) as i128),
                }
            }
            ast::Expr::Path { path, .. } => {
                let key = Self::path_key(path);
                if stack.contains(&key) {
                    self.errors.push(
                        Diagnostic::error(format!("cyclic const expression involving `{key}`"))
                            .with_span(path.span),
                    );
                    return None;
                }
                let expr = self.const_exprs.get(&key)?.clone();
                stack.push(key);
                let value = self.eval_const_int_expr(&expr, stack);
                stack.pop();
                value
            }
            _ => None,
        }
    }

    fn eval_const_usize_expr(&mut self, expr: &ast::Expr) -> Option<usize> {
        let mut stack = Vec::new();
        let value = self.eval_const_int_expr(expr, &mut stack)?;
        usize::try_from(value).ok()
    }

    fn apply_use_tree(
        &mut self,
        tree: &ast::UseTree,
        parent_prefix: &[String],
        mut changes: Option<&mut Vec<(String, Option<DefId>)>>,
    ) {
        let mut prefix = parent_prefix.to_vec();
        prefix.extend(
            tree.prefix
                .segments
                .iter()
                .map(|segment| segment.name.clone()),
        );

        match &tree.kind {
            ast::UseTreeKind::Simple => {
                if let Some(def) = self.lookup_def(&prefix.join("::")) {
                    if let Some(alias) = prefix.last().cloned() {
                        self.import_def(alias, def, changes.as_deref_mut());
                    }
                }
            }
            ast::UseTreeKind::Alias(alias) => {
                if let Some(def) = self.lookup_def(&prefix.join("::")) {
                    self.import_def(alias.name.clone(), def, changes.as_deref_mut());
                }
            }
            ast::UseTreeKind::Glob => {}
            ast::UseTreeKind::Nested(children) => {
                for child in children {
                    self.apply_use_tree(child, &prefix, changes.as_deref_mut());
                }
            }
        }
    }

    fn resolve_type(&mut self, ty: &ast::TypeExpr) -> Ty {
        match ty {
            ast::TypeExpr::Named { path, generics, .. } => {
                let name = path
                    .segments
                    .iter()
                    .map(|s| s.name.as_str())
                    .collect::<Vec<_>>()
                    .join("::");
                let args: Vec<Ty> = generics.iter().map(|g| self.resolve_type(g)).collect();
                if let Some(ty) = self.lookup_type_scope(&name) {
                    return ty;
                }
                match name.as_str() {
                    "bool" => Ty::Bool,
                    "char" => Ty::Char,
                    "str" => Ty::Str,
                    "string" | "String" | "std::core::String" => Ty::String,
                    "i8" => Ty::Int(crate::hir::IntSize::I8),
                    "i16" => Ty::Int(crate::hir::IntSize::I16),
                    "i32" => Ty::Int(crate::hir::IntSize::I32),
                    "i64" => Ty::Int(crate::hir::IntSize::I64),
                    "i128" => Ty::Int(crate::hir::IntSize::I128),
                    "isize" => Ty::Int(crate::hir::IntSize::ISize),
                    "u8" => Ty::Uint(crate::hir::UintSize::U8),
                    "u16" => Ty::Uint(crate::hir::UintSize::U16),
                    "u32" => Ty::Uint(crate::hir::UintSize::U32),
                    "u64" => Ty::Uint(crate::hir::UintSize::U64),
                    "u128" => Ty::Uint(crate::hir::UintSize::U128),
                    "usize" => Ty::Uint(crate::hir::UintSize::USize),
                    "f32" => Ty::Float(crate::hir::FloatSize::F32),
                    "f64" => Ty::Float(crate::hir::FloatSize::F64),
                    "!" => Ty::Never,
                    "Self" => self
                        .lookup_type_scope("Self")
                        .unwrap_or_else(|| self.fresh_ty_var()),
                    _ => {
                        if let Some(def) = self.lookup_def(&name) {
                            Ty::Named { def, args }
                        } else if name.starts_with("Self::") {
                            self.fresh_ty_var()
                        } else {
                            // Unresolved: emit an error and return a fresh type var
                            let span = ty.span();
                            self.errors.push(
                                Diagnostic::error(format!(
                                    "cannot find type `{}` in this scope",
                                    name
                                ))
                                .with_span(span)
                                .with_note(
                                    "import the type or qualify it with its module path, for example `std::core::Option`",
                                )
                                .with_note(
                                    "if this is a local type, check that the file=module path matches the import source",
                                ),
                            );
                            self.fresh_ty_var()
                        }
                    }
                }
            }
            ast::TypeExpr::Ref { mutable, inner, .. } => Ty::Ref {
                mutable: *mutable,
                inner: Box::new(self.resolve_type(inner)),
            },
            ast::TypeExpr::Tuple { elems, .. } => {
                if elems.is_empty() {
                    return Ty::Unit;
                }
                Ty::Tuple(elems.iter().map(|t| self.resolve_type(t)).collect())
            }
            ast::TypeExpr::Slice { elem, .. } => Ty::Slice(Box::new(self.resolve_type(elem))),
            ast::TypeExpr::Array { elem, len, span } => {
                let len = self.eval_const_usize_expr(len).unwrap_or_else(|| {
                    self.errors.push(
                        Diagnostic::error("array length must be a compile-time integer expression")
                            .with_span(*span),
                    );
                    0
                });
                Ty::Array {
                    elem: Box::new(self.resolve_type(elem)),
                    len,
                }
            }
            ast::TypeExpr::FnPtr { params, ret, .. } => Ty::FnPtr {
                params: params.iter().map(|t| self.resolve_type(t)).collect(),
                ret: Box::new(
                    ret.as_ref()
                        .map(|r| self.resolve_type(r))
                        .unwrap_or(Ty::Unit),
                ),
            },
            ast::TypeExpr::Never { .. } => Ty::Never,
            ast::TypeExpr::Infer { .. } => self.fresh_ty_var(),
            ast::TypeExpr::SelfType { .. } => self
                .lookup_type_scope("Self")
                .unwrap_or_else(|| self.fresh_ty_var()),
            ast::TypeExpr::DynTrait { ability, .. } => {
                let name = ability
                    .segments
                    .iter()
                    .map(|s| s.name.as_str())
                    .collect::<Vec<_>>()
                    .join("::");
                if let Some(def) = self.lookup_def(&name) {
                    Ty::DynTrait(def)
                } else {
                    let span = ty.span();
                    self.errors.push(
                        Diagnostic::error(format!(
                            "cannot find ability `{}` for `dyn` object",
                            name
                        ))
                        .with_span(span),
                    );
                    self.fresh_ty_var()
                }
            }
        }
    }

    fn unknown_expr(&mut self, span: Span) -> HirExpr {
        HirExpr {
            id: self.fresh_id(),
            kind: HirExprKind::Lit(HirLit::Unit),
            span,
            ty: self.fresh_ty_var(),
        }
    }

    fn local_lookup<'a>(&self, locals: &'a [HashMap<String, HirId>], name: &str) -> Option<HirId> {
        locals
            .iter()
            .rev()
            .find_map(|scope| scope.get(name).copied())
    }

    fn path_key(path: &ast::Path) -> String {
        path.segments
            .iter()
            .map(|segment| segment.name.as_str())
            .collect::<Vec<_>>()
            .join("::")
    }

    fn lower_lit(&mut self, lit: &ast::Literal) -> HirLit {
        match lit {
            ast::Literal::Integer(value) => i128::try_from(*value)
                .map(HirLit::Integer)
                .unwrap_or(HirLit::Uint(*value)),
            ast::Literal::Float(value) => HirLit::Float(*value),
            ast::Literal::String(value) => HirLit::String(value.clone()),
            ast::Literal::Char(value) => HirLit::Char(*value),
            ast::Literal::Bool(value) => HirLit::Bool(*value),
            ast::Literal::Unit => HirLit::Unit,
        }
    }

    fn lower_pattern(
        &mut self,
        pattern: &Pattern,
        locals: &mut Vec<HashMap<String, HirId>>,
    ) -> HirPattern {
        let kind = match pattern {
            Pattern::Wildcard { .. } => HirPatternKind::Wildcard,
            Pattern::Ident { mutable, name } => {
                if !*mutable {
                    if let Some(def) = self.lookup_def(&name.name) {
                        if self.variant_defs.contains(&def) {
                            HirPatternKind::Variant {
                                def,
                                args: Vec::new(),
                            }
                        } else {
                            let binding = self.fresh_id();
                            if let Some(scope) = locals.last_mut() {
                                scope.insert(name.name.clone(), binding);
                            }
                            HirPatternKind::Binding {
                                id: binding,
                                mutable: *mutable,
                            }
                        }
                    } else {
                        let binding = self.fresh_id();
                        if let Some(scope) = locals.last_mut() {
                            scope.insert(name.name.clone(), binding);
                        }
                        HirPatternKind::Binding {
                            id: binding,
                            mutable: *mutable,
                        }
                    }
                } else {
                    let binding = self.fresh_id();
                    if let Some(scope) = locals.last_mut() {
                        scope.insert(name.name.clone(), binding);
                    }
                    HirPatternKind::Binding {
                        id: binding,
                        mutable: *mutable,
                    }
                }
            }
            Pattern::Literal { lit, .. } => HirPatternKind::Lit(self.lower_lit(lit)),
            Pattern::Tuple { elems, .. } => HirPatternKind::Tuple(
                elems
                    .iter()
                    .map(|elem| self.lower_pattern(elem, locals))
                    .collect(),
            ),
            Pattern::Struct {
                path, fields, rest, ..
            } => {
                let def = self
                    .lookup_def(&Self::path_key(path))
                    .unwrap_or_else(|| self.lookup_or_define(&Self::path_key(path)));
                let fields = fields
                    .iter()
                    .map(|field| {
                        let lowered = match field.pattern.as_ref() {
                            Some(pattern) => self.lower_pattern(pattern, locals),
                            None => {
                                let binding = self.fresh_id();
                                if let Some(scope) = locals.last_mut() {
                                    scope.insert(field.name.name.clone(), binding);
                                }
                                HirPattern {
                                    id: self.fresh_id(),
                                    kind: HirPatternKind::Binding {
                                        id: binding,
                                        mutable: false,
                                    },
                                    span: field.span,
                                    ty: self.fresh_ty_var(),
                                }
                            }
                        };
                        (field.name.name.clone(), lowered)
                    })
                    .collect();
                HirPatternKind::Struct {
                    def,
                    fields,
                    rest: *rest,
                }
            }
            Pattern::Variant { path, args, .. } => {
                let def = self
                    .lookup_def(&Self::path_key(path))
                    .unwrap_or_else(|| self.lookup_or_define(&Self::path_key(path)));
                HirPatternKind::Variant {
                    def,
                    args: args
                        .iter()
                        .map(|arg| self.lower_pattern(arg, locals))
                        .collect(),
                }
            }
            Pattern::Range {
                lo, hi, inclusive, ..
            } => HirPatternKind::Range {
                lo: Box::new(self.lower_pattern(lo, locals)),
                hi: Box::new(self.lower_pattern(hi, locals)),
                inclusive: *inclusive,
            },
            Pattern::Or { alternatives, .. } => HirPatternKind::Or(
                alternatives
                    .iter()
                    .map(|alt| self.lower_pattern(alt, locals))
                    .collect(),
            ),
            Pattern::Ref { mutable, inner, .. } => HirPatternKind::Ref {
                mutable: *mutable,
                inner: Box::new(self.lower_pattern(inner, locals)),
            },
            Pattern::Slice {
                elems, rest_index, ..
            } => HirPatternKind::Slice {
                elems: elems
                    .iter()
                    .map(|elem| self.lower_pattern(elem, locals))
                    .collect(),
                rest_index: *rest_index,
            },
        };

        HirPattern {
            id: self.fresh_id(),
            kind,
            span: pattern.span(),
            ty: self.fresh_ty_var(),
        }
    }

    fn lower_expr(&mut self, expr: &Expr, locals: &mut Vec<HashMap<String, HirId>>) -> HirExpr {
        let kind = match expr {
            Expr::Literal { lit, .. } => HirExprKind::Lit(self.lower_lit(lit)),
            Expr::Path { path, .. } => {
                let key = Self::path_key(path);
                if path.segments.len() == 1 {
                    if let Some(local) = self.local_lookup(locals, &key) {
                        HirExprKind::Var(local)
                    } else if let Some(def) = self.lookup_def(&key) {
                        HirExprKind::DefRef(def)
                    } else {
                        self.errors.push(
                            Diagnostic::error(format!("cannot find value `{}` in this scope", key))
                                .with_span(path.span)
                                .with_note(
                                    "define the binding in the current scope or import it with `import { ... } from ...`",
                                ),
                        );
                        HirExprKind::Lit(HirLit::Unit)
                    }
                } else if let Some(def) = self.lookup_def(&key) {
                    HirExprKind::DefRef(def)
                } else {
                    self.errors.push(
                        Diagnostic::error(format!("cannot resolve path `{}`", key))
                            .with_span(path.span)
                            .with_note(
                                "check the module path and make sure the symbol is re-exported or imported into this file",
                            )
                            .with_note(
                                "for workspace files, prefer `import { name } from \"./module\"` or `import { name } from package_name`",
                            ),
                    );
                    HirExprKind::Lit(HirLit::Unit)
                }
            }
            Expr::Block { stmts, tail, .. } => {
                locals.push(HashMap::new());
                let mut import_changes = Vec::new();
                let stmts = stmts
                    .iter()
                    .map(|stmt| self.lower_stmt(stmt, locals, &mut import_changes))
                    .collect();
                let tail = tail
                    .as_ref()
                    .map(|tail| Box::new(self.lower_expr(tail, locals)));
                self.restore_imports(import_changes);
                locals.pop();
                HirExprKind::Block(stmts, tail)
            }
            Expr::Call { callee, args, .. } => HirExprKind::Call {
                callee: Box::new({
                    let mut lowered = self.lower_expr(callee, locals);
                    if let HirExprKind::DefRef(def) = lowered.kind {
                        if let Some(wrapper_def) = self.default_wrapper_for_call(def, args.len()) {
                            lowered.kind = HirExprKind::DefRef(wrapper_def);
                        }
                    }
                    lowered
                }),
                args: args
                    .iter()
                    .map(|arg| self.lower_expr(arg, locals))
                    .collect(),
            },
            Expr::MethodCall {
                receiver,
                method,
                args,
                ..
            } => {
                let method_id = DefId {
                    file: self.file,
                    index: u32::MAX,
                };
                HirExprKind::MethodCall {
                    receiver: Box::new(self.lower_expr(receiver, locals)),
                    method_name: method.name.clone(),
                    method_id,
                    args: args
                        .iter()
                        .map(|arg| self.lower_expr(arg, locals))
                        .collect(),
                }
            }
            Expr::Index { base, index, .. } => HirExprKind::Index {
                base: Box::new(self.lower_expr(base, locals)),
                index: Box::new(self.lower_expr(index, locals)),
            },
            Expr::Field { base, field, .. } => HirExprKind::Field {
                base: Box::new(self.lower_expr(base, locals)),
                field: field.name.clone(),
                field_index: None,
            },
            Expr::Tuple { elems, .. } => HirExprKind::Tuple(
                elems
                    .iter()
                    .map(|elem| self.lower_expr(elem, locals))
                    .collect(),
            ),
            Expr::Array { elems, .. } => HirExprKind::Array(
                elems
                    .iter()
                    .map(|elem| self.lower_expr(elem, locals))
                    .collect(),
            ),
            Expr::Repeat { elem, count, .. } => {
                let count = match count.as_ref() {
                    Expr::Literal {
                        lit: ast::Literal::Integer(value),
                        ..
                    } => usize::try_from(*value).unwrap_or(0),
                    _ => {
                        self.errors.push(
                            Diagnostic::error("repeat count must be an integer literal for now")
                                .with_span(count.span()),
                        );
                        0
                    }
                };
                HirExprKind::Repeat {
                    elem: Box::new(self.lower_expr(elem, locals)),
                    count,
                }
            }
            Expr::Struct {
                path, fields, rest, ..
            } => HirExprKind::Struct {
                def: self
                    .lookup_def(&Self::path_key(path))
                    .unwrap_or_else(|| self.lookup_or_define(&Self::path_key(path))),
                fields: fields
                    .iter()
                    .map(|field| {
                        let value = match field.value.as_ref() {
                            Some(value) => self.lower_expr(value, locals),
                            None => {
                                if let Some(local) = self.local_lookup(locals, &field.name.name) {
                                    HirExpr {
                                        id: self.fresh_id(),
                                        kind: HirExprKind::Var(local),
                                        span: field.span,
                                        ty: self.fresh_ty_var(),
                                    }
                                } else {
                                    self.errors.push(
                                        Diagnostic::error(format!(
                                            "cannot find field shorthand binding `{}`",
                                            field.name.name
                                        ))
                                        .with_span(field.span),
                                    );
                                    self.unknown_expr(field.span)
                                }
                            }
                        };
                        (field.name.name.clone(), value)
                    })
                    .collect(),
                rest: rest
                    .as_ref()
                    .map(|rest| Box::new(self.lower_expr(rest, locals))),
            },
            Expr::If {
                condition,
                then_branch,
                else_branch,
                ..
            } => HirExprKind::If {
                condition: Box::new(self.lower_expr(condition, locals)),
                then_branch: Box::new(self.lower_expr(then_branch, locals)),
                else_branch: else_branch
                    .as_ref()
                    .map(|branch| Box::new(self.lower_expr(branch, locals))),
            },
            Expr::Match {
                scrutinee, arms, ..
            } => HirExprKind::Match {
                scrutinee: Box::new(self.lower_expr(scrutinee, locals)),
                arms: arms
                    .iter()
                    .map(|arm| {
                        locals.push(HashMap::new());
                        let pattern = self.lower_pattern(&arm.pattern, locals);
                        let guard = arm
                            .guard
                            .as_ref()
                            .map(|guard| self.lower_expr(guard, locals));
                        let body = self.lower_expr(&arm.body, locals);
                        locals.pop();
                        HirArm {
                            id: self.fresh_id(),
                            pattern,
                            guard,
                            body,
                        }
                    })
                    .collect(),
            },
            Expr::While {
                condition, body, ..
            } => HirExprKind::While {
                condition: Box::new(self.lower_expr(condition, locals)),
                body: Box::new(self.lower_expr(body, locals)),
            },
            Expr::WhileLet {
                pattern,
                scrutinee,
                body,
                span,
            } => {
                locals.push(HashMap::new());
                let lowered_pattern = self.lower_pattern(pattern, locals);
                let lowered_body = self.lower_expr(body, locals);
                locals.pop();
                HirExprKind::Loop(Box::new(HirExpr {
                    id: self.fresh_id(),
                    span: *span,
                    ty: Ty::Unit,
                    kind: HirExprKind::Match {
                        scrutinee: Box::new(self.lower_expr(scrutinee, locals)),
                        arms: vec![
                            HirArm {
                                id: self.fresh_id(),
                                pattern: lowered_pattern,
                                guard: None,
                                body: HirExpr {
                                    id: self.fresh_id(),
                                    span: body.span(),
                                    ty: Ty::Never,
                                    kind: HirExprKind::Block(
                                        vec![HirStmt {
                                            id: self.fresh_id(),
                                            kind: HirStmtKind::Expr(lowered_body),
                                            span: body.span(),
                                        }],
                                        Some(Box::new(HirExpr {
                                            id: self.fresh_id(),
                                            span: body.span(),
                                            ty: Ty::Never,
                                            kind: HirExprKind::Continue,
                                        })),
                                    ),
                                },
                            },
                            HirArm {
                                id: self.fresh_id(),
                                pattern: HirPattern {
                                    id: self.fresh_id(),
                                    kind: HirPatternKind::Wildcard,
                                    span: *span,
                                    ty: self.fresh_ty_var(),
                                },
                                guard: None,
                                body: HirExpr {
                                    id: self.fresh_id(),
                                    span: *span,
                                    ty: Ty::Never,
                                    kind: HirExprKind::Break(None),
                                },
                            },
                        ],
                    },
                }))
            }
            Expr::For {
                pattern,
                iterable,
                body,
                ..
            } => {
                locals.push(HashMap::new());
                let binding = match pattern {
                    Pattern::Ident { name, .. } => {
                        let binding = self.fresh_id();
                        if let Some(scope) = locals.last_mut() {
                            scope.insert(name.name.clone(), binding);
                        }
                        binding
                    }
                    _ => {
                        self.errors.push(
                            Diagnostic::error(
                                "for-loop lowering currently requires an identifier pattern",
                            )
                            .with_span(pattern.span()),
                        );
                        self.fresh_id()
                    }
                };
                let body = self.lower_expr(body, locals);
                locals.pop();
                HirExprKind::ForDesugared {
                    iter: Box::new(self.lower_expr(iterable, locals)),
                    binding,
                    body: Box::new(body),
                }
            }
            Expr::Loop { body, .. } => HirExprKind::Loop(Box::new(self.lower_expr(body, locals))),
            Expr::Break { value, .. } => HirExprKind::Break(
                value
                    .as_ref()
                    .map(|value| Box::new(self.lower_expr(value, locals))),
            ),
            Expr::Continue { .. } => HirExprKind::Continue,
            Expr::Return { value, .. } => HirExprKind::Return(
                value
                    .as_ref()
                    .map(|value| Box::new(self.lower_expr(value, locals))),
            ),
            Expr::BinOp { op, lhs, rhs, .. } => HirExprKind::BinOp {
                op: match op {
                    ast::BinOp::Add => HirBinOp::Add,
                    ast::BinOp::Sub => HirBinOp::Sub,
                    ast::BinOp::Mul => HirBinOp::Mul,
                    ast::BinOp::Div => HirBinOp::Div,
                    ast::BinOp::Rem => HirBinOp::Rem,
                    ast::BinOp::BitAnd => HirBinOp::BitAnd,
                    ast::BinOp::BitOr => HirBinOp::BitOr,
                    ast::BinOp::BitXor => HirBinOp::BitXor,
                    ast::BinOp::Shl => HirBinOp::Shl,
                    ast::BinOp::Shr => HirBinOp::Shr,
                    ast::BinOp::And => HirBinOp::And,
                    ast::BinOp::Or => HirBinOp::Or,
                    ast::BinOp::Eq => HirBinOp::Eq,
                    ast::BinOp::Ne => HirBinOp::Ne,
                    ast::BinOp::Lt => HirBinOp::Lt,
                    ast::BinOp::Le => HirBinOp::Le,
                    ast::BinOp::Gt => HirBinOp::Gt,
                    ast::BinOp::Ge => HirBinOp::Ge,
                },
                lhs: Box::new(self.lower_expr(lhs, locals)),
                rhs: Box::new(self.lower_expr(rhs, locals)),
            },
            Expr::UnaryOp { op, operand, .. } => HirExprKind::UnaryOp {
                op: match op {
                    ast::UnaryOp::Neg => HirUnaryOp::Neg,
                    ast::UnaryOp::Not => HirUnaryOp::Not,
                    ast::UnaryOp::BitNot => HirUnaryOp::BitNot,
                },
                operand: Box::new(self.lower_expr(operand, locals)),
            },
            Expr::Assign { target, value, .. } => HirExprKind::Assign {
                target: Box::new(self.lower_expr(target, locals)),
                value: Box::new(self.lower_expr(value, locals)),
            },
            Expr::CompoundAssign {
                op, target, value, ..
            } => {
                let lowered_target = self.lower_expr(target, locals);
                let lowered_value = self.lower_expr(value, locals);
                let bin_op = match op {
                    ast::CompoundOp::Add => HirBinOp::Add,
                    ast::CompoundOp::Sub => HirBinOp::Sub,
                    ast::CompoundOp::Mul => HirBinOp::Mul,
                    ast::CompoundOp::Div => HirBinOp::Div,
                    ast::CompoundOp::Rem => HirBinOp::Rem,
                    ast::CompoundOp::BitAnd => HirBinOp::BitAnd,
                    ast::CompoundOp::BitOr => HirBinOp::BitOr,
                    ast::CompoundOp::BitXor => HirBinOp::BitXor,
                    ast::CompoundOp::Shl => HirBinOp::Shl,
                    ast::CompoundOp::Shr => HirBinOp::Shr,
                };
                let target_clone = lowered_target.clone();
                HirExprKind::Assign {
                    target: Box::new(lowered_target),
                    value: Box::new(HirExpr {
                        id: self.fresh_id(),
                        kind: HirExprKind::BinOp {
                            op: bin_op,
                            lhs: Box::new(target_clone),
                            rhs: Box::new(lowered_value),
                        },
                        span: expr.span(),
                        ty: self.fresh_ty_var(),
                    }),
                }
            }
            Expr::Cast { expr, ty, .. } => HirExprKind::Cast {
                expr: Box::new(self.lower_expr(expr, locals)),
                target_ty: self.resolve_type(ty),
            },
            Expr::Try { expr, .. } => HirExprKind::Try(Box::new(self.lower_expr(expr, locals))),
            Expr::Await { expr, .. } => HirExprKind::Await(Box::new(self.lower_expr(expr, locals))),
            Expr::Closure {
                params,
                ret_ty,
                body,
                ..
            } => {
                let outer_local_ids = locals
                    .iter()
                    .flat_map(|scope| scope.values().copied())
                    .collect::<HashSet<_>>();
                locals.push(HashMap::new());
                let params = params
                    .iter()
                    .map(|param| {
                        let binding = self.fresh_id();
                        let mutable = matches!(param.pattern, Pattern::Ident { mutable: true, .. });
                        if let Pattern::Ident { ref name, .. } = param.pattern {
                            if let Some(scope) = locals.last_mut() {
                                scope.insert(name.name.clone(), binding);
                            }
                        }
                        HirParam {
                            id: self.fresh_id(),
                            binding,
                            mutable,
                            ty: param
                                .ty
                                .as_ref()
                                .map(|ty| self.resolve_type(ty))
                                .unwrap_or_else(|| self.fresh_ty_var()),
                        }
                    })
                    .collect::<Vec<_>>();
                let body = self.lower_expr(body, locals);
                let captures = Self::collect_captures(&body, &outer_local_ids);
                locals.pop();
                HirExprKind::Closure {
                    params,
                    ret_ty: ret_ty
                        .as_ref()
                        .map(|ty| self.resolve_type(ty))
                        .unwrap_or_else(|| self.fresh_ty_var()),
                    body: Box::new(body),
                    captures,
                }
            }
            Expr::Ref { mutable, expr, .. } => HirExprKind::Ref {
                mutable: *mutable,
                expr: Box::new(self.lower_expr(expr, locals)),
            },
            Expr::Deref { expr, .. } => HirExprKind::Deref(Box::new(self.lower_expr(expr, locals))),
            Expr::Range {
                lo, hi, inclusive, ..
            } => HirExprKind::Range {
                lo: lo
                    .as_ref()
                    .map(|expr| Box::new(self.lower_expr(expr, locals))),
                hi: hi
                    .as_ref()
                    .map(|expr| Box::new(self.lower_expr(expr, locals))),
                inclusive: *inclusive,
            },
            Expr::Unsafe { body, .. } => {
                HirExprKind::Unsafe(Box::new(self.lower_expr(body, locals)))
            }
        };

        HirExpr {
            id: self.fresh_id(),
            kind,
            span: expr.span(),
            ty: self.fresh_ty_var(),
        }
    }

    fn collect_captures(expr: &HirExpr, outer: &HashSet<HirId>) -> Vec<HirId> {
        let mut captures = Vec::new();
        let mut seen = HashSet::new();
        Self::collect_captures_expr(expr, outer, &mut seen, &mut captures);
        captures
    }

    fn collect_captures_expr(
        expr: &HirExpr,
        outer: &HashSet<HirId>,
        seen: &mut HashSet<HirId>,
        captures: &mut Vec<HirId>,
    ) {
        use crate::hir::HirExprKind::*;

        match &expr.kind {
            Var(id) => {
                if outer.contains(id) && seen.insert(*id) {
                    captures.push(*id);
                }
            }
            Block(stmts, tail) => {
                for stmt in stmts {
                    Self::collect_captures_stmt(stmt, outer, seen, captures);
                }
                if let Some(tail) = tail {
                    Self::collect_captures_expr(tail, outer, seen, captures);
                }
            }
            Call { callee, args } => {
                Self::collect_captures_expr(callee, outer, seen, captures);
                for arg in args {
                    Self::collect_captures_expr(arg, outer, seen, captures);
                }
            }
            MethodCall { receiver, args, .. } => {
                Self::collect_captures_expr(receiver, outer, seen, captures);
                for arg in args {
                    Self::collect_captures_expr(arg, outer, seen, captures);
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
            | Defer(base) => {
                Self::collect_captures_expr(base, outer, seen, captures);
            }
            Index { base, index }
            | BinOp {
                lhs: base,
                rhs: index,
                ..
            } => {
                Self::collect_captures_expr(base, outer, seen, captures);
                Self::collect_captures_expr(index, outer, seen, captures);
            }
            Tuple(elems) | Array(elems) => {
                for elem in elems {
                    Self::collect_captures_expr(elem, outer, seen, captures);
                }
            }
            Repeat { elem, .. } => {
                Self::collect_captures_expr(elem, outer, seen, captures);
            }
            Struct { fields, rest, .. } => {
                for (_, value) in fields {
                    Self::collect_captures_expr(value, outer, seen, captures);
                }
                if let Some(rest) = rest {
                    Self::collect_captures_expr(rest, outer, seen, captures);
                }
            }
            If {
                condition,
                then_branch,
                else_branch,
            } => {
                Self::collect_captures_expr(condition, outer, seen, captures);
                Self::collect_captures_expr(then_branch, outer, seen, captures);
                if let Some(else_branch) = else_branch {
                    Self::collect_captures_expr(else_branch, outer, seen, captures);
                }
            }
            Match { scrutinee, arms } => {
                Self::collect_captures_expr(scrutinee, outer, seen, captures);
                for arm in arms {
                    if let Some(guard) = &arm.guard {
                        Self::collect_captures_expr(guard, outer, seen, captures);
                    }
                    Self::collect_captures_expr(&arm.body, outer, seen, captures);
                }
            }
            While { condition, body } => {
                Self::collect_captures_expr(condition, outer, seen, captures);
                Self::collect_captures_expr(body, outer, seen, captures);
            }
            ForDesugared { iter, body, .. } => {
                Self::collect_captures_expr(iter, outer, seen, captures);
                Self::collect_captures_expr(body, outer, seen, captures);
            }
            UnaryOp { operand, .. } | Cast { expr: operand, .. } | Ref { expr: operand, .. } => {
                Self::collect_captures_expr(operand, outer, seen, captures);
            }
            Assign { target, value } => {
                Self::collect_captures_expr(target, outer, seen, captures);
                Self::collect_captures_expr(value, outer, seen, captures);
            }
            Return(value) | Break(value) => {
                if let Some(value) = value {
                    Self::collect_captures_expr(value, outer, seen, captures);
                }
            }
            Range { lo, hi, .. } => {
                if let Some(lo) = lo {
                    Self::collect_captures_expr(lo, outer, seen, captures);
                }
                if let Some(hi) = hi {
                    Self::collect_captures_expr(hi, outer, seen, captures);
                }
            }
            Closure { .. } | Lit(_) | DefRef(_) | Continue => {}
        }
    }

    fn collect_captures_stmt(
        stmt: &HirStmt,
        outer: &HashSet<HirId>,
        seen: &mut HashSet<HirId>,
        captures: &mut Vec<HirId>,
    ) {
        match &stmt.kind {
            HirStmtKind::Let { init, .. } => {
                if let Some(init) = init {
                    Self::collect_captures_expr(init, outer, seen, captures);
                }
            }
            HirStmtKind::Expr(expr) | HirStmtKind::Errdefer(expr) | HirStmtKind::Defer(expr) => {
                Self::collect_captures_expr(expr, outer, seen, captures);
            }
            HirStmtKind::Use(_) => {}
        }
    }

    fn lower_stmt(
        &mut self,
        stmt: &Stmt,
        locals: &mut Vec<HashMap<String, HirId>>,
        import_changes: &mut Vec<(String, Option<DefId>)>,
    ) -> HirStmt {
        let stmt_span = match stmt {
            Stmt::Let { span, .. }
            | Stmt::Errdefer { span, .. }
            | Stmt::Defer { span, .. }
            | Stmt::Use { span, .. } => *span,
            Stmt::Expr { expr, .. } => expr.span(),
        };
        let kind = match stmt {
            Stmt::Let {
                pattern, ty, init, ..
            } => {
                let (binding, mutable) = match pattern {
                    Pattern::Ident { mutable, name } => {
                        let binding = self.fresh_id();
                        if let Some(scope) = locals.last_mut() {
                            scope.insert(name.name.clone(), binding);
                        }
                        (binding, *mutable)
                    }
                    _ => {
                        self.errors.push(
                            Diagnostic::error(
                                "let lowering currently requires an identifier pattern",
                            )
                            .with_span(pattern.span()),
                        );
                        (self.fresh_id(), false)
                    }
                };
                HirStmtKind::Let {
                    binding,
                    mutable,
                    ty: ty
                        .as_ref()
                        .map(|ty| self.resolve_type(ty))
                        .unwrap_or_else(|| self.fresh_ty_var()),
                    init: init.as_ref().map(|init| self.lower_expr(init, locals)),
                }
            }
            Stmt::Errdefer { body, .. } => HirStmtKind::Errdefer(self.lower_expr(body, locals)),
            Stmt::Defer { body, .. } => HirStmtKind::Defer(self.lower_expr(body, locals)),
            Stmt::Expr { expr, .. } => HirStmtKind::Expr(self.lower_expr(expr, locals)),
            Stmt::Use { tree, .. } => {
                let lowered = self.resolve_use(tree, stmt_span);
                self.apply_use_tree(tree, &[], Some(import_changes));
                HirStmtKind::Use(lowered)
            }
        };

        HirStmt {
            id: self.fresh_id(),
            kind,
            span: stmt_span,
        }
    }

    fn resolve_fn_params(&mut self, params: &[ast::FnParam]) -> Vec<HirParam> {
        params
            .iter()
            .map(|p| {
                let binding_id = self.fresh_id();
                let ty = self.resolve_type(&p.ty);
                let mutable = matches!(&p.pattern, ast::Pattern::Ident { mutable: true, .. });
                HirParam {
                    id: self.fresh_id(),
                    binding: binding_id,
                    mutable,
                    ty,
                }
            })
            .collect()
    }

    fn validate_default_params(&mut self, f: &ast::FnDef) -> bool {
        if !Self::has_trailing_default_params(&f.params) {
            self.errors.push(
                Diagnostic::error("default parameters must form a trailing suffix")
                    .with_span(f.span)
                    .with_note(
                        "once a parameter has `= value`, every following parameter must also have a default",
                    ),
            );
            return false;
        }

        if Self::first_default_param_index(&f.params).is_some()
            && f.params
                .iter()
                .any(|param| !matches!(param.pattern, ast::Pattern::Ident { .. }))
        {
            self.errors.push(
                Diagnostic::error("default parameters require simple identifier bindings in v1")
                    .with_span(f.span)
                    .with_note(
                        "use `name: Type = expr` parameters instead of destructuring patterns when defaults are present",
                    ),
            );
            return false;
        }

        true
    }

    fn synthesize_default_wrapper_fns(
        &mut self,
        f: &ast::FnDef,
        original_def: DefId,
        params: &[HirParam],
        ret_ty: &Ty,
    ) -> Vec<HirFn> {
        if !Self::can_synthesize_default_wrappers(&f.params) {
            return Vec::new();
        }
        let Some(first_default) = Self::first_default_param_index(&f.params) else {
            return Vec::new();
        };

        let type_params = self.resolve_generic_params(&f.generics);
        let mut wrappers = Vec::new();
        for arg_count in first_default..f.params.len() {
            let Some(wrapper_def) = self.default_wrapper_for_call(original_def, arg_count) else {
                continue;
            };
            let wrapper_params = params[..arg_count].to_vec();
            let mut locals = vec![HashMap::new()];
            let mut resolved_bindings = Vec::with_capacity(params.len());
            for (param_ast, param_hir) in f.params.iter().zip(wrapper_params.iter()) {
                if let ast::Pattern::Ident { name, .. } = &param_ast.pattern {
                    if let Some(scope) = locals.last_mut() {
                        scope.insert(name.name.clone(), param_hir.binding);
                    }
                }
                resolved_bindings.push(param_hir.binding);
            }

            let mut stmts = Vec::new();
            for (param_ast, param_hir) in f.params.iter().zip(params.iter()).skip(arg_count) {
                let Some(default_expr) = &param_ast.default else {
                    return wrappers;
                };
                let ast::Pattern::Ident { name, .. } = &param_ast.pattern else {
                    return wrappers;
                };
                let binding = self.fresh_id();
                let init = self.lower_expr(default_expr, &mut locals);
                stmts.push(HirStmt {
                    id: self.fresh_id(),
                    kind: HirStmtKind::Let {
                        binding,
                        mutable: false,
                        ty: param_hir.ty.clone(),
                        init: Some(init),
                    },
                    span: param_ast.span,
                });
                if let Some(scope) = locals.last_mut() {
                    scope.insert(name.name.clone(), binding);
                }
                resolved_bindings.push(binding);
            }

            let call_args = resolved_bindings
                .into_iter()
                .zip(params.iter())
                .map(|(binding, param)| HirExpr {
                    id: self.fresh_id(),
                    kind: HirExprKind::Var(binding),
                    span: f.span,
                    ty: param.ty.clone(),
                })
                .collect();
            let call = HirExpr {
                id: self.fresh_id(),
                kind: HirExprKind::Call {
                    callee: Box::new(HirExpr {
                        id: self.fresh_id(),
                        kind: HirExprKind::DefRef(original_def),
                        span: f.span,
                        ty: self.fresh_ty_var(),
                    }),
                    args: call_args,
                },
                span: f.span,
                ty: self.fresh_ty_var(),
            };
            let body = HirExpr {
                id: self.fresh_id(),
                kind: HirExprKind::Block(stmts, Some(Box::new(call))),
                span: f.span,
                ty: self.fresh_ty_var(),
            };
            wrappers.push(HirFn {
                id: self.fresh_id(),
                def: wrapper_def,
                type_params: type_params.clone(),
                is_async: f.is_async,
                is_unsafe: f.is_unsafe,
                params: wrapper_params,
                ret_ty: ret_ty.clone(),
                body: Some(body),
                span: f.span,
            });
        }

        wrappers
    }

    fn resolve_fn(&mut self, f: &ast::FnDef) -> Vec<HirFn> {
        let id = self.fresh_id();
        let def = self.define_in_current_module(&f.name.name);

        self.push_generics(&f.generics);
        let type_params = self.resolve_generic_params(&f.generics);
        let defaults_valid = self.validate_default_params(f);
        let params = self.resolve_fn_params(&f.params);

        let ret_ty = f
            .ret_ty
            .as_ref()
            .map(|t| self.resolve_type(t))
            .unwrap_or(Ty::Unit);
        let mut locals = vec![HashMap::new()];
        for (param_ast, param_hir) in f.params.iter().zip(params.iter()) {
            if let ast::Pattern::Ident { name, .. } = &param_ast.pattern {
                if let Some(scope) = locals.last_mut() {
                    scope.insert(name.name.clone(), param_hir.binding);
                }
            }
        }
        let body = f
            .body
            .as_ref()
            .map(|body| self.lower_expr(body, &mut locals));
        let wrappers = if defaults_valid {
            self.synthesize_default_wrapper_fns(f, def, &params, &ret_ty)
        } else {
            Vec::new()
        };
        self.pop_type_scope();

        let mut functions = vec![HirFn {
            id,
            def,
            type_params,
            is_async: f.is_async,
            is_unsafe: f.is_unsafe,
            params,
            ret_ty,
            body,
            span: f.span,
        }];
        functions.extend(wrappers);
        functions
    }

    fn resolve_const(&mut self, c: &ast::ConstDef) -> HirConst {
        let id = self.fresh_id();
        let def = self.define_in_current_module(&c.name.name);
        let ty = self.resolve_type(&c.ty);
        let mut locals = vec![HashMap::new()];
        let value = self.lower_expr(&c.value, &mut locals);

        HirConst {
            id,
            def,
            ty,
            value,
            span: c.span,
        }
    }

    fn resolve_static(&mut self, s: &ast::StaticDef) -> HirStatic {
        let id = self.fresh_id();
        let def = self.define_in_current_module(&s.name.name);
        let ty = self.resolve_type(&s.ty);
        let mut locals = vec![HashMap::new()];
        let value = self.lower_expr(&s.value, &mut locals);

        HirStatic {
            id,
            def,
            mutable: s.mutable,
            ty,
            value,
            span: s.span,
        }
    }

    fn resolve_type_alias(&mut self, alias: &ast::TypeAlias) -> HirTypeAlias {
        let id = self.fresh_id();
        let def = self.define_in_current_module(&alias.name.name);
        self.push_generics(&alias.generics);
        let type_params = self.resolve_generic_params(&alias.generics);
        let ty = self.resolve_type(&alias.ty);
        self.pop_type_scope();
        HirTypeAlias {
            id,
            def,
            type_params,
            ty,
            span: alias.span,
        }
    }

    fn resolve_assoc_fn(&mut self, f: &ast::FnDef, qualified_name: &str) -> HirFn {
        let id = self.fresh_id();
        let def = self.lookup_or_define(qualified_name);
        self.push_generics(&f.generics);
        let type_params = self.resolve_generic_params(&f.generics);

        let params: Vec<HirParam> = f
            .params
            .iter()
            .map(|p| {
                let binding_id = self.fresh_id();
                let ty = self.resolve_type(&p.ty);
                let mutable = matches!(&p.pattern, ast::Pattern::Ident { mutable: true, .. });
                HirParam {
                    id: self.fresh_id(),
                    binding: binding_id,
                    mutable,
                    ty,
                }
            })
            .collect();

        let ret_ty = f
            .ret_ty
            .as_ref()
            .map(|t| self.resolve_type(t))
            .unwrap_or(Ty::Unit);
        let mut locals = vec![HashMap::new()];
        for (param_ast, param_hir) in f.params.iter().zip(params.iter()) {
            if let ast::Pattern::Ident { name, .. } = &param_ast.pattern {
                if let Some(scope) = locals.last_mut() {
                    scope.insert(name.name.clone(), param_hir.binding);
                }
            }
        }
        let body = f
            .body
            .as_ref()
            .map(|body| self.lower_expr(body, &mut locals));
        self.pop_type_scope();

        HirFn {
            id,
            def,
            type_params,
            is_async: f.is_async,
            is_unsafe: f.is_unsafe,
            params,
            ret_ty,
            body,
            span: f.span,
        }
    }

    fn resolve_assoc_items(
        &mut self,
        owner_name: &str,
        items: &[ast::TraitItem],
    ) -> Vec<HirAssocItem> {
        items
            .iter()
            .map(|item| match item {
                ast::TraitItem::Method(method) => {
                    let qualified = format!("{owner_name}::{}", method.name.name);
                    HirAssocItem::Method(self.resolve_assoc_fn(method, &qualified))
                }
                ast::TraitItem::TypeAssoc {
                    name,
                    bounds,
                    default,
                    span,
                } => HirAssocItem::TypeAssoc {
                    name: name.name.clone(),
                    bounds: bounds
                        .iter()
                        .map(|bound| self.resolve_type(bound))
                        .collect(),
                    default: default.as_ref().map(|ty| self.resolve_type(ty)),
                    span: *span,
                },
                ast::TraitItem::Const {
                    name,
                    ty,
                    default,
                    span,
                } => {
                    let mut locals = vec![HashMap::new()];
                    HirAssocItem::Const {
                        name: name.name.clone(),
                        ty: self.resolve_type(ty),
                        default: default
                            .as_ref()
                            .map(|expr| self.lower_expr(expr, &mut locals)),
                        span: *span,
                    }
                }
            })
            .collect()
    }

    fn resolve_trait_def(&mut self, t: &ast::TraitDef) -> HirTrait {
        let id = self.fresh_id();
        let def = self.define_in_current_module(&t.name.name);
        self.push_generics(&t.generics);
        let type_params = self.resolve_generic_params(&t.generics);
        self.push_type_scope(["Self".to_string()]);
        let super_traits = t
            .super_traits
            .iter()
            .map(|super_trait| self.resolve_type(super_trait))
            .collect();
        let items = self.resolve_assoc_items(&t.name.name, &t.items);
        self.pop_type_scope();
        self.pop_type_scope();
        HirTrait {
            id,
            def,
            type_params,
            super_traits,
            items,
            span: t.span,
        }
    }

    fn resolve_interface_def(&mut self, i: &ast::InterfaceDef) -> HirInterface {
        let id = self.fresh_id();
        let def = self.define_in_current_module(&i.name.name);
        self.push_generics(&i.generics);
        let type_params = self.resolve_generic_params(&i.generics);
        self.push_type_scope(["Self".to_string()]);
        let super_traits = i
            .super_traits
            .iter()
            .map(|super_trait| self.resolve_type(super_trait))
            .collect();
        let items = self.resolve_assoc_items(&i.name.name, &i.items);
        self.pop_type_scope();
        self.pop_type_scope();
        HirInterface {
            id,
            def,
            type_params,
            super_traits,
            items,
            span: i.span,
        }
    }

    fn resolve_ability_def(&mut self, ability: &ast::AbilityDef) -> HirAbility {
        let id = self.fresh_id();
        let def = self.define_in_current_module(&ability.name.name);
        let super_abilities = ability
            .super_abilities
            .iter()
            .map(|path| {
                self.resolve_type(&ast::TypeExpr::Named {
                    path: path.clone(),
                    generics: Vec::new(),
                    span: path.span,
                })
            })
            .collect();
        self.push_type_scope(["Self".to_string()]);
        let items = self.resolve_assoc_items(&ability.name.name, &ability.items);
        self.pop_type_scope();
        HirAbility {
            id,
            def,
            super_abilities,
            items,
            span: ability.span,
        }
    }

    fn resolve_use_tree(&mut self, tree: &ast::UseTree) -> HirUseTree {
        HirUseTree {
            prefix: tree
                .prefix
                .segments
                .iter()
                .map(|segment| segment.name.clone())
                .collect(),
            kind: match &tree.kind {
                ast::UseTreeKind::Simple => HirUseKind::Simple,
                ast::UseTreeKind::Alias(alias) => HirUseKind::Alias(alias.name.clone()),
                ast::UseTreeKind::Glob => HirUseKind::Glob,
                ast::UseTreeKind::Nested(children) => HirUseKind::Nested(
                    children
                        .iter()
                        .map(|child| self.resolve_use_tree(child))
                        .collect(),
                ),
            },
            span: tree.span,
        }
    }

    fn resolve_use(&mut self, tree: &ast::UseTree, span: Span) -> HirUse {
        HirUse {
            id: self.fresh_id(),
            tree: self.resolve_use_tree(tree),
            span,
        }
    }

    fn resolve_module_item(&mut self, module: &ast::ModuleDef) -> HirMod {
        let id = self.fresh_id();
        let def = self.define_in_current_module(&module.name.name);
        let body = if let Some(items) = &module.body {
            let mut child = Resolver {
                file: self.file,
                alloc: self.alloc.clone(),
                errors: Vec::new(),
                defs: self.defs.clone(),
                def_names: self.def_names.clone(),
                const_exprs: self.const_exprs.clone(),
                variant_parents: self.variant_parents.clone(),
                variant_defs: self.variant_defs.clone(),
                default_fn_wrappers: self.default_fn_wrappers.clone(),
                module_path: {
                    let mut path = self.module_path.clone();
                    path.push(module.name.name.clone());
                    path
                },
                type_scopes: self.type_scopes.clone(),
                def_counter: self.def_counter,
            };
            let inline_module = ast::Module {
                file: self.file,
                items: items.clone(),
                span: module.span,
            };
            let hir = child.run(&inline_module);
            self.alloc = child.alloc;
            self.def_counter = child.def_counter;
            self.errors.extend(child.errors);
            self.def_names.extend(child.def_names.clone());
            self.variant_parents.extend(child.variant_parents.clone());
            self.variant_defs.extend(child.variant_defs.iter().copied());
            self.default_fn_wrappers
                .extend(child.default_fn_wrappers.clone());
            self.defs.extend(
                child
                    .defs
                    .iter()
                    .filter(|(name, _)| name.contains("::"))
                    .map(|(name, def)| (name.clone(), *def)),
            );
            Some(Box::new(hir))
        } else {
            None
        };

        HirMod {
            id,
            def,
            body,
            span: module.span,
        }
    }

    fn resolve_impl_block(&mut self, imp: &ast::ImplBlock) -> HirImpl {
        let id = self.fresh_id();
        self.push_generics(&imp.generics);
        let type_params = self.resolve_generic_params(&imp.generics);
        let trait_ref = imp.trait_ref.as_ref().map(|ty| self.resolve_type(ty));
        let self_ty = self.resolve_type(&imp.self_ty);
        let mut self_scope = HashMap::new();
        self_scope.insert("Self".to_string(), self_ty.clone());
        self.type_scopes.push(self_scope);
        let self_key = self
            .assoc_owner_name(&self_ty)
            .unwrap_or_else(|| format!("{:?}", self_ty));
        let items = imp
            .items
            .iter()
            .map(|item| match item {
                ast::ImplItem::Method(method) => {
                    let qualified = format!("{self_key}::{}", method.name.name);
                    HirImplItem::Method(self.resolve_assoc_fn(method, &qualified))
                }
                ast::ImplItem::TypeAssoc { name, ty, span } => HirImplItem::TypeAssoc {
                    name: name.name.clone(),
                    ty: self.resolve_type(ty),
                    span: *span,
                },
                ast::ImplItem::Const(const_def) => {
                    let mut locals = vec![HashMap::new()];
                    HirImplItem::Const {
                        name: const_def.name.name.clone(),
                        ty: self.resolve_type(&const_def.ty),
                        value: self.lower_expr(&const_def.value, &mut locals),
                        span: const_def.span,
                    }
                }
            })
            .collect();
        self.pop_type_scope();
        self.pop_type_scope();

        HirImpl {
            id,
            type_params,
            trait_ref,
            self_ty,
            items,
            span: imp.span,
        }
    }

    fn resolve_struct(&mut self, s: &ast::StructDef) -> HirStruct {
        let id = self.fresh_id();
        let def = self.define_in_current_module(&s.name.name);
        self.push_generics(&s.generics);
        let type_params = self.resolve_generic_params(&s.generics);
        let fields = match &s.kind {
            ast::StructKind::Fields(fields) => fields
                .iter()
                .map(|f| HirField {
                    name: f.name.name.clone(),
                    ty: self.resolve_type(&f.ty),
                    span: f.span,
                })
                .collect(),
            ast::StructKind::Tuple(fields) => fields
                .iter()
                .enumerate()
                .map(|(i, f)| HirField {
                    name: i.to_string(),
                    ty: self.resolve_type(&f.ty),
                    span: f.span,
                })
                .collect(),
            ast::StructKind::Unit => Vec::new(),
        };
        self.pop_type_scope();
        HirStruct {
            id,
            def,
            derives: s
                .derives
                .iter()
                .filter_map(|path| path.segments.last().map(|seg| seg.name.clone()))
                .collect(),
            type_params,
            fields,
            span: s.span,
        }
    }

    fn resolve_enum(&mut self, e: &ast::EnumDef) -> HirEnum {
        let id = self.fresh_id();
        let def = self.define_in_current_module(&e.name.name);
        self.push_generics(&e.generics);
        let type_params = self.resolve_generic_params(&e.generics);
        let variants = e
            .variants
            .iter()
            .map(|v| {
                let qualified = format!("{}::{}", self.qualify_name(&e.name.name), v.name.name);
                let variant_def = self.lookup_or_define(&qualified);
                let fields = match &v.kind {
                    ast::VariantKind::Unit => Vec::new(),
                    ast::VariantKind::Tuple(tys) => {
                        tys.iter().map(|t| self.resolve_type(t)).collect()
                    }
                    ast::VariantKind::Struct(fields) => {
                        fields.iter().map(|f| self.resolve_type(&f.ty)).collect()
                    }
                };
                HirVariant {
                    def: variant_def,
                    name: v.name.name.clone(),
                    fields,
                    span: v.span,
                }
            })
            .collect();
        self.pop_type_scope();
        HirEnum {
            id,
            def,
            derives: e
                .derives
                .iter()
                .filter_map(|path| path.segments.last().map(|seg| seg.name.clone()))
                .collect(),
            type_params,
            variants,
            span: e.span,
        }
    }

    fn run(&mut self, module: &Module) -> HirModule {
        let mut hir = HirModule::new(self.file);

        // First pass: predeclare recursively so sibling modules can resolve
        // qualified names like `std::fmt::Formatter` during their own pass.
        self.predeclare_items(&module.items, true);

        // Second pass: resolve items.
        for item in &module.items {
            match item {
                Item::Function(f) => {
                    hir.functions.extend(self.resolve_fn(f));
                }
                Item::Const(c) => {
                    hir.consts.push(self.resolve_const(c));
                }
                Item::Static(s) => {
                    hir.statics.push(self.resolve_static(s));
                }
                Item::TypeAlias(alias) => {
                    hir.type_aliases.push(self.resolve_type_alias(alias));
                }
                Item::Use(tree, span) => {
                    hir.uses.push(self.resolve_use(tree, *span));
                    self.apply_use_tree(tree, &[], None);
                }
                Item::Trait(t) => {
                    hir.traits.push(self.resolve_trait_def(t));
                }
                Item::Interface(i) => {
                    hir.interfaces.push(self.resolve_interface_def(i));
                }
                Item::Ability(ability) => {
                    hir.abilities.push(self.resolve_ability_def(ability));
                }
                Item::Impl(imp) => {
                    hir.impls.push(self.resolve_impl_block(imp));
                }
                Item::Module(module) => {
                    hir.modules.push(self.resolve_module_item(module));
                }
                Item::Struct(s) => {
                    hir.structs.push(self.resolve_struct(s));
                }
                Item::Enum(e) => {
                    hir.enums.push(self.resolve_enum(e));
                }
                Item::ExternBlock(ext) => {
                    for f in &ext.functions {
                        let def = self.defs[&f.name.name];
                        let params: Vec<HirParam> = f
                            .params
                            .iter()
                            .map(|p| {
                                let binding_id = self.fresh_id();
                                let ty = self.resolve_type(&p.ty);
                                let mutable =
                                    matches!(&p.pattern, ast::Pattern::Ident { mutable: true, .. });
                                HirParam {
                                    id: self.fresh_id(),
                                    binding: binding_id,
                                    mutable,
                                    ty,
                                }
                            })
                            .collect();
                        let ret_ty = f
                            .ret_ty
                            .as_ref()
                            .map(|t| self.resolve_type(t))
                            .unwrap_or(Ty::Unit);
                        hir.extern_fns.push(HirExternFn {
                            def,
                            name: f.name.name.clone(),
                            abi: ext.abi.clone(),
                            params,
                            ret_ty,
                            span: f.span,
                        });
                    }
                }
            }
        }

        hir.def_names = self.def_names.clone();

        hir
    }
}

/// Resolve names in the AST, returning a HIR module and diagnostics.
pub fn resolve(file: FileId, module: &Module) -> (HirModule, Vec<Diagnostic>) {
    let mut resolver = Resolver::new(file);
    let hir = resolver.run(module);
    (hir, resolver.errors)
}
