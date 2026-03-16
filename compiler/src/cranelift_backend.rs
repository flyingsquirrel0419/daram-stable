use crate::{
    backend_capabilities::{self, BackendFeature, BackendKind},
    diagnostics::Diagnostic,
    hir::{DefId, FloatSize, IntSize, Ty, UintSize},
    mir::{
        CastKind, MirBinOp, MirConst, MirFn, MirModule, Operand, Place, Rvalue, StatementKind,
        TerminatorKind,
    },
    native_runtime::{self, RuntimeTy},
};
use cranelift_codegen::{
    ir::{
        condcodes::{FloatCC, IntCC},
        types, AbiParam, Block, InstBuilder, MemFlags, Signature, StackSlot, StackSlotData,
        StackSlotKind,
    },
    settings::{self, Configurable},
};
use cranelift_frontend::{FunctionBuilder, FunctionBuilderContext, Switch, Variable};
use cranelift_module::{DataDescription, FuncId, Linkage, Module};
use cranelift_object::{ObjectBuilder, ObjectModule};
use std::collections::HashMap;

pub fn generate_object(
    hir: &crate::hir::HirModule,
    mir: &MirModule,
) -> Result<Vec<u8>, Vec<Diagnostic>> {
    let mut flag_builder = settings::builder();
    flag_builder.set("is_pic", "false").ok();
    let flags = settings::Flags::new(flag_builder);
    let isa_builder = cranelift_native::builder().map_err(|error| {
        vec![Diagnostic::error(format!(
            "failed to create native ISA: {error}"
        ))]
    })?;
    let isa = isa_builder
        .finish(flags)
        .map_err(|error| vec![Diagnostic::error(format!("failed to finish ISA: {error}"))])?;
    let builder = ObjectBuilder::new(isa, "daram", cranelift_module::default_libcall_names())
        .map_err(|error| {
            vec![Diagnostic::error(format!(
                "failed to create object builder: {error}"
            ))]
        })?;
    let module = ObjectModule::new(builder);

    let mut backend = CraneliftBackend::new(hir, mir, module);
    backend.declare_helpers();
    backend.declare_functions();
    backend.compile_functions();
    backend.emit_main_wrapper();

    if !backend.diagnostics.is_empty() {
        return Err(backend.diagnostics);
    }

    backend
        .module
        .finish()
        .emit()
        .map_err(|error| vec![Diagnostic::error(format!("failed to emit object: {error}"))])
}

struct CraneliftBackend<'a> {
    hir: &'a crate::hir::HirModule,
    mir: &'a MirModule,
    module: ObjectModule,
    diagnostics: Vec<Diagnostic>,
    ptr_ty: cranelift_codegen::ir::Type,
    func_ids: HashMap<DefId, FuncId>,
    helper_ids: HashMap<&'static str, FuncId>,
    string_ids: HashMap<String, cranelift_module::DataId>,
}

#[derive(Clone, Copy)]
struct TypeLayout {
    size: u32,
    align: u8,
}

impl<'a> CraneliftBackend<'a> {
    fn new(hir: &'a crate::hir::HirModule, mir: &'a MirModule, module: ObjectModule) -> Self {
        let ptr_ty = module.target_config().pointer_type();
        Self {
            hir,
            mir,
            module,
            diagnostics: Vec::new(),
            ptr_ty,
            func_ids: HashMap::new(),
            helper_ids: HashMap::new(),
            string_ids: HashMap::new(),
        }
    }

    fn declare_helpers(&mut self) {
        for function in native_runtime::exported_runtime_functions() {
            let mut sig = self.module.make_signature();
            for param in function.params {
                sig.params.push(AbiParam::new(self.runtime_ty(*param)));
            }
            for ret in function.returns {
                sig.returns.push(AbiParam::new(self.runtime_ty(*ret)));
            }
            match self
                .module
                .declare_function(function.name, Linkage::Import, &sig)
            {
                Ok(id) => {
                    self.helper_ids.insert(function.name, id);
                }
                Err(error) => self.diagnostics.push(Diagnostic::error(format!(
                    "failed to declare helper `{}`: {error}",
                    function.name
                ))),
            }
        }
    }

    fn declare_functions(&mut self) {
        for function in &self.mir.functions {
            let Some(sig) = self.signature_for(function, false) else {
                self.diagnostics.push(Diagnostic::error(format!(
                    "Cranelift backend does not support function signature for `{}`",
                    self.raw_name(function.def)
                )));
                continue;
            };
            let linkage = Linkage::Local;
            let symbol = self.symbol_name(function.def);
            match self.module.declare_function(&symbol, linkage, &sig) {
                Ok(id) => {
                    self.func_ids.insert(function.def, id);
                }
                Err(error) => self.diagnostics.push(Diagnostic::error(format!(
                    "failed to declare function `{symbol}`: {error}"
                ))),
            }
        }
    }

    fn compile_functions(&mut self) {
        for function in &self.mir.functions {
            let Some(func_id) = self.func_ids.get(&function.def).copied() else {
                continue;
            };
            if let Err(error) = self.compile_function(func_id, function) {
                self.diagnostics.push(error);
            }
        }
    }

    fn emit_main_wrapper(&mut self) {
        let Some(entry) = self
            .mir
            .functions
            .iter()
            .find(|function| self.raw_name(function.def) == "main")
        else {
            return;
        };
        let Some(entry_id) = self.func_ids.get(&entry.def).copied() else {
            return;
        };

        let mut sig = self.module.make_signature();
        sig.returns.push(AbiParam::new(types::I32));
        let Ok(wrapper_id) = self.module.declare_function("main", Linkage::Export, &sig) else {
            return;
        };

        let mut ctx = self.module.make_context();
        ctx.func.signature = sig;
        let mut builder_ctx = FunctionBuilderContext::new();
        let mut builder = FunctionBuilder::new(&mut ctx.func, &mut builder_ctx);
        let block = builder.create_block();
        builder.switch_to_block(block);
        builder.seal_block(block);
        let callee = self.module.declare_func_in_func(entry_id, builder.func);
        let call = builder.ins().call(callee, &[]);
        let return_value = if builder.inst_results(call).is_empty() {
            builder.ins().iconst(types::I32, 0)
        } else {
            let raw = builder.inst_results(call)[0];
            let value = builder.func.dfg.value_type(raw);
            if value == types::I32 {
                raw
            } else if value == types::I64 {
                builder.ins().ireduce(types::I32, raw)
            } else if value == types::I8 || value == types::I16 {
                builder.ins().uextend(types::I32, raw)
            } else {
                builder.ins().iconst(types::I32, 0)
            }
        };
        builder.ins().return_(&[return_value]);
        builder.finalize();

        if let Err(error) = self.module.define_function(wrapper_id, &mut ctx) {
            self.diagnostics.push(Diagnostic::error(format!(
                "failed to define Cranelift main wrapper: {error}"
            )));
        }
        self.module.clear_context(&mut ctx);
    }

    fn compile_function(&mut self, func_id: FuncId, function: &MirFn) -> Result<(), Diagnostic> {
        let mut ctx = self.module.make_context();
        ctx.func.signature = self.signature_for(function, false).ok_or_else(|| {
            Diagnostic::error(format!(
                "unsupported Cranelift signature for `{}`",
                self.raw_name(function.def)
            ))
        })?;

        let mut builder_ctx = FunctionBuilderContext::new();
        let param_types = ctx
            .func
            .signature
            .params
            .iter()
            .map(|param| param.value_type)
            .collect::<Vec<_>>();
        let arg_param_count = function.argc;
        let mut builder = FunctionBuilder::new(&mut ctx.func, &mut builder_ctx);
        let clif_blocks = function
            .basic_blocks
            .iter()
            .map(|_| builder.create_block())
            .collect::<Vec<_>>();
        let predecessor_counts = predecessor_counts(function);
        let mut remaining_preds = predecessor_counts.clone();
        let mut sealed = vec![false; clif_blocks.len()];
        let mut vars = HashMap::new();
        let mut slots = HashMap::new();

        for local in &function.locals {
            let layout = self.layout_of_ty(&local.ty).ok_or_else(|| {
                Diagnostic::error(format!(
                    "unsupported local layout in Cranelift backend for `{}` local l{}: `{:?}`",
                    self.raw_name(function.def),
                    local.id.0,
                    local.ty
                ))
            })?;
            let slot = builder.create_sized_stack_slot(StackSlotData::new(
                StackSlotKind::ExplicitSlot,
                layout.size.max(1),
                layout.align,
            ));
            slots.insert(local.id, slot);
            if let Some(ty) = self.clif_ty(&local.ty) {
                let variable = Variable::from_u32(local.id.0);
                builder.declare_var(variable, ty);
                vars.insert(local.id, variable);
            }
        }

        for param in &param_types {
            builder.append_block_param(clif_blocks[0], *param);
        }
        builder.switch_to_block(clif_blocks[0]);
        for (index, _) in param_types.iter().take(arg_param_count).enumerate() {
            let value = builder.block_params(clif_blocks[0])[index];
            if let Some(variable) = vars.get(&function.locals[index + 1].id).copied() {
                builder.def_var(variable, value);
                if let Some(slot) = slots.get(&function.locals[index + 1].id).copied() {
                    builder.ins().stack_store(value, slot, 0);
                }
            }
        }
        builder.seal_block(clif_blocks[0]);
        sealed[0] = true;

        for block in &function.basic_blocks {
            if block.id.0 != 0 {
                builder.switch_to_block(clif_blocks[block.id.0 as usize]);
            }

            for statement in &block.statements {
                self.translate_statement(&mut builder, function, &vars, &slots, statement)?;
            }

            let terminator = block
                .terminator
                .as_ref()
                .ok_or_else(|| Diagnostic::error("missing MIR terminator"))?;
            self.translate_terminator(
                &mut builder,
                function,
                &vars,
                &slots,
                &clif_blocks,
                &mut remaining_preds,
                &mut sealed,
                terminator,
            )?;
        }

        builder.finalize();
        self.module
            .define_function(func_id, &mut ctx)
            .map_err(|error| {
                Diagnostic::error(format!("failed to define Cranelift function: {error:?}"))
            })?;
        self.module.clear_context(&mut ctx);
        Ok(())
    }

