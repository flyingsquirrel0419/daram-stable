use daram_compiler::{
    lexer::lex_with_errors,
    lower_to_codegen_mir,
    mir::{self, MirConst, Projection, Rvalue, StatementKind, TerminatorKind},
    name_resolution::resolve,
    parser::parse,
    source::FileId,
};

fn parse_clean(source: &str) -> daram_compiler::ast::Module {
    let (tokens, lex_errors) = lex_with_errors(source);
    assert!(
        lex_errors.is_empty(),
        "expected clean lex, got: {:?}",
        lex_errors
            .iter()
            .map(|error| error.message.as_str())
            .collect::<Vec<_>>()
    );

    let (module, parse_errors) = parse(FileId(0), &tokens);
    assert!(
        parse_errors.is_empty(),
        "expected clean parse, got: {:?}",
        parse_errors
            .iter()
            .map(|error| error.message.as_str())
            .collect::<Vec<_>>()
    );
    module
}

fn lower_clean(source: &str) -> daram_compiler::mir::MirModule {
    let module = parse_clean(source);
    let (hir, resolve_errors) = resolve(FileId(0), &module);
    assert!(
        resolve_errors.is_empty(),
        "expected clean lowering, got: {:?}",
        resolve_errors
            .iter()
            .map(|diag| diag.message.as_str())
            .collect::<Vec<_>>()
    );

    let (mir, diagnostics) = mir::lower(&hir);
    assert!(
        diagnostics.is_empty(),
        "expected clean MIR lowering, got: {:?}",
        diagnostics
            .iter()
            .map(|diag| diag.message.as_str())
            .collect::<Vec<_>>()
    );
    mir
}

fn lower_codegen_clean(source: &str) -> daram_compiler::mir::MirModule {
    let module = parse_clean(source);
    let (mut hir, resolve_errors) = resolve(FileId(0), &module);
    assert!(
        resolve_errors.is_empty(),
        "expected clean lowering, got: {:?}",
        resolve_errors
            .iter()
            .map(|diag| diag.message.as_str())
            .collect::<Vec<_>>()
    );

    let type_errors = daram_compiler::type_checker::check_and_prepare(FileId(0), &mut hir);
    assert!(
        type_errors.is_empty(),
        "expected clean type check, got: {:?}",
        type_errors
            .iter()
            .map(|diag| diag.message.as_str())
            .collect::<Vec<_>>()
    );

    let (mir, diagnostics) = lower_to_codegen_mir(&hir);
    assert!(
        diagnostics.is_empty(),
        "expected clean codegen MIR lowering, got: {:?}",
        diagnostics
            .iter()
            .map(|diag| diag.message.as_str())
            .collect::<Vec<_>>()
    );
    mir
}

#[test]
fn mir_lowers_literal_consts() {
    let mir = lower_clean(
        r#"
        const ANSWER: i32 = 42;
        const READY: bool = true;
        "#,
    );

    assert_eq!(mir.consts.len(), 2);
    assert!(matches!(mir.consts[0].value, MirConst::Int(42)));
    assert!(matches!(mir.consts[1].value, MirConst::Bool(true)));
}

#[test]
fn mir_lowers_binary_const_expressions() {
    let mir = lower_clean(
        r#"
        const LIMIT: i32 = 2 + 3 * 4;
        const ENABLED: bool = true && false;
        "#,
    );

    assert_eq!(mir.consts.len(), 2);
    assert!(matches!(mir.consts[0].value, MirConst::Int(14)));
    assert!(matches!(mir.consts[1].value, MirConst::Bool(false)));
}

#[test]
fn mir_lowers_cast_const_expressions() {
    let mir = lower_clean(
        r#"
        const NEG_ONE: i64 = -1 as i64;
        const ZERO: u32 = 0 as u32;
        "#,
    );

    assert_eq!(mir.consts.len(), 2);
    assert!(matches!(mir.consts[0].value, MirConst::Int(-1)));
    assert!(matches!(mir.consts[1].value, MirConst::Uint(0)));
}

#[test]
fn mir_lowers_aggregate_const_expressions() {
    let mir = lower_clean(
        r#"
        struct Point {
            x: i32,
            y: i32,
        }

        const PAIR: (i32, i32) = (1, 2);
        const VALUES: [i32; 3] = [1, 2, 3];
        const ORIGIN: Point = Point { x: 0, y: 0 };
        "#,
    );

    assert_eq!(mir.consts.len(), 3);
    assert!(matches!(mir.consts[0].value, MirConst::Tuple(_)));
    assert!(matches!(mir.consts[1].value, MirConst::Array(_)));
    assert!(matches!(mir.consts[2].value, MirConst::Struct { .. }));
}

