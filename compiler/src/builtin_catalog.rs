#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BuiltinReturnTy {
    Unit,
    Never,
    String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BackendSupport {
    pub interpreter: bool,
    pub c_backend: bool,
    pub cranelift: bool,
}

impl BackendSupport {
    const fn new(interpreter: bool, c_backend: bool, cranelift: bool) -> Self {
        Self {
            interpreter,
            c_backend,
            cranelift,
        }
    }
}

#[derive(Debug)]
pub struct BuiltinSpec {
    pub canonical_name: &'static str,
    pub names: &'static [&'static str],
    pub known_return: Option<BuiltinReturnTy>,
    pub variadic: bool,
    pub support: BackendSupport,
}

macro_rules! builtin {
    ($canonical:literal, [$($name:literal),+ $(,)?], $known_return:expr, $variadic:expr, $support:expr) => {
        BuiltinSpec {
            canonical_name: $canonical,
            names: &[$($name),+],
            known_return: $known_return,
            variadic: $variadic,
            support: $support,
        }
    };
}

const INTERPRETER_ONLY: BackendSupport = BackendSupport::new(true, false, false);
const CORE_NATIVE: BackendSupport = BackendSupport::new(true, true, true);
const CRANELIFT_ONLY_NATIVE: BackendSupport = BackendSupport::new(true, false, true);