    fn translate_statement(
        &mut self,
        builder: &mut FunctionBuilder,
        function: &MirFn,
        vars: &HashMap<crate::mir::Local, Variable>,
        slots: &HashMap<crate::mir::Local, StackSlot>,
        statement: &crate::mir::Statement,
    ) -> Result<(), Diagnostic> {
        match &statement.kind {
            StatementKind::Assign(place, rvalue) => {
                if matches!(rvalue, Rvalue::Aggregate(_, _)) {
                    return self.store_aggregate(builder, function, vars, slots, place, rvalue);
                }
                if let Rvalue::Use(Operand::Copy(src)) | Rvalue::Use(Operand::Move(src)) = rvalue {
                    if self
                        .clif_ty(&function.locals[place.local.0 as usize].ty)
                        .is_none()
                    {
                        return self.copy_aggregate_local(
                            builder,
                            function,
                            slots,
                            *src,
                            place.local,
                        );
                    }
                }
                let expected = self.clif_ty(&function.locals[place.local.0 as usize].ty);
                if let Some(value) =
                    self.translate_rvalue(builder, function, vars, slots, rvalue, expected)?
                {
                    let coerced = self.coerce_value(builder, value, expected);
                    if place.projections.is_empty() {
                        let Some(variable) = self.place_variable(place, vars) else {
                            return Err(Diagnostic::error(
                                "Cranelift backend only supports local assignment places",
                            ));
                        };
                        builder.def_var(variable, coerced);
                        if let Some(slot) = slots.get(&place.local).copied() {
                            builder.ins().stack_store(coerced, slot, 0);
                        }
                    } else if matches!(
                        place.projections.as_slice(),
                        [crate::mir::Projection::Deref]
                    ) {
                        let addr = self
                            .deref_address(builder, vars, place.local)
                            .ok_or_else(|| {
                                Diagnostic::error(
                                    "Cranelift backend only supports deref stores through local references",
                                )
                            })?;
                        builder.ins().store(MemFlags::trusted(), coerced, addr, 0);
                    } else if let Some((addr, ty)) =
                        self.projected_place_address(builder, function, vars, slots, place)
                    {
                        let store_ty = self.clif_ty(&ty).ok_or_else(|| {
                            Diagnostic::error(
                                "Cranelift backend only supports scalar projected stores",
                            )
                        })?;
                        let coerced = self.coerce_value(builder, coerced, Some(store_ty));
                        builder.ins().store(MemFlags::trusted(), coerced, addr, 0);
                    } else {
                        return Err(Diagnostic::error(
                            "Cranelift backend only supports local, deref, or field assignment places",
                        ));
                    }
                }
                Ok(())
            }
            StatementKind::StorageLive(_)
            | StatementKind::StorageDead(_)
            | StatementKind::DeferStart(_)
            | StatementKind::ErrdeferStart(_)
            | StatementKind::Nop => Ok(()),
        }
    }

    fn translate_terminator(
        &mut self,
        builder: &mut FunctionBuilder,
        function: &MirFn,
        vars: &HashMap<crate::mir::Local, Variable>,
        slots: &HashMap<crate::mir::Local, StackSlot>,
        clif_blocks: &[Block],
        remaining_preds: &mut [usize],
        sealed: &mut [bool],
        terminator: &crate::mir::Terminator,
    ) -> Result<(), Diagnostic> {
        match &terminator.kind {
            TerminatorKind::Return => {
                if self.uses_out_pointer_return_ty(&function.locals[0].ty) {
                    let out_ptr = self
                        .out_return_pointer(builder, function, clif_blocks[0])
                        .ok_or_else(|| {
                            Diagnostic::error(
                                "missing out-pointer return parameter in Cranelift backend",
                            )
                        })?;
                    let src_slot = slots.get(&crate::mir::Local(0)).copied().ok_or_else(|| {
                        Diagnostic::error("missing aggregate return slot in Cranelift backend")
                    })?;
                    let src_addr = builder.ins().stack_addr(self.ptr_ty, src_slot, 0);
                    self.copy_aggregate_value(builder, &function.locals[0].ty, src_addr, out_ptr)?;
                    builder.ins().return_(&[]);
                } else if let Some(variable) = vars.get(&crate::mir::Local(0)).copied() {
                    let value = builder.use_var(variable);
                    builder.ins().return_(&[value]);
                } else {
                    builder.ins().return_(&[]);
                }
            }
            TerminatorKind::Goto(target) => {
                builder.ins().jump(clif_blocks[target.0 as usize], &[]);
                seal_if_ready(builder, clif_blocks, remaining_preds, sealed, *target);
            }
            TerminatorKind::SwitchInt {
                discriminant,
                targets,
                otherwise,
            } => {
                let discr = self.translate_operand(builder, function, vars, discriminant, None)?;
                let discr_ty = builder.func.dfg.value_type(discr);
                let mut switch = Switch::new();
                for (value, block) in targets {
                    switch.set_entry(*value, clif_blocks[block.0 as usize]);
                }
                switch.emit(builder, discr, clif_blocks[otherwise.0 as usize]);
                for (_, block) in targets {
                    seal_if_ready(builder, clif_blocks, remaining_preds, sealed, *block);
                }
                seal_if_ready(builder, clif_blocks, remaining_preds, sealed, *otherwise);
                if discr_ty == types::INVALID {
                    return Err(Diagnostic::error("invalid switch discriminant"));
                }
            }
            TerminatorKind::Call {
                callee,
                args,
                destination,
                target,
                unwind,
            } => {
                if unwind.is_some() {
                    return Err(backend_capabilities::unsupported_feature_diagnostic(
                        BackendKind::Cranelift,
                        BackendFeature::UnwindCalls,
                        None,
                    ));
                }
                if self.try_emit_aggregate_helper_call(
                    builder,
                    function,
                    vars,
                    slots,
                    callee,
                    args,
                    destination,
                )? {
                    if let Some(target) = target {
                        builder.ins().jump(clif_blocks[target.0 as usize], &[]);
                        seal_if_ready(builder, clif_blocks, remaining_preds, sealed, *target);
                    } else {
                        builder.ins().return_(&[]);
                    }
                    return Ok(());
                }
                if self.try_emit_out_pointer_call(
                    builder,
                    function,
                    vars,
                    slots,
                    callee,
                    args,
                    destination,
                )? {
                    if let Some(target) = target {
                        builder.ins().jump(clif_blocks[target.0 as usize], &[]);
                        seal_if_ready(builder, clif_blocks, remaining_preds, sealed, *target);
                    } else {
                        builder.ins().return_(&[]);
                    }
                    return Ok(());
                }
                let arg_values = args
                    .iter()
                    .map(|arg| self.translate_operand(builder, function, vars, arg, None))
                    .collect::<Result<Vec<_>, _>>()?;
                let call = self.emit_call(builder, function, vars, callee, args, &arg_values)?;
                if let Some(variable) = self.place_variable(destination, vars) {
                    if let Some(result) = builder.inst_results(call).first().copied() {
                        let expected =
                            self.clif_ty(&function.locals[destination.local.0 as usize].ty);
                        let coerced = self.coerce_value(builder, result, expected);
                        builder.def_var(variable, coerced);
                        if let Some(slot) = slots.get(&destination.local).copied() {
                            builder.ins().stack_store(coerced, slot, 0);
                        }
                    }
                }
                if let Some(target) = target {
                    builder.ins().jump(clif_blocks[target.0 as usize], &[]);
                    seal_if_ready(builder, clif_blocks, remaining_preds, sealed, *target);
                } else {
                    builder.ins().return_(&[]);
                }
            }
            TerminatorKind::Assert {
                cond,
                expected,
                msg,
                target,
            } => {
                let cond_value =
                    self.translate_operand(builder, function, vars, cond, Some(types::I8))?;
                let cond_bool = self.bool_value(builder, cond_value);
                let fail_block = builder.create_block();
                if *expected {
                    builder.ins().brif(
                        cond_bool,
                        clif_blocks[target.0 as usize],
                        &[],
                        fail_block,
                        &[],
                    );
                } else {
                    builder.ins().brif(
                        cond_bool,
                        fail_block,
                        &[],
                        clif_blocks[target.0 as usize],
                        &[],
                    );
                }
                seal_if_ready(builder, clif_blocks, remaining_preds, sealed, *target);
                builder.switch_to_block(fail_block);
                builder.seal_block(fail_block);
                let panic_ref = self.import_helper(builder, "daram_panic_str")?;
                let msg_value = self.string_const(builder, msg)?;
                builder.ins().call(panic_ref, &[msg_value]);
                builder
                    .ins()
                    .trap(cranelift_codegen::ir::TrapCode::unwrap_user(1));
            }
            TerminatorKind::Drop { target, .. } => {
                // TODO(#43): Cranelift currently preserves the drop edge but does not lower any
                // destructor or runtime release operation for the dropped place.
                builder.ins().jump(clif_blocks[target.0 as usize], &[]);
                seal_if_ready(builder, clif_blocks, remaining_preds, sealed, *target);
            }
            TerminatorKind::Unreachable => {
                builder
                    .ins()
                    .trap(cranelift_codegen::ir::TrapCode::unwrap_user(1));
            }
            TerminatorKind::ErrdeferUnwind(_) => {
                return Err(backend_capabilities::unsupported_feature_diagnostic(
                    BackendKind::Cranelift,
                    BackendFeature::ErrdeferUnwind,
                    None,
                ));
            }
        }
        Ok(())
    }

