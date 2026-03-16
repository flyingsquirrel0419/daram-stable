//! Mid-level Intermediate Representation (MIR).
//!
//! The MIR is a control-flow graph (CFG) representation in Static Single
//! Assignment (SSA) form. It is produced from the HIR and is the primary
//! input for optimizations and backend code generation.
//!
//! ## Design notes
//! - Each function is a list of basic blocks.
//! - Each basic block has a list of statements followed by a terminator.
//! - All values are in SSA form: each `Local` is assigned exactly once.
//! - Aggregate types (structs, tuples, arrays) are explicit in the MIR.
//! - `errdefer` and `defer` are lowered to explicit cleanup blocks.

use crate::{
    diagnostics::Diagnostic,
    hir::{DefId, HirId, Ty},
    source::Span,
};
use std::collections::{HashMap, HashSet};

// ─── Basic blocks ─────────────────────────────────────────────────────────────

/// Index into a function's basic block list.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct BlockId(pub u32);

/// A local variable in SSA form.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Local(pub u32);

// ─── MIR values ───────────────────────────────────────────────────────────────

/// A scalar or reference value that can appear in operand position.
#[derive(Debug, Clone)]
pub enum Operand {
    /// An already-allocated local variable.
    Copy(Local),
    /// A value moved out of a local (invalidates the source).
    Move(Local),
    /// A resolved item definition.
    Def(DefId),
    /// A compile-time constant.
    Const(MirConst),
}

#[derive(Debug, Clone)]
pub enum MirConst {
    Bool(bool),
    Int(i128),
    Uint(u128),
    Float(f64),
    Char(char),
    Str(String),
    Tuple(Vec<MirConst>),
    Array(Vec<MirConst>),
    Struct { def: DefId, fields: Vec<MirConst> },
    Ref(Box<MirConst>),
    Unit,
    Undef,
}

/// An lvalue (place in memory) that can be read or written.
#[derive(Debug, Clone)]
pub struct Place {
    pub local: Local,
    pub projections: Vec<Projection>,
}

impl Place {
    pub fn local(local: Local) -> Self {
        Self {
            local,
            projections: Vec::new(),
        }
    }
}

#[derive(Debug, Clone)]
pub enum Projection {
    /// `place.field_idx`
    Field(usize),
    /// `place.<variant_idx>.<field_idx>` for variant-aware enum payload access.
    VariantField {
        variant_idx: usize,
        field_idx: usize,
    },
    /// `place[index]`
    Index(Local),
    /// `*place` (deref)
    Deref,
}

// ─── MIR rvalues ──────────────────────────────────────────────────────────────

/// The right-hand side of an assignment.
#[derive(Debug, Clone)]
pub enum Rvalue {
    Use(Operand),
    Read(Place),
    BinaryOp {
        op: MirBinOp,
        lhs: Operand,
        rhs: Operand,
    },
    UnaryOp {
        op: MirUnaryOp,
        operand: Operand,
    },
    Ref {
        mutable: bool,
        place: Place,
    },
    AddressOf {
        mutable: bool,
        place: Place,
    },
    Cast {
        kind: CastKind,
        operand: Operand,
        target_ty: Ty,
    },
    Aggregate(AggregateKind, Vec<Operand>),
    Discriminant(Place),
    Len(Place),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MirBinOp {
    Add,
    Sub,
    Mul,
    Div,
    Rem,
    BitAnd,
    BitOr,
    BitXor,
    Shl,
    Shr,
    Eq,
    Ne,
    Lt,
    Le,
    Gt,
    Ge,
    Offset,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MirUnaryOp {
    Not,
    Neg,
}

#[derive(Debug, Clone)]
pub enum CastKind {
    IntToInt,
    IntToFloat,
    FloatToInt,
    FloatToFloat,
    PointerCast,
    Transmute,
}

#[derive(Debug, Clone)]
pub enum AggregateKind {
    Tuple,
    Array(Ty),
    Struct(DefId),
    Enum { def: DefId, variant_idx: usize },
    Closure(DefId),
}

// ─── MIR statements ───────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct Statement {
    pub kind: StatementKind,
    pub span: Option<Span>,
}

#[derive(Debug, Clone)]
pub enum StatementKind {
    /// `place = rvalue`
    Assign(Place, Rvalue),
    /// Storage has been allocated for a local.
    StorageLive(Local),
    /// Storage for a local can be reclaimed.
    StorageDead(Local),
    /// Marks a `defer` cleanup region.
    DeferStart(Local),
    /// Marks an `errdefer` cleanup region.
    ErrdeferStart(Local),
    /// No-op (used as a placeholder after optimization).
    Nop,
}

// ─── MIR terminators ──────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct Terminator {
    pub kind: TerminatorKind,
    pub span: Option<Span>,
}

#[derive(Debug, Clone)]
pub enum TerminatorKind {
    /// Unreachable — should be eliminated by the optimizer.
    Unreachable,
    /// Fall through or unconditional jump.
    Goto(BlockId),
    /// Return from the current function.
    Return,
    /// Conditional branch.
    SwitchInt {
        discriminant: Operand,
        targets: Vec<(u128, BlockId)>,
        otherwise: BlockId,
    },
    /// Function call.
    Call {
        callee: Operand,
        args: Vec<Operand>,
        destination: Place,
        target: Option<BlockId>,
        unwind: Option<BlockId>,
    },
    /// `errdefer` unwind path.
    ErrdeferUnwind(BlockId),
    /// Panic with a static message (e.g. index out of bounds).
    Assert {
        cond: Operand,
        expected: bool,
        msg: &'static str,
        target: BlockId,
    },
    /// Drop a place and then branch to `target`.
    ///
    /// Current contract:
    /// - MIR lowering may emit this for lexical cleanup and `errdefer`-adjacent paths.
    /// - The interpreter, C backend, and Cranelift backend currently preserve the edge but do
    ///   not lower destructor or runtime free logic for the dropped place.
    /// - Ownership-sensitive destruction remains a follow-up lowering step.
    ///
    /// This keeps drop edges explicit in MIR even while backend implementations are still
    /// converging on full ownership-based destruction.
    Drop { place: Place, target: BlockId },
}

// ─── Basic block ──────────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct BasicBlock {
    pub id: BlockId,
    pub statements: Vec<Statement>,
    pub terminator: Option<Terminator>,
}

impl BasicBlock {
    pub fn new(id: BlockId) -> Self {
        Self {
            id,
            statements: Vec::new(),
            terminator: None,
        }
    }
}

// ─── MIR function ─────────────────────────────────────────────────────────────

/// Declaration of a local variable in a function.
#[derive(Debug, Clone)]
pub struct LocalDecl {
    pub id: Local,
    /// The HIR id this local was derived from (for debug info).
    pub hir_id: Option<HirId>,
    pub ty: Ty,
    pub mutable: bool,
    pub name: Option<String>,
    pub span: Option<Span>,
}

/// A fully lowered function body in MIR.
#[derive(Debug, Clone)]
pub struct MirFn {
    pub def: DefId,
    /// Index 0 = return place, indices 1..=argc = argument locals.
    pub locals: Vec<LocalDecl>,
    pub argc: usize,
    pub basic_blocks: Vec<BasicBlock>,
    pub span: Option<Span>,
    /// True for `extern "…"` declarations — no MIR body is generated.
    pub is_extern: bool,
    /// ABI string for extern fns (e.g. `"C"`).
    pub abi: Option<String>,
}

impl MirFn {
    pub fn new(def: DefId) -> Self {
        Self {
            def,
            locals: Vec::new(),
            argc: 0,
            basic_blocks: Vec::new(),
            span: None,
            is_extern: false,
            abi: None,
        }
    }

    pub fn fresh_block(&mut self) -> BlockId {
        let id = BlockId(self.basic_blocks.len() as u32);
        self.basic_blocks.push(BasicBlock::new(id));
        id
    }

    pub fn fresh_local(&mut self, ty: Ty, name: Option<String>, span: Option<Span>) -> Local {
        let id = Local(self.locals.len() as u32);
        self.locals.push(LocalDecl {
            id,
            hir_id: None,
            ty,
            mutable: true,
            name,
            span,
        });
        id
    }

    pub fn block_mut(&mut self, id: BlockId) -> &mut BasicBlock {
        &mut self.basic_blocks[id.0 as usize]
    }
}

#[derive(Debug, Clone)]
pub struct MirConstItem {
    pub def: DefId,
    pub ty: Ty,
    pub value: MirConst,
    pub span: Option<Span>,
}

// ─── MIR module ───────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Default)]
pub struct MirModule {
    pub consts: Vec<MirConstItem>,
    pub functions: Vec<MirFn>,
    pub enum_variant_indices: HashMap<DefId, (DefId, usize)>,
    pub enum_variant_names: HashMap<DefId, Vec<String>>,
    pub struct_field_names: HashMap<DefId, Vec<String>>,
    pub display_impls: HashSet<DefId>,
    pub def_names: HashMap<DefId, String>,
}

struct ModuleLowerer<'a> {
    diagnostics: Vec<Diagnostic>,
    mir: MirModule,
    field_indices: HashMap<DefId, HashMap<String, usize>>,
    enum_variant_indices: HashMap<DefId, (DefId, usize)>,
    enum_variant_names: HashMap<DefId, Vec<String>>,
    fn_ret_tys: HashMap<DefId, Ty>,
    defs_by_name: HashMap<String, DefId>,
    synthetic_counter: u32,
    _hir: &'a crate::hir::HirModule,
}

impl<'a> ModuleLowerer<'a> {
    fn new(hir: &'a crate::hir::HirModule) -> Self {
        let mut lowerer = Self {
            diagnostics: Vec::new(),
            mir: MirModule::default(),
            field_indices: HashMap::new(),
            enum_variant_indices: HashMap::new(),
            enum_variant_names: HashMap::new(),
            fn_ret_tys: HashMap::new(),
            defs_by_name: hir
                .def_names
                .iter()
                .map(|(def, name)| (name.clone(), *def))
                .collect(),
            synthetic_counter: 0,
            _hir: hir,
        };
        lowerer.collect_field_indices(hir);
        lowerer.collect_enum_variants(hir);
        lowerer.collect_fn_ret_tys(hir);
        lowerer
    }

    fn collect_field_indices(&mut self, module: &crate::hir::HirModule) {
        for strukt in &module.structs {
            self.field_indices.insert(
                strukt.def,
                strukt
                    .fields
                    .iter()
                    .enumerate()
                    .map(|(index, field)| (field.name.clone(), index))
                    .collect(),
            );
        }
        for nested in &module.modules {
            if let Some(body) = &nested.body {
                self.collect_field_indices(body);
            }
        }
    }

    fn collect_enum_variants(&mut self, module: &crate::hir::HirModule) {
        for enum_def in &module.enums {
            self.enum_variant_names.insert(
                enum_def.def,
                enum_def
                    .variants
                    .iter()
                    .map(|variant| variant.name.clone())
                    .collect(),
            );
            for (variant_idx, variant) in enum_def.variants.iter().enumerate() {
                self.enum_variant_indices
                    .insert(variant.def, (enum_def.def, variant_idx));
            }
        }
        for nested in &module.modules {
            if let Some(body) = &nested.body {
                self.collect_enum_variants(body);
            }
        }
    }

    fn collect_fn_ret_tys(&mut self, module: &crate::hir::HirModule) {
        for function in &module.functions {
            self.fn_ret_tys
                .insert(function.def, function.ret_ty.clone());
        }
        for trait_def in &module.traits {
            self.collect_assoc_fn_ret_tys(&trait_def.items);
        }
        for interface_def in &module.interfaces {
            self.collect_assoc_fn_ret_tys(&interface_def.items);
        }
        for ability_def in &module.abilities {
            self.collect_assoc_fn_ret_tys(&ability_def.items);
        }
        for imp in &module.impls {
            for item in &imp.items {
                if let crate::hir::HirImplItem::Method(method) = item {
                    self.fn_ret_tys.insert(method.def, method.ret_ty.clone());
                }
            }
        }
        for nested in &module.modules {
            if let Some(body) = &nested.body {
                self.collect_fn_ret_tys(body);
            }
        }
    }

    fn collect_assoc_fn_ret_tys(&mut self, items: &[crate::hir::HirAssocItem]) {
        for item in items {
            if let crate::hir::HirAssocItem::Method(method) = item {
                self.fn_ret_tys.insert(method.def, method.ret_ty.clone());
            }
        }
    }

    fn lower_module(mut self, module: &'a crate::hir::HirModule) -> (MirModule, Vec<Diagnostic>) {
        self.mir.enum_variant_indices = self.enum_variant_indices.clone();
        self.mir.enum_variant_names = self.enum_variant_names.clone();
        self.mir.struct_field_names = self
            .field_indices
            .iter()
            .map(|(def, indices)| {
                let mut names = vec![String::new(); indices.len()];
                for (name, index) in indices {
                    if let Some(slot) = names.get_mut(*index) {
                        *slot = name.clone();
                    }
                }
                (*def, names)
            })
            .collect();
        self.mir.display_impls = collect_display_impls(module, &module.def_names);
        self.mir.def_names = module.def_names.clone();
        self.lower_module_recursive(module);
        (self.mir, self.diagnostics)
    }

