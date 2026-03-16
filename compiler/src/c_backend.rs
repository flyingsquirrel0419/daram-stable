use crate::{
    backend_capabilities::{self, BackendFeature, BackendKind},
    diagnostics::Diagnostic,
    hir::{DefId, FloatSize, Ty},
    mir::{
        AggregateKind, MirBinOp, MirConst, MirFn, MirModule, MirUnaryOp, Operand, Place,
        Projection, Rvalue, StatementKind, TerminatorKind,
    },
};
use std::collections::HashMap;

pub fn generate_c(hir: &crate::hir::HirModule, mir: &MirModule) -> Result<String, Vec<Diagnostic>> {
    let mut backend = CBackend {
        hir,
        mir,
        def_names: &mir.def_names,
        diagnostics: Vec::new(),
        next_unwind_id: 0,
    };
    let source = backend.generate_module(mir);
    if backend.diagnostics.is_empty() {
        Ok(source)
    } else {
        Err(backend.diagnostics)
    }
}

struct CBackend<'a> {
    hir: &'a crate::hir::HirModule,
    mir: &'a MirModule,
    def_names: &'a HashMap<DefId, String>,
    diagnostics: Vec<Diagnostic>,
    next_unwind_id: usize,
}

impl<'a> CBackend<'a> {
    fn generate_module(&mut self, mir: &MirModule) -> String {
        let mut out = String::new();
        out.push_str(&crate::native_runtime::c_backend_support_source());

        let aggregate_decls = self.collect_aggregate_decls(mir);
        for ty in aggregate_decls {
            if let Some(decl) = self.aggregate_decl(&ty) {
                out.push_str(&decl);
                out.push('\n');
            }
        }
        out.push('\n');

        // Emit extern "C" declarations from HIR extern_fns
        for ext_fn in &self.hir.extern_fns {
            let ret_ty = self
                .c_type(&ext_fn.ret_ty)
                .unwrap_or_else(|| "void".to_string());
            let params: Vec<String> = ext_fn
                .params
                .iter()
                .enumerate()
                .filter_map(|(i, p)| {
                    let ty = self.c_type(&p.ty)?;
                    Some(format!("{ty} _p{i}"))
                })
                .collect();
            let params_str = if params.is_empty() {
                "void".to_string()
            } else {
                params.join(", ")
            };
            out.push_str(&format!(
                "extern {ret_ty} {}({params_str});\n",
                &ext_fn.name
            ));
        }
        if !self.hir.extern_fns.is_empty() {
            out.push('\n');
        }

        // Collect all closure defs used in the module
        let mut closure_defs: Vec<(DefId, usize)> = Vec::new();
        for function in &mir.functions {
            for block in &function.basic_blocks {
                for stmt in &block.statements {
                    if let StatementKind::Assign(_, rvalue) = &stmt.kind {
                        if let Rvalue::Aggregate(AggregateKind::Closure(def), _) = rvalue {
                            if !closure_defs.iter().any(|(d, _)| d == def) {
                                if let Some(idx) = mir.functions.iter().position(|f| f.def == *def)
                                {
                                    closure_defs.push((*def, idx));
                                }
                            }
                        }
                    }
                }
            }
        }
        for (def, fn_idx) in &closure_defs {
            // We need to get a reference to the function separately to avoid borrow issues
            let ret_ty = mir.functions[*fn_idx].locals.first().map(|l| l.ty.clone());
            if let Some(decl) = ret_ty.and_then(|ty| self.closure_struct_decl(*def, &ty)) {
                out.push_str(&decl);
                out.push('\n');
            }
        }

        for function in &mir.functions {
            if let Some(signature) = self.function_signature(function) {
                out.push_str(&signature);
                out.push_str(";\n");
            }
        }
        out.push('\n');

        for function in &mir.functions {
            if let Some(body) = self.function_definition(function) {
                out.push_str(&body);
                out.push('\n');
            }
        }

        if mir
            .functions
            .iter()
            .any(|function| self.is_entry_main(function.def))
        {
            out.push_str("int main(void) { return daram_entry_main(); }\n");
        }

        out
    }

    fn function_signature(&mut self, function: &MirFn) -> Option<String> {
        let ret_ty = self.c_type(&function.locals.first()?.ty)?;
        let name = self.fn_name(function.def);
        let storage = "static ";
        let mut params = Vec::new();
        for local in function.locals.iter().skip(1).take(function.argc) {
            let ty = self.c_type(&local.ty)?;
            params.push(format!("{ty} {}", self.local_name(local.id.0)));
        }
        Some(format!("{storage}{ret_ty} {name}({})", params.join(", ")))
    }

    fn function_definition(&mut self, function: &MirFn) -> Option<String> {
        if function.is_extern {
            return None;
        }
        let mut out = String::new();
        out.push_str(&self.function_signature(function)?);
        out.push_str(" {\n");
        if self.function_uses_unwind(function) {
            out.push_str("  const char * daram_pending_unwind = NULL;\n");
        }
        for (index, local) in function.locals.iter().enumerate() {
            if index > 0 && index <= function.argc {
                continue;
            }
            let Some(ty) = self.c_type(&local.ty) else {
                return None;
            };
            out.push_str(&format!(
                "  {ty} {} = {};\n",
                self.local_name(local.id.0),
                self.zero_value(&local.ty)?
            ));
        }
        out.push_str("  goto bb0;\n");

        for block in &function.basic_blocks {
            out.push_str(&format!("bb{}:\n", block.id.0));
            for statement in &block.statements {
                if let Some(stmt) = self.statement(function, statement)? {
                    out.push_str("  ");
                    out.push_str(&stmt);
                    out.push('\n');
                }
            }
            let Some(terminator) = &block.terminator else {
                self.diagnostics
                    .push(Diagnostic::error("missing MIR terminator for C backend"));
                return None;
            };
            out.push_str("  ");
            out.push_str(&self.terminator(function, terminator)?);
            out.push('\n');
        }

        out.push_str("}\n");
        Some(out)
    }