    fn translate_rvalue(
        &mut self,
        builder: &mut FunctionBuilder,
        function: &MirFn,
        vars: &HashMap<crate::mir::Local, Variable>,
        slots: &HashMap<crate::mir::Local, StackSlot>,
        rvalue: &Rvalue,
        expected: Option<cranelift_codegen::ir::Type>,
    ) -> Result<Option<cranelift_codegen::ir::Value>, Diagnostic> {
        match rvalue {
            Rvalue::Use(operand) => self
                .translate_operand(builder, function, vars, operand, expected)
                .map(Some),
            Rvalue::Read(place) => self
                .read_place(builder, function, vars, slots, place, expected)
                .map(Some),
            Rvalue::Discriminant(place) => {
                let addr = if place.projections.is_empty() {
                    let slot = slots.get(&place.local).copied().ok_or_else(|| {
                        Diagnostic::error(
                            "Cranelift backend only supports discriminant reads from local enum values",
                        )
                    })?;
                    builder.ins().stack_addr(self.ptr_ty, slot, 0)
                } else {
                    self.projected_place_address(builder, function, vars, slots, place)
                        .map(|(addr, _)| addr)
                        .ok_or_else(|| {
                            Diagnostic::error(
                                "Cranelift backend only supports discriminant reads from addressable places",
                            )
                        })?
                };
                Ok(Some(builder.ins().load(
                    types::I64,
                    MemFlags::trusted(),
                    addr,
                    0,
                )))
            }
            Rvalue::BinaryOp { op, lhs, rhs } => {
                let lhs_ty = self.operand_ty(function, lhs);
                let rhs_ty = self.operand_ty(function, rhs);
                let mut lhs_expected = lhs_ty.as_ref().and_then(|ty| self.clif_ty(ty));
                let mut rhs_expected = rhs_ty.as_ref().and_then(|ty| self.clif_ty(ty));
                if matches!(lhs, Operand::Const(_)) && rhs_expected.is_some() {
                    lhs_expected = rhs_expected;
                }
                if matches!(rhs, Operand::Const(_)) && lhs_expected.is_some() {
                    rhs_expected = lhs_expected;
                }
                if lhs_expected.is_none() {
                    lhs_expected = rhs_expected.or(expected);
                }
                if rhs_expected.is_none() {
                    rhs_expected = lhs_expected.or(expected);
                }
                let lhs = self.translate_operand(builder, function, vars, lhs, lhs_expected)?;
                let rhs = self.translate_operand(builder, function, vars, rhs, rhs_expected)?;
                let ty = builder.func.dfg.value_type(lhs);
                let value = match op {
                    MirBinOp::Add => {
                        if ty.is_float() {
                            builder.ins().fadd(lhs, rhs)
                        } else {
                            builder.ins().iadd(lhs, rhs)
                        }
                    }
                    MirBinOp::Sub => {
                        if ty.is_float() {
                            builder.ins().fsub(lhs, rhs)
                        } else {
                            builder.ins().isub(lhs, rhs)
                        }
                    }
                    MirBinOp::Mul => {
                        if ty.is_float() {
                            builder.ins().fmul(lhs, rhs)
                        } else {
                            builder.ins().imul(lhs, rhs)
                        }
                    }
                    MirBinOp::Div => {
                        if ty.is_float() {
                            builder.ins().fdiv(lhs, rhs)
                        } else {
                            builder.ins().sdiv(lhs, rhs)
                        }
                    }
                    MirBinOp::Rem => {
                        if ty.is_float() {
                            return Err(Diagnostic::error(
                                "float remainder is unsupported in Cranelift backend",
                            ));
                        } else {
                            builder.ins().srem(lhs, rhs)
                        }
                    }
                    MirBinOp::BitAnd => builder.ins().band(lhs, rhs),
                    MirBinOp::BitOr => builder.ins().bor(lhs, rhs),
                    MirBinOp::BitXor => builder.ins().bxor(lhs, rhs),
                    MirBinOp::Shl => builder.ins().ishl(lhs, rhs),
                    MirBinOp::Shr => builder.ins().sshr(lhs, rhs),
                    MirBinOp::Eq => {
                        if ty.is_float() {
                            builder.ins().fcmp(FloatCC::Equal, lhs, rhs)
                        } else {
                            builder.ins().icmp(IntCC::Equal, lhs, rhs)
                        }
                    }
                    MirBinOp::Ne => {
                        if ty.is_float() {
                            builder.ins().fcmp(FloatCC::NotEqual, lhs, rhs)
                        } else {
                            builder.ins().icmp(IntCC::NotEqual, lhs, rhs)
                        }
                    }
                    MirBinOp::Lt => {
                        if ty.is_float() {
                            builder.ins().fcmp(FloatCC::LessThan, lhs, rhs)
                        } else {
                            builder.ins().icmp(IntCC::SignedLessThan, lhs, rhs)
                        }
                    }
                    MirBinOp::Le => {
                        if ty.is_float() {
                            builder.ins().fcmp(FloatCC::LessThanOrEqual, lhs, rhs)
                        } else {
                            builder.ins().icmp(IntCC::SignedLessThanOrEqual, lhs, rhs)
                        }
                    }
                    MirBinOp::Gt => {
                        if ty.is_float() {
                            builder.ins().fcmp(FloatCC::GreaterThan, lhs, rhs)
                        } else {
                            builder.ins().icmp(IntCC::SignedGreaterThan, lhs, rhs)
                        }
                    }
                    MirBinOp::Ge => {
                        if ty.is_float() {
                            builder.ins().fcmp(FloatCC::GreaterThanOrEqual, lhs, rhs)
                        } else {
                            builder
                                .ins()
                                .icmp(IntCC::SignedGreaterThanOrEqual, lhs, rhs)
                        }
                    }
                    MirBinOp::Offset => {
                        return Err(backend_capabilities::unsupported_feature_diagnostic(
                            BackendKind::Cranelift,
                            BackendFeature::PointerOffset,
                            None,
                        ))
                    }
                };
                Ok(Some(value))
            }
            Rvalue::UnaryOp { op, operand } => {
                let operand = self.translate_operand(builder, function, vars, operand, expected)?;
                let value = match op {
                    crate::mir::MirUnaryOp::Not => builder.ins().bnot(operand),
                    crate::mir::MirUnaryOp::Neg => {
                        if builder.func.dfg.value_type(operand).is_float() {
                            builder.ins().fneg(operand)
                        } else {
                            builder.ins().ineg(operand)
                        }
                    }
                };
                Ok(Some(value))
            }
            Rvalue::Cast {
                kind,
                operand,
                target_ty,
            } => {
                let target = self.clif_ty(target_ty).ok_or_else(|| {
                    Diagnostic::error("unsupported cast target type in Cranelift backend")
                })?;
                let value = self.translate_operand(builder, function, vars, operand, None)?;
                let source = builder.func.dfg.value_type(value);
                let source_ty = self.operand_ty(function, operand);
                let casted = match kind {
                    CastKind::IntToInt => {
                        self.coerce_int_cast(builder, value, source_ty.as_ref(), target_ty)
                    }
                    CastKind::PointerCast => self.coerce_value(builder, value, Some(target)),
                    CastKind::Transmute => {
                        self.lower_transmute_cast(builder, value, source_ty.as_ref(), target_ty)?
                    }
                    CastKind::IntToFloat => {
                        self.int_to_float_cast(builder, value, source_ty.as_ref(), target)
                    }
                    CastKind::FloatToInt => self.float_to_int_cast(builder, value, target_ty),
                    CastKind::FloatToFloat => {
                        if source == target {
                            value
                        } else if source == types::F32 && target == types::F64 {
                            builder.ins().fpromote(types::F64, value)
                        } else if source == types::F64 && target == types::F32 {
                            builder.ins().fdemote(types::F32, value)
                        } else {
                            return Err(Diagnostic::error(
                                "unsupported float cast in Cranelift backend",
                            ));
                        }
                    }
                };
                Ok(Some(casted))
            }
            Rvalue::Ref { place, .. } | Rvalue::AddressOf { place, .. } => {
                if place.projections.is_empty() {
                    let slot = slots.get(&place.local).copied().ok_or_else(|| {
                        Diagnostic::error(
                            "Cranelift backend only supports references to local values",
                        )
                    })?;
                    Ok(Some(builder.ins().stack_addr(self.ptr_ty, slot, 0)))
                } else if matches!(
                    place.projections.as_slice(),
                    [crate::mir::Projection::Deref]
                ) {
                    let addr = self
                        .deref_address(builder, vars, place.local)
                        .ok_or_else(|| {
                            Diagnostic::error(
                                "Cranelift backend only supports address-of through local references",
                            )
                        })?;
                    Ok(Some(addr))
                } else {
                    Err(Diagnostic::error(
                        "Cranelift backend only supports references to local values",
                    ))
                }
            }
            _ => Err(Diagnostic::error(
                "unsupported MIR rvalue for Cranelift backend",
            )),
        }
    }

    fn translate_operand(
        &mut self,
        builder: &mut FunctionBuilder,
        function: &MirFn,
        vars: &HashMap<crate::mir::Local, Variable>,
        operand: &Operand,
        expected: Option<cranelift_codegen::ir::Type>,
    ) -> Result<cranelift_codegen::ir::Value, Diagnostic> {
        match operand {
            Operand::Copy(local) | Operand::Move(local) => {
                let variable = vars.get(local).copied().ok_or_else(|| {
                    Diagnostic::error("unsupported local type in Cranelift backend")
                })?;
                Ok(builder.use_var(variable))
            }
            Operand::Const(constant) => self.translate_const(builder, constant, expected),
            Operand::Def(def) => {
                let _ = function;
                let func_id = self
                    .func_ids
                    .get(def)
                    .copied()
                    .or_else(|| self.helper_symbol_to_func(self.raw_name(*def)))
                    .ok_or_else(|| {
                        Diagnostic::error(format!("unsupported callee `{}`", self.raw_name(*def)))
                    })?;
                let func_ref = self.module.declare_func_in_func(func_id, builder.func);
                Ok(builder.ins().func_addr(self.ptr_ty, func_ref))
            }
        }
    }

    fn translate_const(
        &mut self,
        builder: &mut FunctionBuilder,
        constant: &MirConst,
        expected: Option<cranelift_codegen::ir::Type>,
    ) -> Result<cranelift_codegen::ir::Value, Diagnostic> {
        match constant {
            MirConst::Bool(value) => Ok(builder.ins().iconst(types::I8, i64::from(*value))),
            MirConst::Int(value) => Ok(builder
                .ins()
                .iconst(expected.unwrap_or(types::I64), *value as i64)),
            MirConst::Uint(value) => Ok(builder
                .ins()
                .iconst(expected.unwrap_or(types::I64), *value as i64)),
            MirConst::Float(value) => Ok(match expected.unwrap_or(types::F64) {
                types::F32 => builder.ins().f32const(*value as f32),
                _ => builder.ins().f64const(*value),
            }),
            MirConst::Char(value) => Ok(builder.ins().iconst(types::I32, *value as i64)),
            MirConst::Str(value) => self.string_const(builder, value),
            MirConst::Unit => Ok(builder.ins().iconst(types::I8, 0)),
            _ => Err(Diagnostic::error(
                "unsupported constant for Cranelift backend",
            )),
        }
    }

    fn string_const(
        &mut self,
        builder: &mut FunctionBuilder,
        value: &str,
    ) -> Result<cranelift_codegen::ir::Value, Diagnostic> {
        let id = if let Some(id) = self.string_ids.get(value).copied() {
            id
        } else {
            let name = format!("str_{}", self.string_ids.len());
            let data_id = self
                .module
                .declare_data(&name, Linkage::Local, false, false)
                .map_err(|error| {
                    Diagnostic::error(format!("failed to declare data `{name}`: {error}"))
                })?;
            let mut data = DataDescription::new();
            let mut bytes = value.as_bytes().to_vec();
            bytes.push(0);
            data.define(bytes.into_boxed_slice());
            self.module.define_data(data_id, &data).map_err(|error| {
                Diagnostic::error(format!("failed to define data `{name}`: {error}"))
            })?;
            self.string_ids.insert(value.to_string(), data_id);
            data_id
        };
        let data_ref = self.module.declare_data_in_func(id, builder.func);
        Ok(builder.ins().symbol_value(self.ptr_ty, data_ref))
    }

    fn place_variable(
        &self,
        place: &Place,
        vars: &HashMap<crate::mir::Local, Variable>,
    ) -> Option<Variable> {
        if place.projections.is_empty() {
            vars.get(&place.local).copied()
        } else {
            None
        }
    }