    fn lower_module_recursive(&mut self, module: &'a crate::hir::HirModule) {
        for item in &module.consts {
            if let Some(value) = lower_const_expr(&item.value) {
                self.mir.consts.push(MirConstItem {
                    def: item.def,
                    ty: item.ty.clone(),
                    value,
                    span: Some(item.span),
                });
            } else {
                self.diagnostics.push(
                    Diagnostic::error(
                        "MIR lowering for this const initializer is not implemented yet",
                    )
                    .with_span(item.span),
                );
            }
        }

        for function in &module.functions {
            let lowerer = FunctionLowerer::new(
                self._hir,
                &self.field_indices,
                &self.enum_variant_indices,
                &self.enum_variant_names,
                &self.fn_ret_tys,
                &self.defs_by_name,
                &mut self.diagnostics,
                &mut self.synthetic_counter,
                function,
            );
            let (function_mir, nested) = lowerer.lower();
            self.mir.functions.push(function_mir);
            self.mir.functions.extend(nested);
        }

        self.lower_assoc_methods(&module.traits);
        self.lower_assoc_methods(&module.interfaces);
        self.lower_assoc_methods(&module.abilities);

        for imp in &module.impls {
            for item in &imp.items {
                let crate::hir::HirImplItem::Method(method) = item else {
                    continue;
                };
                let lowerer = FunctionLowerer::new(
                    self._hir,
                    &self.field_indices,
                    &self.enum_variant_indices,
                    &self.enum_variant_names,
                    &self.fn_ret_tys,
                    &self.defs_by_name,
                    &mut self.diagnostics,
                    &mut self.synthetic_counter,
                    method,
                );
                let (function_mir, nested) = lowerer.lower();
                self.mir.functions.push(function_mir);
                self.mir.functions.extend(nested);
            }
        }

        for nested in &module.modules {
            if let Some(body) = &nested.body {
                self.lower_module_recursive(body);
            }
        }
    }

    fn lower_assoc_methods<T>(&mut self, defs: &[T])
    where
        T: AssocItemOwner,
    {
        for def in defs {
            for item in def.assoc_items() {
                let crate::hir::HirAssocItem::Method(method) = item else {
                    continue;
                };
                if method.body.is_none() {
                    continue;
                }
                let lowerer = FunctionLowerer::new(
                    self._hir,
                    &self.field_indices,
                    &self.enum_variant_indices,
                    &self.enum_variant_names,
                    &self.fn_ret_tys,
                    &self.defs_by_name,
                    &mut self.diagnostics,
                    &mut self.synthetic_counter,
                    method,
                );
                let (function_mir, nested) = lowerer.lower();
                self.mir.functions.push(function_mir);
                self.mir.functions.extend(nested);
            }
        }
    }
}

trait AssocItemOwner {
    fn assoc_items(&self) -> &[crate::hir::HirAssocItem];
}

impl AssocItemOwner for crate::hir::HirTrait {
    fn assoc_items(&self) -> &[crate::hir::HirAssocItem] {
        &self.items
    }
}

impl AssocItemOwner for crate::hir::HirInterface {
    fn assoc_items(&self) -> &[crate::hir::HirAssocItem] {
        &self.items
    }
}

impl AssocItemOwner for crate::hir::HirAbility {
    fn assoc_items(&self) -> &[crate::hir::HirAssocItem] {
        &self.items
    }
}

fn collect_display_impls(
    module: &crate::hir::HirModule,
    def_names: &HashMap<DefId, String>,
) -> HashSet<DefId> {
    let mut impls = HashSet::new();
    collect_display_impls_in_module(module, def_names, &mut impls);
    impls
}

fn collect_display_impls_in_module(
    module: &crate::hir::HirModule,
    def_names: &HashMap<DefId, String>,
    out: &mut HashSet<DefId>,
) {
    for imp in &module.impls {
        let Some(Ty::Named {
            def: ability_def, ..
        }) = &imp.trait_ref
        else {
            continue;
        };
        let is_display = def_names
            .get(ability_def)
            .and_then(|name| name.rsplit("::").next())
            .is_some_and(|name| name == "Display");
        if !is_display {
            continue;
        }
        if let Some(owner_def) = match &imp.self_ty {
            Ty::Named { def, .. } => Some(*def),
            Ty::Ref { inner, .. } => match inner.as_ref() {
                Ty::Named { def, .. } => Some(*def),
                _ => None,
            },
            _ => None,
        } {
            out.insert(owner_def);
        }
    }
    for nested in &module.modules {
        if let Some(body) = &nested.body {
            collect_display_impls_in_module(body, def_names, out);
        }
    }
}

struct FunctionLowerer<'a> {
    hir: &'a crate::hir::HirModule,
    diagnostics: &'a mut Vec<Diagnostic>,
    field_indices: &'a HashMap<DefId, HashMap<String, usize>>,
    enum_variant_indices: &'a HashMap<DefId, (DefId, usize)>,
    enum_variant_names: &'a HashMap<DefId, Vec<String>>,
    fn_ret_tys: &'a HashMap<DefId, Ty>,
    synthetic_counter: &'a mut u32,
    defs_by_name: &'a HashMap<String, DefId>,
    body: Option<&'a crate::hir::HirExpr>,
    mir: MirFn,
    locals: HashMap<HirId, Local>,
    current_block: BlockId,
    return_local: Local,
    break_targets: Vec<BlockId>,
    continue_targets: Vec<BlockId>,
    deferred_exprs: Vec<&'a crate::hir::HirExpr>,
    errdeferred_exprs: Vec<&'a crate::hir::HirExpr>,
    lowering_err_cleanup: bool,
    nested_functions: Vec<MirFn>,
}

enum TryCarrierMirTy {
    Result {
        def: DefId,
        ok_variant: usize,
        err_variant: usize,
        ok_ty: Ty,
        err_ty: Ty,
    },
    Option {
        def: DefId,
        some_variant: usize,
        none_variant: usize,
        some_ty: Ty,
    },
}

impl<'a> FunctionLowerer<'a> {
    fn new(
        hir: &'a crate::hir::HirModule,
        field_indices: &'a HashMap<DefId, HashMap<String, usize>>,
        enum_variant_indices: &'a HashMap<DefId, (DefId, usize)>,
        enum_variant_names: &'a HashMap<DefId, Vec<String>>,
        fn_ret_tys: &'a HashMap<DefId, Ty>,
        defs_by_name: &'a HashMap<String, DefId>,
        diagnostics: &'a mut Vec<Diagnostic>,
        synthetic_counter: &'a mut u32,
        function: &'a crate::hir::HirFn,
    ) -> Self {
        Self::new_from_parts(
            hir,
            field_indices,
            enum_variant_indices,
            enum_variant_names,
            fn_ret_tys,
            defs_by_name,
            diagnostics,
            synthetic_counter,
            function.def,
            Some(function.span),
            &function.params,
            &function.ret_ty,
            function.body.as_ref(),
        )
    }