    fn statement(
        &mut self,
        function: &MirFn,
        statement: &crate::mir::Statement,
    ) -> Option<Option<String>> {
        match &statement.kind {
            StatementKind::Assign(place, rvalue) => {
                let target = self.place(function, place)?;
                let target_ty = function
                    .locals
                    .get(place.local.0 as usize)
                    .map(|local| &local.ty);
                let value = self.rvalue(function, target_ty, rvalue)?;
                Some(Some(format!("{target} = {value};")))
            }
            StatementKind::StorageLive(_)
            | StatementKind::StorageDead(_)
            | StatementKind::DeferStart(_)
            | StatementKind::ErrdeferStart(_)
            | StatementKind::Nop => Some(None),
        }
    }

    fn terminator(
        &mut self,
        function: &MirFn,
        terminator: &crate::mir::Terminator,
    ) -> Option<String> {
        match &terminator.kind {
            TerminatorKind::Return => Some(format!("return {};", self.local_name(0))),
            TerminatorKind::Goto(target) => Some(format!("goto bb{};", target.0)),
            TerminatorKind::SwitchInt {
                discriminant,
                targets,
                otherwise,
            } => {
                let discr = self.operand(discriminant)?;
                let mut out = String::new();
                out.push_str(&format!("switch ((long long)({discr})) {{ "));
                for (value, block) in targets {
                    out.push_str(&format!("case {}: goto bb{}; ", value, block.0));
                }
                out.push_str(&format!("default: goto bb{}; }}", otherwise.0));
                Some(out)
            }
            TerminatorKind::Call {
                callee,
                args,
                destination,
                target,
                unwind,
            } => {
                let target_block = *target.as_ref()?;
                let dest = self.place(function, destination)?;
                let ret_ty = &function.locals[destination.local.0 as usize].ty;

                // Determine whether this is a direct call or an indirect closure call
                let direct_name = self.callee_name(callee);
                let indirect_local = self.callee_local(callee);

                // Build the call expression and an optional callee name (for void-check)
                let (call, is_void_callee) = if let Some(ref callee_name) = direct_name {
                    // Direct named call — use existing render_call for built-in dispatch
                    let arg_tys = args
                        .iter()
                        .map(|arg| self.operand_ty(function, arg))
                        .collect::<Option<Vec<_>>>()?;
                    let rendered_args = args
                        .iter()
                        .zip(arg_tys.iter())
                        .map(|(arg, ty)| self.render_builtin_arg(arg, ty))
                        .collect::<Option<Vec<_>>>()?;
                    let call = self.render_call(callee_name, &rendered_args, &arg_tys)?;
                    let is_void = self.call_returns_void(callee_name);
                    (call, is_void)
                } else if let Some(local_id) = indirect_local {
                    // Indirect call through a closure struct's fn_ptr field
                    let closure_local = self.local_name(local_id);
                    let rendered_args = args
                        .iter()
                        .map(|arg| self.operand(arg))
                        .collect::<Option<Vec<_>>>()?;
                    let sep = if rendered_args.is_empty() { "" } else { ", " };
                    let args_str = rendered_args.join(", ");
                    let call = format!(
                        "{closure_local}.fn_ptr({closure_local}.env{sep}{args_str}"
                    ) + ")";
                    (call, false)
                } else {
                    self.diagnostics.push(Diagnostic::error(
                        "C backend requires direct function calls or closure locals",
                    ));
                    return None;
                };

                if let Some(unwind_block) = unwind {
                    let frame_id = self.next_unwind_id;
                    self.next_unwind_id += 1;
                    let frame_name = format!("daram_unwind_frame_{frame_id}");
                    let success = if is_void_callee || matches!(ret_ty, Ty::Unit | Ty::Never) {
                        format!("{call}; {dest} = 0;")
                    } else {
                        format!("{dest} = {call};")
                    };
                    Some(format!(
                        "{{ daram_unwind_frame {frame_name}; daram_enter_unwind(&{frame_name}); if (setjmp({frame_name}.buf) == 0) {{ {success} daram_pop_unwind(); goto bb{}; }} else {{ daram_pending_unwind = {frame_name}.message ? {frame_name}.message : \"panic\"; daram_pop_unwind(); goto bb{}; }} }}",
                        target_block.0, unwind_block.0
                    ))
                } else if is_void_callee || matches!(ret_ty, Ty::Unit | Ty::Never) {
                    Some(format!("{call}; {dest} = 0; goto bb{};", target_block.0))
                } else {
                    Some(format!("{dest} = {call}; goto bb{};", target_block.0))
                }
            }
            TerminatorKind::Assert {
                cond,
                expected,
                msg,
                target,
            } => {
                let cond = self.operand(cond)?;
                Some(format!(
                    "if (((bool)({cond})) == {}) goto bb{}; else daram_panic_str({:?});",
                    if *expected { "true" } else { "false" },
                    target.0,
                    msg
                ))
            }
            TerminatorKind::Drop { target, .. } => {
                // TODO(#43): native C lowering still treats MIR drop as a control-flow edge.
                // Actual destructor or runtime free calls need type-directed lowering.
                Some(format!("goto bb{};", target.0))
            }
            TerminatorKind::Unreachable => Some("abort();".to_string()),
            TerminatorKind::ErrdeferUnwind(target) => Some(format!(
                "if (daram_pending_unwind != NULL) {{ daram_resume_unwind_msg(daram_pending_unwind); }} else goto bb{};",
                target.0
            )),
        }
    }