static BUILTINS: &[BuiltinSpec] = &[
    builtin!(
        "print",
        ["print", "std::io::print", "__builtin_print"],
        Some(BuiltinReturnTy::Unit),
        true,
        CORE_NATIVE
    ),
    builtin!(
        "println",
        ["println", "std::io::println", "__builtin_println"],
        Some(BuiltinReturnTy::Unit),
        true,
        CORE_NATIVE
    ),
    builtin!(
        "eprint",
        ["eprint", "std::io::eprint", "__builtin_eprint"],
        Some(BuiltinReturnTy::Unit),
        true,
        CORE_NATIVE
    ),
    builtin!(
        "eprintln",
        ["eprintln", "std::io::eprintln", "__builtin_eprintln"],
        Some(BuiltinReturnTy::Unit),
        true,
        CORE_NATIVE
    ),
    builtin!(
        "assert",
        ["assert", "std::test::assert", "__builtin_assert"],
        Some(BuiltinReturnTy::Unit),
        false,
        CORE_NATIVE
    ),
    builtin!(
        "assert_eq",
        ["assert_eq", "std::test::assert_eq", "__builtin_assert_eq"],
        Some(BuiltinReturnTy::Unit),
        false,
        CORE_NATIVE
    ),
    builtin!(
        "panic_with_fmt",
        [
            "panic_with_fmt",
            "std::test::panic_with_fmt",
            "__builtin_panic_with_fmt",
        ],
        Some(BuiltinReturnTy::Unit),
        false,
        CORE_NATIVE
    ),
    builtin!(
        "format",
        ["format", "std::fmt::format", "__builtin_format"],
        Some(BuiltinReturnTy::String),
        true,
        INTERPRETER_ONLY
    ),
    builtin!(
        "panic",
        ["panic", "std::core::panic", "__builtin_panic"],
        Some(BuiltinReturnTy::Never),
        false,
        CORE_NATIVE
    ),
    builtin!("vec_new", ["__builtin_vec_new"], None, false, CORE_NATIVE),
    builtin!("vec_push", ["__builtin_vec_push"], None, false, CORE_NATIVE),
    builtin!(
        "vec_pop",
        ["__builtin_vec_pop"],
        None,
        false,
        INTERPRETER_ONLY
    ),
    builtin!("vec_len", ["__builtin_vec_len"], None, false, CORE_NATIVE),
    builtin!(
        "vec_get",
        ["__builtin_vec_get"],
        None,
        false,
        INTERPRETER_ONLY
    ),
    builtin!(
        "vec_iter",
        ["__builtin_vec_iter"],
        None,
        false,
        INTERPRETER_ONLY
    ),
    builtin!(
        "vec_iter_next",
        ["__builtin_vec_iter_next"],
        None,
        false,
        INTERPRETER_ONLY
    ),
    builtin!(
        "vec_iter_count",
        ["__builtin_vec_iter_count"],
        None,
        false,
        INTERPRETER_ONLY
    ),
    builtin!(
        "iter_map",
        ["__builtin_iter_map"],
        None,
        false,
        INTERPRETER_ONLY
    ),
    builtin!(
        "iter_map_next",
        ["__builtin_iter_map_next"],
        None,
        false,
        INTERPRETER_ONLY
    ),
    builtin!(
        "iter_filter",
        ["__builtin_iter_filter"],
        None,
        false,
        INTERPRETER_ONLY
    ),
    builtin!(
        "iter_filter_next",
        ["__builtin_iter_filter_next"],
        None,
        false,
        INTERPRETER_ONLY
    ),
    builtin!(
        "iter_collect_vec",
        ["__builtin_iter_collect_vec"],
        None,
        false,
        INTERPRETER_ONLY
    ),
    builtin!(
        "hashmap_new",
        ["__builtin_hashmap_new"],
        None,
        false,
        CORE_NATIVE
    ),
    builtin!(
        "hashmap_insert",
        ["__builtin_hashmap_insert"],
        None,
        false,
        CRANELIFT_ONLY_NATIVE
    ),
    builtin!(
        "hashmap_get",
        ["__builtin_hashmap_get"],
        None,
        false,
        CRANELIFT_ONLY_NATIVE
    ),
    builtin!(
        "hashmap_remove",
        ["__builtin_hashmap_remove"],
        None,
        false,
        CRANELIFT_ONLY_NATIVE
    ),
    builtin!(
        "hashmap_contains",
        ["__builtin_hashmap_contains"],
        None,
        false,
        INTERPRETER_ONLY
    ),
    builtin!(
        "hashmap_len",
        ["__builtin_hashmap_len"],
        None,
        false,
        CORE_NATIVE
    ),
    builtin!(
        "hashmap_iter_get",
        ["__builtin_hashmap_iter_get"],
        None,
        false,
        INTERPRETER_ONLY
    ),
    builtin!(
        "hashmap_iter",
        ["__builtin_hashmap_iter"],
        None,
        false,
        INTERPRETER_ONLY
    ),
    builtin!(
        "hashmap_iter_next",
        ["__builtin_hashmap_iter_next"],
        None,
        false,
        INTERPRETER_ONLY
    ),
    builtin!(
        "hashmap_iter_count",
        ["__builtin_hashmap_iter_count"],
        None,
        false,
        INTERPRETER_ONLY
    ),
    builtin!(
        "string_new",
        ["__builtin_string_new"],
        None,
        false,
        INTERPRETER_ONLY
    ),
    builtin!(
        "string_len",
        ["__builtin_string_len"],
        None,
        false,
        INTERPRETER_ONLY
    ),
    builtin!(
        "string_push",
        ["__builtin_string_push"],
        None,
        false,
        INTERPRETER_ONLY
    ),
    builtin!(
        "string_push_str",
        ["__builtin_string_push_str"],
        None,
        false,
        INTERPRETER_ONLY
    ),
    builtin!(
        "string_contains",
        ["__builtin_string_contains"],
        None,
        false,
        INTERPRETER_ONLY
    ),
    builtin!(
        "string_as_str",
        ["__builtin_string_as_str"],
        None,
        false,
        INTERPRETER_ONLY
    ),
    builtin!(
        "string_split",
        ["__builtin_string_split"],
        None,
        false,
        INTERPRETER_ONLY
    ),
    builtin!(
        "string_split_next",
        ["__builtin_string_split_next"],
        None,
        false,
        INTERPRETER_ONLY
    ),
    builtin!(
        "string_split_count",
        ["__builtin_string_split_count"],
        None,
        false,
        INTERPRETER_ONLY
    ),
    builtin!(
        "string_trim",
        ["__builtin_string_trim"],
        None,
        false,
        INTERPRETER_ONLY
    ),
    builtin!(
        "string_repeat",
        ["__builtin_string_repeat"],
        None,
        false,
        INTERPRETER_ONLY
    ),
    builtin!(
        "string_replace",
        ["__builtin_string_replace"],
        None,
        false,
        INTERPRETER_ONLY
    ),
    builtin!(
        "pathbuf_new",
        ["__builtin_pathbuf_new"],
        None,
        false,
        INTERPRETER_ONLY
    ),
    builtin!(
        "pathbuf_from",
        ["__builtin_pathbuf_from"],
        None,
        false,
        INTERPRETER_ONLY
    ),
    builtin!(
        "pathbuf_as_str",
        ["__builtin_pathbuf_as_str"],
        None,
        false,
        INTERPRETER_ONLY
    ),
    builtin!(
        "pathbuf_file_name",
        ["__builtin_pathbuf_file_name"],
        None,
        false,
        INTERPRETER_ONLY
    ),
    builtin!(
        "pathbuf_extension",
        ["__builtin_pathbuf_extension"],
        None,
        false,
        INTERPRETER_ONLY
    ),
    builtin!(
        "pathbuf_parent",
        ["__builtin_pathbuf_parent"],
        None,
        false,
        INTERPRETER_ONLY
    ),
    builtin!(
        "pathbuf_is_file",
        ["__builtin_pathbuf_is_file"],
        None,
        false,
        INTERPRETER_ONLY
    ),
    builtin!(
        "pathbuf_is_dir",
        ["__builtin_pathbuf_is_dir"],
        None,
        false,
        INTERPRETER_ONLY
    ),
    builtin!(
        "socket_addr_parse",
        ["__builtin_socket_addr_parse"],
        None,
        false,
        INTERPRETER_ONLY
    ),
    builtin!(
        "tcp_connect",
        ["__builtin_tcp_connect"],
        None,
        false,
        INTERPRETER_ONLY
    ),
    builtin!(
        "tcp_connect_timeout",
        ["__builtin_tcp_connect_timeout"],
        None,
        false,
        INTERPRETER_ONLY
    ),
    builtin!(
        "tcp_shutdown",
        ["__builtin_tcp_shutdown"],
        None,
        false,
        INTERPRETER_ONLY
    ),
    builtin!(
        "tcp_bind",
        ["__builtin_tcp_bind"],
        None,
        false,
        INTERPRETER_ONLY
    ),
    builtin!(
        "tcp_accept",
        ["__builtin_tcp_accept"],
        None,
        false,
        INTERPRETER_ONLY
    ),
    builtin!(
        "tcp_read",
        ["__builtin_tcp_read"],
        None,
        false,
        INTERPRETER_ONLY
    ),
    builtin!(
        "tcp_write",
        ["__builtin_tcp_write"],
        None,
        false,
        INTERPRETER_ONLY
    ),
    builtin!(
        "io_read_line",
        ["__builtin_io_read_line"],
        None,
        false,
        INTERPRETER_ONLY
    ),
    builtin!(
        "io_stdout_write_str",
        ["__builtin_io_stdout_write_str"],
        None,
        false,
        INTERPRETER_ONLY
    ),
    builtin!(
        "io_stderr_write_str",
        ["__builtin_io_stderr_write_str"],
        None,
        false,
        INTERPRETER_ONLY
    ),
    builtin!(
        "io_stdout_flush",
        ["__builtin_io_stdout_flush"],
        None,
        false,
        INTERPRETER_ONLY
    ),
    builtin!(
        "io_stderr_flush",
        ["__builtin_io_stderr_flush"],
        None,
        false,
        INTERPRETER_ONLY
    ),
    builtin!(
        "io_stdin_read_to_string",
        ["__builtin_io_stdin_read_to_string"],
        None,
        false,
        INTERPRETER_ONLY
    ),
    builtin!(
        "udp_bind",
        ["__builtin_udp_bind"],
        None,
        false,
        INTERPRETER_ONLY
    ),
    builtin!(
        "udp_send_to",
        ["__builtin_udp_send_to"],
        None,
        false,
        INTERPRETER_ONLY
    ),
    builtin!(
        "udp_recv_from",
        ["__builtin_udp_recv_from"],
        None,
        false,
        INTERPRETER_ONLY
    ),
    builtin!(
        "udp_connect",
        ["__builtin_udp_connect"],
        None,
        false,
        INTERPRETER_ONLY
    ),
    builtin!(
        "udp_send",
        ["__builtin_udp_send"],
        None,
        false,
        INTERPRETER_ONLY
    ),
    builtin!(
        "udp_recv",
        ["__builtin_udp_recv"],
        None,
        false,
        INTERPRETER_ONLY
    ),
    builtin!(
        "http_method_as_str",
        ["__builtin_http_method_as_str"],
        None,
        false,
        INTERPRETER_ONLY
    ),
    builtin!(
        "http_headers_get",
        ["__builtin_http_headers_get"],
        None,
        false,
        INTERPRETER_ONLY
    ),
    builtin!(
        "http_body_json",
        ["__builtin_http_body_json"],
        None,
        false,
        INTERPRETER_ONLY
    ),
    builtin!(
        "http_send",
        ["__builtin_http_send"],
        None,
        false,
        INTERPRETER_ONLY
    ),
    builtin!(
        "task_spawn",
        ["__builtin_task_spawn"],
        None,
        false,
        INTERPRETER_ONLY
    ),
    builtin!(
        "task_join",
        ["__builtin_task_join"],
        None,
        false,
        INTERPRETER_ONLY
    ),
    builtin!(
        "task_block_on",
        ["__builtin_task_block_on"],
        None,
        false,
        INTERPRETER_ONLY
    ),
    builtin!(
        "task_sleep_ms",
        ["__builtin_task_sleep_ms"],
        None,
        false,
        INTERPRETER_ONLY
    ),
    builtin!(
        "fs_read_to_string",
        ["__builtin_fs_read_to_string"],
        None,
        false,
        INTERPRETER_ONLY
    ),
    builtin!(
        "fs_write_str",
        ["__builtin_fs_write_str"],
        None,
        false,
        INTERPRETER_ONLY
    ),
    builtin!(
        "fs_exists",
        ["__builtin_fs_exists"],
        None,
        false,
        INTERPRETER_ONLY
    ),
    builtin!(
        "fs_create_dir_all",
        ["__builtin_fs_create_dir_all"],
        None,
        false,
        INTERPRETER_ONLY
    ),
    builtin!(
        "fs_remove_file",
        ["__builtin_fs_remove_file"],
        None,
        false,
        INTERPRETER_ONLY
    ),
    builtin!(
        "fs_remove_dir",
        ["__builtin_fs_remove_dir"],
        None,
        false,
        INTERPRETER_ONLY
    ),
    builtin!(
        "fs_copy",
        ["__builtin_fs_copy"],
        None,
        false,
        INTERPRETER_ONLY
    ),
    builtin!(
        "time_instant_now",
        ["__builtin_time_instant_now"],
        None,
        false,
        INTERPRETER_ONLY
    ),
    builtin!(
        "time_system_now",
        ["__builtin_time_system_now"],
        None,
        false,
        INTERPRETER_ONLY
    ),
    builtin!(
        "time_format_rfc3339",
        ["__builtin_time_format_rfc3339"],
        None,
        false,
        INTERPRETER_ONLY
    ),
    builtin!(
        "json_parse",
        ["__builtin_json_parse"],
        None,
        false,
        INTERPRETER_ONLY
    ),
    builtin!(
        "json_stringify",
        ["__builtin_json_stringify"],
        None,
        false,
        INTERPRETER_ONLY
    ),
    builtin!(
        "json_get",
        ["__builtin_json_get"],
        None,
        false,
        INTERPRETER_ONLY
    ),
    builtin!(
        "json_index",
        ["__builtin_json_index"],
        None,
        false,
        INTERPRETER_ONLY
    ),
];