    fn read_place(
        &self,
        builder: &mut FunctionBuilder,
        function: &MirFn,
        vars: &HashMap<crate::mir::Local, Variable>,
        slots: &HashMap<crate::mir::Local, StackSlot>,
        place: &Place,
        expected: Option<cranelift_codegen::ir::Type>,
    ) -> Result<cranelift_codegen::ir::Value, Diagnostic> {
        if place.projections.is_empty() {
            let variable = self
                .place_variable(place, vars)
                .ok_or_else(|| Diagnostic::error("Cranelift backend only supports local reads"))?;
            return Ok(builder.use_var(variable));
        }

        if matches!(
            place.projections.as_slice(),
            [crate::mir::Projection::Deref]
        ) {
            let addr = self
                .deref_address(builder, vars, place.local)
                .ok_or_else(|| {
                    Diagnostic::error(
                        "Cranelift backend only supports deref reads through local references",
                    )
                })?;
            let ty = expected
                .or_else(|| self.clif_ty(&function.locals[place.local.0 as usize].ty))
                .ok_or_else(|| {
                    Diagnostic::error("unsupported deref read type in Cranelift backend")
                })?;
            return Ok(builder.ins().load(ty, MemFlags::trusted(), addr, 0));
        }

        if let Some((addr, ty)) =
            self.projected_place_address(builder, function, vars, slots, place)
        {
            let load_ty = expected.or_else(|| self.clif_ty(&ty)).ok_or_else(|| {
                Diagnostic::error("Cranelift backend only supports scalar projected reads")
            })?;
            return Ok(builder.ins().load(load_ty, MemFlags::trusted(), addr, 0));
        }

        Err(Diagnostic::error(
            "Cranelift backend only supports local, deref, or field reads",
        ))
    }

    fn deref_address(
        &self,
        builder: &mut FunctionBuilder,
        vars: &HashMap<crate::mir::Local, Variable>,
        local: crate::mir::Local,
    ) -> Option<cranelift_codegen::ir::Value> {
        let variable = vars.get(&local).copied()?;
        Some(builder.use_var(variable))
    }

    fn import_helper(
        &mut self,
        builder: &mut FunctionBuilder,
        name: &'static str,
    ) -> Result<cranelift_codegen::ir::FuncRef, Diagnostic> {
        let func_id = self
            .helper_ids
            .get(name)
            .copied()
            .ok_or_else(|| Diagnostic::error(format!("missing helper `{name}`")))?;
        Ok(self.module.declare_func_in_func(func_id, builder.func))
    }

    fn declare_callee(
        &mut self,
        builder: &mut FunctionBuilder,
        function: &MirFn,
        operand: &Operand,
        args: &[Operand],
    ) -> Result<cranelift_codegen::ir::FuncRef, Diagnostic> {
        match operand {
            Operand::Def(def) => {
                let name = self.raw_name(*def);
                if let Some(helper_id) = self.helper_symbol_to_func_for_call(name, function, args) {
                    Ok(self.module.declare_func_in_func(helper_id, builder.func))
                } else {
                    let func_id = self
                        .func_ids
                        .get(def)
                        .copied()
                        .ok_or_else(|| Diagnostic::error(format!("unknown callee `{name}`")))?;
                    Ok(self.module.declare_func_in_func(func_id, builder.func))
                }
            }
            _ => Err(backend_capabilities::unsupported_feature_diagnostic(
                BackendKind::Cranelift,
                BackendFeature::IndirectCalls,
                Some(
                    "Cranelift backend requires a concrete function-pointer signature for indirect calls"
                        .to_string(),
                ),
            )),
        }
    }

    fn emit_call(
        &mut self,
        builder: &mut FunctionBuilder,
        function: &MirFn,
        vars: &HashMap<crate::mir::Local, Variable>,
        callee: &Operand,
        args: &[Operand],
        arg_values: &[cranelift_codegen::ir::Value],
    ) -> Result<cranelift_codegen::ir::Inst, Diagnostic> {
        let param_tys = self
            .callee_param_types(function, callee, args)
            .unwrap_or_default();
        let coerced_args = arg_values
            .iter()
            .enumerate()
            .map(|(index, value)| {
                let expected = param_tys.get(index).copied();
                self.coerce_value(builder, *value, expected)
            })
            .collect::<Vec<_>>();
        if matches!(callee, Operand::Def(_)) {
            let callee_ref = self.declare_callee(builder, function, callee, args)?;
            return Ok(builder.ins().call(callee_ref, &coerced_args));
        }

        let sig = self
            .signature_for_callee_operand(function, callee)
            .ok_or_else(|| {
                Diagnostic::error(
                    "Cranelift backend requires a concrete function-pointer signature for indirect calls",
                )
            })?;
        let callee_value =
            self.translate_operand(builder, function, vars, callee, Some(self.ptr_ty))?;
        let sig_ref = builder.import_signature(sig);
        Ok(builder
            .ins()
            .call_indirect(sig_ref, callee_value, &coerced_args))
    }

    fn try_emit_aggregate_helper_call(
        &mut self,
        builder: &mut FunctionBuilder,
        function: &MirFn,
        vars: &HashMap<crate::mir::Local, Variable>,
        slots: &HashMap<crate::mir::Local, StackSlot>,
        callee: &Operand,
        args: &[Operand],
        destination: &Place,
    ) -> Result<bool, Diagnostic> {
        let Operand::Def(def) = callee else {
            return Ok(false);
        };
        let dest_ty = &function.locals[destination.local.0 as usize].ty;
        let Some(helper) =
            self.aggregate_helper_name_for_call(self.raw_name(*def), function, args, dest_ty)
        else {
            return Ok(false);
        };
        let helper_ref = self.import_helper(builder, helper)?;
        let arg_values = args
            .iter()
            .map(|arg| self.translate_operand(builder, function, vars, arg, None))
            .collect::<Result<Vec<_>, _>>()?;
        let param_tys = self.helper_param_types(helper).unwrap_or_default();
        let mut call_args = arg_values
            .iter()
            .enumerate()
            .map(|(index, value)| self.coerce_value(builder, *value, param_tys.get(index).copied()))
            .collect::<Vec<_>>();
        let dst_slot = slots.get(&destination.local).copied().ok_or_else(|| {
            Diagnostic::error("missing destination aggregate slot in Cranelift backend")
        })?;
        let dst_addr = builder.ins().stack_addr(self.ptr_ty, dst_slot, 0);
        call_args.push(dst_addr);
        builder.ins().call(helper_ref, &call_args);
        Ok(true)
    }

    fn try_emit_out_pointer_call(
        &mut self,
        builder: &mut FunctionBuilder,
        function: &MirFn,
        vars: &HashMap<crate::mir::Local, Variable>,
        slots: &HashMap<crate::mir::Local, StackSlot>,
        callee: &Operand,
        args: &[Operand],
        destination: &Place,
    ) -> Result<bool, Diagnostic> {
        let Operand::Def(def) = callee else {
            return Ok(false);
        };
        let Some(callee_fn) = self.mir.functions.iter().find(|item| item.def == *def) else {
            return Ok(false);
        };
        if !self.uses_out_pointer_return_ty(&callee_fn.locals[0].ty) {
            return Ok(false);
        }
        let arg_values = args
            .iter()
            .map(|arg| self.translate_operand(builder, function, vars, arg, None))
            .collect::<Result<Vec<_>, _>>()?;
        let param_tys = self
            .callee_param_types(function, callee, args)
            .unwrap_or_default();
        let mut call_args = arg_values
            .iter()
            .enumerate()
            .map(|(index, value)| self.coerce_value(builder, *value, param_tys.get(index).copied()))
            .collect::<Vec<_>>();
        let dst_slot = slots.get(&destination.local).copied().ok_or_else(|| {
            Diagnostic::error("missing destination aggregate slot in Cranelift backend")
        })?;
        let dst_addr = builder.ins().stack_addr(self.ptr_ty, dst_slot, 0);
        call_args.push(dst_addr);
        let callee_ref = self.declare_callee(builder, function, callee, args)?;
        builder.ins().call(callee_ref, &call_args);
        Ok(true)
    }

    fn callee_param_types(
        &self,
        function: &MirFn,
        callee: &Operand,
        args: &[Operand],
    ) -> Option<Vec<cranelift_codegen::ir::Type>> {
        match callee {
            Operand::Def(def) => {
                let name = self.raw_name(*def);
                if let Some(helper) = self.helper_symbol_to_func_for_call(name, function, args) {
                    let helper_name = self
                        .helper_ids
                        .iter()
                        .find_map(|(label, id)| (*id == helper).then_some(*label))?;
                    self.helper_param_types(helper_name)
                } else {
                    let callee_fn = self.mir.functions.iter().find(|item| item.def == *def)?;
                    Some(
                        callee_fn
                            .locals
                            .iter()
                            .skip(1)
                            .take(callee_fn.argc)
                            .filter_map(|local| self.clif_ty(&local.ty))
                            .collect(),
                    )
                }
            }
            _ => match self.operand_ty(function, callee)? {
                Ty::FnPtr { params, .. } => Some(
                    params
                        .iter()
                        .filter_map(|param| self.clif_ty(param))
                        .collect(),
                ),
                _ => None,
            },
        }
    }

    fn helper_param_types(&self, helper: &'static str) -> Option<Vec<cranelift_codegen::ir::Type>> {
        Some(
            native_runtime::exported_runtime_function(helper)?
                .params
                .iter()
                .map(|param| self.runtime_ty(*param))
                .collect(),
        )
    }

    fn runtime_ty(&self, ty: RuntimeTy) -> cranelift_codegen::ir::Type {
        match ty {
            RuntimeTy::Ptr => self.ptr_ty,
            RuntimeTy::I8 => types::I8,
            RuntimeTy::I64 => types::I64,
        }
    }

    fn signature_for_callee_operand(
        &self,
        function: &MirFn,
        operand: &Operand,
    ) -> Option<Signature> {
        match self.operand_ty(function, operand)? {
            Ty::FnPtr { params, ret } => self.signature_for_fn_ptr_parts(&params, &ret),
            _ => None,
        }
    }

    fn signature_for_fn_ptr_parts(&self, params: &[Ty], ret: &Ty) -> Option<Signature> {
        let mut sig = self.module.make_signature();
        for param in params {
            sig.params.push(AbiParam::new(self.clif_ty(param)?));
        }
        if !matches!(ret, Ty::Unit | Ty::Never) {
            sig.returns.push(AbiParam::new(self.clif_ty(ret)?));
        }
        Some(sig)
    }

