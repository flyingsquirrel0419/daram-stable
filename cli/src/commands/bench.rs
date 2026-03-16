//! `dr bench [filter]` — run benchmarks.

use crate::{terminal, workspace::find_workspace};

pub fn run(args: &[String]) -> i32 {
    let filter = args.first().map(String::as_str).unwrap_or("");

    let ws = match find_workspace() {
        Ok(w) => w,
        Err(e) => {
            terminal::error(&e.to_string());
            return 1;
        }
    };

    terminal::step(&format!(
        "benchmarking `{}` v{}",
        ws.manifest.package.name, ws.manifest.package.version
    ));

    if !filter.is_empty() {
        terminal::info(&format!("filter: `{}`", filter));
    }

    // Benchmarks require a compiled binary and the native backend.
    // The native backend is scheduled for Phase 3.
    terminal::warn(
        "benchmark runner requires the native backend (Phase 3). \
         Benchmark harness functions marked with `#[bench]` are parsed but not yet executed.",
    );
    terminal::info(
        "Reference benchmark suites live under `/root/Dr/benchmarks/suites`, and result conventions live under `/root/Dr/benchmarks/results`.",
    );
    terminal::info(
        "To contribute to the native backend, start with `/root/Dr/compiler/src/mir.rs`, `/root/Dr/compiler/src/cranelift_backend.rs`, and `/root/Dr/compiler/src/c_backend.rs`.",
    );

    0
}