    fn function_uses_unwind(&self, function: &MirFn) -> bool {
        function.basic_blocks.iter().any(|block| {
            block.terminator.as_ref().is_some_and(|terminator| {
                matches!(
                    terminator.kind,
                    TerminatorKind::Call {
                        unwind: Some(_),
                        ..
                    } | TerminatorKind::ErrdeferUnwind(_)
                )
            })
        })
    }

    fn render_call(
        &mut self,
        callee_name: &str,
        rendered_args: &[String],
        arg_tys: &[Ty],
    ) -> Option<String> {
        let rendered = match callee_name {
            "__builtin_vec_new" => "daram_vec_new()".to_string(),
            "__builtin_vec_push" => {
                let helper = match arg_tys.get(1) {
                    Some(ty) if self.is_runtime_slot_i64_like(ty) => "daram_vec_push_i64",
                    Some(ty) if self.is_runtime_slot_ptr_like(ty) => "daram_vec_push_ptr",
                    Some(ty) => {
                        self.diagnostics.push(Diagnostic::error(format!(
                            "C backend does not yet support Vec element type `{:?}`",
                            ty
                        )));
                        return None;
                    }
                    None => {
                        self.diagnostics.push(Diagnostic::error(
                            "C backend expected Vec push element argument",
                        ));
                        return None;
                    }
                };
                format!("{helper}({})", rendered_args.join(", "))
            }
            "__builtin_vec_len" => format!("daram_vec_len({})", rendered_args.join(", ")),
            "__builtin_hashmap_new" => "daram_hashmap_new()".to_string(),
            "__builtin_hashmap_len" => format!("daram_hashmap_len({})", rendered_args.join(", ")),
            "print" | "std::io::print" | "std__io__print" | "__builtin_print" => {
                format!(
                    "{}({})",
                    self.print_helper("daram_print", arg_tys.first()?)?,
                    rendered_args.join(", ")
                )
            }
            "println" | "std::io::println" | "std__io__println" | "__builtin_println" => {
                format!(
                    "{}({})",
                    self.print_helper("daram_println", arg_tys.first()?)?,
                    rendered_args.join(", ")
                )
            }
            "eprint" | "std::io::eprint" | "std__io__eprint" | "__builtin_eprint" => {
                format!(
                    "{}({})",
                    self.print_helper("daram_eprint", arg_tys.first()?)?,
                    rendered_args.join(", ")
                )
            }
            "eprintln" | "std::io::eprintln" | "std__io__eprintln" | "__builtin_eprintln" => {
                format!(
                    "{}({})",
                    self.print_helper("daram_eprintln", arg_tys.first()?)?,
                    rendered_args.join(", ")
                )
            }
            "assert" | "std::test::assert" | "std__test__assert" | "__builtin_assert" => {
                format!("daram_assert({})", rendered_args.join(", "))
            }
            "assert_eq"
            | "std::test::assert_eq"
            | "std__test__assert_eq"
            | "__builtin_assert_eq" => {
                format!("daram_assert_eq_i64({})", rendered_args.join(", "))
            }
            "panic" | "std::core::panic" | "std__core__panic" | "__builtin_panic" => {
                format!("daram_panic_str({})", rendered_args.join(", "))
            }
            "panic_with_fmt"
            | "std::test::panic_with_fmt"
            | "std__test__panic_with_fmt"
            | "__builtin_panic_with_fmt" => {
                format!("daram_panic_with_fmt_i64({})", rendered_args.join(", "))
            }
            other => format!("{}({})", sanitize_c_ident(other), rendered_args.join(", ")),
        };
        Some(rendered)
    }

    fn call_returns_void(&self, callee_name: &str) -> bool {
        matches!(
            callee_name,
            "print"
                | "std::io::print"
                | "std__io__print"
                | "__builtin_print"
                | "println"
                | "std::io::println"
                | "std__io__println"
                | "__builtin_println"
                | "eprint"
                | "std::io::eprint"
                | "std__io__eprint"
                | "__builtin_eprint"
                | "eprintln"
                | "std::io::eprintln"
                | "std__io__eprintln"
                | "__builtin_eprintln"
                | "assert"
                | "std::test::assert"
                | "std__test__assert"
                | "__builtin_assert"
                | "assert_eq"
                | "std::test::assert_eq"
                | "std__test__assert_eq"
                | "__builtin_assert_eq"
                | "panic_with_fmt"
                | "std::test::panic_with_fmt"
                | "std__test__panic_with_fmt"
                | "__builtin_panic_with_fmt"
                | "panic"
                | "std::core::panic"
                | "std__core__panic"
                | "__builtin_panic"
                | "__builtin_vec_push"
        )
    }

    fn print_helper(&mut self, prefix: &str, ty: &Ty) -> Option<String> {
        match self.c_type(ty)?.as_str() {
            "const char *" => Some(format!("{prefix}_str")),
            "long long" | "unsigned long long" | "bool" | "int" => Some(format!("{prefix}_i64")),
            _ => {
                self.diagnostics
                    .push(Diagnostic::error("unsupported builtin print argument type"));
                None
            }
        }
    }

    fn render_builtin_arg(&mut self, operand: &Operand, _ty: &Ty) -> Option<String> {
        self.operand(operand)
    }

    fn callee_name(&mut self, operand: &Operand) -> Option<String> {
        match operand {
            Operand::Def(def) => Some(self.raw_name(*def)),
            _ => None,
        }
    }

