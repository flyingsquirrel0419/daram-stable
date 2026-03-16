use daram_compiler::{
    analyze_to_codegen_mir,
    hir::{DefId, IntSize, Ty},
    interpreter::{self, Value},
    lexer::lex_with_errors,
    lower_to_codegen_mir,
    mir::{self, MirConst, MirFn, MirModule, Operand, Place, Rvalue, Terminator, TerminatorKind},
    name_resolution::resolve,
    parser::parse,
    source::{FileId, Span},
    type_checker,
};
use std::collections::HashMap;
use std::io::{Read, Write};
use std::net::TcpListener;
use std::thread;
use std::time::{SystemTime, UNIX_EPOCH};

fn execute_clean(source: &str, entry: &str) -> Value {
    execute_clean_with_args(source, entry, &[])
}

fn execute_clean_with_args(source: &str, entry: &str, args: &[Value]) -> Value {
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

    interpreter::execute_function(&mir_module, &mir_module.def_names, entry, args)
        .expect("expected clean interpreter execution")
}

fn execute_clean_with_prelude(source: &str, entry: &str) -> Value {
    let bundled = daram_compiler::stdlib_bundle::with_bundled_prelude(source);
    let analyzed = analyze_to_codegen_mir(&bundled, "bundled-prelude.dr");
    assert!(
        analyzed.diagnostics.is_empty(),
        "frontend diagnostics: {:?}",
        analyzed.diagnostics
    );
    let mir_module = analyzed.mir.expect("expected bundled MIR");
    interpreter::execute_function(&mir_module, &mir_module.def_names, entry, &[])
        .expect("expected clean interpreter execution")
}

fn execute_clean_with_prelude_error(source: &str, entry: &str) -> interpreter::RuntimeError {
    let bundled = daram_compiler::stdlib_bundle::with_bundled_prelude(source);
    let analyzed = analyze_to_codegen_mir(&bundled, "bundled-prelude.dr");
    assert!(
        analyzed.diagnostics.is_empty(),
        "frontend diagnostics: {:?}",
        analyzed.diagnostics
    );
    let mir_module = analyzed.mir.expect("expected bundled MIR");
    interpreter::execute_function(&mir_module, &mir_module.def_names, entry, &[])
        .expect_err("expected interpreter execution to fail")
}

fn escape_daram_string_literal(value: &str) -> String {
    value.replace('\\', "\\\\").replace('"', "\\\"")
}

fn unique_temp_file_path(prefix: &str) -> std::path::PathBuf {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system time should be after unix epoch")
        .as_nanos();
    std::env::temp_dir().join(format!("daram-{prefix}-{}-{nanos}.txt", std::process::id()))
}

fn spawn_http_server_once(
    response_body: &str,
    content_type: &str,
) -> (String, thread::JoinHandle<()>) {
    let listener = TcpListener::bind("127.0.0.1:0").expect("expected local listener");
    let addr = listener.local_addr().expect("expected local addr");
    let body = response_body.to_string();
    let content_type = content_type.to_string();
    let handle = thread::spawn(move || {
        let (mut stream, _) = listener.accept().expect("expected client");
        let mut request = [0u8; 4096];
        let _ = stream.read(&mut request);
        let response = format!(
            "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nContent-Type: {}\r\nConnection: close\r\n\r\n{}",
            body.len(),
            content_type,
            body
        );
        stream
            .write_all(response.as_bytes())
            .expect("expected response write");
        stream.flush().expect("expected flush");
    });
    (format!("http://{addr}/"), handle)
}

#[test]
fn interpreter_executes_main_and_returns_int() {
    let value = execute_clean(
        r#"
        fn main() -> i32 {
            7
        }
        "#,
        "main",
    );

    assert!(matches!(value, Value::Int(7)));
}

#[test]
fn interpreter_supports_multiple_generic_function_instantiations() {
    let value = execute_clean(
        r#"
        fun id<T>(value: T): T { value }

        fun main(): i32 {
            const number = id(7);
            const flag = id(true);
            if flag { number } else { 0 }
        }
        "#,
        "main",
    );

    assert!(matches!(value, Value::Int(7)));
}

#[test]
fn interpreter_executes_numeric_casts_at_runtime() {
    let value = execute_clean(
        r#"
        fun main(): usize {
            7 as usize
        }
        "#,
        "main",
    );

    assert!(matches!(value, Value::Uint(7)));
}

#[test]
fn interpreter_supports_builtin_asserts() {
    let value = execute_clean(
        r#"
        fn main() {
            assert(true);
            assert_eq(1, 1);
        }
        "#,
        "main",
    );

    assert!(matches!(value, Value::Unit));
}

#[test]
fn interpreter_executes_with_bundled_stdlib_prelude() {
    let value = execute_clean_with_prelude(
        r#"
        fun main(): i32 {
            std::io::println("from bundled prelude");
            42
        }
        "#,
        "main",
    );

    assert!(matches!(value, Value::Int(42)));
}

#[test]
fn interpreter_executes_bundled_json_subset() {
    let value = execute_clean_with_prelude(
        r#"
        fun main(): bool {
            const value = std::json::bool_val(true);
            match value {
                std::json::Value::Bool(flag) => flag,
                _ => false,
            }
        }
        "#,
        "main",
    );

    assert!(matches!(value, Value::Bool(true)), "got {value:?}");
}

#[test]
fn interpreter_propagates_result_try_success_and_failure() {
    let source = r#"
        fun ok_value(): std::core::Result<i32, i32> {
            std::core::Result::Ok(41)
        }

        fun err_value(): std::core::Result<i32, i32> {
            std::core::Result::Err(9)
        }

        fun success(): std::core::Result<i32, i32> {
            const value = ok_value()?;
            std::core::Result::Ok(value + 1)
        }

        fun failure(): std::core::Result<i32, i32> {
            const value = err_value()?;
            std::core::Result::Ok(value + 1)
        }
    "#;

    let success = execute_clean_with_prelude(source, "success");
    assert!(matches!(
        success,
        Value::Enum {
            variant_idx: 0,
            ref fields,
            ..
        } if matches!(fields.as_slice(), [Value::Int(42)])
    ));

    let failure = execute_clean_with_prelude(source, "failure");
    assert!(matches!(
        failure,
        Value::Enum {
            variant_idx: 1,
            ref fields,
            ..
        } if matches!(fields.as_slice(), [Value::Int(9)])
    ));
}