    fn new_from_parts<'b>(
        hir: &'b crate::hir::HirModule,
        field_indices: &'b HashMap<DefId, HashMap<String, usize>>,
        enum_variant_indices: &'b HashMap<DefId, (DefId, usize)>,
        enum_variant_names: &'b HashMap<DefId, Vec<String>>,
        fn_ret_tys: &'b HashMap<DefId, Ty>,
        defs_by_name: &'b HashMap<String, DefId>,
        diagnostics: &'b mut Vec<Diagnostic>,
        synthetic_counter: &'b mut u32,
        def: DefId,
        span: Option<Span>,
        params: &'b [crate::hir::HirParam],
        ret_ty: &'b Ty,
        body: Option<&'b crate::hir::HirExpr>,
    ) -> FunctionLowerer<'b> {
        let mut mir = MirFn::new(def);
        mir.span = span;
        let entry = mir.fresh_block();
        let return_local = mir.fresh_local(ret_ty.clone(), Some("return".into()), span);
        mir.locals[return_local.0 as usize].mutable = true;

        let mut locals = HashMap::new();
        for param in params {
            let local = mir.fresh_local(param.ty.clone(), None, span);
            let decl = &mut mir.locals[local.0 as usize];
            decl.hir_id = Some(param.binding);
            decl.mutable = param.mutable;
            locals.insert(param.binding, local);
        }
        mir.argc = params.len();

        FunctionLowerer {
            hir,
            diagnostics,
            field_indices,
            enum_variant_indices,
            enum_variant_names,
            fn_ret_tys,
            defs_by_name,
            synthetic_counter,
            body,
            mir,
            locals,
            current_block: entry,
            return_local,
            break_targets: Vec::new(),
            continue_targets: Vec::new(),
            deferred_exprs: Vec::new(),
            errdeferred_exprs: Vec::new(),
            lowering_err_cleanup: false,
            nested_functions: Vec::new(),
        }
    }

    fn lower(mut self) -> (MirFn, Vec<MirFn>) {
        let span = self.mir.span;
        if let Some(body) = self.body {
            let body_span = body.span;
            let result = self.lower_expr_to_operand(body);
            if self.mir.block_mut(self.current_block).terminator.is_none() {
                self.emit_deferred(body_span);
                self.emit_assign(
                    Place::local(self.return_local),
                    Rvalue::Use(result),
                    Some(body_span),
                );
                self.set_terminator(TerminatorKind::Return, Some(body_span));
            }
        } else if self.mir.block_mut(self.current_block).terminator.is_none() {
            if let Some(span) = span {
                self.emit_deferred(span);
            }
            self.set_terminator(TerminatorKind::Return, span);
        }
        (self.mir, self.nested_functions)
    }

    fn fresh_temp(&mut self, ty: Ty, span: Span) -> Local {
        let local = self.mir.fresh_local(ty, None, Some(span));
        self.emit_statement(StatementKind::StorageLive(local), Some(span));
        local
    }

    fn emit_statement(&mut self, kind: StatementKind, span: Option<Span>) {
        self.mir
            .block_mut(self.current_block)
            .statements
            .push(Statement { kind, span });
    }

    fn emit_assign(&mut self, place: Place, value: Rvalue, span: Option<Span>) {
        self.emit_statement(StatementKind::Assign(place, value), span);
    }

    fn set_terminator(&mut self, kind: TerminatorKind, span: Option<Span>) {
        self.mir.block_mut(self.current_block).terminator = Some(Terminator { kind, span });
    }

    fn ensure_fallthrough_block(&mut self) {
        if self.mir.block_mut(self.current_block).terminator.is_some() {
            self.current_block = self.mir.fresh_block();
        }
    }

    fn fresh_synthetic_def(&mut self) -> DefId {
        let index = 0x8000_0000u32.saturating_add(*self.synthetic_counter);
        *self.synthetic_counter += 1;
        DefId {
            file: self.mir.def.file,
            index,
        }
    }

    fn def_by_name(&self, name: &str) -> Option<DefId> {
        self.defs_by_name.get(name).copied()
    }

    fn fallback_method_id_from_operand(
        &self,
        receiver: &Operand,
        method_name: &str,
    ) -> Option<DefId> {
        let receiver_ty = match receiver {
            Operand::Copy(local) | Operand::Move(local) => self
                .mir
                .locals
                .get(local.0 as usize)
                .map(|local| local.ty.clone())?,
            _ => return None,
        };
        let owner_name = self.method_owner_name(&receiver_ty)?;
        self.def_by_name(&format!("{owner_name}::{method_name}"))
            .or_else(|| {
                owner_name
                    .rsplit("::")
                    .next()
                    .and_then(|short| self.def_by_name(&format!("{short}::{method_name}")))
            })
    }

    fn method_owner_name(&self, ty: &Ty) -> Option<String> {
        match ty {
            Ty::Named { def, .. } | Ty::DynTrait(def) => self
                .defs_by_name
                .iter()
                .find_map(|(name, candidate)| (*candidate == *def).then_some(name.clone())),
            Ty::String => Some("std::core::String".to_string()),
            Ty::Ref { inner, .. } => self.method_owner_name(inner),
            _ => None,
        }
    }

    fn for_iter_binding_ty(&self, iter_ty: &Ty) -> Option<Ty> {
        match iter_ty {
            Ty::Named { def, args } => self
                .defs_by_name
                .get("std::collections::Vec")
                .or_else(|| self.defs_by_name.get("Vec"))
                .filter(|vec_def| *vec_def == def)
                .and_then(|_| args.first().cloned())
                .map(|elem| Ty::Ref {
                    mutable: false,
                    inner: Box::new(elem),
                })
                .or_else(|| {
                    self.defs_by_name
                        .get("std::collections::HashMap")
                        .or_else(|| self.defs_by_name.get("HashMap"))
                        .filter(|map_def| *map_def == def)
                        .and_then(|_| match args.as_slice() {
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
                        })
                }),
            Ty::Array { elem, .. } | Ty::Slice(elem) => Some(Ty::Ref {
                mutable: false,
                inner: elem.clone(),
            }),
            _ => None,
        }
    }

    fn emit_call_operand(
        &mut self,
        callee: Operand,
        args: Vec<Operand>,
        ret_ty: Ty,
        span: Span,
    ) -> Operand {
        let dest = self.fresh_temp(ret_ty, span);
        let unwind = if !self.errdeferred_exprs.is_empty() && !self.lowering_err_cleanup {
            Some(self.mir.fresh_block())
        } else {
            None
        };
        let next = self.mir.fresh_block();
        self.set_terminator(
            TerminatorKind::Call {
                callee,
                args,
                destination: Place::local(dest),
                target: Some(next),
                unwind,
            },
            Some(span),
        );
        if let Some(unwind_block) = unwind {
            self.current_block = unwind_block;
            self.emit_errdeferred(span);
            if self.mir.block_mut(self.current_block).terminator.is_none() {
                self.set_terminator(TerminatorKind::ErrdeferUnwind(next), Some(span));
            }
        }
        self.current_block = next;
        Operand::Copy(dest)
    }

    fn lower_discriminant(&mut self, place: Place, span: Span) -> Local {
        let discr_local = self.fresh_temp(Ty::Uint(crate::hir::UintSize::U64), span);
        self.emit_assign(
            Place::local(discr_local),
            Rvalue::Discriminant(place),
            Some(span),
        );
        discr_local
    }

    fn emit_deferred(&mut self, span: Span) {
        let deferred = self.deferred_exprs.clone();
        for expr in deferred.into_iter().rev() {
            self.ensure_fallthrough_block();
            let _ = self.lower_expr_to_operand(expr);
        }
        self.ensure_fallthrough_block();
        let _ = span;
    }

    fn emit_errdeferred(&mut self, span: Span) {
        let errdeferred = self.errdeferred_exprs.clone();
        let saved = self.lowering_err_cleanup;
        self.lowering_err_cleanup = true;
        for expr in errdeferred.into_iter().rev() {
            self.ensure_fallthrough_block();
            let _ = self.lower_expr_to_operand(expr);
        }
        self.lowering_err_cleanup = saved;
        self.ensure_fallthrough_block();
        let _ = span;
    }

    fn operand_to_local(&mut self, operand: Operand, ty: Ty, span: Span) -> Local {
        match operand {
            Operand::Copy(local) | Operand::Move(local) => local,
            other => {
                let temp = self.fresh_temp(ty, span);
                self.emit_assign(Place::local(temp), Rvalue::Use(other), Some(span));
                temp
            }
        }
    }

    fn lower_stmt(&mut self, stmt: &'a crate::hir::HirStmt) {
        self.ensure_fallthrough_block();
        match &stmt.kind {
            crate::hir::HirStmtKind::Let {
                binding,
                mutable,
                ty,
                init,
            } => {
                let local_ty = init
                    .as_ref()
                    .filter(|_| matches!(ty, Ty::Var(_)))
                    .map(|init| self.concrete_expr_ty(init))
                    .unwrap_or_else(|| ty.clone());
                let local = self.mir.fresh_local(local_ty, None, Some(stmt.span));
                let decl = &mut self.mir.locals[local.0 as usize];
                decl.hir_id = Some(*binding);
                decl.mutable = *mutable;
                self.locals.insert(*binding, local);
                self.emit_statement(StatementKind::StorageLive(local), Some(stmt.span));
                if let Some(init) = init {
                    let value = self.lower_expr_to_operand(init);
                    self.emit_assign(Place::local(local), Rvalue::Use(value), Some(stmt.span));
                }
            }
            crate::hir::HirStmtKind::Expr(expr) => {
                let _ = self.lower_expr_to_operand(expr);
            }
            crate::hir::HirStmtKind::Defer(expr) => {
                self.deferred_exprs.push(expr);
            }
            crate::hir::HirStmtKind::Errdefer(expr) => {
                self.errdeferred_exprs.push(expr);
            }
            crate::hir::HirStmtKind::Use(_) => {}
        }
    }

    fn lower_expr_to_operand(&mut self, expr: &'a crate::hir::HirExpr) -> Operand {
        use crate::hir::HirExprKind::*;

        self.ensure_fallthrough_block();
        match &expr.kind {
            Lit(lit) => Operand::Const(lower_lit(lit)),
            Var(id) => self
                .locals
                .get(id)
                .copied()
                .map(Operand::Copy)
                .unwrap_or(Operand::Const(MirConst::Undef)),
            DefRef(def) => {
                if let Some((enum_def, variant_idx)) = self.enum_variant_indices.get(def).copied() {
                    let temp = self.fresh_temp(self.concrete_expr_ty(expr), expr.span);
                    self.emit_assign(
                        Place::local(temp),
                        Rvalue::Aggregate(
                            AggregateKind::Enum {
                                def: enum_def,
                                variant_idx,
                            },
                            Vec::new(),
                        ),
                        Some(expr.span),
                    );
                    Operand::Copy(temp)
                } else {
                    Operand::Def(*def)
                }
            }
            Block(stmts, tail) => {
                for stmt in stmts {
                    self.lower_stmt(stmt);
                }
                tail.as_deref()
                    .map(|tail| self.lower_expr_to_operand(tail))
                    .unwrap_or(Operand::Const(MirConst::Unit))
            }
            Tuple(elems) => self.lower_aggregate(expr, AggregateKind::Tuple, elems),
            Array(elems) => {
                let elem_ty = match &expr.ty {
                    Ty::Array { elem, .. } => *elem.clone(),
                    _ => Ty::Unit,
                };
                self.lower_aggregate(expr, AggregateKind::Array(elem_ty), elems)
            }
            Struct { def, fields, .. } => {
                let operands = fields
                    .iter()
                    .map(|(_, value)| self.lower_expr_to_operand(value))
                    .collect();
                let temp = self.fresh_temp(self.concrete_expr_ty(expr), expr.span);
                self.emit_assign(
                    Place::local(temp),
                    Rvalue::Aggregate(AggregateKind::Struct(*def), operands),
                    Some(expr.span),
                );
                Operand::Copy(temp)
            }
            Call { callee, args } => self.lower_call(expr, callee, args),
            MethodCall {
                receiver,
                method_id,
                method_name,
                args,
                ..
            } => {
                let receiver_operand = self.lower_expr_to_operand(receiver);
                let mut call_args = Vec::with_capacity(args.len() + 1);
                call_args.push(receiver_operand.clone());
                call_args.extend(args.iter().map(|arg| self.lower_expr_to_operand(arg)));
                let resolved_method = if method_id.index == u32::MAX {
                    self.fallback_method_id_from_operand(&receiver_operand, method_name)
                        .unwrap_or(*method_id)
                } else {
                    *method_id
                };
                self.lower_call_with_operand(expr, Operand::Def(resolved_method), call_args)
            }
            BinOp {
                op: crate::hir::HirBinOp::And,
                lhs,
                rhs,
            } => self.lower_short_circuit(expr, lhs, rhs, false),
            BinOp {
                op: crate::hir::HirBinOp::Or,
                lhs,
                rhs,
            } => self.lower_short_circuit(expr, lhs, rhs, true),
            BinOp { op, lhs, rhs } => {
                let lhs = self.lower_expr_to_operand(lhs);
                let rhs = self.lower_expr_to_operand(rhs);
                let temp = self.fresh_temp(self.concrete_expr_ty(expr), expr.span);
                self.emit_assign(
                    Place::local(temp),
                    Rvalue::BinaryOp {
                        op: lower_bin_op(*op),
                        lhs,
                        rhs,
                    },
                    Some(expr.span),
                );
                Operand::Copy(temp)
            }
            UnaryOp { op, operand } => {
                let operand = self.lower_expr_to_operand(operand);
                let temp = self.fresh_temp(self.concrete_expr_ty(expr), expr.span);
                self.emit_assign(
                    Place::local(temp),
                    Rvalue::UnaryOp {
                        op: lower_unary_op(*op),
                        operand,
                    },
                    Some(expr.span),
                );
                Operand::Copy(temp)
            }
            Assign { target, value } => {
                if let Some(place) = self.lower_place(target) {
                    let value = self.lower_expr_to_operand(value);
                    self.emit_assign(place, Rvalue::Use(value), Some(expr.span));
                    Operand::Const(MirConst::Unit)
                } else {
                    self.unsupported(expr.span, "assignment target");
                    Operand::Const(MirConst::Undef)
                }
            }
            Return(value) => {
                if let Some(value) = value {
                    let value = self.lower_expr_to_operand(value);
                    self.emit_assign(
                        Place::local(self.return_local),
                        Rvalue::Use(value),
                        Some(expr.span),
                    );
                }
                self.emit_deferred(expr.span);
                self.set_terminator(TerminatorKind::Return, Some(expr.span));
                Operand::Const(MirConst::Undef)
            }
            If {
                condition,
                then_branch,
                else_branch,
            } => self.lower_if(expr, condition, then_branch, else_branch.as_deref()),
            While { condition, body } => {
                self.lower_while(expr, condition, body);
                Operand::Const(MirConst::Unit)
            }
            Loop(body) => {
                self.lower_loop(expr, body);
                Operand::Const(MirConst::Unit)
            }
            Break(_) => {
                if let Some(target) = self.break_targets.last().copied() {
                    self.set_terminator(TerminatorKind::Goto(target), Some(expr.span));
                } else {
                    self.unsupported(expr.span, "`break` outside loop");
                }
                Operand::Const(MirConst::Undef)
            }
            Continue => {
                if let Some(target) = self.continue_targets.last().copied() {
                    self.set_terminator(TerminatorKind::Goto(target), Some(expr.span));
                } else {
                    self.unsupported(expr.span, "`continue` outside loop");
                }
                Operand::Const(MirConst::Undef)
            }
            Match { scrutinee, arms } => self.lower_match(expr, scrutinee, arms),
            Ref { mutable, expr } => {
                if let Some(place) = self.lower_place(expr) {
                    let temp = self.fresh_temp(expr.ty.clone(), expr.span);
                    self.emit_assign(
                        Place::local(temp),
                        Rvalue::Ref {
                            mutable: *mutable,
                            place,
                        },
                        Some(expr.span),
                    );
                    Operand::Copy(temp)
                } else {
                    self.unsupported(expr.span, "borrow expression");
                    Operand::Const(MirConst::Undef)
                }
            }
            Field { .. } | Index { .. } | Deref(_) => {
                if let Some(place) = self.lower_read_place(expr) {
                    let temp = self.fresh_temp(expr.ty.clone(), expr.span);
                    self.emit_assign(Place::local(temp), Rvalue::Read(place), Some(expr.span));
                    Operand::Copy(temp)
                } else {
                    self.unsupported(expr.span, "place expression");
                    Operand::Const(MirConst::Undef)
                }
            }
            Cast {
                expr: inner,
                target_ty,
            } => {
                let operand = self.lower_expr_to_operand(inner);
                let temp = self.fresh_temp(target_ty.clone(), expr.span);
                self.emit_assign(
                    Place::local(temp),
                    Rvalue::Cast {
                        kind: CastKind::Transmute,
                        operand,
                        target_ty: target_ty.clone(),
                    },
                    Some(expr.span),
                );
                Operand::Copy(temp)
            }
            Closure {
                params,
                ret_ty,
                body,
                captures,
            } => {
                let def = self.fresh_synthetic_def();
                let mut synthetic_params = Vec::with_capacity(captures.len() + params.len());
                let mut capture_operands = Vec::with_capacity(captures.len());
                for capture in captures {
                    let Some(local) = self.locals.get(capture).copied() else {
                        self.unsupported(expr.span, "closure capture binding");
                        return Operand::Const(MirConst::Undef);
                    };
                    let decl = &self.mir.locals[local.0 as usize];
                    synthetic_params.push(crate::hir::HirParam {
                        id: *capture,
                        binding: *capture,
                        mutable: decl.mutable,
                        ty: decl.ty.clone(),
                    });
                    capture_operands.push(Operand::Copy(local));
                }
                synthetic_params.extend(params.iter().cloned());
                let lowerer = Self::new_from_parts(
                    self.hir,
                    self.field_indices,
                    self.enum_variant_indices,
                    self.enum_variant_names,
                    self.fn_ret_tys,
                    self.defs_by_name,
                    self.diagnostics,
                    self.synthetic_counter,
                    def,
                    Some(expr.span),
                    &synthetic_params,
                    ret_ty,
                    Some(body),
                );
                let (mir_fn, nested) = lowerer.lower();
                self.nested_functions.push(mir_fn);
                self.nested_functions.extend(nested);

                let temp = self.fresh_temp(expr.ty.clone(), expr.span);
                self.emit_assign(
                    Place::local(temp),
                    Rvalue::Aggregate(AggregateKind::Closure(def), capture_operands),
                    Some(expr.span),
                );
                Operand::Copy(temp)
            }
            Unsafe(body) | AsyncBlock(body) | Defer(body) | Await(body) => {
                self.lower_expr_to_operand(body)
            }
            Try(body) => self.lower_try(expr, body),
            Errdefer(body) => {
                self.errdeferred_exprs.push(body);
                Operand::Const(MirConst::Unit)
            }
            ForDesugared {
                iter,
                binding,
                body,
            } => {
                self.lower_for_desugared(expr, iter, *binding, body);
                Operand::Const(MirConst::Unit)
            }
            Repeat { .. } | Range { .. } => {
                self.unsupported(expr.span, "expression");
                Operand::Const(MirConst::Undef)
            }
        }
    }

    fn lower_aggregate(
        &mut self,
        expr: &'a crate::hir::HirExpr,
        kind: AggregateKind,
        elems: &'a [crate::hir::HirExpr],
    ) -> Operand {
        let aggregate_ty = match &kind {
            AggregateKind::Tuple if matches!(expr.ty, Ty::Var(_)) => Ty::Tuple(
                elems
                    .iter()
                    .map(|elem| self.concrete_expr_ty(elem))
                    .collect::<Vec<_>>(),
            ),
            AggregateKind::Array(_) if matches!(expr.ty, Ty::Var(_)) => {
                let elem_ty = elems
                    .first()
                    .map(|elem| self.concrete_expr_ty(elem))
                    .unwrap_or(Ty::Unit);
                Ty::Array {
                    elem: Box::new(elem_ty),
                    len: elems.len(),
                }
            }
            _ => expr.ty.clone(),
        };
        let operands = elems
            .iter()
            .map(|elem| self.lower_expr_to_operand(elem))
            .collect();
        let temp = self.fresh_temp(aggregate_ty, expr.span);
        self.emit_assign(
            Place::local(temp),
            Rvalue::Aggregate(kind, operands),
            Some(expr.span),
        );
        Operand::Copy(temp)
    }

    fn lower_call(
        &mut self,
        expr: &'a crate::hir::HirExpr,
        callee: &'a crate::hir::HirExpr,
        args: &'a [crate::hir::HirExpr],
    ) -> Operand {
        if let crate::hir::HirExprKind::DefRef(def) = callee.kind {
            if let Some((enum_def, variant_idx)) = self.enum_variant_indices.get(&def).copied() {
                let operands = args
                    .iter()
                    .map(|arg| self.lower_expr_to_operand(arg))
                    .collect();
                let temp = self.fresh_temp(self.concrete_expr_ty(expr), expr.span);
                self.emit_assign(
                    Place::local(temp),
                    Rvalue::Aggregate(
                        AggregateKind::Enum {
                            def: enum_def,
                            variant_idx,
                        },
                        operands,
                    ),
                    Some(expr.span),
                );
                return Operand::Copy(temp);
            }
            if let Some(fields) = self.field_indices.get(&def) {
                let is_tuple_or_unit_struct = fields.len() == args.len()
                    && (0..fields.len()).all(|index| {
                        fields
                            .get(&index.to_string())
                            .is_some_and(|value| *value == index)
                    });
                if is_tuple_or_unit_struct {
                    let operands = args
                        .iter()
                        .map(|arg| self.lower_expr_to_operand(arg))
                        .collect();
                    let temp = self.fresh_temp(self.concrete_expr_ty(expr), expr.span);
                    self.emit_assign(
                        Place::local(temp),
                        Rvalue::Aggregate(AggregateKind::Struct(def), operands),
                        Some(expr.span),
                    );
                    return Operand::Copy(temp);
                }
            }
        }
        let callee = self.lower_expr_to_operand(callee);
        let args = args
            .iter()
            .map(|arg| self.lower_expr_to_operand(arg))
            .collect();
        self.lower_call_with_operand(expr, callee, args)
    }

    fn lower_call_with_operand(
        &mut self,
        expr: &crate::hir::HirExpr,
        callee: Operand,
        args: Vec<Operand>,
    ) -> Operand {
        self.emit_call_operand(callee, args, self.concrete_expr_ty(expr), expr.span)
    }

    fn lower_short_circuit(
        &mut self,
        expr: &'a crate::hir::HirExpr,
        lhs: &'a crate::hir::HirExpr,
        rhs: &'a crate::hir::HirExpr,
        short_value: bool,
    ) -> Operand {
        let lhs = self.lower_expr_to_operand(lhs);
        let rhs_block = self.mir.fresh_block();
        let short_block = self.mir.fresh_block();
        let join_block = self.mir.fresh_block();
        let dest = self.fresh_temp(expr.ty.clone(), expr.span);

        let (targets, otherwise) = if short_value {
            (vec![(1, short_block)], rhs_block)
        } else {
            (vec![(1, rhs_block)], short_block)
        };

        self.set_terminator(
            TerminatorKind::SwitchInt {
                discriminant: lhs,
                targets,
                otherwise,
            },
            Some(expr.span),
        );

        self.current_block = rhs_block;
        let rhs_value = self.lower_expr_to_operand(rhs);
        if self.mir.block_mut(self.current_block).terminator.is_none() {
            self.emit_assign(Place::local(dest), Rvalue::Use(rhs_value), Some(rhs.span));
            self.set_terminator(TerminatorKind::Goto(join_block), Some(rhs.span));
        }

        self.current_block = short_block;
        if self.mir.block_mut(self.current_block).terminator.is_none() {
            self.emit_assign(
                Place::local(dest),
                Rvalue::Use(Operand::Const(MirConst::Bool(short_value))),
                Some(expr.span),
            );
            self.set_terminator(TerminatorKind::Goto(join_block), Some(expr.span));
        }

        self.current_block = join_block;
        Operand::Copy(dest)
    }

    fn lower_for_desugared(
        &mut self,
        expr: &'a crate::hir::HirExpr,
        iter: &'a crate::hir::HirExpr,
        binding: HirId,
        body: &'a crate::hir::HirExpr,
    ) {
        let iter_ty = match &iter.kind {
            crate::hir::HirExprKind::Var(id) => self
                .locals
                .get(id)
                .map(|local| self.mir.locals[local.0 as usize].ty.clone())
                .unwrap_or_else(|| self.concrete_expr_ty(iter)),
            _ => self.concrete_expr_ty(iter),
        };
        let Some(binding_ty) = self.for_iter_binding_ty(&iter_ty) else {
            self.unsupported(expr.span, "for-loop iterable");
            return;
        };

        let saved_binding = self.locals.get(&binding).copied();
        let iter_operand = self.lower_expr_to_operand(iter);
        let iter_local = self.operand_to_local(iter_operand, iter_ty.clone(), iter.span);
        let index_local = self.fresh_temp(Ty::Uint(crate::hir::UintSize::USize), expr.span);
        self.emit_assign(
            Place::local(index_local),
            Rvalue::Use(Operand::Const(MirConst::Uint(0))),
            Some(expr.span),
        );

        let binding_local = self
            .mir
            .fresh_local(binding_ty.clone(), None, Some(expr.span));
        let decl = &mut self.mir.locals[binding_local.0 as usize];
        decl.hir_id = Some(binding);
        decl.mutable = false;
        self.locals.insert(binding, binding_local);
        self.emit_statement(StatementKind::StorageLive(binding_local), Some(expr.span));

        let cond_block = self.mir.fresh_block();
        let load_block = self.mir.fresh_block();
        let body_block = self.mir.fresh_block();
        let step_block = self.mir.fresh_block();
        let exit_block = self.mir.fresh_block();
        self.set_terminator(TerminatorKind::Goto(cond_block), Some(expr.span));

        self.current_block = cond_block;
        let len_operand = match &iter_ty {
            Ty::Named { def, .. }
                if self
                    .def_by_name("std::collections::Vec")
                    .or_else(|| self.def_by_name("Vec"))
                    .is_some_and(|vec_def| vec_def == *def) =>
            {
                let Some(vec_len_def) = self.def_by_name("__builtin_vec_len") else {
                    self.unsupported(expr.span, "Vec iteration builtin");
                    self.current_block = exit_block;
                    if let Some(local) = saved_binding {
                        self.locals.insert(binding, local);
                    } else {
                        self.locals.remove(&binding);
                    }
                    return;
                };
                self.emit_call_operand(
                    Operand::Def(vec_len_def),
                    vec![Operand::Copy(iter_local)],
                    Ty::Uint(crate::hir::UintSize::USize),
                    iter.span,
                )
            }
            Ty::Named { def, .. }
                if self
                    .def_by_name("std::collections::HashMap")
                    .or_else(|| self.def_by_name("HashMap"))
                    .is_some_and(|map_def| map_def == *def) =>
            {
                let Some(hashmap_len_def) = self.def_by_name("__builtin_hashmap_len") else {
                    self.unsupported(expr.span, "HashMap iteration builtin");
                    self.current_block = exit_block;
                    if let Some(local) = saved_binding {
                        self.locals.insert(binding, local);
                    } else {
                        self.locals.remove(&binding);
                    }
                    return;
                };
                self.emit_call_operand(
                    Operand::Def(hashmap_len_def),
                    vec![Operand::Copy(iter_local)],
                    Ty::Uint(crate::hir::UintSize::USize),
                    iter.span,
                )
            }
            Ty::Array { len, .. } => Operand::Const(MirConst::Uint(*len as u128)),
            Ty::Slice(_) => {
                self.unsupported(expr.span, "slice iteration");
                self.current_block = exit_block;
                if let Some(local) = saved_binding {
                    self.locals.insert(binding, local);
                } else {
                    self.locals.remove(&binding);
                }
                return;
            }
            _ => {
                self.unsupported(expr.span, "for-loop iterable");
                self.current_block = exit_block;
                if let Some(local) = saved_binding {
                    self.locals.insert(binding, local);
                } else {
                    self.locals.remove(&binding);
                }
                return;
            }
        };
        let cond_local = self.fresh_temp(Ty::Bool, expr.span);
        self.emit_assign(
            Place::local(cond_local),
            Rvalue::BinaryOp {
                op: MirBinOp::Lt,
                lhs: Operand::Copy(index_local),
                rhs: len_operand,
            },
            Some(expr.span),
        );
        self.set_terminator(
            TerminatorKind::SwitchInt {
                discriminant: Operand::Copy(cond_local),
                targets: vec![(1, load_block)],
                otherwise: exit_block,
            },
            Some(expr.span),
        );

        self.current_block = load_block;
        match &iter_ty {
            Ty::Named { def, .. }
                if self
                    .def_by_name("std::collections::Vec")
                    .or_else(|| self.def_by_name("Vec"))
                    .is_some_and(|vec_def| vec_def == *def) =>
            {
                let Some(vec_get_def) = self.def_by_name("__builtin_vec_get") else {
                    self.unsupported(expr.span, "Vec iteration builtin");
                    self.set_terminator(TerminatorKind::Goto(exit_block), Some(expr.span));
                    self.current_block = exit_block;
                    if let Some(local) = saved_binding {
                        self.locals.insert(binding, local);
                    } else {
                        self.locals.remove(&binding);
                    }
                    return;
                };
                let Some(option_def) = self
                    .def_by_name("std::core::Option")
                    .or_else(|| self.def_by_name("Option"))
                else {
                    self.unsupported(expr.span, "Vec iteration option carrier");
                    self.set_terminator(TerminatorKind::Goto(exit_block), Some(expr.span));
                    self.current_block = exit_block;
                    if let Some(local) = saved_binding {
                        self.locals.insert(binding, local);
                    } else {
                        self.locals.remove(&binding);
                    }
                    return;
                };
                let option_ty = Ty::Named {
                    def: option_def,
                    args: vec![binding_ty.clone()],
                };
                let option_value = self.emit_call_operand(
                    Operand::Def(vec_get_def),
                    vec![Operand::Copy(iter_local), Operand::Copy(index_local)],
                    option_ty.clone(),
                    iter.span,
                );
                let option_local =
                    self.operand_to_local(option_value, option_ty.clone(), iter.span);
                let Some(TryCarrierMirTy::Option {
                    some_variant,
                    none_variant,
                    ..
                }) = self.try_carrier_ty(&option_ty)
                else {
                    self.unsupported(expr.span, "Vec iteration option carrier");
                    self.set_terminator(TerminatorKind::Goto(exit_block), Some(expr.span));
                    self.current_block = exit_block;
                    if let Some(local) = saved_binding {
                        self.locals.insert(binding, local);
                    } else {
                        self.locals.remove(&binding);
                    }
                    return;
                };
                let some_block = self.mir.fresh_block();
                let discr_local = self.lower_discriminant(Place::local(option_local), expr.span);
                self.set_terminator(
                    TerminatorKind::SwitchInt {
                        discriminant: Operand::Copy(discr_local),
                        targets: vec![(some_variant as u128, some_block)],
                        otherwise: exit_block,
                    },
                    Some(expr.span),
                );
                self.current_block = some_block;
                self.emit_assign(
                    Place::local(binding_local),
                    Rvalue::Read(Place {
                        local: option_local,
                        projections: vec![Projection::VariantField {
                            variant_idx: some_variant,
                            field_idx: 0,
                        }],
                    }),
                    Some(expr.span),
                );
                let _ = none_variant;
            }
            Ty::Named { def, .. }
                if self
                    .def_by_name("std::collections::HashMap")
                    .or_else(|| self.def_by_name("HashMap"))
                    .is_some_and(|map_def| map_def == *def) =>
            {
                let Some(hashmap_get_def) = self.def_by_name("__builtin_hashmap_iter_get") else {
                    self.unsupported(expr.span, "HashMap iteration builtin");
                    self.set_terminator(TerminatorKind::Goto(exit_block), Some(expr.span));
                    self.current_block = exit_block;
                    if let Some(local) = saved_binding {
                        self.locals.insert(binding, local);
                    } else {
                        self.locals.remove(&binding);
                    }
                    return;
                };
                let Some(option_def) = self
                    .def_by_name("std::core::Option")
                    .or_else(|| self.def_by_name("Option"))
                else {
                    self.unsupported(expr.span, "HashMap iteration option carrier");
                    self.set_terminator(TerminatorKind::Goto(exit_block), Some(expr.span));
                    self.current_block = exit_block;
                    if let Some(local) = saved_binding {
                        self.locals.insert(binding, local);
                    } else {
                        self.locals.remove(&binding);
                    }
                    return;
                };
                let option_ty = Ty::Named {
                    def: option_def,
                    args: vec![binding_ty.clone()],
                };
                let option_value = self.emit_call_operand(
                    Operand::Def(hashmap_get_def),
                    vec![Operand::Copy(iter_local), Operand::Copy(index_local)],
                    option_ty.clone(),
                    iter.span,
                );
                let option_local =
                    self.operand_to_local(option_value, option_ty.clone(), iter.span);
                let Some(TryCarrierMirTy::Option { some_variant, .. }) =
                    self.try_carrier_ty(&option_ty)
                else {
                    self.unsupported(expr.span, "HashMap iteration option carrier");
                    self.set_terminator(TerminatorKind::Goto(exit_block), Some(expr.span));
                    self.current_block = exit_block;
                    if let Some(local) = saved_binding {
                        self.locals.insert(binding, local);
                    } else {
                        self.locals.remove(&binding);
                    }
                    return;
                };
                let some_block = self.mir.fresh_block();
                let discr_local = self.lower_discriminant(Place::local(option_local), expr.span);
                self.set_terminator(
                    TerminatorKind::SwitchInt {
                        discriminant: Operand::Copy(discr_local),
                        targets: vec![(some_variant as u128, some_block)],
                        otherwise: exit_block,
                    },
                    Some(expr.span),
                );
                self.current_block = some_block;
                self.emit_assign(
                    Place::local(binding_local),
                    Rvalue::Read(Place {
                        local: option_local,
                        projections: vec![Projection::VariantField {
                            variant_idx: some_variant,
                            field_idx: 0,
                        }],
                    }),
                    Some(expr.span),
                );
            }
            Ty::Array { .. } => {
                self.emit_assign(
                    Place::local(binding_local),
                    Rvalue::Ref {
                        mutable: false,
                        place: Place {
                            local: iter_local,
                            projections: vec![Projection::Index(index_local)],
                        },
                    },
                    Some(expr.span),
                );
            }
            Ty::Slice(_) | _ => {}
        }
        self.set_terminator(TerminatorKind::Goto(body_block), Some(expr.span));

        self.current_block = body_block;
        self.break_targets.push(exit_block);
        self.continue_targets.push(step_block);
        let _ = self.lower_expr_to_operand(body);
        self.continue_targets.pop();
        self.break_targets.pop();
        if self.mir.block_mut(self.current_block).terminator.is_none() {
            self.set_terminator(TerminatorKind::Goto(step_block), Some(expr.span));
        }

        self.current_block = step_block;
        let next_index = self.fresh_temp(Ty::Uint(crate::hir::UintSize::USize), expr.span);
        self.emit_assign(
            Place::local(next_index),
            Rvalue::BinaryOp {
                op: MirBinOp::Add,
                lhs: Operand::Copy(index_local),
                rhs: Operand::Const(MirConst::Uint(1)),
            },
            Some(expr.span),
        );
        self.emit_assign(
            Place::local(index_local),
            Rvalue::Use(Operand::Copy(next_index)),
            Some(expr.span),
        );
        self.set_terminator(TerminatorKind::Goto(cond_block), Some(expr.span));

        self.current_block = exit_block;
        if let Some(local) = saved_binding {
            self.locals.insert(binding, local);
        } else {
            self.locals.remove(&binding);
        }
    }

    fn lower_try(
        &mut self,
        expr: &'a crate::hir::HirExpr,
        inner: &'a crate::hir::HirExpr,
    ) -> Operand {
        let source_ty = self.concrete_expr_ty(inner);
        let current_return_ty = self.mir.locals[self.return_local.0 as usize].ty.clone();
        let Some(source_carrier) = self.try_carrier_ty(&source_ty) else {
            self.unsupported(expr.span, "`?` operand");
            return Operand::Const(MirConst::Undef);
        };
        let Some(return_carrier) = self.try_carrier_ty(&current_return_ty) else {
            self.unsupported(expr.span, "`?` return type");
            return Operand::Const(MirConst::Undef);
        };

        let source_operand = self.lower_expr_to_operand(inner);
        let source_local = self.operand_to_local(source_operand, source_ty.clone(), inner.span);
        let discr_local = self.fresh_temp(Ty::Uint(crate::hir::UintSize::U64), expr.span);
        self.emit_assign(
            Place::local(discr_local),
            Rvalue::Discriminant(Place::local(source_local)),
            Some(expr.span),
        );

        let success_block = self.mir.fresh_block();
        let failure_block = self.mir.fresh_block();
        let join_block = self.mir.fresh_block();

        let (success_variant, success_ty) = match (&source_carrier, &return_carrier) {
            (
                TryCarrierMirTy::Result {
                    ok_variant, ok_ty, ..
                },
                TryCarrierMirTy::Result { .. },
            ) => (*ok_variant, ok_ty.clone()),
            (
                TryCarrierMirTy::Option {
                    some_variant,
                    none_variant: _,
                    some_ty,
                    ..
                },
                TryCarrierMirTy::Option { .. },
            ) => (*some_variant, some_ty.clone()),
            _ => {
                self.unsupported(expr.span, "`?` carrier mismatch");
                return Operand::Const(MirConst::Undef);
            }
        };

        self.set_terminator(
            TerminatorKind::SwitchInt {
                discriminant: Operand::Copy(discr_local),
                targets: vec![(success_variant as u128, success_block)],
                otherwise: failure_block,
            },
            Some(expr.span),
        );

        self.current_block = failure_block;
        self.emit_errdeferred(expr.span);
        self.emit_deferred(expr.span);
        let failure_value = match (&source_carrier, &return_carrier) {
            (
                TryCarrierMirTy::Result {
                    err_variant,
                    err_ty,
                    ..
                },
                TryCarrierMirTy::Result {
                    def,
                    err_variant: return_err_variant,
                    ..
                },
            ) => {
                let error_value = self.read_place_operand(
                    &Place {
                        local: source_local,
                        projections: vec![Projection::VariantField {
                            variant_idx: *err_variant,
                            field_idx: 0,
                        }],
                    },
                    err_ty.clone(),
                    expr.span,
                );
                Rvalue::Aggregate(
                    AggregateKind::Enum {
                        def: *def,
                        variant_idx: *return_err_variant,
                    },
                    vec![error_value],
                )
            }
            (
                TryCarrierMirTy::Option { .. },
                TryCarrierMirTy::Option {
                    def,
                    none_variant: return_none_variant,
                    ..
                },
            ) => Rvalue::Aggregate(
                AggregateKind::Enum {
                    def: *def,
                    variant_idx: *return_none_variant,
                },
                Vec::new(),
            ),
            _ => {
                self.unsupported(expr.span, "`?` carrier mismatch");
                return Operand::Const(MirConst::Undef);
            }
        };
        self.emit_assign(
            Place::local(self.return_local),
            failure_value,
            Some(expr.span),
        );
        self.set_terminator(TerminatorKind::Return, Some(expr.span));

        self.current_block = success_block;
        let success_value = self.read_place_operand(
            &Place {
                local: source_local,
                projections: vec![Projection::VariantField {
                    variant_idx: success_variant,
                    field_idx: 0,
                }],
            },
            success_ty,
            expr.span,
        );
        self.set_terminator(TerminatorKind::Goto(join_block), Some(expr.span));
        self.current_block = join_block;
        success_value
    }

    fn lower_if(
        &mut self,
        expr: &'a crate::hir::HirExpr,
        condition: &'a crate::hir::HirExpr,
        then_branch: &'a crate::hir::HirExpr,
        else_branch: Option<&'a crate::hir::HirExpr>,
    ) -> Operand {
        let condition = self.lower_expr_to_operand(condition);
        let then_block = self.mir.fresh_block();
        let else_block = self.mir.fresh_block();
        let join_block = self.mir.fresh_block();
        let dest = self.fresh_temp(expr.ty.clone(), expr.span);

        self.set_terminator(
            TerminatorKind::SwitchInt {
                discriminant: condition,
                targets: vec![(1, then_block)],
                otherwise: else_block,
            },
            Some(expr.span),
        );

        self.current_block = then_block;
        let then_value = self.lower_expr_to_operand(then_branch);
        if self.mir.block_mut(self.current_block).terminator.is_none() {
            self.emit_assign(
                Place::local(dest),
                Rvalue::Use(then_value),
                Some(then_branch.span),
            );
            self.set_terminator(TerminatorKind::Goto(join_block), Some(then_branch.span));
        }

        self.current_block = else_block;
        let else_value = else_branch
            .map(|else_branch| self.lower_expr_to_operand(else_branch))
            .unwrap_or(Operand::Const(MirConst::Unit));
        if self.mir.block_mut(self.current_block).terminator.is_none() {
            let else_span = else_branch.map(|branch| branch.span).unwrap_or(expr.span);
            self.emit_assign(Place::local(dest), Rvalue::Use(else_value), Some(else_span));
            self.set_terminator(TerminatorKind::Goto(join_block), Some(else_span));
        }

        self.current_block = join_block;
        Operand::Copy(dest)
    }

    fn lower_while(
        &mut self,
        expr: &'a crate::hir::HirExpr,
        condition: &'a crate::hir::HirExpr,
        body: &'a crate::hir::HirExpr,
    ) {
        let cond_block = self.mir.fresh_block();
        let body_block = self.mir.fresh_block();
        let exit_block = self.mir.fresh_block();
        self.set_terminator(TerminatorKind::Goto(cond_block), Some(expr.span));

        self.current_block = cond_block;
        let condition_operand = self.lower_expr_to_operand(condition);
        self.set_terminator(
            TerminatorKind::SwitchInt {
                discriminant: condition_operand,
                targets: vec![(1, body_block)],
                otherwise: exit_block,
            },
            Some(condition.span),
        );

        self.break_targets.push(exit_block);
        self.continue_targets.push(cond_block);
        self.current_block = body_block;
        let _ = self.lower_expr_to_operand(body);
        if self.mir.block_mut(self.current_block).terminator.is_none() {
            self.set_terminator(TerminatorKind::Goto(cond_block), Some(body.span));
        }
        self.break_targets.pop();
        self.continue_targets.pop();
        self.current_block = exit_block;
    }

    fn lower_loop(&mut self, expr: &'a crate::hir::HirExpr, body: &'a crate::hir::HirExpr) {
        let body_block = self.mir.fresh_block();
        let exit_block = self.mir.fresh_block();
        self.set_terminator(TerminatorKind::Goto(body_block), Some(expr.span));

        self.break_targets.push(exit_block);
        self.continue_targets.push(body_block);
        self.current_block = body_block;
        let _ = self.lower_expr_to_operand(body);
        if self.mir.block_mut(self.current_block).terminator.is_none() {
            self.set_terminator(TerminatorKind::Goto(body_block), Some(body.span));
        }
        self.break_targets.pop();
        self.continue_targets.pop();
        self.current_block = exit_block;
    }

    fn lower_match(
        &mut self,
        expr: &'a crate::hir::HirExpr,
        scrutinee: &'a crate::hir::HirExpr,
        arms: &'a [crate::hir::HirArm],
    ) -> Operand {
        let scrutinee_place = self.lower_place(scrutinee).unwrap_or_else(|| {
            let operand = self.lower_expr_to_operand(scrutinee);
            let local = self.operand_to_local(operand, scrutinee.ty.clone(), scrutinee.span);
            Place::local(local)
        });
        let dest = self.fresh_temp(expr.ty.clone(), expr.span);
        let join = self.mir.fresh_block();
        let otherwise = self.mir.fresh_block();
        let mut check_blocks = Vec::with_capacity(arms.len());
        for _ in arms {
            check_blocks.push(self.mir.fresh_block());
        }
        if let Some(first) = check_blocks.first().copied() {
            self.set_terminator(TerminatorKind::Goto(first), Some(expr.span));
        } else {
            self.set_terminator(TerminatorKind::Goto(otherwise), Some(expr.span));
        }

        for (index, arm) in arms.iter().enumerate() {
            self.current_block = check_blocks[index];
            let saved = self.locals.clone();
            let body_block = self.mir.fresh_block();
            let next_check = check_blocks.get(index + 1).copied().unwrap_or(otherwise);
            self.lower_pattern_match(&scrutinee_place, &arm.pattern, body_block, next_check);

            self.current_block = body_block;
            if let Some(guard) = &arm.guard {
                let guard_true = self.mir.fresh_block();
                let guard_cond = self.lower_expr_to_operand(guard);
                self.branch_on_bool(guard_cond, guard_true, next_check, guard.span);
                self.current_block = guard_true;
            }

            let value = self.lower_expr_to_operand(&arm.body);
            if self.mir.block_mut(self.current_block).terminator.is_none() {
                self.emit_assign(Place::local(dest), Rvalue::Use(value), Some(arm.body.span));
                self.set_terminator(TerminatorKind::Goto(join), Some(arm.body.span));
            }
            self.locals = saved;
        }

        self.current_block = otherwise;
        if self.mir.block_mut(self.current_block).terminator.is_none() {
            self.set_terminator(TerminatorKind::Unreachable, Some(expr.span));
        }
        self.current_block = join;
        Operand::Copy(dest)
    }

    fn lower_pattern_match(
        &mut self,
        place: &Place,
        pattern: &'a crate::hir::HirPattern,
        success: BlockId,
        failure: BlockId,
    ) {
        use crate::hir::{HirLit, HirPatternKind};

        match &pattern.kind {
            HirPatternKind::Wildcard => {
                self.set_terminator(TerminatorKind::Goto(success), Some(pattern.span));
            }
            HirPatternKind::Binding { id, .. } => {
                let temp = self.fresh_temp(pattern.ty.clone(), pattern.span);
                self.emit_assign(
                    Place::local(temp),
                    Rvalue::Read(place.clone()),
                    Some(pattern.span),
                );
                self.locals.insert(*id, temp);
                self.set_terminator(TerminatorKind::Goto(success), Some(pattern.span));
            }
            HirPatternKind::Lit(HirLit::Unit) => {
                self.set_terminator(TerminatorKind::Goto(success), Some(pattern.span));
            }
            HirPatternKind::Lit(lit) => {
                let lhs = self.read_place_operand(place, pattern.ty.clone(), pattern.span);
                let rhs = Operand::Const(lower_lit(lit));
                let test = self.fresh_temp(Ty::Bool, pattern.span);
                self.emit_assign(
                    Place::local(test),
                    Rvalue::BinaryOp {
                        op: MirBinOp::Eq,
                        lhs,
                        rhs,
                    },
                    Some(pattern.span),
                );
                self.branch_on_bool(Operand::Copy(test), success, failure, pattern.span);
            }
            HirPatternKind::Tuple(elems) => {
                if elems.is_empty() {
                    self.set_terminator(TerminatorKind::Goto(success), Some(pattern.span));
                    return;
                }
                let mut entry = self.current_block;
                for (index, elem) in elems.iter().enumerate() {
                    let next = if index + 1 == elems.len() {
                        success
                    } else {
                        self.mir.fresh_block()
                    };
                    self.current_block = entry;
                    let mut field_place = place.clone();
                    field_place.projections.push(Projection::Field(index));
                    self.lower_pattern_match(&field_place, elem, next, failure);
                    entry = next;
                }
            }
            HirPatternKind::Struct { def, fields, .. } => {
                if fields.is_empty() {
                    self.set_terminator(TerminatorKind::Goto(success), Some(pattern.span));
                    return;
                }
                let mut entry = self.current_block;
                for (field_pos, (field_name, field_pattern)) in fields.iter().enumerate() {
                    let Some(field_index) = self
                        .field_indices
                        .get(def)
                        .and_then(|indices| indices.get(field_name))
                        .copied()
                    else {
                        self.unsupported(pattern.span, "struct match pattern");
                        self.set_terminator(TerminatorKind::Goto(failure), Some(pattern.span));
                        return;
                    };
                    let next = if field_pos + 1 == fields.len() {
                        success
                    } else {
                        self.mir.fresh_block()
                    };
                    self.current_block = entry;
                    let mut field_place = place.clone();
                    field_place.projections.push(Projection::Field(field_index));
                    self.lower_pattern_match(&field_place, field_pattern, next, failure);
                    entry = next;
                }
            }
            HirPatternKind::Variant { def, args } => {
                let Some((_, variant_idx)) = self.enum_variant_indices.get(def).copied() else {
                    self.unsupported(pattern.span, "enum match pattern");
                    self.set_terminator(TerminatorKind::Goto(failure), Some(pattern.span));
                    return;
                };
                let variant_block = if args.is_empty() {
                    success
                } else {
                    self.mir.fresh_block()
                };
                let discr_local =
                    self.fresh_temp(Ty::Uint(crate::hir::UintSize::U64), pattern.span);
                self.emit_assign(
                    Place::local(discr_local),
                    Rvalue::Discriminant(place.clone()),
                    Some(pattern.span),
                );
                self.set_terminator(
                    TerminatorKind::SwitchInt {
                        discriminant: Operand::Copy(discr_local),
                        targets: vec![(variant_idx as u128, variant_block)],
                        otherwise: failure,
                    },
                    Some(pattern.span),
                );
                if !args.is_empty() {
                    let mut entry = variant_block;
                    for (field_index, arg) in args.iter().enumerate() {
                        let next = if field_index + 1 == args.len() {
                            success
                        } else {
                            self.mir.fresh_block()
                        };
                        self.current_block = entry;
                        let mut field_place = place.clone();
                        field_place.projections.push(Projection::VariantField {
                            variant_idx,
                            field_idx: field_index,
                        });
                        self.lower_pattern_match(&field_place, arg, next, failure);
                        entry = next;
                    }
                }
            }
            HirPatternKind::Or(alternatives) => {
                if alternatives.iter().any(Self::pattern_contains_binding) {
                    self.unsupported(pattern.span, "or-pattern bindings");
                    self.set_terminator(TerminatorKind::Goto(failure), Some(pattern.span));
                    return;
                }
                let saved = self.locals.clone();
                let mut entry = self.current_block;
                for (index, alternative) in alternatives.iter().enumerate() {
                    let next = if index + 1 == alternatives.len() {
                        failure
                    } else {
                        self.mir.fresh_block()
                    };
                    self.current_block = entry;
                    self.locals = saved.clone();
                    self.lower_pattern_match(place, alternative, success, next);
                    entry = next;
                }
                self.locals = saved;
            }
            HirPatternKind::Ref { inner, .. } => {
                let mut inner_place = place.clone();
                inner_place.projections.push(Projection::Deref);
                self.lower_pattern_match(&inner_place, inner, success, failure);
            }
            HirPatternKind::Range { .. } | HirPatternKind::Slice { .. } => {
                self.unsupported(pattern.span, "match pattern");
                self.set_terminator(TerminatorKind::Goto(failure), Some(pattern.span));
            }
        }
    }

    fn read_place_operand(&mut self, place: &Place, ty: Ty, span: Span) -> Operand {
        let temp = self.fresh_temp(ty, span);
        self.emit_assign(Place::local(temp), Rvalue::Read(place.clone()), Some(span));
        Operand::Copy(temp)
    }

    fn branch_on_bool(&mut self, cond: Operand, on_true: BlockId, on_false: BlockId, span: Span) {
        self.set_terminator(
            TerminatorKind::SwitchInt {
                discriminant: cond,
                targets: vec![(1, on_true)],
                otherwise: on_false,
            },
            Some(span),
        );
    }

    fn pattern_contains_binding(pattern: &crate::hir::HirPattern) -> bool {
        use crate::hir::HirPatternKind;

        match &pattern.kind {
            HirPatternKind::Binding { .. } => true,
            HirPatternKind::Tuple(patterns)
            | HirPatternKind::Or(patterns)
            | HirPatternKind::Slice {
                elems: patterns, ..
            } => patterns.iter().any(Self::pattern_contains_binding),
            HirPatternKind::Struct { fields, .. } => fields
                .iter()
                .any(|(_, pattern)| Self::pattern_contains_binding(pattern)),
            HirPatternKind::Variant { args, .. } => args.iter().any(Self::pattern_contains_binding),
            HirPatternKind::Range { lo, hi, .. } => {
                Self::pattern_contains_binding(lo) || Self::pattern_contains_binding(hi)
            }
            HirPatternKind::Ref { inner, .. } => Self::pattern_contains_binding(inner),
            HirPatternKind::Wildcard | HirPatternKind::Lit(_) => false,
        }
    }

    fn lower_place(&mut self, expr: &'a crate::hir::HirExpr) -> Option<Place> {
        use crate::hir::HirExprKind::*;

        match &expr.kind {
            Var(id) => self.locals.get(id).copied().map(Place::local),
            Field {
                base,
                field,
                field_index,
            } => {
                let mut place = self.lower_place(base)?;
                let index = if let Some(index) = field_index {
                    *index
                } else if let Ok(index) = field.parse::<usize>() {
                    index
                } else if let Ty::Named { def, .. } =
                    self.place_ty(&place).unwrap_or_else(|| base.ty.clone())
                {
                    self.field_indices
                        .get(&def)
                        .and_then(|fields| fields.get(field))
                        .copied()?
                } else {
                    return None;
                };
                place.projections.push(Projection::Field(index));
                Some(place)
            }
            Index { base, index } => {
                let mut place = self.lower_place(base)?;
                let index_operand = self.lower_expr_to_operand(index);
                let local = self.operand_to_local(index_operand, index.ty.clone(), index.span);
                place.projections.push(Projection::Index(local));
                Some(place)
            }
            Deref(inner) => {
                let inner_operand = self.lower_expr_to_operand(inner);
                let base = self.operand_to_local(inner_operand, inner.ty.clone(), inner.span);
                let mut place = Place::local(base);
                place.projections.push(Projection::Deref);
                Some(place)
            }
            _ => None,
        }
    }

    fn lower_read_place(&mut self, expr: &'a crate::hir::HirExpr) -> Option<Place> {
        use crate::hir::HirExprKind::*;

        match &expr.kind {
            Var(id) => self.locals.get(id).copied().map(Place::local),
            Field {
                base,
                field,
                field_index,
            } => {
                let mut place = if let Some(place) = self.lower_read_place(base) {
                    place
                } else {
                    let operand = self.lower_expr_to_operand(base);
                    let local = self.operand_to_local(operand, base.ty.clone(), base.span);
                    Place::local(local)
                };
                let index = if let Some(index) = field_index {
                    *index
                } else if let Ok(index) = field.parse::<usize>() {
                    index
                } else if let Ty::Named { def, .. } =
                    self.place_ty(&place).unwrap_or_else(|| base.ty.clone())
                {
                    self.field_indices
                        .get(&def)
                        .and_then(|fields| fields.get(field))
                        .copied()?
                } else {
                    return None;
                };
                place.projections.push(Projection::Field(index));
                Some(place)
            }
            Index { base, index } => {
                let mut place = if let Some(place) = self.lower_read_place(base) {
                    place
                } else {
                    let operand = self.lower_expr_to_operand(base);
                    let local = self.operand_to_local(operand, base.ty.clone(), base.span);
                    Place::local(local)
                };
                let index_operand = self.lower_expr_to_operand(index);
                let local = self.operand_to_local(index_operand, index.ty.clone(), index.span);
                place.projections.push(Projection::Index(local));
                Some(place)
            }
            Deref(inner) => {
                let inner_operand = self.lower_expr_to_operand(inner);
                let base = self.operand_to_local(inner_operand, inner.ty.clone(), inner.span);
                let mut place = Place::local(base);
                place.projections.push(Projection::Deref);
                Some(place)
            }
            _ => None,
        }
    }

    fn place_ty(&self, place: &Place) -> Option<Ty> {
        let mut current = self
            .mir
            .locals
            .get(place.local.0 as usize)
            .map(|local| local.ty.clone())?;
        for projection in &place.projections {
            current = match projection {
                Projection::Field(index) => match current {
                    Ty::Tuple(items) => items.get(*index).cloned()?,
                    Ty::Named { def, .. } => self
                        .find_struct(def)?
                        .fields
                        .get(*index)
                        .map(|field| field.ty.clone())?,
                    _ => return None,
                },
                Projection::VariantField {
                    variant_idx,
                    field_idx,
                } => match current {
                    Ty::Named { def, .. } => self
                        .find_enum(def)?
                        .variants
                        .get(*variant_idx)?
                        .fields
                        .get(*field_idx)
                        .cloned()?,
                    _ => return None,
                },
                Projection::Index(_) => match current {
                    Ty::Array { elem, .. } | Ty::Slice(elem) => *elem,
                    Ty::Ref { inner, .. } => match *inner {
                        Ty::Array { elem, .. } | Ty::Slice(elem) => *elem,
                        _ => return None,
                    },
                    _ => return None,
                },
                Projection::Deref => match current {
                    Ty::Ref { inner, .. } | Ty::RawPtr { inner, .. } => *inner,
                    _ => return None,
                },
            };
        }
        Some(current)
    }

    fn find_struct(&self, def: DefId) -> Option<&crate::hir::HirStruct> {
        fn visit(module: &crate::hir::HirModule, def: DefId) -> Option<&crate::hir::HirStruct> {
            module
                .structs
                .iter()
                .find(|item| item.def == def)
                .or_else(|| {
                    module
                        .modules
                        .iter()
                        .filter_map(|child| child.body.as_deref())
                        .find_map(|child| visit(child, def))
                })
        }

        visit(self.hir, def)
    }

    fn find_enum(&self, def: DefId) -> Option<&crate::hir::HirEnum> {
        fn visit(module: &crate::hir::HirModule, def: DefId) -> Option<&crate::hir::HirEnum> {
            module
                .enums
                .iter()
                .find(|item| item.def == def)
                .or_else(|| {
                    module
                        .modules
                        .iter()
                        .filter_map(|child| child.body.as_deref())
                        .find_map(|child| visit(child, def))
                })
        }

        visit(self.hir, def)
    }

    fn unsupported(&mut self, span: Span, what: &str) {
        self.diagnostics.push(
            Diagnostic::error(format!("MIR lowering for {what} is not implemented yet"))
                .with_span(span),
        );
    }

    fn concrete_expr_ty(&self, expr: &crate::hir::HirExpr) -> Ty {
        use crate::hir::HirExprKind::*;

        match &expr.kind {
            Tuple(elems) if matches!(expr.ty, Ty::Var(_)) => Ty::Tuple(
                elems
                    .iter()
                    .map(|elem| self.concrete_expr_ty(elem))
                    .collect::<Vec<_>>(),
            ),
            Array(elems) if matches!(expr.ty, Ty::Var(_)) => {
                let elem_ty = elems
                    .first()
                    .map(|elem| self.concrete_expr_ty(elem))
                    .unwrap_or(Ty::Unit);
                Ty::Array {
                    elem: Box::new(elem_ty),
                    len: elems.len(),
                }
            }
            Struct { def, fields, .. } if matches!(expr.ty, Ty::Var(_)) => {
                // Recover concrete type args by matching each struct field's
                // declared type against the actual field value's type.
                // For e.g. `Wrapper<T> { value: v }` this yields
                // `Named { def: wrapper, args: [ty_of_v] }` instead of
                // `Named { def: wrapper, args: [] }`.
                let args = if let Some(strukt) = self.hir.structs.iter().find(|s| s.def == *def) {
                    // Collect the struct's generic var IDs in field order.
                    let mut struct_vars: Vec<u32> = Vec::new();
                    let mut seen: std::collections::HashSet<u32> = std::collections::HashSet::new();
                    for field_def in &strukt.fields {
                        Self::collect_struct_field_ty_vars(
                            &field_def.ty,
                            &mut struct_vars,
                            &mut seen,
                        );
                    }
                    if struct_vars.is_empty() {
                        Vec::new()
                    } else {
                        // Build a mapping from struct generic var IDs to
                        // the concrete field value types.
                        let actual: std::collections::HashMap<String, Ty> = fields
                            .iter()
                            .map(|(name, val)| (name.clone(), val.ty.clone()))
                            .collect();
                        let mut var_to_actual: std::collections::HashMap<u32, Ty> =
                            std::collections::HashMap::new();
                        for field_def in &strukt.fields {
                            if let Some(val_ty) = actual.get(&field_def.name) {
                                Self::bind_struct_vars(
                                    &field_def.ty,
                                    val_ty,
                                    &struct_vars,
                                    &mut var_to_actual,
                                );
                            }
                        }
                        struct_vars
                            .iter()
                            .map(|id| var_to_actual.get(id).cloned().unwrap_or(Ty::Var(*id)))
                            .collect()
                    }
                } else {
                    Vec::new()
                };
                Ty::Named { def: *def, args }
            }
            Cast { target_ty, .. } if matches!(expr.ty, Ty::Var(_)) => target_ty.clone(),
            BinOp { op, lhs, .. } if matches!(expr.ty, Ty::Var(_)) => match op {
                crate::hir::HirBinOp::Eq
                | crate::hir::HirBinOp::Ne
                | crate::hir::HirBinOp::Lt
                | crate::hir::HirBinOp::Le
                | crate::hir::HirBinOp::Gt
                | crate::hir::HirBinOp::Ge
                | crate::hir::HirBinOp::And
                | crate::hir::HirBinOp::Or => Ty::Bool,
                _ => self.concrete_expr_ty(lhs),
            },
            UnaryOp { operand, .. } if matches!(expr.ty, Ty::Var(_)) => {
                self.concrete_expr_ty(operand)
            }
            DefRef(def) if matches!(expr.ty, Ty::Var(_)) => self
                .enum_variant_indices
                .get(def)
                .map(|(enum_def, _)| Ty::Named {
                    def: *enum_def,
                    args: Vec::new(),
                })
                .unwrap_or_else(|| expr.ty.clone()),
            Call { callee, .. } if matches!(expr.ty, Ty::Var(_)) => {
                if let DefRef(def) = &callee.kind {
                    self.builtin_concrete_call_ty(*def)
                        .or_else(|| self.fn_ret_tys.get(def).cloned())
                        .or_else(|| {
                            self.enum_variant_indices
                                .get(def)
                                .map(|(enum_def, _)| Ty::Named {
                                    def: *enum_def,
                                    args: Vec::new(),
                                })
                        })
                        .unwrap_or_else(|| expr.ty.clone())
                } else {
                    expr.ty.clone()
                }
            }
            MethodCall { method_id, .. } if matches!(expr.ty, Ty::Var(_)) => self
                .fn_ret_tys
                .get(method_id)
                .cloned()
                .unwrap_or_else(|| expr.ty.clone()),
            Try(inner) if matches!(expr.ty, Ty::Var(_)) => self
                .try_carrier_ty(&self.concrete_expr_ty(inner))
                .map(|carrier| match carrier {
                    TryCarrierMirTy::Result { ok_ty, .. } => ok_ty,
                    TryCarrierMirTy::Option { some_ty, .. } => some_ty,
                })
                .unwrap_or_else(|| expr.ty.clone()),
            _ => expr.ty.clone(),
        }
    }

    fn builtin_concrete_call_ty(&self, def: DefId) -> Option<Ty> {
        let name = self
            .defs_by_name
            .iter()
            .find_map(|(name, builtin_def)| (*builtin_def == def).then_some(name.as_str()))?;
        match name {
            "__builtin_vec_new" => Some(self.mir.locals[self.return_local.0 as usize].ty.clone()),
            "__builtin_vec_push" => Some(Ty::Unit),
            "__builtin_vec_len" => Some(Ty::Uint(crate::hir::UintSize::USize)),
            "__builtin_vec_iter" => Some(self.mir.locals[self.return_local.0 as usize].ty.clone()),
            "__builtin_vec_iter_next" => {
                Some(self.mir.locals[self.return_local.0 as usize].ty.clone())
            }
            "__builtin_vec_iter_count" => Some(Ty::Uint(crate::hir::UintSize::USize)),
            "__builtin_iter_map" => Some(self.mir.locals[self.return_local.0 as usize].ty.clone()),
            "__builtin_iter_map_next" => {
                Some(self.mir.locals[self.return_local.0 as usize].ty.clone())
            }
            "__builtin_iter_filter" => {
                Some(self.mir.locals[self.return_local.0 as usize].ty.clone())
            }
            "__builtin_iter_filter_next" => {
                Some(self.mir.locals[self.return_local.0 as usize].ty.clone())
            }
            "__builtin_iter_collect_vec" => {
                Some(self.mir.locals[self.return_local.0 as usize].ty.clone())
            }
            "__builtin_hashmap_new" => {
                Some(self.mir.locals[self.return_local.0 as usize].ty.clone())
            }
            "__builtin_hashmap_insert" => {
                Some(self.mir.locals[self.return_local.0 as usize].ty.clone())
            }
            "__builtin_hashmap_get" => {
                Some(self.mir.locals[self.return_local.0 as usize].ty.clone())
            }
            "__builtin_hashmap_remove" => {
                Some(self.mir.locals[self.return_local.0 as usize].ty.clone())
            }
            "__builtin_hashmap_len" => Some(Ty::Uint(crate::hir::UintSize::USize)),
            "__builtin_hashmap_iter" => {
                Some(self.mir.locals[self.return_local.0 as usize].ty.clone())
            }
            "__builtin_hashmap_iter_next" => {
                Some(self.mir.locals[self.return_local.0 as usize].ty.clone())
            }
            "__builtin_hashmap_iter_count" => Some(Ty::Uint(crate::hir::UintSize::USize)),
            _ => None,
        }
    }

    fn try_carrier_ty(&self, ty: &Ty) -> Option<TryCarrierMirTy> {
        let Ty::Named { def, args } = ty else {
            return None;
        };
        let variants = self.enum_variant_names.get(def)?;
        match (variants.as_slice(), args.as_slice()) {
            ([ok, err], [ok_ty, err_ty]) if ok == "Ok" && err == "Err" => {
                Some(TryCarrierMirTy::Result {
                    def: *def,
                    ok_variant: 0,
                    err_variant: 1,
                    ok_ty: ok_ty.clone(),
                    err_ty: err_ty.clone(),
                })
            }
            ([some, none], [some_ty]) if some == "Some" && none == "None" => {
                Some(TryCarrierMirTy::Option {
                    def: *def,
                    some_variant: 0,
                    none_variant: 1,
                    some_ty: some_ty.clone(),
                })
            }
            _ => None,
        }
    }

    /// Collect type-variable IDs from a struct field's declared type.
    fn collect_struct_field_ty_vars(
        ty: &Ty,
        out: &mut Vec<u32>,
        seen: &mut std::collections::HashSet<u32>,
    ) {
        match ty {
            Ty::Var(id) => {
                if seen.insert(*id) {
                    out.push(*id);
                }
            }
            Ty::Ref { inner, .. } | Ty::RawPtr { inner, .. } | Ty::Slice(inner) => {
                Self::collect_struct_field_ty_vars(inner, out, seen);
            }
            Ty::Array { elem, .. } => Self::collect_struct_field_ty_vars(elem, out, seen),
            Ty::Tuple(elems) => {
                for elem in elems {
                    Self::collect_struct_field_ty_vars(elem, out, seen);
                }
            }
            Ty::Named { args, .. } => {
                for arg in args {
                    Self::collect_struct_field_ty_vars(arg, out, seen);
                }
            }
            _ => {}
        }
    }

    /// Match `struct_ty` (a struct field's declared type containing generic vars)
    /// against `actual_ty` (the concrete type of the field value), binding vars.
    fn bind_struct_vars(
        struct_ty: &Ty,
        actual_ty: &Ty,
        generic_vars: &[u32],
        out: &mut std::collections::HashMap<u32, Ty>,
    ) {
        match (struct_ty, actual_ty) {
            (Ty::Var(id), _) if generic_vars.contains(id) => {
                out.entry(*id).or_insert_with(|| actual_ty.clone());
            }
            (Ty::Ref { inner: si, .. }, Ty::Ref { inner: ai, .. }) => {
                Self::bind_struct_vars(si, ai, generic_vars, out)
            }
            (Ty::RawPtr { inner: si, .. }, Ty::RawPtr { inner: ai, .. }) => {
                Self::bind_struct_vars(si, ai, generic_vars, out)
            }
            (Ty::Slice(si), Ty::Slice(ai)) => Self::bind_struct_vars(si, ai, generic_vars, out),
            (Ty::Array { elem: se, .. }, Ty::Array { elem: ae, .. }) => {
                Self::bind_struct_vars(se, ae, generic_vars, out);
            }
            (Ty::Tuple(se), Ty::Tuple(ae)) if se.len() == ae.len() => {
                for (s, a) in se.iter().zip(ae.iter()) {
                    Self::bind_struct_vars(s, a, generic_vars, out);
                }
            }
            (Ty::Named { def: sd, args: sa }, Ty::Named { def: ad, args: aa })
                if sd == ad && sa.len() == aa.len() =>
            {
                for (s, a) in sa.iter().zip(aa.iter()) {
                    Self::bind_struct_vars(s, a, generic_vars, out);
                }
            }
            _ => {}
        }
    }
}