    fn helper_symbol_to_func(&self, name: &str) -> Option<FuncId> {
        match name {
            "print" | "std::io::print" | "__builtin_print" => {
                self.helper_ids.get("daram_print_str").copied()
            }
            "println" | "std::io::println" | "__builtin_println" => {
                self.helper_ids.get("daram_println_str").copied()
            }
            "eprint" | "std::io::eprint" | "__builtin_eprint" => {
                self.helper_ids.get("daram_eprint_str").copied()
            }
            "eprintln" | "std::io::eprintln" | "__builtin_eprintln" => {
                self.helper_ids.get("daram_eprintln_str").copied()
            }
            "assert" | "std::test::assert" | "__builtin_assert" => {
                self.helper_ids.get("daram_assert").copied()
            }
            "assert_eq" | "std::test::assert_eq" | "__builtin_assert_eq" => {
                self.helper_ids.get("daram_assert_eq_i64").copied()
            }
            "panic" | "std::core::panic" | "__builtin_panic" => {
                self.helper_ids.get("daram_panic_str").copied()
            }
            "panic_with_fmt" | "std::test::panic_with_fmt" | "__builtin_panic_with_fmt" => {
                self.helper_ids.get("daram_panic_with_fmt_i64").copied()
            }
            "__builtin_vec_new" => self.helper_ids.get("daram_vec_new").copied(),
            "__builtin_vec_push" => self.helper_ids.get("daram_vec_push_i64").copied(),
            "__builtin_vec_len" => self.helper_ids.get("daram_vec_len").copied(),
            "__builtin_hashmap_new" => self.helper_ids.get("daram_hashmap_new").copied(),
            "__builtin_hashmap_len" => self.helper_ids.get("daram_hashmap_len").copied(),
            _ => None,
        }
    }

    fn helper_symbol_to_func_for_call(
        &self,
        name: &str,
        function: &MirFn,
        args: &[Operand],
    ) -> Option<FuncId> {
        let arg_tys = args
            .iter()
            .map(|arg| self.operand_ty(function, arg))
            .collect::<Option<Vec<_>>>()?;
        match name {
            "print" | "std::io::print" | "__builtin_print" => match arg_tys.as_slice() {
                [ty] if self.is_string_like(ty) => self.helper_ids.get("daram_print_str").copied(),
                [ty] if self.is_i64_like(ty) => self.helper_ids.get("daram_print_i64").copied(),
                _ => self.helper_symbol_to_func(name),
            },
            "println" | "std::io::println" | "__builtin_println" => match arg_tys.as_slice() {
                [ty] if self.is_string_like(ty) => {
                    self.helper_ids.get("daram_println_str").copied()
                }
                [ty] if self.is_i64_like(ty) => self.helper_ids.get("daram_println_i64").copied(),
                _ => self.helper_symbol_to_func(name),
            },
            "eprint" | "std::io::eprint" | "__builtin_eprint" => match arg_tys.as_slice() {
                [ty] if self.is_string_like(ty) => self.helper_ids.get("daram_eprint_str").copied(),
                [ty] if self.is_i64_like(ty) => self.helper_ids.get("daram_eprint_i64").copied(),
                _ => self.helper_symbol_to_func(name),
            },
            "eprintln" | "std::io::eprintln" | "__builtin_eprintln" => match arg_tys.as_slice() {
                [ty] if self.is_string_like(ty) => {
                    self.helper_ids.get("daram_eprintln_str").copied()
                }
                [ty] if self.is_i64_like(ty) => self.helper_ids.get("daram_eprintln_i64").copied(),
                _ => self.helper_symbol_to_func(name),
            },
            "__builtin_vec_push" => match arg_tys.as_slice() {
                [vec_ty, elem_ty]
                    if self.is_runtime_handle_ty(vec_ty) && self.is_i64_like(elem_ty) =>
                {
                    self.helper_ids.get("daram_vec_push_i64").copied()
                }
                [vec_ty, elem_ty]
                    if self.is_runtime_handle_ty(vec_ty) && self.is_ptr_like(elem_ty) =>
                {
                    self.helper_ids.get("daram_vec_push_ptr").copied()
                }
                _ => self.helper_symbol_to_func(name),
            },
            _ => self.helper_symbol_to_func(name),
        }
    }

