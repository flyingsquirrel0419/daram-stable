use super::*;

impl<'hir> TypeChecker<'hir> {
    pub(super) fn method_owner_def(&self, ty: &Ty) -> Option<DefId> {
        match self.uf.resolve(ty) {
            Ty::Named { def, .. } => Some(def),
            Ty::Ref { inner, .. } => self.method_owner_def(&inner),
            Ty::String => self.string_def(),
            Ty::DynTrait(def) => Some(def),
            _ => None,
        }
    }

    pub(super) fn lookup_method_def(&self, owner_ty: &Ty, method_name: &str) -> Option<DefId> {
        let owner_def = self.method_owner_def(owner_ty)?;
        self.method_defs
            .get(&(owner_def, method_name.to_string()))
            .copied()
    }
}