fn lower_lit(lit: &crate::hir::HirLit) -> MirConst {
    match lit {
        crate::hir::HirLit::Bool(value) => MirConst::Bool(*value),
        crate::hir::HirLit::Integer(value) => MirConst::Int(*value),
        crate::hir::HirLit::Uint(value) => MirConst::Uint(*value),
        crate::hir::HirLit::Float(value) => MirConst::Float(*value),
        crate::hir::HirLit::Char(value) => MirConst::Char(*value),
        crate::hir::HirLit::String(value) => MirConst::Str(value.clone()),
        crate::hir::HirLit::Unit => MirConst::Unit,
    }
}

fn lower_const_expr(expr: &crate::hir::HirExpr) -> Option<MirConst> {
    match &expr.kind {
        crate::hir::HirExprKind::Lit(lit) => Some(lower_lit(lit)),
        crate::hir::HirExprKind::Call { callee, args } => match &callee.kind {
            crate::hir::HirExprKind::DefRef(def) => Some(MirConst::Struct {
                def: *def,
                fields: args
                    .iter()
                    .map(lower_const_expr)
                    .collect::<Option<Vec<_>>>()?,
            }),
            _ => None,
        },
        crate::hir::HirExprKind::Tuple(elems) => Some(MirConst::Tuple(
            elems
                .iter()
                .map(lower_const_expr)
                .collect::<Option<Vec<_>>>()?,
        )),
        crate::hir::HirExprKind::Array(elems) => Some(MirConst::Array(
            elems
                .iter()
                .map(lower_const_expr)
                .collect::<Option<Vec<_>>>()?,
        )),
        crate::hir::HirExprKind::Struct { def, fields, .. } => Some(MirConst::Struct {
            def: *def,
            fields: fields
                .iter()
                .map(|(_, value)| lower_const_expr(value))
                .collect::<Option<Vec<_>>>()?,
        }),
        crate::hir::HirExprKind::Ref { expr, .. } => {
            Some(MirConst::Ref(Box::new(lower_const_expr(expr)?)))
        }
        crate::hir::HirExprKind::Cast { expr, target_ty } => {
            let value = lower_const_expr(expr)?;
            lower_const_cast(value, target_ty)
        }
        crate::hir::HirExprKind::UnaryOp { op, operand } => {
            let operand = lower_const_expr(operand)?;
            match (op, operand) {
                (crate::hir::HirUnaryOp::Neg, MirConst::Int(value)) => Some(MirConst::Int(-value)),
                (crate::hir::HirUnaryOp::Neg, MirConst::Float(value)) => {
                    Some(MirConst::Float(-value))
                }
                (
                    crate::hir::HirUnaryOp::Not | crate::hir::HirUnaryOp::BitNot,
                    MirConst::Bool(value),
                ) => Some(MirConst::Bool(!value)),
                (crate::hir::HirUnaryOp::BitNot, MirConst::Int(value)) => {
                    Some(MirConst::Int(!value))
                }
                (crate::hir::HirUnaryOp::BitNot, MirConst::Uint(value)) => {
                    Some(MirConst::Uint(!value))
                }
                _ => None,
            }
        }
        crate::hir::HirExprKind::BinOp { op, lhs, rhs } => {
            let lhs = lower_const_expr(lhs)?;
            let rhs = lower_const_expr(rhs)?;
            lower_const_bin_op(*op, lhs, rhs)
        }
        _ => None,
    }
}

