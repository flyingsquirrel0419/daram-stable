use super::*;

impl<'hir> TypeChecker<'hir> {
    pub(super) fn collect_module_items(&mut self, hir: &HirModule) {
        self.def_names.extend(hir.def_names.clone());
        type_checker_abilities::collect_abilities(
            &hir.abilities,
            &self.def_names,
            &mut self.abilities,
            &mut self.method_defs,
        );
        for ability in &hir.abilities {
            self.collect_assoc_items(&ability.items);
        }
        for (def, name) in &hir.def_names {
            if let Some(return_ty) = Self::builtin_return_ty(name) {
                self.fn_ret_tys.insert(*def, return_ty);
            }
        }
        for function in &hir.functions {
            self.fn_ret_tys
                .insert(function.def, function.ret_ty.clone());
            if !self.is_variadic_builtin_def(function.def) {
                let param_tys = function
                    .params
                    .iter()
                    .map(|param| param.ty.clone())
                    .collect::<Vec<_>>();
                self.fn_generic_vars.insert(
                    function.def,
                    self.collect_fn_generic_vars(&param_tys, &function.ret_ty),
                );
                self.fn_param_tys.insert(function.def, param_tys);
            }
        }
        for item in &hir.consts {
            self.item_tys.insert(item.def, item.ty.clone());
        }
        for item in &hir.statics {
            self.item_tys.insert(item.def, item.ty.clone());
        }
        for strukt in &hir.structs {
            self.struct_fields.insert(
                strukt.def,
                strukt
                    .fields
                    .iter()
                    .map(|field| (field.name.clone(), field.ty.clone()))
                    .collect(),
            );
            self.struct_generic_vars
                .insert(strukt.def, self.collect_struct_generic_vars(strukt));
        }
        for enum_def in &hir.enums {
            self.enum_variants
                .insert(enum_def.def, enum_def.variants.clone());
            self.enum_generic_vars
                .insert(enum_def.def, self.collect_enum_generic_vars(enum_def));
            for variant in &enum_def.variants {
                self.variant_parents.insert(variant.def, enum_def.def);
            }
        }
        for trait_def in &hir.traits {
            self.collect_assoc_items(&trait_def.items);
        }
        for interface_def in &hir.interfaces {
            self.collect_assoc_items(&interface_def.items);
        }
        for imp in &hir.impls {
            self.collect_impl_ability(imp);
            self.collect_impl_items(&imp.items);
            if let Some(owner_def) = self.method_owner_def(&imp.self_ty) {
                for item in &imp.items {
                    if let HirImplItem::Method(method) = item {
                        let name = self
                            .def_names
                            .get(&method.def)
                            .and_then(|qualified| qualified.rsplit("::").next())
                            .unwrap_or_default()
                            .to_string();
                        self.method_defs.insert((owner_def, name), method.def);
                    }
                }
            }
        }
        for module in &hir.modules {
            if let Some(body) = &module.body {
                self.collect_module_items(body);
            }
        }
    }

    pub(super) fn collect_impl_ability(&mut self, imp: &HirImpl) {
        let Some(trait_ref) = &imp.trait_ref else {
            return;
        };
        let Some(ability) = self.ability_kind_for_ty(trait_ref) else {
            return;
        };
        let Some(self_def) = self
            .named_def(&imp.self_ty)
            .or_else(|| self.method_owner_def(&imp.self_ty))
        else {
            return;
        };
        self.ability_impls
            .entry(self_def)
            .or_default()
            .insert(ability);
    }

    pub(super) fn collect_assoc_items(&mut self, items: &[HirAssocItem]) {
        for item in items {
            match item {
                HirAssocItem::Method(method) => {
                    self.fn_ret_tys.insert(method.def, method.ret_ty.clone());
                    if !self.is_variadic_builtin_def(method.def) {
                        let param_tys = method
                            .params
                            .iter()
                            .map(|param| param.ty.clone())
                            .collect::<Vec<_>>();
                        self.fn_generic_vars.insert(
                            method.def,
                            self.collect_fn_generic_vars(&param_tys, &method.ret_ty),
                        );
                        self.fn_param_tys.insert(method.def, param_tys);
                    }
                }
                HirAssocItem::Const { .. } | HirAssocItem::TypeAssoc { .. } => {}
            }
        }
    }

    pub(super) fn collect_impl_items(&mut self, items: &[HirImplItem]) {
        for item in items {
            match item {
                HirImplItem::Method(method) => {
                    self.fn_ret_tys.insert(method.def, method.ret_ty.clone());
                    if !self.is_variadic_builtin_def(method.def) {
                        let param_tys = method
                            .params
                            .iter()
                            .map(|param| param.ty.clone())
                            .collect::<Vec<_>>();
                        self.fn_generic_vars.insert(
                            method.def,
                            self.collect_fn_generic_vars(&param_tys, &method.ret_ty),
                        );
                        self.fn_param_tys.insert(method.def, param_tys);
                    }
                }
                HirImplItem::Const { .. } | HirImplItem::TypeAssoc { .. } => {}
            }
        }
    }

    pub(super) fn collect_enum_generic_vars(&self, enum_def: &crate::hir::HirEnum) -> Vec<u32> {
        let mut ordered = Vec::new();
        let mut seen = HashSet::new();
        for variant in &enum_def.variants {
            for field in &variant.fields {
                Self::collect_ty_vars(field, &mut ordered, &mut seen);
            }
        }
        ordered
    }

    pub(super) fn collect_struct_generic_vars(&self, strukt: &crate::hir::HirStruct) -> Vec<u32> {
        let mut ordered = Vec::new();
        let mut seen = HashSet::new();
        for field in &strukt.fields {
            Self::collect_ty_vars(&field.ty, &mut ordered, &mut seen);
        }
        ordered
    }

    pub(super) fn collect_fn_generic_vars(&self, params: &[Ty], ret_ty: &Ty) -> Vec<u32> {
        let mut ordered = Vec::new();
        let mut seen = HashSet::new();
        for param in params {
            Self::collect_ty_vars(param, &mut ordered, &mut seen);
        }
        Self::collect_ty_vars(ret_ty, &mut ordered, &mut seen);
        ordered
    }

    pub(super) fn collect_ty_vars(ty: &Ty, ordered: &mut Vec<u32>, seen: &mut HashSet<u32>) {
        match ty {
            Ty::Var(id) => {
                if seen.insert(*id) {
                    ordered.push(*id);
                }
            }
            Ty::Ref { inner, .. } | Ty::RawPtr { inner, .. } | Ty::Slice(inner) => {
                Self::collect_ty_vars(inner, ordered, seen);
            }
            Ty::Array { elem, .. } => Self::collect_ty_vars(elem, ordered, seen),
            Ty::Tuple(elems) => {
                for elem in elems {
                    Self::collect_ty_vars(elem, ordered, seen);
                }
            }
            Ty::Named { args, .. } => {
                for arg in args {
                    Self::collect_ty_vars(arg, ordered, seen);
                }
            }
            Ty::FnPtr { params, ret } => {
                for param in params {
                    Self::collect_ty_vars(param, ordered, seen);
                }
                Self::collect_ty_vars(ret, ordered, seen);
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
}
