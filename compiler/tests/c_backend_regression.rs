use daram_compiler::{
    analyze_to_codegen_mir, c_backend, lexer::lex_with_errors, lower_to_codegen_mir,
    name_resolution::resolve, parser::parse, source::FileId, type_checker,
};
use std::collections::{HashMap, HashSet, VecDeque};
use std::{
    fs,
    path::PathBuf,
    process::{Command, Output},
    time::{SystemTime, UNIX_EPOCH},
};

fn codegen_clean(source: &str) -> String {
    let (tokens, lex_errors) = lex_with_errors(source);
    assert!(lex_errors.is_empty(), "lex errors: {:?}", lex_errors);

    let (ast, parse_errors) = parse(FileId(0), &tokens);
    assert!(parse_errors.is_empty(), "parse errors: {:?}", parse_errors);

    let (mut hir, resolve_errors) = resolve(FileId(0), &ast);
    assert!(
        resolve_errors.is_empty(),
        "resolve errors: {:?}",
        resolve_errors
    );

    let type_errors = type_checker::check_and_prepare(FileId(0), &mut hir);
    assert!(type_errors.is_empty(), "type errors: {:?}", type_errors);

    let (mir_module, mir_errors) = lower_to_codegen_mir(&hir);
    assert!(mir_errors.is_empty(), "mir errors: {:?}", mir_errors);

    c_backend::generate_c(&hir, &mir_module).expect("expected C backend codegen to succeed")
}

fn codegen_with_prelude(source: &str) -> String {
    let bundled = daram_compiler::stdlib_bundle::with_bundled_prelude(source);
    let analyzed = analyze_to_codegen_mir(&bundled, "bundled-prelude.dr");
    assert!(
        analyzed.diagnostics.is_empty(),
        "frontend diagnostics: {:?}",
        analyzed.diagnostics
    );
    let hir = analyzed.hir.expect("expected bundled HIR");
    let mir_module = analyzed.mir.expect("expected bundled MIR");
    let mir_module = prune_to_reachable_defs(&hir, &mir_module, "main");
    c_backend::generate_c(&hir, &mir_module).expect("expected C backend codegen to succeed")
}

fn unique_temp_dir(prefix: &str) -> PathBuf {
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("expected monotonic system time")
        .as_nanos();
    let dir = std::env::temp_dir().join(format!(
        "daram-c-backend-{prefix}-{}-{stamp}",
        std::process::id()
    ));
    fs::create_dir_all(&dir).expect("expected temp dir creation to succeed");
    dir
}

fn compile_and_run_c(source: &str) -> Output {
    let dir = unique_temp_dir("runtime");
    let c_path = dir.join("main.c");
    let binary_path = dir.join("main.out");
    fs::write(&c_path, source).expect("expected C source write to succeed");

    let compile = Command::new("cc")
        .arg("-std=c99")
        .arg(&c_path)
        .arg("-o")
        .arg(&binary_path)
        .output()
        .expect("expected `cc` to run");
    assert!(
        compile.status.success(),
        "C compile failed: stdout={}\nstderr={}",
        String::from_utf8_lossy(&compile.stdout),
        String::from_utf8_lossy(&compile.stderr)
    );

    Command::new(&binary_path)
        .output()
        .expect("expected generated binary to run")
}