#[test]
fn interpreter_propagates_option_try_success_and_failure() {
    let source = r#"
        fun some_value(): std::core::Option<i32> {
            std::core::Option::Some(7)
        }

        fun none_value(): std::core::Option<i32> {
            std::core::Option::None
        }

        fun success(): std::core::Option<i32> {
            const value = some_value()?;
            std::core::Option::Some(value + 1)
        }

        fun failure(): std::core::Option<i32> {
            const value = none_value()?;
            std::core::Option::Some(value + 1)
        }
    "#;

    let success = execute_clean_with_prelude(source, "success");
    assert!(matches!(
        success,
        Value::Enum {
            variant_idx: 0,
            ref fields,
            ..
        } if matches!(fields.as_slice(), [Value::Int(8)])
    ));

    let failure = execute_clean_with_prelude(source, "failure");
    assert!(matches!(
        failure,
        Value::Enum {
            variant_idx: 1,
            ref fields,
            ..
        } if fields.is_empty()
    ));
}

#[test]
fn interpreter_executes_bundled_vec_push_and_len() {
    let value = execute_clean_with_prelude(
        r#"
        fun main(): usize {
            const values: std::collections::Vec<i32> = std::collections::Vec::new();
            values.push(10);
            values.push(20);
            values.push(30);
            values.len()
        }
        "#,
        "main",
    );

    assert!(matches!(value, Value::Uint(3)));
}

#[test]
fn interpreter_executes_bundled_vec_get_with_option_ref() {
    let value = execute_clean_with_prelude(
        r#"
        fun main(): i32 {
            const values: std::collections::Vec<i32> = std::collections::Vec::new();
            values.push(42);
            match values.get(0) {
                std::core::Option::Some(value) => *value,
                std::core::Option::None => -1,
            }
        }
        "#,
        "main",
    );

    assert!(matches!(value, Value::Int(42)));
}

#[test]
fn interpreter_executes_for_loop_over_bundled_vec() {
    let value = execute_clean_with_prelude(
        r#"
        fun main(): i32 {
            const values: std::collections::Vec<i32> = std::collections::Vec::new();
            values.push(1);
            values.push(2);
            values.push(3);
            let total: i32 = 0;
            for item in values {
                total += *item;
            }
            total
        }
        "#,
        "main",
    );

    assert!(matches!(value, Value::Int(6)), "got {value:?}");
}

#[test]
fn interpreter_executes_bundled_vec_iter_next_sequence() {
    let value = execute_clean_with_prelude(
        r#"
        fun main(): i32 {
            const values: std::collections::Vec<i32> = std::collections::Vec::new();
            values.push(4);
            values.push(7);
            const iter = values.iter();
            const first = match iter.next() {
                std::core::Option::Some(value) => *value,
                std::core::Option::None => -100,
            };
            const second = match iter.next() {
                std::core::Option::Some(value) => *value,
                std::core::Option::None => -200,
            };
            const third = match iter.next() {
                std::core::Option::Some(value) => *value,
                std::core::Option::None => 0,
            };
            first + second + third
        }
        "#,
        "main",
    );

    assert!(matches!(value, Value::Int(11)), "got {value:?}");
}

#[test]
fn interpreter_executes_bundled_vec_iter_count() {
    let value = execute_clean_with_prelude(
        r#"
        fun main(): usize {
            const values: std::collections::Vec<i32> = std::collections::Vec::new();
            values.push(2);
            values.push(4);
            values.push(6);
            values.iter().count()
        }
        "#,
        "main",
    );

    assert!(matches!(value, Value::Uint(3)), "got {value:?}");
}

#[test]
fn interpreter_executes_bundled_string_push_len_and_contains() {
    let value = execute_clean_with_prelude(
        r#"
        fun main(): usize {
            const text: std::core::String = std::core::String::new();
            text.push_str("da");
            text.push('r');
            text.push_str("am");
            if text.contains("ram") {
                text.len()
            } else {
                0
            }
        }
        "#,
        "main",
    );

    assert!(matches!(value, Value::Uint(5)));
}

#[test]
fn interpreter_executes_bundled_hashmap_integer_keys() {
    let value = execute_clean_with_prelude(
        r#"
        fun main(): bool {
            const map: std::collections::HashMap<i32, i32> =
                std::collections::HashMap::new();
            std::collections::HashMap::insert(map, 1, 7);
            std::collections::HashMap::insert(map, 2, 9);
            const has_alpha = std::collections::HashMap::contains_key(map, 1);
            const removed = match std::collections::HashMap::remove(map, 2) {
                std::core::Option::Some(value) => value,
                std::core::Option::None => 0,
            };
            match std::collections::HashMap::get(map, 1) {
                std::core::Option::Some(value) =>
                    has_alpha && *value == 7 && removed == 9,
                std::core::Option::None => false,
            }
        }
        "#,
        "main",
    );

    assert!(matches!(value, Value::Bool(true)), "got {value:?}");
}

#[test]
fn interpreter_executes_bundled_hashmap_len() {
    let value = execute_clean_with_prelude(
        r#"
        fun main(): usize {
            const map: std::collections::HashMap<i32, i32> =
                std::collections::HashMap::new();
            std::collections::HashMap::insert(map, 1, 7);
            std::collections::HashMap::insert(map, 2, 9);
            std::collections::HashMap::len(map)
        }
        "#,
        "main",
    );

    assert!(matches!(value, Value::Uint(2)), "got {value:?}");
}

#[test]
fn interpreter_executes_for_loop_over_bundled_hashmap() {
    let value = execute_clean_with_prelude(
        r#"
        fun main(): i32 {
            const map: std::collections::HashMap<i32, i32> =
                std::collections::HashMap::new();
            std::collections::HashMap::insert(map, 1, 10);
            std::collections::HashMap::insert(map, 2, 20);
            let total: i32 = 0;
            for entry in map {
                total += *entry.0 + *entry.1;
            }
            total
        }
        "#,
        "main",
    );

    assert!(matches!(value, Value::Int(33)), "got {value:?}");
}

#[test]
fn interpreter_executes_bundled_hashmap_iter_next_sequence() {
    let value = execute_clean_with_prelude(
        r#"
        fun main(): bool {
            const map: std::collections::HashMap<i32, i32> =
                std::collections::HashMap::new();
            std::collections::HashMap::insert(map, 1, 10);
            std::collections::HashMap::insert(map, 2, 20);
            const iter = map.iter();
            const first = match iter.next() {
                std::core::Option::Some(entry) => *entry.0 + *entry.1,
                std::core::Option::None => -1,
            };
            const second = match iter.next() {
                std::core::Option::Some(entry) => *entry.0 + *entry.1,
                std::core::Option::None => -1,
            };
            const third = match iter.next() {
                std::core::Option::Some(_) => false,
                std::core::Option::None => true,
            };
            first != second && (first + second) == 33 && third
        }
        "#,
        "main",
    );

    assert!(matches!(value, Value::Bool(true)), "got {value:?}");
}