pub fn all_builtin_specs() -> &'static [BuiltinSpec] {
    BUILTINS
}

pub fn all_builtin_names() -> impl Iterator<Item = &'static str> {
    BUILTINS.iter().flat_map(|spec| spec.names.iter().copied())
}

pub fn lookup(name: &str) -> Option<&'static BuiltinSpec> {
    BUILTINS.iter().find(|spec| spec.names.contains(&name))
}

pub fn canonical_name(name: &str) -> Option<&'static str> {
    lookup(name).map(|spec| spec.canonical_name)
}

pub fn known_return(name: &str) -> Option<BuiltinReturnTy> {
    lookup(name).and_then(|spec| spec.known_return)
}

pub fn is_variadic(name: &str) -> bool {
    lookup(name).is_some_and(|spec| spec.variadic)
}

#[cfg(test)]
mod tests {
    use super::{
        all_builtin_names, all_builtin_specs, canonical_name, is_variadic, known_return, lookup,
        BuiltinReturnTy,
    };
    use std::collections::HashSet;

    #[test]
    fn builtin_names_are_unique() {
        let mut seen = HashSet::new();
        for name in all_builtin_names() {
            assert!(
                seen.insert(name),
                "duplicate builtin name in catalog: {name}"
            );
        }
    }

    #[test]
    fn canonicalizes_aliases() {
        assert_eq!(canonical_name("std::io::print"), Some("print"));
        assert_eq!(canonical_name("__builtin_print"), Some("print"));
        assert_eq!(
            canonical_name("std::test::panic_with_fmt"),
            Some("panic_with_fmt")
        );
    }