    fn callee_local(&self, operand: &Operand) -> Option<u32> {
        match operand {
            Operand::Copy(local) | Operand::Move(local) => Some(local.0),
            _ => None,
        }
    }

    fn operand_ty(&mut self, function: &MirFn, operand: &Operand) -> Option<Ty> {
        match operand {
            Operand::Copy(local) | Operand::Move(local) => function
                .locals
                .get(local.0 as usize)
                .map(|decl| decl.ty.clone()),
            Operand::Const(value) => Some(self.const_ty(value)),
            Operand::Def(_) => Some(Ty::Unit),
        }
    }

    fn const_ty(&self, value: &MirConst) -> Ty {
        match value {
            MirConst::Bool(_) => Ty::Bool,
            MirConst::Int(_) => Ty::Int(crate::hir::IntSize::I64),
            MirConst::Uint(_) => Ty::Uint(crate::hir::UintSize::U64),
            MirConst::Char(_) => Ty::Char,
            MirConst::Str(_) => Ty::Str,
            MirConst::Unit => Ty::Unit,
            MirConst::Float(_) => Ty::Float(crate::hir::FloatSize::F64),
            MirConst::Tuple(_)
            | MirConst::Array(_)
            | MirConst::Struct { .. }
            | MirConst::Ref(_)
            | MirConst::Undef => Ty::Unit,
        }
    }

    fn rvalue(
        &mut self,
        function: &MirFn,
        target_ty: Option<&Ty>,
        rvalue: &Rvalue,
    ) -> Option<String> {
        match rvalue {
            Rvalue::Use(operand) => self.operand(operand),
            Rvalue::Read(place) => self.place(function, place),
            Rvalue::BinaryOp { op, lhs, rhs } => {
                let lhs = self.operand(lhs)?;
                let rhs = self.operand(rhs)?;
                Some(format!("({lhs} {} {rhs})", self.bin_op(*op)?))
            }
            Rvalue::UnaryOp { op, operand } => {
                let operand = self.operand(operand)?;
                Some(format!("({}{operand})", self.unary_op(*op)?))
            }
            Rvalue::Cast {
                operand, target_ty, ..
            } => {
                let operand = self.operand(operand)?;
                Some(format!("(({})({operand}))", self.c_type(target_ty)?))
            }
            Rvalue::Discriminant(place) => {
                let (rendered, ty) =
                    self.projected_place(&function.locals[place.local.0 as usize].ty, place)?;
                match ty {
                    Ty::Named { def, .. } if self.find_enum(def).is_some() => {
                        Some(format!("{rendered}.tag"))
                    }
                    _ => Some(rendered),
                }
            }
            Rvalue::Len(place) => {
                let (rendered, ty) =
                    self.projected_place(&function.locals[place.local.0 as usize].ty, place)?;
                match ty {
                    Ty::Array { len, .. } => Some(format!("{len}ULL")),
                    Ty::Slice(_) => Some(format!("{rendered}.len")),
                    _ => Some(format!("sizeof({rendered})")),
                }
            }
            Rvalue::Aggregate(AggregateKind::Struct(def), operands) => {
                let aggregate_ty = target_ty.cloned().unwrap_or(Ty::Named {
                    def: *def,
                    args: Vec::new(),
                });
                let ty = self.c_named_type(&aggregate_ty)?;
                let fields = operands
                    .iter()
                    .map(|operand| self.operand(operand))
                    .collect::<Option<Vec<_>>>()?;
                Some(format!("({ty}){{ {} }}", fields.join(", ")))
            }
            Rvalue::Aggregate(AggregateKind::Enum { def, variant_idx }, operands) => {
                let aggregate_ty = target_ty.cloned().unwrap_or(Ty::Named {
                    def: *def,
                    args: Vec::new(),
                });
                let ty = self.c_named_type(&aggregate_ty)?;
                let fields = operands
                    .iter()
                    .map(|operand| self.operand(operand))
                    .collect::<Option<Vec<_>>>()?;
                if fields.is_empty() {
                    Some(format!("({ty}){{ .tag = {variant_idx}LL }}"))
                } else {
                    Some(format!(
                        "({ty}){{ .tag = {variant_idx}LL, .payload.v{variant_idx} = {{ {} }} }}",
                        fields.join(", ")
                    ))
                }
            }
            Rvalue::Aggregate(AggregateKind::Array(elem_ty), operands) => {
                let aggregate_ty = target_ty.cloned().unwrap_or(Ty::Array {
                    elem: Box::new(elem_ty.clone()),
                    len: operands.len(),
                });
                let ty = self.c_aggregate_type(&aggregate_ty)?;
                let fields = operands
                    .iter()
                    .map(|operand| self.operand(operand))
                    .collect::<Option<Vec<_>>>()?;
                Some(format!("({ty}){{ .data = {{ {} }} }}", fields.join(", ")))
            }
            Rvalue::Ref { place, .. } | Rvalue::AddressOf { place, .. } => {
                Some(format!("(&{})", self.place(function, place)?))
            }
            Rvalue::Aggregate(AggregateKind::Closure(def), _operands) => {
                let fn_name = self.fn_name(*def);
                let struct_name = format!("daram_closure_{}", def.index);
                // Find the closure function's return type to build a matching cast.
                // The cast must match the fn_ptr field type emitted by closure_struct_decl.
                let ret_ty = self
                    .mir
                    .functions
                    .iter()
                    .find(|f| f.def == *def)
                    .and_then(|f| f.locals.first())
                    .and_then(|l| self.c_type(&l.ty))
                    .unwrap_or_else(|| "void*".to_string());
                Some(format!(
                    "({struct_name}){{ .fn_ptr = ({ret_ty}(*)(void*)){fn_name}, .env = NULL }}"
                ))
            }
            Rvalue::Aggregate(_, _) => {
                self.diagnostics.push(Diagnostic::error(
                    "C backend does not yet support this MIR aggregate",
                ));
                None
            }
        }
    }