fn prune_to_reachable_defs(
    hir: &daram_compiler::hir::HirModule,
    mir_module: &daram_compiler::mir::MirModule,
    entry_name: &str,
) -> daram_compiler::mir::MirModule {
    use daram_compiler::mir::{
        AggregateKind, MirConstItem, MirFn, Operand, Rvalue, StatementKind, TerminatorKind,
    };

    let function_map = mir_module
        .functions
        .iter()
        .map(|function| (function.def, function.clone()))
        .collect::<HashMap<_, _>>();
    let const_map = mir_module
        .consts
        .iter()
        .map(|item| (item.def, item.clone()))
        .collect::<HashMap<_, _>>();

    let Some(entry_def) = mir_module
        .functions
        .iter()
        .find(|function| {
            hir.def_names
                .get(&function.def)
                .is_some_and(|name| name == entry_name)
        })
        .map(|function| function.def)
    else {
        return mir_module.clone();
    };

    fn operand_defs(operand: &Operand, out: &mut Vec<daram_compiler::hir::DefId>) {
        if let Operand::Def(def) = operand {
            out.push(*def);
        }
    }

    fn rvalue_defs(rvalue: &Rvalue, out: &mut Vec<daram_compiler::hir::DefId>) {
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

    fn function_defs(function: &MirFn) -> Vec<daram_compiler::hir::DefId> {
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

    fn const_value_defs(
        value: &daram_compiler::mir::MirConst,
        out: &mut Vec<daram_compiler::hir::DefId>,
    ) {
        match value {
            daram_compiler::mir::MirConst::Tuple(items)
            | daram_compiler::mir::MirConst::Array(items) => {
                for item in items {
                    const_value_defs(item, out);
                }
            }
            daram_compiler::mir::MirConst::Struct { def, fields } => {
                out.push(*def);
                for field in fields {
                    const_value_defs(field, out);
                }
            }
            daram_compiler::mir::MirConst::Ref(inner) => const_value_defs(inner, out),
            daram_compiler::mir::MirConst::Bool(_)
            | daram_compiler::mir::MirConst::Int(_)
            | daram_compiler::mir::MirConst::Uint(_)
            | daram_compiler::mir::MirConst::Float(_)
            | daram_compiler::mir::MirConst::Char(_)
            | daram_compiler::mir::MirConst::Str(_)
            | daram_compiler::mir::MirConst::Unit
            | daram_compiler::mir::MirConst::Undef => {}
        }
    }

    fn const_defs(item: &MirConstItem) -> Vec<daram_compiler::hir::DefId> {
        let mut defs = Vec::new();
        const_value_defs(&item.value, &mut defs);
        defs
    }

    let mut reachable_fns = HashSet::new();
    let mut reachable_consts = HashSet::new();
    let mut queue = VecDeque::from([entry_def]);

    while let Some(def) = queue.pop_front() {
        if let Some(function) = function_map.get(&def) {
            if !reachable_fns.insert(def) {
                continue;
            }
            for nested in function_defs(function) {
                if function_map.contains_key(&nested) && !reachable_fns.contains(&nested) {
                    queue.push_back(nested);
                }
                if const_map.contains_key(&nested) && reachable_consts.insert(nested) {
                    if let Some(item) = const_map.get(&nested) {
                        for nested_const in const_defs(item) {
                            if function_map.contains_key(&nested_const)
                                && !reachable_fns.contains(&nested_const)
                            {
                                queue.push_back(nested_const);
                            }
                        }
                    }
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

    daram_compiler::mir::MirModule {
        functions: mir_module
            .functions
            .iter()
            .filter(|function| reachable_fns.contains(&function.def))
            .cloned()
            .collect(),
        consts: mir_module
            .consts
            .iter()
            .filter(|item| reachable_consts.contains(&item.def))
            .cloned()
            .collect(),
        enum_variant_indices: mir_module.enum_variant_indices.clone(),
        enum_variant_names: mir_module.enum_variant_names.clone(),
        struct_field_names: mir_module.struct_field_names.clone(),
        display_impls: mir_module.display_impls.clone(),
        def_names: mir_module.def_names.clone(),
    }
}

#[test]
fn c_backend_generates_entry_wrapper_and_builtin_call() {
    let code = codegen_clean(
        r#"
        fn helper() -> i32 {
            7
        }

        fn main() -> i32 {
            std::io::println("hello c backend");
            helper()
        }
        "#,
    );

    assert!(code.contains("int main(void) { return daram_entry_main(); }"));
    assert!(code.contains("daram_println_str(\"hello c backend\"); l1 = 0;"));
    assert!(code.contains("static long long daram_entry_main()"));
}

#[test]
fn c_backend_does_not_redeclare_argument_locals_inside_function_body() {
    let code = codegen_clean(
        r#"
        fun echo(value: string): string {
            value
        }
        "#,
    );

    assert!(code.contains("static const char * echo(const char * l1)"));
    assert!(!code.contains("const char * l1 = \"\";"));
}

#[test]
fn c_backend_supports_runtime_vec_handles_from_bundled_stdlib() {
    let code = codegen_with_prelude(
        r#"
        fun main(): i32 {
            const values = std::collections::Vec::new();
            std::collections::Vec::push(values, 1);
            std::collections::Vec::push(values, 2);
            std::collections::Vec::len(values) as i32
        }
        "#,
    );

    assert!(code.contains("static void * std__collections__Vec__new()"));
    assert!(code.contains("daram_vec_new()"));
    assert!(code.contains("daram_vec_push_i64"));
    assert!(code.contains("daram_vec_len"));
}

#[test]
fn c_backend_supports_runtime_hashmap_handles_from_bundled_stdlib() {
    let code = codegen_with_prelude(
        r#"
        fun main(): i32 {
            const values = std::collections::HashMap::new();
            values.len() as i32
        }
        "#,
    );

    assert!(code.contains("static void * std__collections__HashMap__new()"));
    assert!(code.contains("daram_hashmap_new()"));
    assert!(code.contains("daram_hashmap_len"));
}

#[test]
fn c_backend_supports_tuple_struct_aggregates_and_field_reads() {
    let code = codegen_clean(
        r#"
        struct Pair(i32, i32)

        fun first(pair: Pair): i32 {
            pair.0
        }

        fun main(): i32 {
            const pair = Pair(3, 4);
            first(pair)
        }
        "#,
    );

    assert!(code.contains("struct Pair"));
    assert!(code.contains("struct Pair l"));
    assert!(code.contains(".f0"));
}

#[test]
fn c_backend_supports_named_struct_field_assignment() {
    let code = codegen_clean(
        r#"
        struct Counter {
            value: i32,
        }

        fun main(): i32 {
            let counter = Counter { value: 1 };
            counter.value = 7;
            counter.value
        }
        "#,
    );

    assert!(code.contains("struct Counter"));
    assert!(code.contains(".f0 = 7LL;"));
}

#[test]
fn c_backend_supports_enum_discriminants_and_variant_payload_projection() {
    let code = codegen_clean(
        r#"
        enum Value {
            Int(i32),
            Flag(bool),
        }

        fun unwrap(value: Value): i32 {
            match value {
                Value::Int(inner) => inner,
                Value::Flag(flag) => if flag { 1 } else { 0 },
            }
        }

        fun main(): i32 {
            const value = Value::Int(7);
            unwrap(value)
        }
        "#,
    );

    assert!(code.contains("struct Value"));
    assert!(code.contains("long long tag;"));
    assert!(code.contains(".payload.v0.f0"));
}

#[test]
fn c_backend_supports_generic_option_payloads_from_bundled_prelude() {
    let code = codegen_with_prelude(
        r#"
        fun main(): i32 {
            const value = std::core::Option::Some(7);
            match value {
                std::core::Option::Some(inner) => inner,
                std::core::Option::None => 0,
            }
        }
        "#,
    );

    assert!(code.contains("struct std__core__Option"));
    assert!(code.contains("long long tag;"));
    assert!(code.contains(".payload.v0.f0"));
}

#[test]
fn c_backend_supports_struct_aggregate_returns_and_field_chains() {
    let code = codegen_clean(
        r#"
        struct Inner {
            value: i32,
        }

        struct Outer {
            inner: Inner,
        }

        fun make_outer(): Outer {
            Outer { inner: Inner { value: 9 } }
        }

        fun main(): i32 {
            make_outer().inner.value
        }
        "#,
    );

    assert!(code.contains("static struct Outer make_outer()"));
    assert!(code.contains("return l0;"));
    assert!(code.contains(".f0.f0"));
}

#[test]
fn c_backend_distinguishes_generic_enum_instantiations() {
    let code = codegen_with_prelude(
        r#"
        fun main(): i32 {
            let left: std::core::Option<i32> = std::core::Option::Some(7);
            let right: std::core::Option<bool> = std::core::Option::Some(true);
            match left {
                std::core::Option::Some(inner) => inner + match right {
                    std::core::Option::Some(flag) => if flag { 1 } else { 0 },
                    std::core::Option::None => 0,
                },
                std::core::Option::None => 0,
            }
        }
        "#,
    );

    assert!(code.contains("struct std__core__Option__i32"));
    assert!(code.contains("struct std__core__Option__bool"));
}

#[test]
fn c_backend_distinguishes_generic_struct_instantiations() {
    let code = codegen_clean(
        r#"
        struct Box<T> {
            value: T,
        }

        fun main(): i32 {
            let left: Box<i32> = Box { value: 4 };
            let right: Box<bool> = Box { value: true };
            left.value + if right.value { 1 } else { 0 }
        }
        "#,
    );

    assert!(code.contains("struct Box__i32"));
    assert!(code.contains("struct Box__bool"));
}

#[test]
fn c_backend_supports_array_index_projection_and_assignment() {
    let code = codegen_clean(
        r#"
        fun main(): i32 {
            let values: [i32; 3] = [1, 2, 3];
            values[1] = 5;
            values[1]
        }
        "#,
    );

    assert!(code.contains("struct array_3_i32"));
    assert!(code.contains(".data["));
    assert!(code.contains("= 5LL;"));
}

#[test]
fn c_backend_supports_array_iteration_lowering() {
    let code = codegen_clean(
        r#"
        fun main(): i32 {
            let values: [i32; 3] = [1, 2, 3];
            let total = 0;
            for item in values {
                total += *item;
            }
            total
        }
        "#,
    );

    assert!(code.contains("struct array_3_i32"));
    assert!(code.contains(".data["));
    assert!(code.contains("3ULL"));
}

#[test]
fn c_backend_runs_errdefer_cleanup_before_resuming_original_panic() {
    let code = codegen_clean(
        r#"
        fun fail() {
            panic("boom");
        }

        fun cleanup() {
            std::io::eprintln("cleanup");
        }

        fun main(): i32 {
            errdefer {
                cleanup();
            }
            fail();
            0
        }
        "#,
    );

    assert!(code.contains("setjmp("));
    assert!(code.contains("daram_resume_unwind_msg"));

    let output = compile_and_run_c(&code);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        !output.status.success(),
        "expected panic exit, status={:?}, stderr={stderr}",
        output.status
    );
    assert!(stderr.contains("cleanup"), "stderr was: {stderr}");
    assert!(stderr.contains("boom"), "stderr was: {stderr}");
}

#[test]
fn c_backend_errdefer_cleanup_panic_overrides_original_failure() {
    let code = codegen_clean(
        r#"
        fun fail() {
            panic("boom");
        }

        fun cleanup() {
            panic("cleanup");
        }

        fun main(): i32 {
            errdefer {
                cleanup();
            }
            fail();
            0
        }
        "#,
    );

    let output = compile_and_run_c(&code);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        !output.status.success(),
        "expected panic exit, status={:?}, stderr={stderr}",
        output.status
    );
    assert!(stderr.contains("cleanup"), "stderr was: {stderr}");
    assert!(!stderr.contains("boom"), "stderr was: {stderr}");
}

#[test]
fn c_backend_runs_errdefer_cleanup_on_builtin_assert_failure() {
    let code = codegen_clean(
        r#"
        fun cleanup() {
            std::io::eprintln("cleanup");
        }

        fun main(): i32 {
            errdefer {
                cleanup();
            }
            assert(false);
            0
        }
        "#,
    );

    let output = compile_and_run_c(&code);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        !output.status.success(),
        "expected assertion exit, status={:?}, stderr={stderr}",
        output.status
    );
    assert!(stderr.contains("cleanup"), "stderr was: {stderr}");
    assert!(stderr.contains("assertion failed"), "stderr was: {stderr}");
}

#[test]
fn c_backend_supports_f32_locals_and_comparisons() {
    let code = codegen_clean(
        r#"
        fun main(): i32 {
            let value: f32 = 1.25 as f32;
            if value > (1.0 as f32) {
                7
            } else {
                0
            }
        }
        "#,
    );

    assert!(code.contains("float l"));

    let output = compile_and_run_c(&code);
    assert_eq!(output.status.code(), Some(7));
}
