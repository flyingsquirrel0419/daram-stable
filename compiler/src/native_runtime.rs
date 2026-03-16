//! Shared native runtime helpers and ABI metadata.
//!
//! Ownership contract today:
//! - runtime collection helpers allocate heap-backed handles (`Vec` / `HashMap`) and return
//!   them as opaque pointers;
//! - string values passed through the ABI are borrowed C strings unless a helper explicitly
//!   allocates internal storage;
//! - there is not yet a backend-wide destructor path that frees these handles on MIR `Drop`.
//!
//! The exported helper list in this module is therefore both a linkage surface and the current
//! source of truth for what native backends may rely on before full drop semantics land.

const SHARED_IO_HELPERS: &str = include_str!("../runtime/shared_io_helpers.c");
const C_BACKEND_RUNTIME_SUPPORT: &str = include_str!("../runtime/c_backend_runtime_support.c");
const NATIVE_RUNTIME_CORE: &str = include_str!("../runtime/native_runtime_core.c");

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RuntimeTy {
    Ptr,
    I8,
    I64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RuntimeOwnershipKind {
    BorrowedInput,
    OpaqueHandleProcessLifetime,
    InteriorHeapStorageProcessLifetime,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RuntimeOwnershipRule {
    pub resource: &'static str,
    pub ownership: RuntimeOwnershipKind,
    pub drop_behavior: &'static str,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ExportedRuntimeFn {
    pub name: &'static str,
    pub params: &'static [RuntimeTy],
    pub returns: &'static [RuntimeTy],
}

static EXPORTED_RUNTIME_FNS: &[ExportedRuntimeFn] = &[
    ExportedRuntimeFn {
        name: "daram_print_str",
        params: &[RuntimeTy::Ptr],
        returns: &[],
    },
    ExportedRuntimeFn {
        name: "daram_println_str",
        params: &[RuntimeTy::Ptr],
        returns: &[],
    },
    ExportedRuntimeFn {
        name: "daram_eprint_str",
        params: &[RuntimeTy::Ptr],
        returns: &[],
    },
    ExportedRuntimeFn {
        name: "daram_eprintln_str",
        params: &[RuntimeTy::Ptr],
        returns: &[],
    },
    ExportedRuntimeFn {
        name: "daram_print_i64",
        params: &[RuntimeTy::I64],
        returns: &[],
    },
    ExportedRuntimeFn {
        name: "daram_println_i64",
        params: &[RuntimeTy::I64],
        returns: &[],
    },
    ExportedRuntimeFn {
        name: "daram_eprint_i64",
        params: &[RuntimeTy::I64],
        returns: &[],
    },
    ExportedRuntimeFn {
        name: "daram_eprintln_i64",
        params: &[RuntimeTy::I64],
        returns: &[],
    },
    ExportedRuntimeFn {
        name: "daram_assert",
        params: &[RuntimeTy::I8],
        returns: &[],
    },
    ExportedRuntimeFn {
        name: "daram_assert_eq_i64",
        params: &[RuntimeTy::I64, RuntimeTy::I64],
        returns: &[],
    },
    ExportedRuntimeFn {
        name: "daram_panic_str",
        params: &[RuntimeTy::Ptr],
        returns: &[],
    },
    ExportedRuntimeFn {
        name: "daram_panic_with_fmt_i64",
        params: &[RuntimeTy::Ptr, RuntimeTy::I64, RuntimeTy::I64],
        returns: &[],
    },
    ExportedRuntimeFn {
        name: "daram_vec_new",
        params: &[],
        returns: &[RuntimeTy::Ptr],
    },
    ExportedRuntimeFn {
        name: "daram_vec_push_i64",
        params: &[RuntimeTy::Ptr, RuntimeTy::I64],
        returns: &[],
    },
    ExportedRuntimeFn {
        name: "daram_vec_push_ptr",
        params: &[RuntimeTy::Ptr, RuntimeTy::Ptr],
        returns: &[],
    },
    ExportedRuntimeFn {
        name: "daram_vec_len",
        params: &[RuntimeTy::Ptr],
        returns: &[RuntimeTy::I64],
    },
    ExportedRuntimeFn {
        name: "daram_hashmap_new",
        params: &[],
        returns: &[RuntimeTy::Ptr],
    },
    ExportedRuntimeFn {
        name: "daram_hashmap_len",
        params: &[RuntimeTy::Ptr],
        returns: &[RuntimeTy::I64],
    },
    ExportedRuntimeFn {
        name: "daram_hashmap_insert_i64_i64",
        params: &[
            RuntimeTy::Ptr,
            RuntimeTy::I64,
            RuntimeTy::I64,
            RuntimeTy::Ptr,
        ],
        returns: &[],
    },
    ExportedRuntimeFn {
        name: "daram_hashmap_get_i64_ref_i64",
        params: &[RuntimeTy::Ptr, RuntimeTy::I64, RuntimeTy::Ptr],
        returns: &[],
    },
    ExportedRuntimeFn {
        name: "daram_hashmap_remove_i64_i64",
        params: &[RuntimeTy::Ptr, RuntimeTy::I64, RuntimeTy::Ptr],
        returns: &[],
    },
    ExportedRuntimeFn {
        name: "daram_hashmap_insert_str_i64",
        params: &[
            RuntimeTy::Ptr,
            RuntimeTy::Ptr,
            RuntimeTy::I64,
            RuntimeTy::Ptr,
        ],
        returns: &[],
    },
    ExportedRuntimeFn {
        name: "daram_hashmap_get_str_ref_i64",
        params: &[RuntimeTy::Ptr, RuntimeTy::Ptr, RuntimeTy::Ptr],
        returns: &[],
    },
    ExportedRuntimeFn {
        name: "daram_hashmap_remove_str_i64",
        params: &[RuntimeTy::Ptr, RuntimeTy::Ptr, RuntimeTy::Ptr],
        returns: &[],
    },
    ExportedRuntimeFn {
        name: "daram_hashmap_insert_i64_ptr",
        params: &[
            RuntimeTy::Ptr,
            RuntimeTy::I64,
            RuntimeTy::Ptr,
            RuntimeTy::Ptr,
        ],
        returns: &[],
    },
    ExportedRuntimeFn {
        name: "daram_hashmap_get_i64_ref_ptr",
        params: &[RuntimeTy::Ptr, RuntimeTy::I64, RuntimeTy::Ptr],
        returns: &[],
    },
    ExportedRuntimeFn {
        name: "daram_hashmap_remove_i64_ptr",
        params: &[RuntimeTy::Ptr, RuntimeTy::I64, RuntimeTy::Ptr],
        returns: &[],
    },
    ExportedRuntimeFn {
        name: "daram_hashmap_insert_str_ptr",
        params: &[
            RuntimeTy::Ptr,
            RuntimeTy::Ptr,
            RuntimeTy::Ptr,
            RuntimeTy::Ptr,
        ],
        returns: &[],
    },
    ExportedRuntimeFn {
        name: "daram_hashmap_get_str_ref_ptr",
        params: &[RuntimeTy::Ptr, RuntimeTy::Ptr, RuntimeTy::Ptr],
        returns: &[],
    },
    ExportedRuntimeFn {
        name: "daram_hashmap_remove_str_ptr",
        params: &[RuntimeTy::Ptr, RuntimeTy::Ptr, RuntimeTy::Ptr],
        returns: &[],
    },
];

static RUNTIME_OWNERSHIP_RULES: &[RuntimeOwnershipRule] = &[
    RuntimeOwnershipRule {
        resource: "string inputs",
        ownership: RuntimeOwnershipKind::BorrowedInput,
        drop_behavior: "borrowed C string inputs are not freed by runtime helpers",
    },
    RuntimeOwnershipRule {
        resource: "Vec handles",
        ownership: RuntimeOwnershipKind::OpaqueHandleProcessLifetime,
        drop_behavior: "heap-backed vector handles currently live until process exit",
    },
    RuntimeOwnershipRule {
        resource: "HashMap handles",
        ownership: RuntimeOwnershipKind::OpaqueHandleProcessLifetime,
        drop_behavior: "heap-backed hashmap handles currently live until process exit",
    },
    RuntimeOwnershipRule {
        resource: "HashMap string keys",
        ownership: RuntimeOwnershipKind::InteriorHeapStorageProcessLifetime,
        drop_behavior: "duplicated string keys are retained by the runtime and not reclaimed yet",
    },
];

fn append_linkage_macro(out: &mut String, linkage: &str) {
    out.push_str("#ifndef DARAM_RUNTIME_LINKAGE\n");
    out.push_str("#define DARAM_RUNTIME_LINKAGE ");
    out.push_str(linkage);
    out.push_str("\n#endif\n\n");
}

pub fn c_backend_support_source() -> String {
    let mut out = String::new();
    out.push_str(
        "#include <stdbool.h>\n#include <stdint.h>\n#include <setjmp.h>\n#include <stdio.h>\n#include <stdlib.h>\n\n",
    );
    append_linkage_macro(&mut out, "static");
    out.push_str(C_BACKEND_RUNTIME_SUPPORT);
    out.push('\n');
    out.push_str(SHARED_IO_HELPERS);
    out.push('\n');
    out
}

pub fn link_runtime_source() -> String {
    let mut out = String::new();
    out.push_str(
        "#include <stdbool.h>\n#include <stdint.h>\n#include <stdio.h>\n#include <stdlib.h>\n#include <string.h>\n\n",
    );
    append_linkage_macro(&mut out, "");
    out.push_str(SHARED_IO_HELPERS);
    out.push('\n');
    out.push_str(NATIVE_RUNTIME_CORE);
    out.push('\n');
    out
}

pub fn exported_runtime_functions() -> &'static [ExportedRuntimeFn] {
    EXPORTED_RUNTIME_FNS
}

pub fn runtime_ownership_rules() -> &'static [RuntimeOwnershipRule] {
    RUNTIME_OWNERSHIP_RULES
}

pub fn exported_runtime_function(name: &str) -> Option<&'static ExportedRuntimeFn> {
    EXPORTED_RUNTIME_FNS
        .iter()
        .find(|function| function.name == name)
}

#[cfg(test)]
mod tests {
    use super::{exported_runtime_functions, link_runtime_source};

    #[test]
    fn link_runtime_contains_all_exported_symbols() {
        let source = link_runtime_source();
        for function in exported_runtime_functions() {
            assert!(
                source.contains(function.name),
                "runtime source missing exported helper {}",
                function.name
            );
        }
    }
}