fn lower_const_cast(value: MirConst, target_ty: &crate::hir::Ty) -> Option<MirConst> {
    match (value, target_ty) {
        (MirConst::Bool(value), crate::hir::Ty::Bool) => Some(MirConst::Bool(value)),
        (MirConst::Bool(value), crate::hir::Ty::Int(_)) => Some(MirConst::Int(i128::from(value))),
        (MirConst::Bool(value), crate::hir::Ty::Uint(_)) => Some(MirConst::Uint(u128::from(value))),
        (MirConst::Int(value), crate::hir::Ty::Int(_)) => Some(MirConst::Int(value)),
        (MirConst::Int(value), crate::hir::Ty::Uint(_)) => {
            Some(MirConst::Uint(u128::try_from(value).ok()?))
        }
        (MirConst::Int(value), crate::hir::Ty::Float(_)) => Some(MirConst::Float(value as f64)),
        (MirConst::Int(value), crate::hir::Ty::Char) => {
            Some(MirConst::Char(char::from_u32(u32::try_from(value).ok()?)?))
        }
        (MirConst::Uint(value), crate::hir::Ty::Uint(_)) => Some(MirConst::Uint(value)),
        (MirConst::Uint(value), crate::hir::Ty::Int(_)) => {
            Some(MirConst::Int(i128::try_from(value).ok()?))
        }
        (MirConst::Uint(value), crate::hir::Ty::Float(_)) => Some(MirConst::Float(value as f64)),
        (MirConst::Uint(value), crate::hir::Ty::Char) => {
            Some(MirConst::Char(char::from_u32(u32::try_from(value).ok()?)?))
        }
        (MirConst::Float(value), crate::hir::Ty::Float(_)) => Some(MirConst::Float(value)),
        (MirConst::Float(value), crate::hir::Ty::Int(_)) if value.is_finite() => {
            Some(MirConst::Int(value.trunc() as i128))
        }
        (MirConst::Float(value), crate::hir::Ty::Uint(_)) if value.is_finite() && value >= 0.0 => {
            Some(MirConst::Uint(value.trunc() as u128))
        }
        (MirConst::Char(value), crate::hir::Ty::Char) => Some(MirConst::Char(value)),
        (MirConst::Char(value), crate::hir::Ty::Int(_)) => Some(MirConst::Int(value as i128)),
        (MirConst::Char(value), crate::hir::Ty::Uint(_)) => {
            Some(MirConst::Uint(value as u32 as u128))
        }
        (MirConst::Unit, crate::hir::Ty::Unit) => Some(MirConst::Unit),
        _ => None,
    }
}