#[test]
fn mir_lowers_calls_and_returns_into_cfg() {
    let mir = lower_clean(
        r#"
        fn helper() -> i32 {
            1
        }

        fn main() -> i32 {
            let value: i32 = helper();
            value
        }
        "#,
    );

    assert_eq!(mir.functions.len(), 2);
    let main_fn = mir
        .functions
        .iter()
        .find(|function| function.argc == 0 && function.locals.len() >= 3)
        .expect("expected main function in MIR");

    assert!(
        main_fn.basic_blocks.iter().any(|block| matches!(
            block.terminator.as_ref().map(|term| &term.kind),
            Some(TerminatorKind::Call { .. })
        )),
        "expected call terminator in MIR blocks: {:?}",
        main_fn.basic_blocks
    );
    assert!(
        main_fn.basic_blocks.iter().any(|block| matches!(
            block.terminator.as_ref().map(|term| &term.kind),
            Some(TerminatorKind::Return)
        )),
        "expected return terminator in MIR blocks: {:?}",
        main_fn.basic_blocks
    );
}

#[test]
fn mir_lowers_ability_default_method_bodies() {
    let mir = lower_clean(
        r#"
        ability Iterator {
            type Item;
            fun next(self): Self::Item;
            fun count(self): usize { 0 }
        }
        "#,
    );

    assert!(
        mir.functions.iter().any(|function| {
            mir.def_names.get(&function.def).map(String::as_str) == Some("Iterator::count")
        }),
        "expected MIR to contain lowered ability default method, got defs: {:?}",
        mir.def_names.values().collect::<Vec<_>>()
    );
}

#[test]
fn mir_lowers_if_while_and_match_branches() {
    let mir = lower_clean(
        r#"
        fn main(flag: bool) -> i32 {
            let mut value: i32 = 0;
            while flag {
                value = 1;
                break;
            }
            match flag {
                true => value,
                false => 2,
            }
        }
        "#,
    );

    let main_fn = mir.functions.first().expect("expected MIR function");
    let switch_count = main_fn
        .basic_blocks
        .iter()
        .filter(|block| {
            matches!(
                block.terminator.as_ref().map(|term| &term.kind),
                Some(TerminatorKind::SwitchInt { .. })
            )
        })
        .count();

    assert!(
        switch_count >= 2,
        "expected branch terminators for while/match, got blocks: {:?}",
        main_fn.basic_blocks
    );
}

#[test]
fn mir_lowers_short_circuit_boolean_control_flow() {
    let mir = lower_clean(
        r#"
        fn fail() -> bool {
            panic("boom")
        }

        fn main(flag: bool) -> bool {
            flag && fail()
        }
        "#,
    );

    let main_fn = mir
        .functions
        .iter()
        .find(|function| function.argc == 1)
        .expect("expected main function");

    let switch_count = main_fn
        .basic_blocks
        .iter()
        .filter(|block| {
            matches!(
                block.terminator.as_ref().map(|term| &term.kind),
                Some(TerminatorKind::SwitchInt { .. })
            )
        })
        .count();

    assert!(
        switch_count >= 1,
        "expected switch-based short-circuit lowering, got blocks: {:?}",
        main_fn.basic_blocks
    );
}

#[test]
fn mir_lowers_enum_match_patterns() {
    let mir = lower_clean(
        r#"
        enum Maybe {
            None,
            Some(i32),
        }

        fn main(value: Maybe) -> i32 {
            match value {
                None => 0,
                Some(inner) => inner,
            }
        }
        "#,
    );

    let main_fn = mir
        .functions
        .iter()
        .find(|function| function.argc == 1)
        .expect("expected main function");

    assert!(
        main_fn.basic_blocks.iter().any(|block| {
            matches!(
                block.terminator.as_ref().map(|term| &term.kind),
                Some(TerminatorKind::SwitchInt { .. })
            )
        }),
        "expected enum match to lower to switch, got blocks: {:?}",
        main_fn.basic_blocks
    );
}

