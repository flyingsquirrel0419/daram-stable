use daram_compiler::{
    analyze_to_codegen_mir,
    backend_capabilities::{supports, BackendFeature, BackendKind},
    c_backend, cranelift_backend,
    interpreter::{self, Value},
    native_runtime::{
        c_backend_support_source, exported_runtime_functions, link_runtime_source,
        runtime_ownership_rules, RuntimeOwnershipKind, RuntimeTy,
    },
};
use std::collections::HashSet;
use std::{
    fs,
    path::PathBuf,
    process::{Command, Output},
    time::{SystemTime, UNIX_EPOCH},
};

fn unique_temp_dir(prefix: &str) -> PathBuf {
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("expected monotonic system time")
        .as_nanos();
    let dir = std::env::temp_dir().join(format!(
        "daram-runtime-contract-{prefix}-{}-{stamp}",
        std::process::id()
    ));
    fs::create_dir_all(&dir).expect("expected temp dir creation to succeed");
    dir
}

fn compile_and_run_native_sources(
    prefix: &str,
    main_source_name: &str,
    main_source: &[u8],
    extra_sources: &[(&str, &str)],
) -> Output {
    let dir = unique_temp_dir(prefix);
    let main_path = dir.join(main_source_name);
    let binary_path = dir.join("main.out");
    fs::write(&main_path, main_source).expect("expected main source write to succeed");

    let mut command = Command::new("cc");
    command.arg("-std=c99").arg(&main_path);

    for (name, source) in extra_sources {
        let path = dir.join(name);
        fs::write(&path, source).expect("expected extra source write to succeed");
        command.arg(path);
    }

    let compile = command
        .arg("-o")
        .arg(&binary_path)
        .output()
        .expect("expected `cc` to run");
    assert!(
        compile.status.success(),
        "native compile failed: stdout={}\nstderr={}",
        String::from_utf8_lossy(&compile.stdout),
        String::from_utf8_lossy(&compile.stderr)
    );

    Command::new(&binary_path)
        .output()
        .expect("expected generated binary to run")
}

fn runtime_ty_name(ty: RuntimeTy) -> &'static str {
    match ty {
        RuntimeTy::Ptr => "ptr",
        RuntimeTy::I8 => "i8",
        RuntimeTy::I64 => "i64",
    }
}

fn analyze_clean_to_mir(
    source: &str,
    file_name: &str,
) -> (
    daram_compiler::hir::HirModule,
    daram_compiler::mir::MirModule,
) {
    let analyzed = analyze_to_codegen_mir(source, file_name);
    assert!(
        analyzed.diagnostics.is_empty(),
        "frontend diagnostics: {:?}",
        analyzed.diagnostics
    );
    (
        analyzed.hir.expect("expected lowered HIR"),
        analyzed.mir.expect("expected lowered MIR"),
    )
}

#[test]
fn exported_runtime_functions_are_unique_and_linked() {
    let exported = exported_runtime_functions();
    let mut seen = HashSet::new();
    let link_source = link_runtime_source();
    let c_backend_source = c_backend_support_source();

    for function in exported {
        assert!(
            seen.insert(function.name),
            "duplicate exported runtime helper `{}`",
            function.name
        );
        assert!(
            link_source.contains(function.name),
            "linked runtime source is missing `{}`",
            function.name
        );
        if function.name.starts_with("daram_print")
            || function.name.starts_with("daram_eprint")
            || function.name.starts_with("daram_assert")
            || function.name.starts_with("daram_panic")
        {
            assert!(
                c_backend_source.contains(function.name),
                "C backend runtime support is missing `{}`",
                function.name
            );
        }
    }
}