    #[test]
    fn exposes_known_return_types() {
        assert_eq!(known_return("print"), Some(BuiltinReturnTy::Unit));
        assert_eq!(
            known_return("__builtin_format"),
            Some(BuiltinReturnTy::String)
        );
        assert_eq!(
            known_return("std::core::panic"),
            Some(BuiltinReturnTy::Never)
        );
    }

    #[test]
    fn tracks_variadic_builtins() {
        assert!(is_variadic("print"));
        assert!(is_variadic("__builtin_format"));
        assert!(!is_variadic("assert"));
    }

    #[test]
    fn includes_stdlib_builtin_only_entries() {
        assert!(lookup("__builtin_io_read_line").is_some());
    }

    #[test]
    fn every_builtin_has_explicit_backend_support() {
        for builtin in all_builtin_specs() {
            assert!(
                builtin.support.interpreter
                    || builtin.support.c_backend
                    || builtin.support.cranelift,
                "builtin `{}` has no backend support metadata",
                builtin.canonical_name
            );
        }
    }

    #[test]
    fn catalog_does_not_expose_map_or_concurrency_builtins_yet() {
        for builtin in all_builtin_specs() {
            assert!(
                !builtin.canonical_name.contains("map")
                    || builtin.canonical_name == "hashmap_new"
                    || builtin.canonical_name == "hashmap_insert"
                    || builtin.canonical_name == "hashmap_get"
                    || builtin.canonical_name == "hashmap_remove"
                    || builtin.canonical_name == "hashmap_contains"
                    || builtin.canonical_name == "hashmap_len"
                    || builtin.canonical_name == "hashmap_iter_get"
                    || builtin.canonical_name == "hashmap_iter"
                    || builtin.canonical_name == "hashmap_iter_next"
                    || builtin.canonical_name == "hashmap_iter_count"
                    || builtin.canonical_name == "iter_map"
                    || builtin.canonical_name == "iter_map_next",
                "unexpected new map-like builtin surface `{}`",
                builtin.canonical_name
            );
            assert!(
                !builtin.canonical_name.contains("concurrency"),
                "unexpected concurrency builtin surface `{}`",
                builtin.canonical_name
            );
        }
    }
}