#[test]
fn interpreter_executes_bundled_hashmap_iter_count() {
    let value = execute_clean_with_prelude(
        r#"
        fun main(): usize {
            const map: std::collections::HashMap<i32, i32> =
                std::collections::HashMap::new();
            std::collections::HashMap::insert(map, 1, 10);
            std::collections::HashMap::insert(map, 2, 20);
            map.iter().count()
        }
        "#,
        "main",
    );

    assert!(matches!(value, Value::Uint(2)), "got {value:?}");
}

#[test]
fn interpreter_executes_bundled_vec_iter_map_collect() {
    let value = execute_clean_with_prelude(
        r#"
        fun main(): bool {
            const values: std::collections::Vec<i32> = std::collections::Vec::new();
            values.push(1);
            values.push(2);
            values.push(3);
            const doubled = values.iter().map(fun(value: &i32): i32 { *value * 2 }).collect_vec();
            std::fmt::format("{}", doubled) == "[2, 4, 6]"
        }
        "#,
        "main",
    );

    assert!(matches!(value, Value::Bool(true)), "got {value:?}");
}

#[test]
fn interpreter_executes_bundled_vec_iter_filter_map_collect() {
    let value = execute_clean_with_prelude(
        r#"
        fun main(): bool {
            const values: std::collections::Vec<i32> = std::collections::Vec::new();
            values.push(1);
            values.push(2);
            values.push(3);
            values.push(4);
            const evens = values
                .iter()
                .filter(fun(value: &i32): bool { *value % 2 == 0 })
                .map(fun(value: &i32): i32 { *value })
                .collect_vec();
            std::fmt::format("{}", evens) == "[2, 4]"
        }
        "#,
        "main",
    );

    assert!(matches!(value, Value::Bool(true)), "got {value:?}");
}

#[test]
fn interpreter_executes_self_receiver_method_calls_inside_impl_bodies() {
    let value = execute_clean(
        r#"
        struct Counter {
            value: i32,
        }

        extend Counter {
            fun bump(self): i32 {
                self.value
            }

            fun read(self): i32 {
                self.bump()
            }
        }

        fun main(): i32 {
            const counter = Counter { value: 1 };
            counter.read()
        }
        "#,
        "main",
    );

    assert!(matches!(value, Value::Int(1)), "got {value:?}");
}

#[test]
fn interpreter_executes_synthesized_ability_default_methods() {
    let value = execute_clean(
        r#"
        ability Meter {
            fun value(self): i32;
            fun plus_one(self): i32 {
                self.value() + 1
            }
        }

        struct Reader {
            n: i32,
        }

        struct Counter {
            n: i32,
        }

        extend Reader implements Meter {
            fun value(self): i32 {
                self.n
            }
        }

        extend Counter implements Meter {
            fun value(self): i32 {
                self.n + 10
            }
        }

        fun main(): i32 {
            const reader = Reader { n: 4 };
            const counter = Counter { n: 5 };
            reader.plus_one() + counter.plus_one()
        }
        "#,
        "main",
    );

    assert!(matches!(value, Value::Int(21)), "got {value:?}");
}

#[test]
fn interpreter_executes_bundled_fs_read_write_and_exists() {
    let path = unique_temp_file_path("fs");
    let escaped = escape_daram_string_literal(path.to_string_lossy().as_ref());
    let source = format!(
        r#"
        fun main(): bool {{
            const path = std::fs::PathBuf::from("{escaped}");
            match std::fs::write_str(&path, "hello from daram") {{
                std::core::Result::Ok(()) => match std::fs::read_to_string(&path) {{
                    std::core::Result::Ok(content) =>
                        std::fs::PathBuf::exists(path) && content.contains("daram"),
                    std::core::Result::Err(_) => false,
                }},
                std::core::Result::Err(_) => false,
            }}
        }}
        "#
    );

    let value = execute_clean_with_prelude(&source, "main");
    assert!(matches!(value, Value::Bool(true)));

    let _ = std::fs::remove_file(path);
}

#[test]
fn interpreter_executes_bundled_json_parse_and_stringify() {
    let value = execute_clean_with_prelude(
        r#"
        fun parse_flag(): bool {
            match std::json::parse("true") {
                std::core::Result::Ok(value) => match value {
                    std::json::Value::Bool(flag) => flag,
                    _ => false,
                },
                std::core::Result::Err(_) => false,
            }
        }

        fun stringify_flag(): bool {
            const flag = std::json::bool_val(true);
            const rendered = std::json::to_string(&flag);
            rendered.contains("true")
        }

        fun main(): bool {
            parse_flag() && stringify_flag()
        }
        "#,
        "main",
    );

    assert!(matches!(value, Value::Bool(true)), "got {value:?}");
}

#[test]
fn interpreter_executes_bundled_json_object_get_and_index() {
    let value = execute_clean_with_prelude(
        r#"
        fun object_flag(): bool {
            match std::json::parse("{\"ok\":true}") {
                std::core::Result::Ok(value) => match value.get("ok") {
                    std::core::Option::Some(flag) => match *flag {
                        std::json::Value::Bool(inner) => inner,
                        _ => false,
                    },
                    std::core::Option::None => false,
                },
                std::core::Result::Err(_) => false,
            }
        }

        fun second_item(): bool {
            match std::json::parse("[1,2,3]") {
                std::core::Result::Ok(value) => match value.index(1) {
                    std::core::Option::Some(item) => match *item {
                        std::json::Value::Number(inner) => inner == 2.0,
                        _ => false,
                    },
                    std::core::Option::None => false,
                },
                std::core::Result::Err(_) => false,
            }
        }

        fun main(): bool {
            object_flag() && second_item()
        }
        "#,
        "main",
    );

    assert!(matches!(value, Value::Bool(true)), "got {value:?}");
}

#[test]
fn interpreter_formats_placeholder_strings_with_bundled_fmt_builtin() {
    let value = execute_clean_with_prelude(
        r#"
        fun main(): bool {
            const rendered = std::fmt::format("{} + {} = {}", 2, 3, 5);
            rendered == "2 + 3 = 5"
        }
        "#,
        "main",
    );

    assert!(matches!(value, Value::Bool(true)));
}

