use daram_compiler::{
    analyze_to_codegen_mir, cranelift_backend, lexer::lex_with_errors, lower_to_codegen_mir,
    name_resolution::resolve, parser::parse, source::FileId, type_checker,
};
use std::collections::{HashMap, HashSet, VecDeque};

fn codegen_object_clean(source: &str) -> Vec<u8> {
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

    cranelift_backend::generate_object(&hir, &mir_module)
        .expect("expected Cranelift backend object generation to succeed")
}

fn codegen_object_with_prelude(source: &str) -> Vec<u8> {
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
    cranelift_backend::generate_object(&hir, &mir_module)
        .expect("expected Cranelift backend object generation to succeed")
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
fn cranelift_backend_emits_native_object_for_simple_program() {
    let object = codegen_object_clean(
        r#"
        fn helper() -> i32 {
            7
        }

        fn main() -> i32 {
            std::io::println("hello cranelift");
            helper()
        }
        "#,
    );

    assert!(!object.is_empty());
    assert!(
        object.starts_with(&[0x7f, b'E', b'L', b'F'])
            || object.starts_with(&[0xcf, 0xfa])
            || object.starts_with(&[0x4d, 0x5a])
    );
}

#[test]
fn cranelift_backend_specializes_generic_function_calls() {
    let object = codegen_object_clean(
        r#"
        fun id<T>(value: T): T { value }

        fun main(): i32 {
            const number = id(7);
            const flag = id(true);
            if flag { number } else { 0 }
        }
        "#,
    );

    assert!(!object.is_empty(), "expected emitted object file");
}

#[test]
fn cranelift_backend_supports_scalar_casts() {
    let object = codegen_object_clean(
        r#"
        fn helper() -> i64 {
            let value = 7 as i64;
            value
        }

        fn main() -> i32 {
            helper() as i32
        }
        "#,
    );

    assert!(!object.is_empty());
}

#[test]
fn cranelift_backend_supports_float_and_unsigned_casts() {
    let object = codegen_object_clean(
        r#"
        fn main() -> i32 {
            let small = 250 as u8;
            let as_float = small as f64;
            let back: u32 = as_float as u32;
            back as i32
        }
        "#,
    );

    assert!(!object.is_empty());
}

#[test]
fn cranelift_backend_supports_indirect_function_calls() {
    let object = codegen_object_clean(
        r#"
        fun add_one(value: i32): i32 {
            value + 1
        }

        fun apply(f: fun(i32): i32, value: i32): i32 {
            f(value)
        }

        fun main(): i32 {
            apply(add_one, 41)
        }
        "#,
    );

    assert!(!object.is_empty());
}

#[test]
fn cranelift_backend_handles_control_flow_blocks() {
    let object = codegen_object_clean(
        r#"
        fn main() -> i32 {
            let mut total = 0;
            let mut index = 0;
            while index < 4 {
                if index == 2 {
                    total = total + 5;
                } else {
                    total = total + index;
                }
                index = index + 1;
            }
            total
        }
        "#,
    );

    assert!(!object.is_empty());
}

#[test]
fn cranelift_backend_selects_integer_builtin_helpers() {
    let object = codegen_object_clean(
        r#"
        fn main() -> i32 {
            std::io::println(7);
            std::io::eprintln(9);
            0
        }
        "#,
    );

    assert!(!object.is_empty());
}

#[test]
fn cranelift_backend_supports_scalar_references_and_deref_places() {
    let object = codegen_object_clean(
        r#"
        fn main() -> i32 {
            let value = 7;
            let ptr = &value;
            *ptr
        }
        "#,
    );

    assert!(!object.is_empty());
}

#[test]
fn cranelift_backend_supports_tuple_aggregate_and_field_reads() {
    let object = codegen_object_clean(
        r#"
        fn main() -> i32 {
            let pair = (3, 7);
            pair.1
        }
        "#,
    );

    assert!(!object.is_empty());
}

#[test]
fn cranelift_backend_supports_struct_aggregate_and_field_reads() {
    let object = codegen_object_clean(
        r#"
        struct Point {
            x: i32,
            y: i32,
        }

        fn main() -> i32 {
            let point: Point = Point { x: 4, y: 9 };
            point.y
        }
        "#,
    );

    assert!(!object.is_empty());
}

#[test]
fn cranelift_backend_supports_array_aggregate_and_index_reads() {
    let object = codegen_object_clean(
        r#"
        fn main() -> i32 {
            let values = [2, 5, 9];
            values[1]
        }
        "#,
    );

    assert!(!object.is_empty());
}

#[test]
fn cranelift_backend_supports_unit_enum_match() {
    let object = codegen_object_clean(
        r#"
        enum State {
            Idle,
            Busy,
        }

        fn main() -> i32 {
            let state = State::Busy;
            match state {
                State::Idle => 1,
                State::Busy => 2,
            }
        }
        "#,
    );

    assert!(!object.is_empty());
}

#[test]
fn cranelift_backend_supports_payload_enum_locals() {
    let object = codegen_object_clean(
        r#"
        enum OptionI32 {
            None,
            Some(i32),
        }

        fn main() -> i32 {
            let _value = OptionI32::Some(7);
            0
        }
        "#,
    );

    assert!(!object.is_empty());
}

#[test]
fn cranelift_backend_supports_common_enum_payload_match_reads() {
    let object = codegen_object_clean(
        r#"
        enum OptionI32 {
            None,
            Some(i32),
        }

        fn main() -> i32 {
            let value = OptionI32::Some(11);
            match value {
                OptionI32::Some(inner) => inner,
                OptionI32::None => 0,
            }
        }
        "#,
    );

    assert!(!object.is_empty());
}

#[test]
fn cranelift_backend_supports_complex_enum_payload_match_reads() {
    let object = codegen_object_clean(
        r#"
        enum Value {
            Int(i32),
            Flag(bool),
        }

        fn main() -> i32 {
            let value = Value::Int(13);
            match value {
                Value::Int(inner) => inner,
                Value::Flag(flag) => if flag { 1 } else { 0 },
            }
        }
        "#,
    );

    assert!(!object.is_empty());
}

#[test]
fn cranelift_backend_supports_indirect_function_pointer_calls() {
    let object = codegen_object_clean(
        r#"
        fn add_one(value: i32) -> i32 {
            value + 1
        }

        fn apply(f: fn(i32) -> i32, value: i32) -> i32 {
            f(value)
        }

        fn main() -> i32 {
            apply(add_one, 9)
        }
        "#,
    );

    assert!(!object.is_empty());
}

#[test]
fn cranelift_backend_supports_runtime_vec_handles_from_bundled_stdlib() {
    let object = codegen_object_with_prelude(
        r#"
        fun main(): i32 {
            const values = std::collections::Vec::new();
            values.push(1);
            values.push(2);
            values.len() as i32
        }
        "#,
    );

    assert!(!object.is_empty());
}

#[test]
fn cranelift_backend_supports_runtime_hashmap_handles_from_bundled_stdlib() {
    let object = codegen_object_with_prelude(
        r#"
        fun main(): i32 {
            const values = std::collections::HashMap::new();
            values.len() as i32
        }
        "#,
    );

    assert!(!object.is_empty());
}

#[test]
fn cranelift_backend_supports_runtime_hashmap_option_paths_from_bundled_stdlib() {
    let object = codegen_object_with_prelude(
        r#"
        fun main(): i32 {
            const values: std::collections::HashMap<i32, i32> =
                std::collections::HashMap::new();
            values.insert(1, 7);
            const removed = match values.remove(2) {
                std::core::Option::Some(value) => value,
                std::core::Option::None => 0,
            };
            match values.get(1) {
                std::core::Option::Some(value) => *value + removed,
                std::core::Option::None => 0,
            }
        }
        "#,
    );

    assert!(!object.is_empty());
}

#[test]
fn cranelift_backend_supports_runtime_hashmap_string_key_paths() {
    let object = codegen_object_with_prelude(
        r#"
        fun main(): i32 {
            const values: std::collections::HashMap<&str, i32> =
                std::collections::HashMap::new();
            values.insert("alpha", 7);
            values.insert("beta", 9);
            const removed = match values.remove("beta") {
                std::core::Option::Some(value) => value,
                std::core::Option::None => 0,
            };
            match values.get("alpha") {
                std::core::Option::Some(value) => *value + removed,
                std::core::Option::None => 0,
            }
        }
        "#,
    );

    assert!(!object.is_empty());
}

#[test]
fn cranelift_backend_supports_runtime_hashmap_string_key_string_value_paths() {
    let object = codegen_object_with_prelude(
        r#"
        fun main(): i32 {
            const values: std::collections::HashMap<&str, &str> =
                std::collections::HashMap::new();
            values.insert("alpha", "one");
            values.insert("beta", "two");
            const removed = match values.remove("beta") {
                std::core::Option::Some(_) => 9,
                std::core::Option::None => 0,
            };
            match values.get("alpha") {
                std::core::Option::Some(_) => 7 + removed,
                std::core::Option::None => 0,
            }
        }
        "#,
    );

    assert!(!object.is_empty());
}

#[test]
fn cranelift_backend_supports_runtime_hashmap_i64_key_string_value_paths() {
    let object = codegen_object_with_prelude(
        r#"
        fun main(): i32 {
            const values: std::collections::HashMap<i64, &str> =
                std::collections::HashMap::new();
            values.insert(1 as i64, "one");
            values.insert(2 as i64, "two");
            const removed = match values.remove(2 as i64) {
                std::core::Option::Some(_) => 9,
                std::core::Option::None => 0,
            };
            match values.get(1 as i64) {
                std::core::Option::Some(_) => 7 + removed,
                std::core::Option::None => 0,
            }
        }
        "#,
    );

    assert!(!object.is_empty());
}

#[test]
fn cranelift_backend_specializes_generic_struct_field_access() {
    // User-defined generic struct: the monomorphizer should create a concrete
    // specialization and Cranelift should be able to emit object code for it.
    // Note: aggregate parameters aren't yet supported by the Cranelift backend,
    // so we wrap and immediately read the field in the same function.
    let object = codegen_object_clean(
        r#"
        struct Wrapper<T> {
            value: T,
        }

        fun wrap_and_get<T>(v: T): T {
            const w = Wrapper { value: v };
            w.value
        }

        fun main(): i32 {
            wrap_and_get(42)
        }
        "#,
    );
    assert!(
        !object.is_empty(),
        "expected object file for generic struct"
    );
}