    fn aggregate_helper_name_for_call(
        &self,
        name: &str,
        function: &MirFn,
        args: &[Operand],
        dest_ty: &Ty,
    ) -> Option<&'static str> {
        let arg_tys = args
            .iter()
            .map(|arg| self.operand_ty(function, arg))
            .collect::<Option<Vec<_>>>()?;
        match name {
            "__builtin_hashmap_insert" => {
                match (arg_tys.as_slice(), self.option_payload_ty(dest_ty)) {
                    ([map_ty, key_ty, value_ty], Some(payload))
                        if (self.is_runtime_handle_ty(map_ty) || matches!(map_ty, Ty::Var(_)))
                            && self.is_i64_like(key_ty)
                            && self.is_i64_like(value_ty)
                            && self.is_i64_like(&payload) =>
                    {
                        Some("daram_hashmap_insert_i64_i64")
                    }
                    ([map_ty, key_ty, value_ty], Some(payload))
                        if (self.is_runtime_handle_ty(map_ty) || matches!(map_ty, Ty::Var(_)))
                            && self.is_string_like(key_ty)
                            && self.is_i64_like(value_ty)
                            && self.is_i64_like(&payload) =>
                    {
                        Some("daram_hashmap_insert_str_i64")
                    }
                    ([map_ty, key_ty, value_ty], Some(payload))
                        if (self.is_runtime_handle_ty(map_ty) || matches!(map_ty, Ty::Var(_)))
                            && self.is_i64_like(key_ty)
                            && self.is_ptr_like(value_ty)
                            && self.is_ptr_like(&payload) =>
                    {
                        Some("daram_hashmap_insert_i64_ptr")
                    }
                    ([map_ty, key_ty, value_ty], Some(payload))
                        if (self.is_runtime_handle_ty(map_ty) || matches!(map_ty, Ty::Var(_)))
                            && self.is_string_like(key_ty)
                            && self.is_ptr_like(value_ty)
                            && self.is_ptr_like(&payload) =>
                    {
                        Some("daram_hashmap_insert_str_ptr")
                    }
                    _ => None,
                }
            }
            "__builtin_hashmap_get" => {
                match (arg_tys.as_slice(), self.option_payload_ty(dest_ty)) {
                    ([map_ty, key_ty], Some(Ty::Ref { inner, .. }))
                        if (self.is_runtime_handle_ty(map_ty) || matches!(map_ty, Ty::Var(_)))
                            && self.is_i64_like(key_ty)
                            && self.is_i64_like(&inner) =>
                    {
                        Some("daram_hashmap_get_i64_ref_i64")
                    }
                    ([map_ty, key_ty], Some(Ty::Ref { inner, .. }))
                        if (self.is_runtime_handle_ty(map_ty) || matches!(map_ty, Ty::Var(_)))
                            && self.is_string_like(key_ty)
                            && self.is_i64_like(&inner) =>
                    {
                        Some("daram_hashmap_get_str_ref_i64")
                    }
                    ([map_ty, key_ty], Some(Ty::Ref { inner, .. }))
                        if (self.is_runtime_handle_ty(map_ty) || matches!(map_ty, Ty::Var(_)))
                            && self.is_i64_like(key_ty)
                            && self.is_ptr_like(&inner) =>
                    {
                        Some("daram_hashmap_get_i64_ref_ptr")
                    }
                    ([map_ty, key_ty], Some(Ty::Ref { inner, .. }))
                        if (self.is_runtime_handle_ty(map_ty) || matches!(map_ty, Ty::Var(_)))
                            && self.is_string_like(key_ty)
                            && self.is_ptr_like(&inner) =>
                    {
                        Some("daram_hashmap_get_str_ref_ptr")
                    }
                    _ => None,
                }
            }
            "__builtin_hashmap_remove" => {
                match (arg_tys.as_slice(), self.option_payload_ty(dest_ty)) {
                    ([map_ty, key_ty], Some(payload))
                        if (self.is_runtime_handle_ty(map_ty) || matches!(map_ty, Ty::Var(_)))
                            && self.is_i64_like(key_ty)
                            && self.is_i64_like(&payload) =>
                    {
                        Some("daram_hashmap_remove_i64_i64")
                    }
                    ([map_ty, key_ty], Some(payload))
                        if (self.is_runtime_handle_ty(map_ty) || matches!(map_ty, Ty::Var(_)))
                            && self.is_string_like(key_ty)
                            && self.is_i64_like(&payload) =>
                    {
                        Some("daram_hashmap_remove_str_i64")
                    }
                    ([map_ty, key_ty], Some(payload))
                        if (self.is_runtime_handle_ty(map_ty) || matches!(map_ty, Ty::Var(_)))
                            && self.is_i64_like(key_ty)
                            && self.is_ptr_like(&payload) =>
                    {
                        Some("daram_hashmap_remove_i64_ptr")
                    }
                    ([map_ty, key_ty], Some(payload))
                        if (self.is_runtime_handle_ty(map_ty) || matches!(map_ty, Ty::Var(_)))
                            && self.is_string_like(key_ty)
                            && self.is_ptr_like(&payload) =>
                    {
                        Some("daram_hashmap_remove_str_ptr")
                    }
                    _ => None,
                }
            }
            _ => None,
        }
    }

    fn signature_for(&self, function: &MirFn, export: bool) -> Option<Signature> {
        let mut sig = self.module.make_signature();
        for local in function.locals.iter().skip(1).take(function.argc) {
            sig.params.push(AbiParam::new(self.clif_ty(&local.ty)?));
        }
        if self.uses_out_pointer_return_ty(&function.locals[0].ty) {
            sig.params.push(AbiParam::new(self.ptr_ty));
        } else if export || !matches!(function.locals[0].ty, Ty::Unit | Ty::Never) {
            if let Some(ret) = self.clif_ty(&function.locals[0].ty) {
                sig.returns.push(AbiParam::new(ret));
            }
        }
        Some(sig)
    }

    fn clif_ty(&self, ty: &Ty) -> Option<cranelift_codegen::ir::Type> {
        match ty {
            Ty::Bool => Some(types::I8),
            Ty::Char => Some(types::I32),
            Ty::Int(size) => Some(match size {
                IntSize::I8 => types::I8,
                IntSize::I16 => types::I16,
                IntSize::I32 => types::I32,
                IntSize::I64 | IntSize::I128 | IntSize::ISize => types::I64,
            }),
            Ty::Uint(size) => Some(match size {
                UintSize::U8 => types::I8,
                UintSize::U16 => types::I16,
                UintSize::U32 => types::I32,
                UintSize::U64 | UintSize::U128 | UintSize::USize => types::I64,
            }),
            Ty::Float(FloatSize::F32) => Some(types::F32),
            Ty::Float(FloatSize::F64) => Some(types::F64),
            Ty::Unit | Ty::Never => None,
            Ty::Str | Ty::String | Ty::Ref { .. } | Ty::RawPtr { .. } | Ty::FnPtr { .. } => {
                Some(self.ptr_ty)
            }
            Ty::Named { def, .. } if self.is_runtime_handle_def(*def) => Some(self.ptr_ty),
            Ty::Var(_) => Some(types::I64),
            _ => None,
        }
    }

    fn bool_value(
        &self,
        builder: &mut FunctionBuilder,
        value: cranelift_codegen::ir::Value,
    ) -> cranelift_codegen::ir::Value {
        let ty = builder.func.dfg.value_type(value);
        if ty == types::I8 || ty == types::I16 || ty == types::I32 || ty == types::I64 {
            builder.ins().icmp_imm(IntCC::NotEqual, value, 0)
        } else {
            value
        }
    }

    fn coerce_value(
        &self,
        builder: &mut FunctionBuilder,
        value: cranelift_codegen::ir::Value,
        expected: Option<cranelift_codegen::ir::Type>,
    ) -> cranelift_codegen::ir::Value {
        let Some(expected) = expected else {
            return value;
        };
        let actual = builder.func.dfg.value_type(value);
        if actual == expected {
            return value;
        }
        if actual.is_int()
            && actual.bits() == 1
            && (expected == types::I8
                || expected == types::I16
                || expected == types::I32
                || expected == types::I64)
        {
            return builder.ins().uextend(expected, value);
        }
        if (actual == types::I8
            || actual == types::I16
            || actual == types::I32
            || actual == types::I64)
            && (expected == types::I8
                || expected == types::I16
                || expected == types::I32
                || expected == types::I64)
        {
            return if actual.bytes() < expected.bytes() {
                builder.ins().sextend(expected, value)
            } else {
                builder.ins().ireduce(expected, value)
            };
        }
        if actual == types::F32 && expected == types::F64 {
            return builder.ins().fpromote(types::F64, value);
        }
        if actual == types::F64 && expected == types::F32 {
            return builder.ins().fdemote(types::F32, value);
        }
        value
    }

    fn coerce_int_cast(
        &self,
        builder: &mut FunctionBuilder,
        value: cranelift_codegen::ir::Value,
        source_ty: Option<&Ty>,
        target_ty: &Ty,
    ) -> cranelift_codegen::ir::Value {
        let target = self
            .clif_ty(target_ty)
            .unwrap_or_else(|| builder.func.dfg.value_type(value));
        let actual = builder.func.dfg.value_type(value);
        if actual == target {
            return value;
        }
        let is_unsigned = matches!(
            source_ty,
            Some(Ty::Uint(_)) | Some(Ty::Char) | Some(Ty::Bool)
        );
        if actual.bytes() < target.bytes() {
            if is_unsigned {
                builder.ins().uextend(target, value)
            } else {
                builder.ins().sextend(target, value)
            }
        } else {
            builder.ins().ireduce(target, value)
        }
    }

    fn int_to_float_cast(
        &self,
        builder: &mut FunctionBuilder,
        value: cranelift_codegen::ir::Value,
        source_ty: Option<&Ty>,
        target: cranelift_codegen::ir::Type,
    ) -> cranelift_codegen::ir::Value {
        let widened = if builder.func.dfg.value_type(value).bits() <= 8 {
            if matches!(
                source_ty,
                Some(Ty::Uint(_)) | Some(Ty::Char) | Some(Ty::Bool)
            ) {
                builder.ins().uextend(types::I32, value)
            } else {
                builder.ins().sextend(types::I32, value)
            }
        } else {
            value
        };
        if matches!(
            source_ty,
            Some(Ty::Uint(_)) | Some(Ty::Char) | Some(Ty::Bool)
        ) {
            builder.ins().fcvt_from_uint(target, widened)
        } else {
            builder.ins().fcvt_from_sint(target, widened)
        }
    }

    fn float_to_int_cast(
        &self,
        builder: &mut FunctionBuilder,
        value: cranelift_codegen::ir::Value,
        target_ty: &Ty,
    ) -> cranelift_codegen::ir::Value {
        let target = self.clif_ty(target_ty).unwrap_or(types::I64);
        if matches!(target_ty, Ty::Uint(_) | Ty::Char | Ty::Bool) {
            builder.ins().fcvt_to_uint_sat(target, value)
        } else {
            builder.ins().fcvt_to_sint_sat(target, value)
        }
    }

    fn lower_transmute_cast(
        &self,
        builder: &mut FunctionBuilder,
        value: cranelift_codegen::ir::Value,
        source_ty: Option<&Ty>,
        target_ty: &Ty,
    ) -> Result<cranelift_codegen::ir::Value, Diagnostic> {
        match (source_ty, target_ty) {
            (Some(Ty::Float(_)), Ty::Int(_) | Ty::Uint(_) | Ty::Char | Ty::Bool) => {
                Ok(self.float_to_int_cast(builder, value, target_ty))
            }
            (
                Some(Ty::Int(_)) | Some(Ty::Uint(_)) | Some(Ty::Char) | Some(Ty::Bool),
                Ty::Float(_),
            ) => {
                let target = self.clif_ty(target_ty).ok_or_else(|| {
                    Diagnostic::error("unsupported float cast target in Cranelift backend")
                })?;
                Ok(self.int_to_float_cast(builder, value, source_ty, target))
            }
            (Some(Ty::Float(_)), Ty::Float(_)) => {
                let target = self.clif_ty(target_ty).ok_or_else(|| {
                    Diagnostic::error("unsupported float cast target in Cranelift backend")
                })?;
                let source = builder.func.dfg.value_type(value);
                if source == target {
                    Ok(value)
                } else if source == types::F32 && target == types::F64 {
                    Ok(builder.ins().fpromote(types::F64, value))
                } else if source == types::F64 && target == types::F32 {
                    Ok(builder.ins().fdemote(types::F32, value))
                } else {
                    Err(Diagnostic::error(
                        "unsupported float cast in Cranelift backend",
                    ))
                }
            }
            (
                Some(Ty::Int(_)) | Some(Ty::Uint(_)) | Some(Ty::Char) | Some(Ty::Bool),
                Ty::Int(_) | Ty::Uint(_) | Ty::Char | Ty::Bool,
            ) => Ok(self.coerce_int_cast(builder, value, source_ty, target_ty)),
            _ => Ok(self.coerce_value(builder, value, self.clif_ty(target_ty))),
        }
    }

    fn symbol_name(&self, def: DefId) -> String {
        let raw = self
            .mir
            .def_names
            .get(&def)
            .cloned()
            .unwrap_or_else(|| format!("def_{}", def.index));
        if raw == "main" {
            "daram_entry_main".to_string()
        } else {
            sanitize_symbol(&raw)
        }
    }

    fn raw_name(&self, def: DefId) -> &str {
        self.mir
            .def_names
            .get(&def)
            .map(String::as_str)
            .unwrap_or("anonymous")
    }

    fn operand_ty(&self, function: &MirFn, operand: &Operand) -> Option<Ty> {
        match operand {
            Operand::Copy(local) | Operand::Move(local) => function
                .locals
                .get(local.0 as usize)
                .map(|local| local.ty.clone()),
            Operand::Const(constant) => Some(self.ty_for_const(constant)),
            Operand::Def(_) => None,
        }
    }

    fn ty_for_const(&self, constant: &MirConst) -> Ty {
        match constant {
            MirConst::Bool(_) => Ty::Bool,
            MirConst::Int(_) => Ty::Int(IntSize::I64),
            MirConst::Uint(_) => Ty::Uint(UintSize::U64),
            MirConst::Float(_) => Ty::Float(FloatSize::F64),
            MirConst::Char(_) => Ty::Char,
            MirConst::Str(_) => Ty::Str,
            MirConst::Tuple(values) => Ty::Tuple(
                values
                    .iter()
                    .map(|value| self.ty_for_const(value))
                    .collect(),
            ),
            MirConst::Array(values) => Ty::Array {
                elem: Box::new(
                    values
                        .first()
                        .map(|value| self.ty_for_const(value))
                        .unwrap_or(Ty::Unit),
                ),
                len: values.len(),
            },
            MirConst::Struct { def, .. } => Ty::Named {
                def: *def,
                args: Vec::new(),
            },
            MirConst::Ref(value) => Ty::Ref {
                mutable: false,
                inner: Box::new(self.ty_for_const(value)),
            },
            MirConst::Unit | MirConst::Undef => Ty::Unit,
        }
    }

    fn is_string_like(&self, ty: &Ty) -> bool {
        match ty {
            Ty::Str | Ty::String => true,
            Ty::Ref { inner, .. } => matches!(&**inner, Ty::Str | Ty::String),
            _ => false,
        }
    }

    fn is_i64_like(&self, ty: &Ty) -> bool {
        matches!(
            ty,
            Ty::Bool | Ty::Char | Ty::Int(_) | Ty::Uint(_) | Ty::Var(_)
        )
    }

    fn option_payload_ty(&self, ty: &Ty) -> Option<Ty> {
        let Ty::Named { def, args } = ty else {
            return None;
        };
        matches!(self.raw_name(*def), "std::core::Option" | "Option")
            .then(|| args.first().cloned())
            .flatten()
    }

    fn uses_out_pointer_return_ty(&self, ty: &Ty) -> bool {
        !matches!(ty, Ty::Unit | Ty::Never) && self.clif_ty(ty).is_none()
    }

    fn out_return_pointer(
        &self,
        builder: &mut FunctionBuilder,
        function: &MirFn,
        entry_block: Block,
    ) -> Option<cranelift_codegen::ir::Value> {
        self.uses_out_pointer_return_ty(&function.locals[0].ty)
            .then(|| builder.block_params(entry_block).last().copied())
            .flatten()
    }

    fn is_ptr_like(&self, ty: &Ty) -> bool {
        match ty {
            Ty::Str | Ty::String | Ty::Ref { .. } | Ty::RawPtr { .. } | Ty::FnPtr { .. } => true,
            Ty::Named { def, .. } => self.is_runtime_handle_def(*def),
            _ => false,
        }
    }

    fn is_runtime_handle_ty(&self, ty: &Ty) -> bool {
        matches!(ty, Ty::Named { def, .. } if self.is_runtime_handle_def(*def))
    }

    fn find_struct<'b>(&'b self, def: DefId) -> Option<&'b crate::hir::HirStruct> {
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

    fn find_enum<'b>(&'b self, def: DefId) -> Option<&'b crate::hir::HirEnum> {
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

    fn layout_of_ty(&self, ty: &Ty) -> Option<TypeLayout> {
        match ty {
            Ty::Bool => Some(TypeLayout { size: 1, align: 0 }),
            Ty::Char => Some(TypeLayout { size: 4, align: 2 }),
            Ty::Int(size) => Some(match size {
                IntSize::I8 => TypeLayout { size: 1, align: 0 },
                IntSize::I16 => TypeLayout { size: 2, align: 1 },
                IntSize::I32 => TypeLayout { size: 4, align: 2 },
                IntSize::I64 | IntSize::I128 | IntSize::ISize => TypeLayout { size: 8, align: 3 },
            }),
            Ty::Uint(size) => Some(match size {
                UintSize::U8 => TypeLayout { size: 1, align: 0 },
                UintSize::U16 => TypeLayout { size: 2, align: 1 },
                UintSize::U32 => TypeLayout { size: 4, align: 2 },
                UintSize::U64 | UintSize::U128 | UintSize::USize => {
                    TypeLayout { size: 8, align: 3 }
                }
            }),
            Ty::Float(FloatSize::F32) => Some(TypeLayout { size: 4, align: 2 }),
            Ty::Float(FloatSize::F64) => Some(TypeLayout { size: 8, align: 3 }),
            Ty::Str | Ty::String | Ty::Ref { .. } | Ty::RawPtr { .. } | Ty::FnPtr { .. } => {
                Some(TypeLayout {
                    size: self.ptr_ty.bytes() as u32,
                    align: (self.ptr_ty.bytes() as u32).trailing_zeros() as u8,
                })
            }
            Ty::Named { def, .. } if self.is_runtime_handle_def(*def) => Some(TypeLayout {
                size: self.ptr_ty.bytes() as u32,
                align: (self.ptr_ty.bytes() as u32).trailing_zeros() as u8,
            }),
            Ty::Var(_) => Some(TypeLayout { size: 8, align: 3 }),
            Ty::Tuple(elems) => self.aggregate_layout(elems),
            Ty::Array { elem, len } => {
                let elem_layout = self.layout_of_ty(elem)?;
                let elem_size = align_to(elem_layout.size.max(1), 1u32 << elem_layout.align);
                Some(TypeLayout {
                    size: elem_size.saturating_mul(*len as u32).max(1),
                    align: elem_layout.align,
                })
            }
            Ty::Named { def, .. } => self
                .hir
                .structs
                .iter()
                .find(|item| item.def == *def)
                .and_then(|item| {
                    self.aggregate_layout(
                        &item
                            .fields
                            .iter()
                            .map(|field| field.ty.clone())
                            .collect::<Vec<_>>(),
                    )
                })
                .or_else(|| self.enum_layout(*def)),
            Ty::Unit | Ty::Never => Some(TypeLayout { size: 1, align: 0 }),
            _ => None,
        }
    }

    fn is_runtime_handle_def(&self, def: DefId) -> bool {
        matches!(
            self.raw_name(def),
            "std::collections::Vec"
                | "Vec"
                | "std::collections::HashMap"
                | "HashMap"
                | "std::collections::HashSet"
                | "HashSet"
                | "std::fs::PathBuf"
                | "PathBuf"
        )
    }

    fn aggregate_layout(&self, elems: &[Ty]) -> Option<TypeLayout> {
        let mut size = 0u32;
        let mut align = 0u8;
        for elem in elems {
            let layout = self.layout_of_ty(elem)?;
            let align_bytes = 1u32 << layout.align;
            size = align_to(size, align_bytes);
            size += layout.size.max(1);
            align = align.max(layout.align);
        }
        Some(TypeLayout {
            size: size.max(1),
            align,
        })
    }

    fn field_offset_and_ty(&self, ty: &Ty, index: usize) -> Option<(u32, Ty)> {
        match ty {
            Ty::Tuple(elems) => {
                let mut offset = 0u32;
                for (field_index, field_ty) in elems.iter().enumerate() {
                    let layout = self.layout_of_ty(field_ty)?;
                    offset = align_to(offset, 1u32 << layout.align);
                    if field_index == index {
                        return Some((offset, field_ty.clone()));
                    }
                    offset += layout.size.max(1);
                }
                None
            }
            Ty::Named { def, .. } => {
                if let Some(strukt) = self.find_struct(*def) {
                    let mut offset = 0u32;
                    for (field_index, field) in strukt.fields.iter().enumerate() {
                        let layout = self.layout_of_ty(&field.ty)?;
                        offset = align_to(offset, 1u32 << layout.align);
                        if field_index == index {
                            return Some((offset, field.ty.clone()));
                        }
                        offset += layout.size.max(1);
                    }
                    None
                } else {
                    self.common_enum_field_offset_and_ty(*def, index)
                }
            }
            Ty::Array { elem, len } => {
                if index >= *len {
                    return None;
                }
                let layout = self.layout_of_ty(elem)?;
                let stride = align_to(layout.size.max(1), 1u32 << layout.align);
                Some((stride.saturating_mul(index as u32), (**elem).clone()))
            }
            _ => None,
        }
    }

    fn enum_layout(&self, def: DefId) -> Option<TypeLayout> {
        let enum_def = self.find_enum(def)?;
        let discrim_layout = TypeLayout { size: 8, align: 3 };
        let payload_align = enum_def
            .variants
            .iter()
            .flat_map(|variant| variant.fields.iter())
            .filter_map(|field_ty| self.layout_of_ty(field_ty).map(|layout| layout.align))
            .max()
            .unwrap_or(0);
        let payload_offset = align_to(discrim_layout.size, 1u32 << payload_align);
        let payload_size = enum_def
            .variants
            .iter()
            .filter_map(|variant| {
                self.aggregate_layout(&variant.fields)
                    .map(|layout| layout.size)
            })
            .max()
            .unwrap_or(0);
        let align = discrim_layout.align.max(payload_align);
        Some(TypeLayout {
            size: align_to(payload_offset + payload_size, 1u32 << align).max(1),
            align,
        })
    }

    fn enum_variant_field_offset_and_ty(
        &self,
        def: DefId,
        variant_idx: usize,
        field_index: usize,
    ) -> Option<(u32, Ty)> {
        let enum_def = self.find_enum(def)?;
        let variant = enum_def.variants.get(variant_idx)?;
        let payload_align = enum_def
            .variants
            .iter()
            .flat_map(|candidate| candidate.fields.iter())
            .filter_map(|field_ty| self.layout_of_ty(field_ty).map(|layout| layout.align))
            .max()
            .unwrap_or(0);
        let mut offset = align_to(8, 1u32 << payload_align);
        for (index, field_ty) in variant.fields.iter().enumerate() {
            let layout = self.layout_of_ty(field_ty)?;
            offset = align_to(offset, 1u32 << layout.align);
            if index == field_index {
                return Some((offset, field_ty.clone()));
            }
            offset += layout.size.max(1);
        }
        None
    }

    fn common_enum_field_offset_and_ty(&self, def: DefId, field_index: usize) -> Option<(u32, Ty)> {
        let enum_def = self.find_enum(def)?;
        let mut matches = enum_def
            .variants
            .iter()
            .enumerate()
            .filter_map(|(variant_idx, _)| {
                self.enum_variant_field_offset_and_ty(def, variant_idx, field_index)
            });
        let first = matches.next()?;
        if matches.all(|candidate| candidate == first) {
            Some(first)
        } else {
            None
        }
    }

    fn projected_place_address(
        &self,
        builder: &mut FunctionBuilder,
        function: &MirFn,
        vars: &HashMap<crate::mir::Local, Variable>,
        slots: &HashMap<crate::mir::Local, StackSlot>,
        place: &Place,
    ) -> Option<(cranelift_codegen::ir::Value, Ty)> {
        let base_ty = function.locals.get(place.local.0 as usize)?.ty.clone();
        let base_slot = slots.get(&place.local).copied()?;
        let mut addr = builder.ins().stack_addr(self.ptr_ty, base_slot, 0);
        let mut current_ty = base_ty;
        for projection in &place.projections {
            match projection {
                crate::mir::Projection::Field(index) => {
                    let (offset, field_ty) = self.field_offset_and_ty(&current_ty, *index)?;
                    if offset != 0 {
                        addr = builder.ins().iadd_imm(addr, i64::from(offset));
                    }
                    current_ty = field_ty;
                }
                crate::mir::Projection::VariantField {
                    variant_idx,
                    field_idx,
                } => {
                    let Ty::Named { def, .. } = current_ty else {
                        return None;
                    };
                    let (offset, field_ty) =
                        self.enum_variant_field_offset_and_ty(def, *variant_idx, *field_idx)?;
                    addr = builder.ins().iadd_imm(addr, i64::from(offset));
                    current_ty = field_ty;
                }
                crate::mir::Projection::Deref => {
                    let ptr = match current_ty {
                        Ty::Ref { inner, .. } | Ty::RawPtr { inner, .. } => inner,
                        _ => return None,
                    };
                    let local_ptr = self.deref_address(builder, vars, place.local)?;
                    addr = local_ptr;
                    current_ty = *ptr;
                }
                crate::mir::Projection::Index(local) => {
                    let Ty::Array { elem, .. } = current_ty else {
                        return None;
                    };
                    let layout = self.layout_of_ty(&elem)?;
                    let stride = i64::from(align_to(layout.size.max(1), 1u32 << layout.align));
                    let index_var = vars.get(local).copied()?;
                    let index_value = builder.use_var(index_var);
                    let scaled = if builder.func.dfg.value_type(index_value) == self.ptr_ty {
                        if stride == 1 {
                            index_value
                        } else {
                            let stride_value = builder.ins().iconst(self.ptr_ty, stride);
                            builder.ins().imul(index_value, stride_value)
                        }
                    } else {
                        let index_ptr = self.coerce_value(builder, index_value, Some(self.ptr_ty));
                        if stride == 1 {
                            index_ptr
                        } else {
                            let stride_value = builder.ins().iconst(self.ptr_ty, stride);
                            builder.ins().imul(index_ptr, stride_value)
                        }
                    };
                    addr = builder.ins().iadd(addr, scaled);
                    current_ty = *elem;
                }
            }
        }
        Some((addr, current_ty))
    }

    fn store_aggregate(
        &mut self,
        builder: &mut FunctionBuilder,
        function: &MirFn,
        vars: &HashMap<crate::mir::Local, Variable>,
        slots: &HashMap<crate::mir::Local, StackSlot>,
        place: &Place,
        rvalue: &Rvalue,
    ) -> Result<(), Diagnostic> {
        let Rvalue::Aggregate(kind, operands) = rvalue else {
            return Ok(());
        };
        if !place.projections.is_empty() {
            return Err(Diagnostic::error(
                "Cranelift backend only supports aggregate assignment to local places",
            ));
        }
        let base_ty = function.locals[place.local.0 as usize].ty.clone();
        let base_slot = slots.get(&place.local).copied().ok_or_else(|| {
            Diagnostic::error("missing stack slot for aggregate local in Cranelift backend")
        })?;
        let base_addr = builder.ins().stack_addr(self.ptr_ty, base_slot, 0);
        match kind {
            crate::mir::AggregateKind::Tuple => {
                let Ty::Tuple(elem_tys) = base_ty else {
                    return Err(Diagnostic::error(
                        "tuple aggregate assigned to non-tuple local in Cranelift backend",
                    ));
                };
                for (index, operand) in operands.iter().enumerate() {
                    let (offset, field_ty) = self
                        .field_offset_and_ty(&Ty::Tuple(elem_tys.clone()), index)
                        .ok_or_else(|| {
                            Diagnostic::error("invalid tuple field layout in Cranelift backend")
                        })?;
                    let value = self.translate_operand(
                        builder,
                        function,
                        vars,
                        operand,
                        self.clif_ty(&field_ty),
                    )?;
                    let addr = if offset == 0 {
                        base_addr
                    } else {
                        builder.ins().iadd_imm(base_addr, i64::from(offset))
                    };
                    builder.ins().store(MemFlags::trusted(), value, addr, 0);
                }
                Ok(())
            }
            crate::mir::AggregateKind::Struct(_) => {
                for (index, operand) in operands.iter().enumerate() {
                    let (offset, field_ty) =
                        self.field_offset_and_ty(&base_ty, index).ok_or_else(|| {
                            Diagnostic::error("invalid struct field layout in Cranelift backend")
                        })?;
                    let value = self.translate_operand(
                        builder,
                        function,
                        vars,
                        operand,
                        self.clif_ty(&field_ty),
                    )?;
                    let addr = if offset == 0 {
                        base_addr
                    } else {
                        builder.ins().iadd_imm(base_addr, i64::from(offset))
                    };
                    builder.ins().store(MemFlags::trusted(), value, addr, 0);
                }
                Ok(())
            }
            crate::mir::AggregateKind::Array(_) => {
                for (index, operand) in operands.iter().enumerate() {
                    let (offset, field_ty) =
                        self.field_offset_and_ty(&base_ty, index).ok_or_else(|| {
                            Diagnostic::error("invalid array element layout in Cranelift backend")
                        })?;
                    let value = self.translate_operand(
                        builder,
                        function,
                        vars,
                        operand,
                        self.clif_ty(&field_ty),
                    )?;
                    let addr = if offset == 0 {
                        base_addr
                    } else {
                        builder.ins().iadd_imm(base_addr, i64::from(offset))
                    };
                    builder.ins().store(MemFlags::trusted(), value, addr, 0);
                }
                Ok(())
            }
            crate::mir::AggregateKind::Enum { def, variant_idx } => {
                let discr = builder.ins().iconst(types::I64, *variant_idx as i64);
                builder
                    .ins()
                    .store(MemFlags::trusted(), discr, base_addr, 0);
                for (index, operand) in operands.iter().enumerate() {
                    let (offset, field_ty) = self
                        .enum_variant_field_offset_and_ty(*def, *variant_idx, index)
                        .ok_or_else(|| {
                            Diagnostic::error("invalid enum field layout in Cranelift backend")
                        })?;
                    let value = self.translate_operand(
                        builder,
                        function,
                        vars,
                        operand,
                        self.clif_ty(&field_ty),
                    )?;
                    let addr = builder.ins().iadd_imm(base_addr, i64::from(offset));
                    builder.ins().store(MemFlags::trusted(), value, addr, 0);
                }
                Ok(())
            }
            _ => Err(Diagnostic::error(
                "Cranelift backend only supports tuple/struct/array/enum aggregates for now",
            )),
        }
    }

    fn copy_aggregate_local(
        &self,
        builder: &mut FunctionBuilder,
        function: &MirFn,
        slots: &HashMap<crate::mir::Local, StackSlot>,
        src: crate::mir::Local,
        dst: crate::mir::Local,
    ) -> Result<(), Diagnostic> {
        let src_ty = function.locals[src.0 as usize].ty.clone();
        let dst_ty = function.locals[dst.0 as usize].ty.clone();
        if matches!(src_ty, Ty::Unit | Ty::Never) && matches!(dst_ty, Ty::Unit | Ty::Never) {
            return Ok(());
        }
        if src_ty != dst_ty {
            return Err(Diagnostic::error(format!(
                "aggregate copy requires matching source and destination types in Cranelift backend (`{:?}` -> `{:?}` in `{}`)",
                src_ty,
                dst_ty,
                self.raw_name(function.def)
            )));
        }
        let src_slot = slots.get(&src).copied().ok_or_else(|| {
            Diagnostic::error("missing source aggregate slot in Cranelift backend")
        })?;
        let dst_slot = slots.get(&dst).copied().ok_or_else(|| {
            Diagnostic::error("missing destination aggregate slot in Cranelift backend")
        })?;
        let src_addr = builder.ins().stack_addr(self.ptr_ty, src_slot, 0);
        let dst_addr = builder.ins().stack_addr(self.ptr_ty, dst_slot, 0);
        self.copy_aggregate_value(builder, &src_ty, src_addr, dst_addr)
    }

    fn copy_aggregate_value(
        &self,
        builder: &mut FunctionBuilder,
        ty: &Ty,
        src_addr: cranelift_codegen::ir::Value,
        dst_addr: cranelift_codegen::ir::Value,
    ) -> Result<(), Diagnostic> {
        match ty {
            Ty::Tuple(elems) => {
                for (index, field_ty) in elems.iter().enumerate() {
                    let (offset, _) = self.field_offset_and_ty(ty, index).ok_or_else(|| {
                        Diagnostic::error("invalid tuple copy layout in Cranelift backend")
                    })?;
                    let src = if offset == 0 {
                        src_addr
                    } else {
                        builder.ins().iadd_imm(src_addr, i64::from(offset))
                    };
                    let dst = if offset == 0 {
                        dst_addr
                    } else {
                        builder.ins().iadd_imm(dst_addr, i64::from(offset))
                    };
                    self.copy_scalar_or_nested(builder, field_ty, src, dst)?;
                }
                Ok(())
            }
            Ty::Named { def, .. } if self.find_struct(*def).is_some() => {
                let strukt = self.find_struct(*def).ok_or_else(|| {
                    Diagnostic::error("unknown struct layout in Cranelift backend")
                })?;
                for (index, field) in strukt.fields.iter().enumerate() {
                    let (offset, _) = self.field_offset_and_ty(ty, index).ok_or_else(|| {
                        Diagnostic::error("invalid struct copy layout in Cranelift backend")
                    })?;
                    let src = if offset == 0 {
                        src_addr
                    } else {
                        builder.ins().iadd_imm(src_addr, i64::from(offset))
                    };
                    let dst = if offset == 0 {
                        dst_addr
                    } else {
                        builder.ins().iadd_imm(dst_addr, i64::from(offset))
                    };
                    self.copy_scalar_or_nested(builder, &field.ty, src, dst)?;
                }
                Ok(())
            }
            Ty::Array { elem, len } => {
                for index in 0..*len {
                    let (offset, _) = self.field_offset_and_ty(ty, index).ok_or_else(|| {
                        Diagnostic::error("invalid array copy layout in Cranelift backend")
                    })?;
                    let src = if offset == 0 {
                        src_addr
                    } else {
                        builder.ins().iadd_imm(src_addr, i64::from(offset))
                    };
                    let dst = if offset == 0 {
                        dst_addr
                    } else {
                        builder.ins().iadd_imm(dst_addr, i64::from(offset))
                    };
                    self.copy_scalar_or_nested(builder, elem, src, dst)?;
                }
                Ok(())
            }
            Ty::Named { def, .. } if self.find_enum(*def).is_some() => {
                let layout = self
                    .enum_layout(*def)
                    .ok_or_else(|| Diagnostic::error("unknown enum layout in Cranelift backend"))?;
                self.copy_bytes(builder, src_addr, dst_addr, layout.size)
            }
            _ => Err(Diagnostic::error(
                "aggregate copy only supports tuple/struct/array/enum locals in Cranelift backend",
            )),
        }
    }

    fn copy_scalar_or_nested(
        &self,
        builder: &mut FunctionBuilder,
        ty: &Ty,
        src_addr: cranelift_codegen::ir::Value,
        dst_addr: cranelift_codegen::ir::Value,
    ) -> Result<(), Diagnostic> {
        if let Some(clif_ty) = self.clif_ty(ty) {
            let value = builder
                .ins()
                .load(clif_ty, MemFlags::trusted(), src_addr, 0);
            builder.ins().store(MemFlags::trusted(), value, dst_addr, 0);
            Ok(())
        } else {
            self.copy_aggregate_value(builder, ty, src_addr, dst_addr)
        }
    }

    fn copy_bytes(
        &self,
        builder: &mut FunctionBuilder,
        src_addr: cranelift_codegen::ir::Value,
        dst_addr: cranelift_codegen::ir::Value,
        size: u32,
    ) -> Result<(), Diagnostic> {
        let mut offset = 0u32;
        while offset < size {
            let remaining = size - offset;
            let (chunk_ty, chunk_size) = if remaining >= 8 {
                (types::I64, 8)
            } else if remaining >= 4 {
                (types::I32, 4)
            } else if remaining >= 2 {
                (types::I16, 2)
            } else {
                (types::I8, 1)
            };
            let src = if offset == 0 {
                src_addr
            } else {
                builder.ins().iadd_imm(src_addr, i64::from(offset))
            };
            let dst = if offset == 0 {
                dst_addr
            } else {
                builder.ins().iadd_imm(dst_addr, i64::from(offset))
            };
            let value = builder.ins().load(chunk_ty, MemFlags::trusted(), src, 0);
            builder.ins().store(MemFlags::trusted(), value, dst, 0);
            offset += chunk_size;
        }
        Ok(())
    }
}