#[test]
fn interpreter_formats_user_types_with_names() {
    let value = execute_clean_with_prelude(
        r#"
        struct Point {
            x: i32,
            y: i32,
        }

        enum Status {
            Ready,
            Value(i32),
        }

        fun main(): bool {
            const point = Point { x: 3, y: 4 };
            const status = Status::Value(7);
            const rendered = std::fmt::format("{} {}", point, status);
            rendered == "Point { x: 3, y: 4 } Status::Value(7)"
        }
        "#,
        "main",
    );

    assert!(matches!(value, Value::Bool(true)), "got {value:?}");
}

#[test]
fn interpreter_dispatches_user_display_impls() {
    let value = execute_clean_with_prelude(
        r#"
        import { Display, Result } from "std/core";

        struct Point {
            x: i32,
            y: i32,
        }

        extend Point implements Display {
            fun fmt(self, f: &mut std::fmt::Formatter): Result<(), std::fmt::Error> {
                (*f).write_str("point(");
                (*f).write_str(std::fmt::format("{}", self.x));
                (*f).write_str(", ");
                (*f).write_str(std::fmt::format("{}", self.y));
                (*f).write_str(")");
                std::core::Result::Ok(())
            }
        }

        fun main(): bool {
            const point = Point { x: 3, y: 4 };
            std::fmt::format("{}", point) == "point(3, 4)"
        }
        "#,
        "main",
    );

    assert!(matches!(value, Value::Bool(true)), "got {value:?}");
}

#[test]
fn interpreter_supports_hashmap_keys_for_derived_structs() {
    let value = execute_clean_with_prelude(
        r#"
        @derive(Hash, Eq, PartialEq)
        struct Point {
            x: i32,
            y: i32,
        }

        fun main(): bool {
            const points: std::collections::HashMap<Point, i32> =
                std::collections::HashMap::new();
            points.insert(Point { x: 3, y: 4 }, 7);

            match points.get(Point { x: 3, y: 4 }) {
                std::core::Option::Some(value) => *value == 7,
                std::core::Option::None => false,
            }
        }
        "#,
        "main",
    );

    assert!(matches!(value, Value::Bool(true)), "got {value:?}");
}

#[test]
fn interpreter_supports_hashmap_keys_for_derived_enums() {
    let value = execute_clean_with_prelude(
        r#"
        @derive(Hash, Eq, PartialEq)
        enum Axis {
            X,
            Y,
        }

        fun main(): bool {
            const axes: std::collections::HashMap<Axis, i32> =
                std::collections::HashMap::new();
            axes.insert(Axis::Y, 9);

            match axes.get(Axis::Y) {
                std::core::Option::Some(axis) => *axis == 9,
                std::core::Option::None => false,
            }
        }
        "#,
        "main",
    );

    assert!(matches!(value, Value::Bool(true)), "got {value:?}");
}

#[test]
fn interpreter_supports_socket_addr_parse_and_tcp_connect() {
    let listener = TcpListener::bind("127.0.0.1:0").expect("expected local listener");
    let addr = listener.local_addr().expect("expected listener addr");
    let handle = thread::spawn(move || {
        let _ = listener.accept().expect("expected tcp client");
    });
    let source = format!(
        r#"
        fun main(): bool {{
            const cap = std::net::NetCap {{ _private: () }};
            match std::net::SocketAddr::from_str("127.0.0.1:{port}") {{
                std::core::Result::Ok(addr) => {{
                    std::net::TcpStream::connect(addr, cap);
                    true
                }}
                std::core::Result::Err(_) => false,
            }}
        }}
        "#,
        port = addr.port()
    );
    let value = execute_clean_with_prelude(&source, "main");
    handle.join().expect("expected server thread");
    assert!(matches!(value, Value::Bool(true)), "got {value:?}");
}

#[test]
fn interpreter_supports_http_client_get_and_body_helpers() {
    let (url, handle) = spawn_http_server_once("{\"ok\":true}", "application/json");
    let (json_url, json_handle) = spawn_http_server_once("{\"ok\":true}", "application/json");
    let source = format!(
        r#"
        fun main(): bool {{
            const cap = std::net::NetCap {{ _private: () }};
            const client = std::http::Client::new(cap);
            match client.get("{url}") {{
                std::core::Result::Ok(response) => {{
                    const body_ok = match response.body_str() {{
                        std::core::Result::Ok(body) => body == "{{\"ok\":true}}",
                        std::core::Result::Err(_) => false,
                    }};
                    const json_client = std::http::Client::new(cap);
                    const json_ok = match json_client.get("{json_url}") {{
                        std::core::Result::Ok(json_response) => match json_response.body_json() {{
                            std::core::Result::Ok(value) => match value.get("ok") {{
                                std::core::Option::Some(_) => true,
                                std::core::Option::None => false,
                            }},
                            std::core::Result::Err(_) => false,
                        }},
                        std::core::Result::Err(_) => false,
                    }};
                    body_ok && json_ok
                }}
                std::core::Result::Err(_) => false,
            }}
        }}
        "#,
        url = url,
        json_url = json_url
    );
    let value = execute_clean_with_prelude(&source, "main");
    handle.join().expect("expected server thread");
    json_handle.join().expect("expected server thread");
    assert!(matches!(value, Value::Bool(true)), "got {value:?}");
}

#[test]
fn interpreter_supports_async_task_runtime() {
    let value = execute_clean_with_prelude(
        r#"
        async fun add_one(value: i32): i32 {
            value + 1
        }

        fun main(): i32 {
            const handle = std::task::spawn(fun(): i32 {
                add_one(41) await
            });
            std::task::sleep_ms(0 as u64);
            std::task::block_on(fun(): i32 {
                handle.join()
            })
        }
        "#,
        "main",
    );

    assert!(matches!(value, Value::Int(42)), "got {value:?}");
}

#[test]
fn interpreter_executes_synthesized_clone_method() {
    let value = execute_clean_with_prelude(
        r#"
        @derive(Clone)
        struct Point {
            x: i32,
            y: i32,
        }

        fun main(): bool {
            const cloned = Point { x: 3, y: 4 }.clone();
            cloned.x == 3 && cloned.y == 4
        }
        "#,
        "main",
    );

    assert!(matches!(value, Value::Bool(true)), "got {value:?}");
}

#[test]
fn interpreter_executes_synthesized_default_method() {
    let value = execute_clean_with_prelude(
        r#"
        @derive(Default)
        struct Point {
            x: i32,
            y: i32,
        }

        fun main(): bool {
            const point = Point::default();
            point.x == 0 && point.y == 0
        }
        "#,
        "main",
    );

    assert!(matches!(value, Value::Bool(true)), "got {value:?}");
}