fn lower_const_bin_op(op: crate::hir::HirBinOp, lhs: MirConst, rhs: MirConst) -> Option<MirConst> {
    match (lhs, rhs) {
        (MirConst::Int(lhs), MirConst::Int(rhs)) => match op {
            crate::hir::HirBinOp::Add => Some(MirConst::Int(lhs.checked_add(rhs)?)),
            crate::hir::HirBinOp::Sub => Some(MirConst::Int(lhs.checked_sub(rhs)?)),
            crate::hir::HirBinOp::Mul => Some(MirConst::Int(lhs.checked_mul(rhs)?)),
            crate::hir::HirBinOp::Div => Some(MirConst::Int(lhs.checked_div(rhs)?)),
            crate::hir::HirBinOp::Rem => Some(MirConst::Int(lhs.checked_rem(rhs)?)),
            crate::hir::HirBinOp::BitAnd => Some(MirConst::Int(lhs & rhs)),
            crate::hir::HirBinOp::BitOr => Some(MirConst::Int(lhs | rhs)),
            crate::hir::HirBinOp::BitXor => Some(MirConst::Int(lhs ^ rhs)),
            crate::hir::HirBinOp::Shl => {
                Some(MirConst::Int(lhs.checked_shl(u32::try_from(rhs).ok()?)?))
            }
            crate::hir::HirBinOp::Shr => {
                Some(MirConst::Int(lhs.checked_shr(u32::try_from(rhs).ok()?)?))
            }
            crate::hir::HirBinOp::Eq => Some(MirConst::Bool(lhs == rhs)),
            crate::hir::HirBinOp::Ne => Some(MirConst::Bool(lhs != rhs)),
            crate::hir::HirBinOp::Lt => Some(MirConst::Bool(lhs < rhs)),
            crate::hir::HirBinOp::Le => Some(MirConst::Bool(lhs <= rhs)),
            crate::hir::HirBinOp::Gt => Some(MirConst::Bool(lhs > rhs)),
            crate::hir::HirBinOp::Ge => Some(MirConst::Bool(lhs >= rhs)),
            crate::hir::HirBinOp::And => Some(MirConst::Bool(lhs != 0 && rhs != 0)),
            crate::hir::HirBinOp::Or => Some(MirConst::Bool(lhs != 0 || rhs != 0)),
        },
        (MirConst::Uint(lhs), MirConst::Uint(rhs)) => match op {
            crate::hir::HirBinOp::Add => Some(MirConst::Uint(lhs.checked_add(rhs)?)),
            crate::hir::HirBinOp::Sub => Some(MirConst::Uint(lhs.checked_sub(rhs)?)),
            crate::hir::HirBinOp::Mul => Some(MirConst::Uint(lhs.checked_mul(rhs)?)),
            crate::hir::HirBinOp::Div => Some(MirConst::Uint(lhs.checked_div(rhs)?)),
            crate::hir::HirBinOp::Rem => Some(MirConst::Uint(lhs.checked_rem(rhs)?)),
            crate::hir::HirBinOp::BitAnd => Some(MirConst::Uint(lhs & rhs)),
            crate::hir::HirBinOp::BitOr => Some(MirConst::Uint(lhs | rhs)),
            crate::hir::HirBinOp::BitXor => Some(MirConst::Uint(lhs ^ rhs)),
            crate::hir::HirBinOp::Shl => {
                Some(MirConst::Uint(lhs.checked_shl(u32::try_from(rhs).ok()?)?))
            }
            crate::hir::HirBinOp::Shr => {
                Some(MirConst::Uint(lhs.checked_shr(u32::try_from(rhs).ok()?)?))
            }
            crate::hir::HirBinOp::Eq => Some(MirConst::Bool(lhs == rhs)),
            crate::hir::HirBinOp::Ne => Some(MirConst::Bool(lhs != rhs)),
            crate::hir::HirBinOp::Lt => Some(MirConst::Bool(lhs < rhs)),
            crate::hir::HirBinOp::Le => Some(MirConst::Bool(lhs <= rhs)),
            crate::hir::HirBinOp::Gt => Some(MirConst::Bool(lhs > rhs)),
            crate::hir::HirBinOp::Ge => Some(MirConst::Bool(lhs >= rhs)),
            crate::hir::HirBinOp::And => Some(MirConst::Bool(lhs != 0 && rhs != 0)),
            crate::hir::HirBinOp::Or => Some(MirConst::Bool(lhs != 0 || rhs != 0)),
        },
        (MirConst::Bool(lhs), MirConst::Bool(rhs)) => match op {
            crate::hir::HirBinOp::And => Some(MirConst::Bool(lhs && rhs)),
            crate::hir::HirBinOp::Or => Some(MirConst::Bool(lhs || rhs)),
            crate::hir::HirBinOp::Eq => Some(MirConst::Bool(lhs == rhs)),
            crate::hir::HirBinOp::Ne => Some(MirConst::Bool(lhs != rhs)),
            _ => None,
        },
        (MirConst::Float(lhs), MirConst::Float(rhs)) => match op {
            crate::hir::HirBinOp::Add => Some(MirConst::Float(lhs + rhs)),
            crate::hir::HirBinOp::Sub => Some(MirConst::Float(lhs - rhs)),
            crate::hir::HirBinOp::Mul => Some(MirConst::Float(lhs * rhs)),
            crate::hir::HirBinOp::Div => Some(MirConst::Float(lhs / rhs)),
            crate::hir::HirBinOp::Rem => Some(MirConst::Float(lhs % rhs)),
            crate::hir::HirBinOp::Eq => Some(MirConst::Bool(lhs == rhs)),
            crate::hir::HirBinOp::Ne => Some(MirConst::Bool(lhs != rhs)),
            crate::hir::HirBinOp::Lt => Some(MirConst::Bool(lhs < rhs)),
            crate::hir::HirBinOp::Le => Some(MirConst::Bool(lhs <= rhs)),
            crate::hir::HirBinOp::Gt => Some(MirConst::Bool(lhs > rhs)),
            crate::hir::HirBinOp::Ge => Some(MirConst::Bool(lhs >= rhs)),
            _ => None,
        },
        _ => None,
    }
}