#[test]
fn backend_capability_matrix_matches_current_expectations() {
    for feature in [
        BackendFeature::UnwindCalls,
        BackendFeature::ErrdeferUnwind,
        BackendFeature::IndirectCalls,
        BackendFeature::PointerOffset,
        BackendFeature::AggregateReturn,
    ] {
        assert!(supports(BackendKind::Interpreter, feature));
    }
    assert!(!supports(
        BackendKind::Interpreter,
        BackendFeature::DropSemantics
    ));

    assert!(supports(BackendKind::C, BackendFeature::UnwindCalls));
    assert!(supports(BackendKind::C, BackendFeature::ErrdeferUnwind));
    assert!(supports(BackendKind::C, BackendFeature::IndirectCalls));
    assert!(supports(BackendKind::C, BackendFeature::AggregateReturn));
    assert!(!supports(BackendKind::C, BackendFeature::PointerOffset));
    assert!(!supports(BackendKind::C, BackendFeature::DropSemantics));

    assert!(supports(
        BackendKind::Cranelift,
        BackendFeature::AggregateReturn
    ));
    assert!(!supports(
        BackendKind::Cranelift,
        BackendFeature::UnwindCalls
    ));
    assert!(!supports(
        BackendKind::Cranelift,
        BackendFeature::ErrdeferUnwind
    ));
    assert!(!supports(
        BackendKind::Cranelift,
        BackendFeature::IndirectCalls
    ));
    assert!(!supports(
        BackendKind::Cranelift,
        BackendFeature::PointerOffset
    ));
    assert!(!supports(
        BackendKind::Cranelift,
        BackendFeature::DropSemantics
    ));
}

#[test]
fn exported_runtime_function_signatures_match_snapshot() {
    let snapshot = include_str!("fixtures/runtime_abi_snapshot.txt").trim();
    let rendered = exported_runtime_functions()
        .iter()
        .map(|function| {
            let params = function
                .params
                .iter()
                .map(|ty| runtime_ty_name(*ty))
                .collect::<Vec<_>>()
                .join(", ");
            let returns = function
                .returns
                .iter()
                .map(|ty| runtime_ty_name(*ty))
                .collect::<Vec<_>>()
                .join(", ");
            format!("{}({params}) -> ({returns})", function.name)
        })
        .collect::<Vec<_>>()
        .join("\n");

    assert_eq!(rendered, snapshot);
}

#[test]
fn runtime_ownership_rules_cover_current_heap_resources() {
    let rules = runtime_ownership_rules();
    assert!(rules.iter().any(|rule| {
        rule.resource == "string inputs" && rule.ownership == RuntimeOwnershipKind::BorrowedInput
    }));
    assert!(rules.iter().any(|rule| {
        rule.resource == "Vec handles"
            && rule.ownership == RuntimeOwnershipKind::OpaqueHandleProcessLifetime
    }));
    assert!(rules.iter().any(|rule| {
        rule.resource == "HashMap handles"
            && rule.ownership == RuntimeOwnershipKind::OpaqueHandleProcessLifetime
    }));
}

#[test]
fn simple_entry_round_trips_across_interpreter_c_and_cranelift() {
    let source = r#"
        fun main(): i32 {
            7
        }
    "#;
    let (hir, mir) = analyze_clean_to_mir(source, "roundtrip.dr");

    let interpreted = interpreter::execute_function(&mir, &mir.def_names, "main", &[])
        .expect("expected interpreter execution to succeed");
    assert!(matches!(interpreted, Value::Int(7)), "got {interpreted:?}");

    let generated_c =
        c_backend::generate_c(&hir, &mir).expect("expected C backend code generation");
    let c_output = compile_and_run_native_sources("c", "main.c", generated_c.as_bytes(), &[]);
    assert_eq!(c_output.status.code(), Some(7));

    let object = cranelift_backend::generate_object(&hir, &mir)
        .expect("expected Cranelift object generation");
    let cranelift_output = compile_and_run_native_sources(
        "cranelift",
        "module.o",
        &object,
        &[("runtime.c", &link_runtime_source())],
    );
    assert_eq!(cranelift_output.status.code(), Some(7));
}