#[test]
fn interpreter_executes_synthesized_debug_method() {
    let value = execute_clean_with_prelude(
        r#"
        @derive(Debug)
        struct Point {
            x: i32,
            y: i32,
        }

        fun main(): bool {
            let formatter = std::fmt::Formatter::new();
            Point { x: 0, y: 0 }.fmt(&mut formatter);
            formatter.finish() == "Point { x: 0, y: 0 }"
        }
        "#,
        "main",
    );

    assert!(matches!(value, Value::Bool(true)), "got {value:?}");
}

#[test]
fn codegen_mir_keeps_synthesized_debug_support_functions() {
    let analyzed = analyze_to_codegen_mir(
        &daram_compiler::stdlib_bundle::with_bundled_prelude(
            r#"
            @derive(Debug)
            struct Point {
                x: i32,
                y: i32,
            }

            fun main(): bool {
                let formatter = std::fmt::Formatter::new();
                Point { x: 0, y: 0 }.fmt(&mut formatter);
                formatter.finish() == "Point { x: 0, y: 0 }"
            }
            "#,
        ),
        "bundled-prelude.dr",
    );
    assert!(
        analyzed.diagnostics.is_empty(),
        "frontend diagnostics: {:?}",
        analyzed.diagnostics
    );
    let mir = analyzed.mir.expect("expected MIR");

    let function_names = mir
        .functions
        .iter()
        .filter_map(|function| mir.def_names.get(&function.def).cloned())
        .collect::<Vec<_>>();
    let function_defs = mir
        .functions
        .iter()
        .map(|function| function.def)
        .collect::<std::collections::HashSet<_>>();
    let builtin_names = [
        "print",
        "println",
        "eprint",
        "eprintln",
        "assert",
        "assert_eq",
        "panic_with_fmt",
        "format",
        "panic",
        "__builtin_print",
        "__builtin_println",
        "__builtin_eprint",
        "__builtin_eprintln",
        "__builtin_assert",
        "__builtin_assert_eq",
        "__builtin_panic_with_fmt",
        "__builtin_format",
        "__builtin_panic",
        "std::io::print",
        "std::io::println",
        "std::io::eprint",
        "std::io::eprintln",
        "std::test::assert",
        "std::test::assert_eq",
        "std::test::panic_with_fmt",
        "std::fmt::format",
        "std::core::panic",
        "std::collections::HashMap::new",
        "std::collections::HashMap::insert",
    ];
    let mut missing_callees = Vec::new();
    for function in &mir.functions {
        for block in &function.basic_blocks {
            let Some(Terminator {
                kind: TerminatorKind::Call { callee, .. },
                ..
            }) = &block.terminator
            else {
                continue;
            };
            let Operand::Def(def) = callee else {
                continue;
            };
            let Some(name) = mir.def_names.get(def) else {
                continue;
            };
            if !function_defs.contains(def)
                && !name.starts_with("__builtin_")
                && !builtin_names.contains(&name.as_str())
            {
                missing_callees.push(name.clone());
            }
        }
    }
    missing_callees.sort();
    missing_callees.dedup();

    assert!(
        function_names
            .iter()
            .any(|name| name.ends_with("Point::fmt")),
        "expected derived debug MIR function, got: {function_names:?}"
    );
    assert!(
        function_names
            .iter()
            .any(|name| name == "std::fmt::Formatter::write_str"),
        "expected formatter write_str MIR function, got: {function_names:?}"
    );
    assert!(
        function_names
            .iter()
            .any(|name| name == "std::fmt::Formatter::new"),
        "expected formatter new MIR function, got: {function_names:?}"
    );
    assert!(
        function_names
            .iter()
            .any(|name| name == "std::fmt::Formatter::finish"),
        "expected formatter finish MIR function, got: {function_names:?}"
    );
    assert!(
        function_names
            .iter()
            .any(|name| name == "std::core::String::as_str"),
        "expected string as_str MIR function, got: {function_names:?}"
    );
    assert!(
        missing_callees.is_empty(),
        "expected all direct call callees to be lowered, missing: {missing_callees:?}"
    );
}

#[test]
fn interpreter_supports_string_runtime_helpers() {
    let value = execute_clean_with_prelude(
        r#"
        fun main(): bool {
            const text = std::core::String::new();
            text.push_str("red,blue,green");

            const pieces = text.split(",");
            const first_ok = match pieces.next() {
                std::core::Option::Some(value) => value.as_str() == "red",
                std::core::Option::None => false,
            };
            const second_ok = match pieces.next() {
                std::core::Option::Some(value) => value.as_str() == "blue",
                std::core::Option::None => false,
            };
            const spaced = std::core::String::new();
            spaced.push_str("  red,blue,green  ");
            const repeated = text.repeat(2);
            const replaced = repeated.replace("blue", "gold");

            first_ok
                && second_ok
                && spaced.trim() == "red,blue,green"
                && replaced.as_str() == "red,gold,greenred,gold,green"
        }
        "#,
        "main",
    );

    assert!(matches!(value, Value::Bool(true)), "got {value:?}");
}

#[test]
fn interpreter_supports_formatted_std_io_println_calls() {
    let value = execute_clean_with_prelude(
        r#"
        fun main(): i32 {
            std::io::println("{} is {}", "daram", 2);
            0
        }
        "#,
        "main",
    );

    assert!(matches!(value, Value::Int(0)));
}

#[test]
fn interpreter_runs_errdefer_on_try_failure() {
    let error = execute_clean_with_prelude_error(
        r#"
        fun cleanup(): ! {
            std::core::panic("cleanup")
        }

        fun fail(): std::core::Result<i32, i32> {
            std::core::Result::Err(1)
        }

        fun main(): std::core::Result<i32, i32> {
            errdefer {
                cleanup();
            }
            const value = fail()?;
            std::core::Result::Ok(value)
        }
        "#,
        "main",
    );

    assert!(error.message.contains("cleanup"));
}

#[test]
fn interpreter_supports_std_io_println_alias() {
    let value = execute_clean(
        r#"
        fn main() {
            std::io::println("hello");
        }
        "#,
        "main",
    );

    assert!(matches!(value, Value::Unit));
}

#[test]
fn interpreter_supports_std_io_eprintln_alias() {
    let value = execute_clean(
        r#"
        fn main() {
            std::io::eprintln("hello", 1);
        }
        "#,
        "main",
    );

    assert!(matches!(value, Value::Unit));
}

#[test]
fn interpreter_executes_zero_capture_closures() {
    let value = execute_clean(
        r#"
        fn main() -> i32 {
            let add_one = fn(value: i32) -> i32 {
                value + 1
            };
            add_one(6)
        }
        "#,
        "main",
    );

    assert!(matches!(value, Value::Int(7)));
}