fn lower_bin_op(op: crate::hir::HirBinOp) -> MirBinOp {
    match op {
        crate::hir::HirBinOp::Add => MirBinOp::Add,
        crate::hir::HirBinOp::Sub => MirBinOp::Sub,
        crate::hir::HirBinOp::Mul => MirBinOp::Mul,
        crate::hir::HirBinOp::Div => MirBinOp::Div,
        crate::hir::HirBinOp::Rem => MirBinOp::Rem,
        crate::hir::HirBinOp::BitAnd => MirBinOp::BitAnd,
        crate::hir::HirBinOp::BitOr => MirBinOp::BitOr,
        crate::hir::HirBinOp::BitXor => MirBinOp::BitXor,
        crate::hir::HirBinOp::Shl => MirBinOp::Shl,
        crate::hir::HirBinOp::Shr => MirBinOp::Shr,
        crate::hir::HirBinOp::Eq => MirBinOp::Eq,
        crate::hir::HirBinOp::Ne => MirBinOp::Ne,
        crate::hir::HirBinOp::Lt => MirBinOp::Lt,
        crate::hir::HirBinOp::Le => MirBinOp::Le,
        crate::hir::HirBinOp::Gt => MirBinOp::Gt,
        crate::hir::HirBinOp::Ge => MirBinOp::Ge,
        crate::hir::HirBinOp::And | crate::hir::HirBinOp::Or => MirBinOp::BitAnd,
    }
}

fn lower_unary_op(op: crate::hir::HirUnaryOp) -> MirUnaryOp {
    match op {
        crate::hir::HirUnaryOp::Neg => MirUnaryOp::Neg,
        crate::hir::HirUnaryOp::Not | crate::hir::HirUnaryOp::BitNot => MirUnaryOp::Not,
    }
}

pub fn lower(hir: &crate::hir::HirModule) -> (MirModule, Vec<Diagnostic>) {
    ModuleLowerer::new(hir).lower_module(hir)
}