    fn operand(&mut self, operand: &Operand) -> Option<String> {
        match operand {
            Operand::Copy(local) | Operand::Move(local) => Some(self.local_name(local.0)),
            Operand::Def(def) => Some(self.fn_name(*def)),
            Operand::Const(value) => self.const_value(value),
        }
    }

    fn const_value(&mut self, value: &MirConst) -> Option<String> {
        match value {
            MirConst::Bool(value) => Some(if *value {
                "true".into()
            } else {
                "false".into()
            }),
            MirConst::Int(value) => Some(format!("{value}LL")),
            MirConst::Uint(value) => Some(format!("{value}ULL")),
            MirConst::Float(value) => Some(format!("{value}")),
            MirConst::Char(value) => Some(format!("{}", *value as u32)),
            MirConst::Str(value) => Some(format!("{value:?}")),
            MirConst::Unit => Some("0".into()),
            MirConst::Undef => Some("0".into()),
            MirConst::Tuple(_) | MirConst::Ref(_) => {
                self.diagnostics.push(Diagnostic::error(
                    "C backend does not yet support aggregate constants",
                ));
                None
            }
            MirConst::Struct { def, fields } => {
                let ty = self.c_named_type(&Ty::Named {
                    def: *def,
                    args: Vec::new(),
                })?;
                let fields = fields
                    .iter()
                    .map(|field| self.const_value(field))
                    .collect::<Option<Vec<_>>>()?;
                Some(format!("({ty}){{ {} }}", fields.join(", ")))
            }
            MirConst::Array(items) => {
                let elem_ty = items
                    .first()
                    .map(|item| self.const_ty(item))
                    .unwrap_or(Ty::Unit);
                let ty = self.c_aggregate_type(&Ty::Array {
                    elem: Box::new(elem_ty),
                    len: items.len(),
                })?;
                let items = items
                    .iter()
                    .map(|item| self.const_value(item))
                    .collect::<Option<Vec<_>>>()?;
                Some(format!("({ty}){{ .data = {{ {} }} }}", items.join(", ")))
            }
        }
    }

    fn place(&mut self, function: &MirFn, place: &Place) -> Option<String> {
        let base_ty = function.locals.get(place.local.0 as usize)?.ty.clone();
        self.place_with_ty(&base_ty, place)
    }

    fn place_with_ty(&mut self, base_ty: &Ty, place: &Place) -> Option<String> {
        self.projected_place(base_ty, place)
            .map(|(rendered, _)| rendered)
    }

    fn projected_place(&mut self, base_ty: &Ty, place: &Place) -> Option<(String, Ty)> {
        let mut rendered = self.local_name(place.local.0);
        let mut current_ty = base_ty.clone();
        for projection in &place.projections {
            match projection {
                Projection::Deref => {
                    rendered = format!("(*{rendered})");
                    current_ty = match current_ty {
                        Ty::Ref { inner, .. } | Ty::RawPtr { inner, .. } => *inner,
                        other => {
                            self.diagnostics.push(Diagnostic::error(format!(
                                "C backend cannot dereference non-pointer type `{:?}`",
                                other
                            )));
                            return None;
                        }
                    };
                }
                Projection::Field(index) => {
                    rendered = format!("{rendered}.f{index}");
                    current_ty = self.field_ty(&current_ty, *index)?;
                }
                Projection::VariantField {
                    variant_idx,
                    field_idx,
                } => {
                    rendered = format!("{rendered}.payload.v{variant_idx}.f{field_idx}");
                    current_ty =
                        self.enum_variant_field_ty(&current_ty, *variant_idx, *field_idx)?;
                }
                Projection::Index(local) => match &current_ty {
                    Ty::Array { elem, .. } => {
                        rendered = format!("{rendered}.data[{}]", self.local_name(local.0));
                        current_ty = *elem.clone();
                    }
                    Ty::Slice(elem) => {
                        rendered = format!("{rendered}.data[{}]", self.local_name(local.0));
                        current_ty = *elem.clone();
                    }
                    other => {
                        self.diagnostics.push(Diagnostic::error(format!(
                            "C backend cannot index into type `{:?}`",
                            other
                        )));
                        return None;
                    }
                },
            }
        }
        Some((rendered, current_ty))
    }

    fn c_type(&mut self, ty: &Ty) -> Option<String> {
        match ty {
            Ty::Bool => Some("bool".into()),
            Ty::Int(_) => Some("long long".into()),
            Ty::Uint(_) => Some("unsigned long long".into()),
            Ty::Float(FloatSize::F32) => Some("float".into()),
            Ty::Float(FloatSize::F64) => Some("double".into()),
            Ty::Char => Some("int".into()),
            Ty::Unit => Some("int".into()),
            Ty::Never => Some("int".into()),
            Ty::String | Ty::Str => Some("const char *".into()),
            Ty::Ref { inner, .. } | Ty::RawPtr { inner, .. } => {
                Some(format!("{}*", self.c_type(inner)?.trim_end()))
            }
            Ty::Array { .. } | Ty::Slice(_) => self.c_aggregate_type(ty),
            Ty::Named { def, .. } if self.is_runtime_handle_def(*def) => Some("void *".into()),
            Ty::Named { def, .. }
                if self.find_struct(*def).is_some() || self.find_enum(*def).is_some() =>
            {
                self.c_named_type(ty)
            }
            Ty::Var(_) => Some("long long".into()),
            Ty::Named { def, .. } => {
                let name = self.raw_name(*def);
                self.diagnostics.push(Diagnostic::error(format!(
                    "C backend does not yet support type `{:?}` ({name})",
                    ty
                )));
                None
            }
            _ => {
                self.diagnostics.push(Diagnostic::error(format!(
                    "C backend does not yet support type `{:?}`",
                    ty
                )));
                None
            }
        }
    }