#[test]
fn interpreter_executes_capture_closures() {
    let value = execute_clean(
        r#"
        fn main() -> i32 {
            let base = 40;
            let add = fn(value: i32) -> i32 {
                base + value
            };
            add(2)
        }
        "#,
        "main",
    );

    assert!(matches!(value, Value::Int(42)));
}

#[test]
fn interpreter_executes_enum_variant_values_and_match() {
    let value = execute_clean(
        r#"
        enum Answer {
            No,
            Yes(i32),
        }

        fn main() -> i32 {
            let answer = Yes(42);
            match answer {
                No => 0,
                Yes(value) => value,
            }
        }
        "#,
        "main",
    );

    assert!(matches!(value, Value::Int(42)));
}

#[test]
fn interpreter_executes_guarded_tuple_or_match() {
    let value = execute_clean_with_args(
        r#"
        fn main(pair: (bool, bool)) -> i32 {
            match pair {
                (true, value) if value => 7,
                (true, false) | (false, true) => 3,
                (false, false) => 1,
            }
        }
        "#,
        "main",
        &[Value::Tuple(vec![Value::Bool(true), Value::Bool(true)])],
    );

    assert!(matches!(value, Value::Int(7)));
}

#[test]
fn interpreter_executes_while_let_loops() {
    let value = execute_clean_with_prelude(
        r#"
        fun main(): i32 {
            let current = std::core::Option::Some(3);
            let total = 0;
            while let std::core::Option::Some(value) = current {
                total += value;
                current = std::core::Option::None;
            }
            total
        }
        "#,
        "main",
    );

    assert!(matches!(value, Value::Int(3)), "got {value:?}");
}

#[test]
fn interpreter_runs_defer_on_return() {
    let (tokens, lex_errors) = lex_with_errors(
        r#"
        fn fail() {
            panic("boom");
        }

        fn main() -> i32 {
            defer {
                fail();
            }
            0
        }
        "#,
    );
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

    let (mir_module, mir_errors) = mir::lower(&hir);
    assert!(mir_errors.is_empty(), "mir errors: {:?}", mir_errors);

    let error = interpreter::execute_function(&mir_module, &hir.def_names, "main", &[])
        .expect_err("expected deferred panic to surface");
    assert!(error.message.contains("boom"));
}

#[test]
fn interpreter_runs_errdefer_on_call_failure() {
    let (tokens, lex_errors) = lex_with_errors(
        r#"
        fn fail() {
            panic("boom");
        }

        fn cleanup() {
            panic("cleanup");
        }

        fn main() {
            errdefer {
                cleanup();
            }
            fail();
        }
        "#,
    );
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

    let (mir_module, mir_errors) = mir::lower(&hir);
    assert!(mir_errors.is_empty(), "mir errors: {:?}", mir_errors);

    let error = interpreter::execute_function(&mir_module, &hir.def_names, "main", &[])
        .expect_err("expected errdefer cleanup to surface");
    assert!(error.message.contains("cleanup"));
}

#[test]
fn interpreter_runs_errdefer_on_builtin_assert_failure() {
    let (tokens, lex_errors) = lex_with_errors(
        r#"
        fn cleanup() {
            panic("cleanup");
        }

        fn main() {
            errdefer {
                cleanup();
            }
            assert(false);
        }
        "#,
    );
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

    let (mir_module, mir_errors) = mir::lower(&hir);
    assert!(mir_errors.is_empty(), "mir errors: {:?}", mir_errors);

    let error = interpreter::execute_function(&mir_module, &hir.def_names, "main", &[])
        .expect_err("expected errdefer cleanup to surface");
    assert!(error.message.contains("cleanup"));
}

#[test]
fn interpreter_supports_std_test_panic_with_fmt_builtin() {
    let (tokens, lex_errors) = lex_with_errors(
        r#"
        fn main() {
            std::test::panic_with_fmt("assertion failed", 1, 2);
        }
        "#,
    );
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

    let (mir_module, mir_errors) = mir::lower(&hir);
    assert!(mir_errors.is_empty(), "mir errors: {:?}", mir_errors);

    let error = interpreter::execute_function(&mir_module, &hir.def_names, "main", &[])
        .expect_err("expected panic_with_fmt to raise a runtime error");
    assert_eq!(error.kind, interpreter::RuntimeErrorKind::Panic);
    assert!(error.message.contains("assertion failed"));
    assert!(error.message.contains("left=`1`"));
    assert!(error.message.contains("right=`2`"));
}

#[test]
fn interpreter_short_circuits_boolean_operators() {
    let value = execute_clean(
        r#"
        fn fail() -> bool {
            if false {
                true
            } else {
                panic("rhs evaluated")
            }
        }

        fn main() -> bool {
            let a = true || fail();
            let b = false && fail();
            a && !b
        }
        "#,
        "main",
    );

    assert!(matches!(value, Value::Bool(true)));
}

#[test]
fn interpreter_enforces_execution_step_limit() {
    let (tokens, lex_errors) = lex_with_errors(
        r#"
        fn main() {
            loop {}
        }
        "#,
    );
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

    let (mir_module, mir_errors) = mir::lower(&hir);
    assert!(mir_errors.is_empty(), "mir errors: {:?}", mir_errors);

    let error = interpreter::execute_function_with_limits(
        &mir_module,
        &hir.def_names,
        "main",
        &[],
        interpreter::ExecutionLimits {
            max_steps: 32,
            max_call_depth: 64,
        },
    )
    .expect_err("expected step limit error");

    assert_eq!(error.kind, interpreter::RuntimeErrorKind::LimitExceeded);
    assert!(error.message.contains("step limit exceeded"));
}

#[test]
fn interpreter_enforces_call_depth_limit() {
    let (tokens, lex_errors) = lex_with_errors(
        r#"
        fn recur() -> i32 {
            recur()
        }

        fn main() -> i32 {
            recur()
        }
        "#,
    );
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

    let (mir_module, mir_errors) = mir::lower(&hir);
    assert!(mir_errors.is_empty(), "mir errors: {:?}", mir_errors);

    let error = interpreter::execute_function_with_limits(
        &mir_module,
        &hir.def_names,
        "main",
        &[],
        interpreter::ExecutionLimits {
            max_steps: 1_000,
            max_call_depth: 8,
        },
    )
    .expect_err("expected call depth error");

    assert_eq!(error.kind, interpreter::RuntimeErrorKind::LimitExceeded);
    assert!(error.message.contains("call depth limit exceeded"));
}