#[test]
fn mir_lowers_place_projection_reads_and_refs() {
    let mir = lower_clean(
        r#"
        fn main() -> i32 {
            let pair: (i32, i32) = (1, 2);
            let ptr = &pair.0;
            *ptr
        }
        "#,
    );

    let main_fn = mir
        .functions
        .iter()
        .find(|function| {
            mir.def_names
                .get(&function.def)
                .is_some_and(|name| name == "main")
        })
        .expect("expected main function");

    assert!(
        main_fn.basic_blocks.iter().flat_map(|block| &block.statements).any(|stmt| {
            matches!(
                &stmt.kind,
                daram_compiler::mir::StatementKind::Assign(
                    _,
                    Rvalue::Ref {
                        place,
                        ..
                    }
                ) if place.projections.iter().any(|projection| matches!(projection, Projection::Field(_)))
            )
        }),
        "expected field reference projection in MIR, got blocks: {:?}",
        main_fn.basic_blocks
    );
    assert!(
        main_fn.basic_blocks.iter().flat_map(|block| &block.statements).any(|stmt| {
            matches!(
                &stmt.kind,
                daram_compiler::mir::StatementKind::Assign(_, Rvalue::Read(place))
                    if place.projections.iter().any(|projection| matches!(projection, Projection::Deref))
            )
        }),
        "expected deref read projection in MIR, got blocks: {:?}",
        main_fn.basic_blocks
    );
}

#[test]
fn codegen_mir_prunes_unspecialized_generic_templates() {
    let mir = lower_codegen_clean(
        r#"
        fun id<T>(value: T): T { value }

        fun main(): i32 {
            const number = id(7);
            const flag = id(true);
            if flag { number } else { 0 }
        }
        "#,
    );

    assert!(
        mir.def_names
            .values()
            .any(|name| name.contains("id__mono_")),
        "expected specialized id clones in MIR names: {:?}",
        mir.def_names
    );
    assert!(
        !mir.functions.iter().any(|function| {
            mir.def_names
                .get(&function.def)
                .is_some_and(|name| name == "id")
        }),
        "expected unspecialized generic template to be pruned: {:?}",
        mir.def_names
    );
}

#[test]
fn mir_lowers_complex_tuple_and_or_match_patterns() {
    let mir = lower_clean(
        r#"
        fn main(pair: (bool, bool)) -> i32 {
            match pair {
                (true, false) | (false, true) => 1,
                (true, true) => 2,
                (false, false) => 3,
            }
        }
        "#,
    );

    let main_fn = mir
        .functions
        .iter()
        .find(|function| function.argc == 1)
        .expect("expected main function");

    assert!(
        main_fn
            .basic_blocks
            .iter()
            .filter(|block| {
                matches!(
                    block.terminator.as_ref().map(|term| &term.kind),
                    Some(TerminatorKind::SwitchInt { .. })
                )
            })
            .count()
            >= 3,
        "expected multiple branch blocks for complex tuple/or match, got blocks: {:?}",
        main_fn.basic_blocks
    );
}

#[test]
fn mir_lowers_variant_aware_enum_payload_projection() {
    let mir = lower_clean(
        r#"
        enum Value {
            Int(i32),
            Flag(bool),
        }

        fn main(value: Value) -> i32 {
            match value {
                Value::Int(inner) => inner,
                Value::Flag(flag) => if flag { 1 } else { 0 },
            }
        }
        "#,
    );

    let main_fn = mir
        .functions
        .iter()
        .find(|function| function.argc == 1)
        .expect("expected main function");

    assert!(
        main_fn
            .basic_blocks
            .iter()
            .flat_map(|block| &block.statements)
            .any(|stmt| {
                matches!(
                    &stmt.kind,
                    daram_compiler::mir::StatementKind::Assign(
                        _,
                        Rvalue::Read(place)
                    ) if place.projections.iter().any(|projection| matches!(
                        projection,
                        Projection::VariantField { .. }
                    ))
                )
            }),
        "expected variant-aware enum payload projection in MIR, got blocks: {:?}",
        main_fn.basic_blocks
    );
}

#[test]
fn mir_lowers_field_chains_over_rvalue_struct_results() {
    let mir = lower_codegen_clean(
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

    let main_fn = mir
        .functions
        .iter()
        .find(|function| {
            mir.def_names
                .get(&function.def)
                .is_some_and(|name| name == "main")
        })
        .expect("expected main function");

    assert!(
        main_fn
            .basic_blocks
            .iter()
            .flat_map(|block| &block.statements)
            .any(|stmt| {
                matches!(
                    &stmt.kind,
                    StatementKind::Assign(_, Rvalue::Read(place))
                        if matches!(
                            place.projections.as_slice(),
                            [Projection::Field(_), Projection::Field(_)]
                        )
                )
            }),
        "expected nested field chain to lower as projected place, got blocks: {:?}",
        main_fn.basic_blocks
    );
}