fn align_to(value: u32, align: u32) -> u32 {
    if align <= 1 {
        value
    } else {
        let mask = align - 1;
        (value + mask) & !mask
    }
}

fn predecessor_counts(function: &MirFn) -> Vec<usize> {
    let mut counts = vec![0; function.basic_blocks.len()];
    for block in &function.basic_blocks {
        let Some(terminator) = &block.terminator else {
            continue;
        };
        match &terminator.kind {
            TerminatorKind::Goto(target) => counts[target.0 as usize] += 1,
            TerminatorKind::SwitchInt {
                targets, otherwise, ..
            } => {
                for (_, block) in targets {
                    counts[block.0 as usize] += 1;
                }
                counts[otherwise.0 as usize] += 1;
            }
            TerminatorKind::Call { target, unwind, .. } => {
                if let Some(target) = target {
                    counts[target.0 as usize] += 1;
                }
                if let Some(unwind) = unwind {
                    counts[unwind.0 as usize] += 1;
                }
            }
            TerminatorKind::Assert { target, .. } => counts[target.0 as usize] += 1,
            TerminatorKind::Drop { target, .. } => counts[target.0 as usize] += 1,
            TerminatorKind::ErrdeferUnwind(target) => counts[target.0 as usize] += 1,
            TerminatorKind::Return | TerminatorKind::Unreachable => {}
        }
    }
    counts
}

fn seal_if_ready(
    builder: &mut FunctionBuilder,
    clif_blocks: &[Block],
    remaining_preds: &mut [usize],
    sealed: &mut [bool],
    block: crate::mir::BlockId,
) {
    let slot = &mut remaining_preds[block.0 as usize];
    *slot = slot.saturating_sub(1);
    if *slot == 0 && !sealed[block.0 as usize] {
        builder.seal_block(clif_blocks[block.0 as usize]);
        sealed[block.0 as usize] = true;
    }
}

fn sanitize_symbol(name: &str) -> String {
    name.chars()
        .map(|ch| if ch.is_ascii_alphanumeric() { ch } else { '_' })
        .collect()
}