#[test]
fn interpreter_treats_drop_as_control_flow_only() {
    let def = DefId {
        file: FileId(0),
        index: 0,
    };
    let span = Span::new(FileId(0), 0, 0);
    let mut function = MirFn::new(def);
    let entry = function.fresh_block();
    let after_assert = function.fresh_block();
    let after_drop = function.fresh_block();
    let return_local = function.fresh_local(Ty::Unit, Some("return".into()), Some(span));
    let dropped_local =
        function.fresh_local(Ty::Int(IntSize::I32), Some("value".into()), Some(span));
    function.argc = 0;
    function.locals[return_local.0 as usize].mutable = true;

    function.block_mut(entry).terminator = Some(Terminator {
        kind: TerminatorKind::Assert {
            cond: Operand::Const(MirConst::Bool(true)),
            expected: true,
            msg: "assert failed",
            target: after_assert,
        },
        span: Some(span),
    });

    function
        .block_mut(after_assert)
        .statements
        .push(crate::mir::Statement {
            kind: crate::mir::StatementKind::Assign(
                Place::local(dropped_local),
                Rvalue::Use(Operand::Const(MirConst::Int(7))),
            ),
            span: Some(span),
        });
    function.block_mut(after_assert).terminator = Some(Terminator {
        kind: TerminatorKind::Drop {
            place: Place::local(dropped_local),
            target: after_drop,
        },
        span: Some(span),
    });

    function
        .block_mut(after_drop)
        .statements
        .push(crate::mir::Statement {
            kind: crate::mir::StatementKind::Assign(
                Place::local(return_local),
                Rvalue::Read(Place::local(dropped_local)),
            ),
            span: Some(span),
        });
    function.block_mut(after_drop).terminator = Some(Terminator {
        kind: TerminatorKind::Return,
        span: Some(span),
    });

    let mut def_names = HashMap::new();
    def_names.insert(def, "main".to_string());
    let mir_module = MirModule {
        consts: Vec::new(),
        functions: vec![function],
        enum_variant_indices: HashMap::new(),
        enum_variant_names: HashMap::new(),
        struct_field_names: HashMap::new(),
        display_impls: std::collections::HashSet::new(),
        def_names: def_names.clone(),
    };

    let value = interpreter::execute_function(&mir_module, &def_names, "main", &[])
        .expect("expected assert/drop execution to succeed");
    assert!(matches!(value, Value::Int(7)));
}

#[test]
fn interpreter_executes_default_parameter_wrappers() {
    let value = execute_clean(
        r#"
        fun connect(host: i32, port: i32 = host + 1, timeout: i32 = port + 1): i32 {
            timeout
        }

        fun main(): i32 {
            connect(40)
        }
        "#,
        "main",
    );

    assert!(matches!(value, Value::Int(42)));
}

#[test]
fn interpreter_executes_pathbuf_file_name_extension_parent() {
    let value = execute_clean_with_prelude(
        r#"
        fun main(): bool {
            const name_ok = match std::fs::PathBuf::from("/tmp/hello.txt").file_name() {
                std::core::Option::Some(name) => name == "hello.txt",
                std::core::Option::None => false,
            };
            const ext_ok = match std::fs::PathBuf::from("/tmp/hello.txt").extension() {
                std::core::Option::Some(ext) => ext == "txt",
                std::core::Option::None => false,
            };
            const parent_ok = match std::fs::PathBuf::from("/tmp/hello.txt").parent() {
                std::core::Option::Some(p) => p.as_str() == "/tmp",
                std::core::Option::None => false,
            };
            name_ok && ext_ok && parent_ok
        }
        "#,
        "main",
    );

    assert!(matches!(value, Value::Bool(true)), "got {value:?}");
}

#[test]
fn interpreter_executes_fs_create_dir_all_write_copy_remove() {
    let base_dir = {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("system time after epoch")
            .as_nanos();
        std::path::PathBuf::from(format!(
            "/tmp/daram-fs-test-{}-{}",
            std::process::id(),
            nanos
        ))
    };
    let base_escaped = escape_daram_string_literal(base_dir.to_string_lossy().as_ref());
    let file_path = base_dir.join("data.txt");
    let file_escaped = escape_daram_string_literal(file_path.to_string_lossy().as_ref());
    let copy_path = base_dir.join("copy.txt");
    let copy_escaped = escape_daram_string_literal(copy_path.to_string_lossy().as_ref());

    let source = format!(
        r#"
        fun check_create_dir(): bool {{
            const p = std::fs::PathBuf::from("{base_escaped}");
            match std::fs::create_dir_all(&p) {{
                std::core::Result::Ok(()) => true,
                std::core::Result::Err(_) => false,
            }}
        }}

        fun check_write(): bool {{
            const p = std::fs::PathBuf::from("{file_escaped}");
            match std::fs::write_str(&p, "daram-test") {{
                std::core::Result::Ok(()) => true,
                std::core::Result::Err(_) => false,
            }}
        }}

        fun check_file_exists(): bool {{
            std::fs::PathBuf::from("{file_escaped}").is_file()
        }}

        fun check_copy(): bool {{
            const src = std::fs::PathBuf::from("{file_escaped}");
            const dst = std::fs::PathBuf::from("{copy_escaped}");
            match std::fs::copy(&src, &dst) {{
                std::core::Result::Ok(_) => true,
                std::core::Result::Err(_) => false,
            }}
        }}

        fun check_copy_exists(): bool {{
            std::fs::PathBuf::from("{copy_escaped}").is_file()
        }}

        fun check_remove(): bool {{
            const p = std::fs::PathBuf::from("{file_escaped}");
            match std::fs::remove_file(&p) {{
                std::core::Result::Ok(()) => true,
                std::core::Result::Err(_) => false,
            }}
        }}

        fun check_removed(): bool {{
            !std::fs::PathBuf::from("{file_escaped}").is_file()
        }}

        fun main(): bool {{
            check_create_dir()
                && check_write()
                && check_file_exists()
                && check_copy()
                && check_copy_exists()
                && check_remove()
                && check_removed()
        }}
        "#
    );

    let value = execute_clean_with_prelude(&source, "main");
    let _ = std::fs::remove_dir_all(&base_dir);
    assert!(matches!(value, Value::Bool(true)), "got {value:?}");
}

#[test]
fn interpreter_executes_time_instant_now_and_system_now() {
    let value = execute_clean_with_prelude(
        r#"
        fun main(): bool {
            const t1 = std::time::Instant::now();
            const t2 = std::time::Instant::now();
            const instants_ok = t2._nanos >= t1._nanos;
            const sys = std::time::SystemTime::now();
            const sys_ok = sys._secs > 0 as i64;
            instants_ok && sys_ok
        }
        "#,
        "main",
    );

    assert!(matches!(value, Value::Bool(true)), "got {value:?}");
}