    fn zero_value(&mut self, ty: &Ty) -> Option<String> {
        let c_ty = self.c_type(ty)?;
        match c_ty.as_str() {
            "bool" => Some("false".into()),
            "const char *" => Some("\"\"".into()),
            "void *" => Some("NULL".into()),
            _ if c_ty.ends_with('*') => Some("NULL".into()),
            _ if matches!(ty, Ty::Array { .. } | Ty::Slice(_)) => Some(format!("({c_ty}){{0}}")),
            _ if matches!(ty, Ty::Named { def, .. } if self.find_struct(*def).is_some() || self.find_enum(*def).is_some()) => {
                Some(format!("({c_ty}){{0}}"))
            }
            _ => Some("0".into()),
        }
    }

    fn bin_op(&mut self, op: MirBinOp) -> Option<&'static str> {
        Some(match op {
            MirBinOp::Add => "+",
            MirBinOp::Sub => "-",
            MirBinOp::Mul => "*",
            MirBinOp::Div => "/",
            MirBinOp::Rem => "%",
            MirBinOp::BitAnd => "&",
            MirBinOp::BitOr => "|",
            MirBinOp::BitXor => "^",
            MirBinOp::Shl => "<<",
            MirBinOp::Shr => ">>",
            MirBinOp::Eq => "==",
            MirBinOp::Ne => "!=",
            MirBinOp::Lt => "<",
            MirBinOp::Le => "<=",
            MirBinOp::Gt => ">",
            MirBinOp::Ge => ">=",
            MirBinOp::Offset => {
                self.diagnostics
                    .push(backend_capabilities::unsupported_feature_diagnostic(
                        BackendKind::C,
                        BackendFeature::PointerOffset,
                        None,
                    ));
                return None;
            }
        })
    }

    fn unary_op(&mut self, op: MirUnaryOp) -> Option<&'static str> {
        Some(match op {
            MirUnaryOp::Not => "!",
            MirUnaryOp::Neg => "-",
        })
    }

    fn fn_name(&self, def: DefId) -> String {
        if self.is_entry_main(def) {
            "daram_entry_main".to_string()
        } else {
            sanitize_c_ident(&self.raw_name(def))
        }
    }

    fn raw_name(&self, def: DefId) -> String {
        self.def_names
            .get(&def)
            .cloned()
            .unwrap_or_else(|| format!("def_{}", def.index))
    }

    fn is_entry_main(&self, def: DefId) -> bool {
        let raw = self.raw_name(def);
        raw == "main" || raw.ends_with("::main") || raw.ends_with("__main")
    }

    fn local_name(&self, id: u32) -> String {
        format!("l{id}")
    }

    fn is_runtime_handle_def(&self, def: DefId) -> bool {
        matches!(
            self.raw_name(def).as_str(),
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

    fn is_runtime_slot_i64_like(&self, ty: &Ty) -> bool {
        matches!(
            ty,
            Ty::Bool | Ty::Char | Ty::Int(_) | Ty::Uint(_) | Ty::Var(_)
        )
    }

    fn is_runtime_slot_ptr_like(&self, ty: &Ty) -> bool {
        match ty {
            Ty::Str | Ty::String | Ty::Ref { .. } | Ty::RawPtr { .. } | Ty::FnPtr { .. } => true,
            Ty::Named { def, .. } => self.is_runtime_handle_def(*def),
            _ => false,
        }
    }

    fn collect_aggregate_decls(&self, mir: &MirModule) -> Vec<Ty> {
        fn visit_ty(backend: &CBackend<'_>, ty: &Ty, seen: &mut Vec<Ty>, visiting: &mut Vec<Ty>) {
            match ty {
                Ty::Named { .. }
                    if backend.named_struct_fields(ty).is_some()
                        || backend.named_enum_variants(ty).is_some() =>
                {
                    if seen.iter().any(|item| item == ty) || visiting.iter().any(|item| item == ty)
                    {
                        return;
                    }
                    visiting.push(ty.clone());
                    if let Some(fields) = backend.named_struct_fields(ty) {
                        for field_ty in fields {
                            visit_ty(backend, &field_ty, seen, visiting);
                        }
                    }
                    if let Some(variants) = backend.named_enum_variants(ty) {
                        for fields in variants {
                            for field_ty in fields {
                                visit_ty(backend, &field_ty, seen, visiting);
                            }
                        }
                    }
                    visiting.pop();
                    seen.push(ty.clone());
                }
                Ty::Array { elem, .. } | Ty::Slice(elem) => {
                    if seen.iter().any(|item| item == ty) || visiting.iter().any(|item| item == ty)
                    {
                        return;
                    }
                    visiting.push(ty.clone());
                    visit_ty(backend, elem, seen, visiting);
                    visiting.pop();
                    seen.push(ty.clone());
                }
                Ty::Ref { inner, .. } | Ty::RawPtr { inner, .. } => {
                    visit_ty(backend, inner, seen, visiting)
                }
                Ty::Tuple(items) => {
                    for item in items {
                        visit_ty(backend, item, seen, visiting);
                    }
                }
                _ => {}
            }
        }

        let mut decls = Vec::new();
        let mut visiting = Vec::new();
        for function in &mir.functions {
            for local in &function.locals {
                visit_ty(self, &local.ty, &mut decls, &mut visiting);
            }
        }
        for item in &mir.consts {
            visit_ty(self, &item.ty, &mut decls, &mut visiting);
        }
        decls
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

    fn c_named_type(&self, ty: &Ty) -> Option<String> {
        let Ty::Named { def, .. } = ty else {
            return None;
        };
        if self.find_struct(*def).is_none() && self.find_enum(*def).is_none() {
            return None;
        }
        Some(format!(
            "struct {}",
            sanitize_c_ident(&self.type_mangle(ty))
        ))
    }

    fn c_aggregate_type(&self, ty: &Ty) -> Option<String> {
        match ty {
            Ty::Array { .. } | Ty::Slice(_) => Some(format!(
                "struct {}",
                sanitize_c_ident(&self.type_mangle(ty))
            )),
            Ty::Named { .. } => self.c_named_type(ty),
            _ => None,
        }
    }

    fn aggregate_decl(&mut self, ty: &Ty) -> Option<String> {
        match ty {
            Ty::Named { def, .. } => {
                if self.find_struct(*def).is_some() {
                    return self.struct_decl(ty);
                }
                if self.find_enum(*def).is_some() {
                    return self.enum_decl(ty);
                }
                None
            }
            Ty::Array { .. } => self.array_decl(ty),
            Ty::Slice(_) => self.slice_decl(ty),
            _ => None,
        }
    }

    fn struct_decl(&mut self, ty: &Ty) -> Option<String> {
        self.find_struct(match ty {
            Ty::Named { def, .. } => *def,
            _ => return None,
        })?;
        let struct_ty = self.c_named_type(ty)?;
        let mut out = format!("{struct_ty} {{\n");
        for (index, field_ty) in self.named_struct_fields(ty)?.iter().enumerate() {
            out.push_str(&format!("  {} f{};\n", self.c_type(field_ty)?, index));
        }
        out.push_str("};\n");
        Some(out)
    }

    fn enum_decl(&mut self, ty: &Ty) -> Option<String> {
        let variants = self.named_enum_variants(ty)?;
        let ty_name = self.c_named_type(ty)?;
        let mut out = format!("{ty_name} {{\n  long long tag;\n  union {{\n");
        for (variant_idx, fields) in variants.iter().enumerate() {
            out.push_str("    struct {\n");
            if fields.is_empty() {
                out.push_str("      int _unused;\n");
            } else {
                for (field_idx, field_ty) in fields.iter().enumerate() {
                    out.push_str(&format!(
                        "      {} f{};\n",
                        self.c_type(field_ty)?,
                        field_idx
                    ));
                }
            }
            out.push_str(&format!("    }} v{variant_idx};\n"));
        }
        out.push_str("  } payload;\n};\n");
        Some(out)
    }

    fn array_decl(&mut self, ty: &Ty) -> Option<String> {
        let Ty::Array { elem, len } = ty else {
            return None;
        };
        let ty_name = self.c_aggregate_type(ty)?;
        let elem_ty = self.c_type(elem)?;
        Some(format!("{ty_name} {{\n  {elem_ty} data[{len}];\n}};\n"))
    }

    fn slice_decl(&mut self, ty: &Ty) -> Option<String> {
        let Ty::Slice(elem) = ty else {
            return None;
        };
        let ty_name = self.c_aggregate_type(ty)?;
        let elem_ty = self.c_type(elem)?;
        Some(format!(
            "{ty_name} {{\n  {elem_ty}* data;\n  unsigned long long len;\n}};\n"
        ))
    }

    fn field_ty(&self, ty: &Ty, index: usize) -> Option<Ty> {
        match ty {
            Ty::Tuple(items) => items.get(index).cloned(),
            Ty::Array { elem, .. } => Some(*elem.clone()),
            Ty::Named { .. } => self.named_struct_fields(ty)?.get(index).cloned(),
            _ => None,
        }
    }

    fn enum_variant_field_ty(&self, ty: &Ty, variant_idx: usize, field_idx: usize) -> Option<Ty> {
        self.named_enum_variants(ty)?
            .get(variant_idx)?
            .get(field_idx)
            .cloned()
    }

    fn named_struct_fields(&self, ty: &Ty) -> Option<Vec<Ty>> {
        let Ty::Named { def, args } = ty else {
            return None;
        };
        let strukt = self.find_struct(*def)?;
        let vars = self.collect_ty_vars(
            &strukt
                .fields
                .iter()
                .map(|field| field.ty.clone())
                .collect::<Vec<_>>(),
        );
        let mapping = vars
            .into_iter()
            .zip(args.iter().cloned())
            .collect::<HashMap<_, _>>();
        Some(
            strukt
                .fields
                .iter()
                .map(|field| self.substitute_ty(&field.ty, &mapping))
                .collect(),
        )
    }

    fn named_enum_variants(&self, ty: &Ty) -> Option<Vec<Vec<Ty>>> {
        let Ty::Named { def, args } = ty else {
            return None;
        };
        let enum_def = self.find_enum(*def)?;
        let vars = self.collect_ty_vars(
            &enum_def
                .variants
                .iter()
                .flat_map(|variant| variant.fields.iter().cloned())
                .collect::<Vec<_>>(),
        );
        let mapping = vars
            .into_iter()
            .zip(args.iter().cloned())
            .collect::<HashMap<_, _>>();
        Some(
            enum_def
                .variants
                .iter()
                .map(|variant| {
                    variant
                        .fields
                        .iter()
                        .map(|field| self.substitute_ty(field, &mapping))
                        .collect()
                })
                .collect(),
        )
    }

    fn collect_ty_vars(&self, tys: &[Ty]) -> Vec<u32> {
        fn visit(ty: &Ty, ordered: &mut Vec<u32>) {
            match ty {
                Ty::Var(id) => {
                    if !ordered.contains(id) {
                        ordered.push(*id);
                    }
                }
                Ty::Ref { inner, .. } | Ty::RawPtr { inner, .. } | Ty::Slice(inner) => {
                    visit(inner, ordered)
                }
                Ty::Array { elem, .. } => visit(elem, ordered),
                Ty::Tuple(items) => {
                    for item in items {
                        visit(item, ordered);
                    }
                }
                Ty::Named { args, .. } => {
                    for arg in args {
                        visit(arg, ordered);
                    }
                }
                Ty::FnPtr { params, ret } => {
                    for param in params {
                        visit(param, ordered);
                    }
                    visit(ret, ordered);
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

        let mut ordered = Vec::new();
        for ty in tys {
            visit(ty, &mut ordered);
        }
        ordered
    }

    fn substitute_ty(&self, ty: &Ty, mapping: &HashMap<u32, Ty>) -> Ty {
        match ty {
            Ty::Var(id) => mapping.get(id).cloned().unwrap_or(Ty::Var(*id)),
            Ty::Ref { mutable, inner } => Ty::Ref {
                mutable: *mutable,
                inner: Box::new(self.substitute_ty(inner, mapping)),
            },
            Ty::RawPtr { mutable, inner } => Ty::RawPtr {
                mutable: *mutable,
                inner: Box::new(self.substitute_ty(inner, mapping)),
            },
            Ty::Array { elem, len } => Ty::Array {
                elem: Box::new(self.substitute_ty(elem, mapping)),
                len: *len,
            },
            Ty::Slice(inner) => Ty::Slice(Box::new(self.substitute_ty(inner, mapping))),
            Ty::Tuple(items) => Ty::Tuple(
                items
                    .iter()
                    .map(|item| self.substitute_ty(item, mapping))
                    .collect(),
            ),
            Ty::Named { def, args } => Ty::Named {
                def: *def,
                args: args
                    .iter()
                    .map(|arg| self.substitute_ty(arg, mapping))
                    .collect(),
            },
            Ty::FnPtr { params, ret } => Ty::FnPtr {
                params: params
                    .iter()
                    .map(|param| self.substitute_ty(param, mapping))
                    .collect(),
                ret: Box::new(self.substitute_ty(ret, mapping)),
            },
            other => other.clone(),
        }
    }

    fn closure_struct_decl(&mut self, def: DefId, ret_ty: &Ty) -> Option<String> {
        let c_ret_ty = self.c_type(ret_ty)?;
        let name = format!("daram_closure_{}", def.index);
        let fn_ptr_ty = format!("{c_ret_ty} (*fn_ptr)(void*)");
        Some(format!(
            "typedef struct {{ {fn_ptr_ty}; void* env; }} {name};\n"
        ))
    }

    fn type_mangle(&self, ty: &Ty) -> String {
        match ty {
            Ty::Bool => "bool".into(),
            Ty::Char => "char".into(),
            Ty::Int(size) => format!("{size:?}").to_lowercase(),
            Ty::Uint(size) => format!("{size:?}").to_lowercase(),
            Ty::Float(size) => format!("{size:?}").to_lowercase(),
            Ty::Unit => "unit".into(),
            Ty::Never => "never".into(),
            Ty::Ref { mutable, inner } => {
                let prefix = if *mutable { "mutref" } else { "ref" };
                format!("{prefix}_{}", self.type_mangle(inner))
            }
            Ty::RawPtr { mutable, inner } => {
                let prefix = if *mutable { "mutptr" } else { "ptr" };
                format!("{prefix}_{}", self.type_mangle(inner))
            }
            Ty::Array { elem, len } => format!("array_{len}_{}", self.type_mangle(elem)),
            Ty::Slice(inner) => format!("slice_{}", self.type_mangle(inner)),
            Ty::Tuple(items) => format!(
                "tuple_{}",
                items
                    .iter()
                    .map(|item| self.type_mangle(item))
                    .collect::<Vec<_>>()
                    .join("_")
            ),
            Ty::Named { def, args } => {
                let mut name = self.raw_name(*def);
                if !args.is_empty() {
                    name.push_str("__");
                    name.push_str(
                        &args
                            .iter()
                            .map(|arg| self.type_mangle(arg))
                            .collect::<Vec<_>>()
                            .join("__"),
                    );
                }
                name
            }
            Ty::FnPtr { params, ret } => format!(
                "fn_{}_to_{}",
                params
                    .iter()
                    .map(|param| self.type_mangle(param))
                    .collect::<Vec<_>>()
                    .join("_"),
                self.type_mangle(ret)
            ),
            Ty::Var(id) => format!("var_{id}"),
            Ty::ImplTrait(_) => "impl_trait".into(),
            Ty::DynTrait(def) => format!("dyn_{}", self.raw_name(*def)),
            Ty::Str => "str".into(),
            Ty::String => "string".into(),
        }
    }
}

fn sanitize_c_ident(name: &str) -> String {
    let mut out = String::new();
    for ch in name.chars() {
        if ch.is_ascii_alphanumeric() {
            out.push(ch);
        } else {
            out.push('_');
        }
    }
    if out.is_empty() {
        "unnamed".to_string()
    } else {
        out
    }
}