#[test]
fn interpreter_executes_time_format_rfc3339_returns_nonempty_string() {
    let value = execute_clean_with_prelude(
        r#"
        fun main(): bool {
            const sys = std::time::SystemTime::now();
            const s = std::time::format_rfc3339(sys);
            s.len() > 0 as usize
        }
        "#,
        "main",
    );

    assert!(matches!(value, Value::Bool(true)), "got {value:?}");
}

#[test]
fn interpreter_executes_tcp_listener_bind_succeeds() {
    let value = execute_clean_with_prelude(
        r#"
        fun main(): bool {
            const cap = std::net::NetCap { _private: () };
            match std::net::SocketAddr::from_str("127.0.0.1:0") {
                std::core::Result::Ok(addr) => {
                    std::net::TcpListener::bind(addr, cap);
                    true
                }
                std::core::Result::Err(_) => false,
            }
        }
        "#,
        "main",
    );

    assert!(matches!(value, Value::Bool(true)), "got {value:?}");
}

#[test]
fn interpreter_executes_stdout_write_via_io_builtin() {
    let value = execute_clean_with_prelude(
        r#"
        fun main(): bool {
            const out = std::io::stdout();
            // write_all calls write internally via the Write interface
            const s = "hello from write\n";
            match __builtin_io_stdout_write_str(s) {
                std::core::Result::Ok(_n) => true,
                std::core::Result::Err(_) => false,
            }
        }
        "#,
        "main",
    );
    assert!(matches!(value, Value::Bool(true)), "got {value:?}");
}

#[test]
fn interpreter_executes_closure_as_higher_order_argument() {
    let value = execute_clean(
        r#"
        fn apply(f: fn(i32) -> i32, x: i32) -> i32 {
            f(x)
        }

        fn main() -> i32 {
            let multiplier = 3;
            let scale = fn(x: i32) -> i32 { x * multiplier };
            apply(scale, 7)
        }
        "#,
        "main",
    );
    assert!(
        matches!(value, Value::Int(21)),
        "expected 21, got {value:?}"
    );
}

#[test]
fn interpreter_executes_closure_returning_closure() {
    let value = execute_clean(
        r#"
        fn make_adder(n: i32) -> fn(i32) -> i32 {
            fn(x: i32) -> i32 { x + n }
        }

        fn main() -> i32 {
            let add5 = make_adder(5);
            add5(37)
        }
        "#,
        "main",
    );
    assert!(
        matches!(value, Value::Int(42)),
        "expected 42, got {value:?}"
    );
}

#[test]
fn interpreter_executes_format_fn_with_integer() {
    let value = execute_clean_with_prelude(
        r#"
        fun main(): bool {
            const n = 42;
            const s = format("{}", n);
            s == "42"
        }
        "#,
        "main",
    );
    assert!(matches!(value, Value::Bool(true)), "got {value:?}");
}

#[test]
fn interpreter_executes_format_fn_with_string() {
    let value = execute_clean_with_prelude(
        r#"
        fun main(): bool {
            const greeting = "world";
            const s = format("hello {}", greeting);
            s == "hello world"
        }
        "#,
        "main",
    );
    assert!(matches!(value, Value::Bool(true)), "got {value:?}");
}

#[test]
fn interpreter_executes_derive_debug_fmt_via_formatter() {
    let value = execute_clean_with_prelude(
        r#"
        @derive(Debug)
        struct Pair {
            first: i32,
            second: i32,
        }

        fun main(): bool {
            let mut f = std::fmt::Formatter::new();
            Pair { first: 1, second: 2 }.fmt(&mut f);
            const result = f.finish();
            result == "Pair { first: 1, second: 2 }"
        }
        "#,
        "main",
    );
    assert!(matches!(value, Value::Bool(true)), "got {value:?}");
}

#[test]
fn interpreter_supports_generic_struct_instantiation() {
    let value = execute_clean(
        r#"
        struct Wrapper<T> {
            value: T,
        }

        fun get_value<T>(w: Wrapper<T>): T {
            w.value
        }

        fun main(): i32 {
            const w = Wrapper { value: 99 };
            get_value(w)
        }
        "#,
        "main",
    );
    assert!(
        matches!(value, Value::Int(99)),
        "expected 99, got {value:?}"
    );
}

#[test]
fn interpreter_supports_generic_enum_option_pattern() {
    let value = execute_clean(
        r#"
        enum Option<T> {
            Some(T),
            None,
        }

        fun unwrap_or<T>(opt: Option<T>, default: T): T {
            match opt {
                Some(v) => v,
                None => default,
            }
        }

        fun main(): i32 {
            const x = unwrap_or(Some(42), 0);
            const y = unwrap_or(None, 7);
            x + y
        }
        "#,
        "main",
    );
    assert!(
        matches!(value, Value::Int(49)),
        "expected 49, got {value:?}"
    );
}

#[test]
fn interpreter_executes_udp_bind_succeeds() {
    let value = execute_clean_with_prelude(
        r#"
        fun main(): bool {
            const cap = std::net::NetCap { _private: () };
            match std::net::SocketAddr::from_str("127.0.0.1:0") {
                std::core::Result::Ok(addr) => {
                    // bind returns IoResult<UdpSocket>; just verify it doesn't panic
                    std::net::UdpSocket::bind(addr, cap);
                    true
                }
                std::core::Result::Err(_) => false,
            }
        }
        "#,
        "main",
    );
    assert!(matches!(value, Value::Bool(true)), "got {value:?}");
}

#[test]
fn interpreter_dispatches_dyn_ability_method() {
    // Two concrete types implementing the same ability; a function takes `dyn`
    // and dispatches to whichever concrete impl was passed at the call site.
    let value = execute_clean(
        r#"
        ability Greet {
            fun greet(self): i32;
        }

        struct Hello {}
        struct Goodbye {}

        extend Hello implements Greet {
            fun greet(self): i32 { 1 }
        }

        extend Goodbye implements Greet {
            fun greet(self): i32 { 2 }
        }

        fun call_greet(x: dyn Greet): i32 {
            x.greet()
        }

        fun main(): i32 {
            const a = call_greet(Hello {});
            const b = call_greet(Goodbye {});
            a + b
        }
        "#,
        "main",
    );
    assert!(
        matches!(value, Value::Int(3)),
        "expected dyn dispatch result 3, got {value:?}"
    );
}
