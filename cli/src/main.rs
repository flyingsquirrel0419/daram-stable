//! # dr — The Daram Language CLI
//!
//! Official command-line interface for the Daram programming language.
//!
//! ## Commands
//! ```text
//! dr new      Create a new project
//! dr init     Initialise the current directory as a Daram project
//! dr add      Add a dependency
//! dr remove   Remove a dependency
//! dr install  Install all dependencies listed in daram.toml
//! dr build    Compile the current project
//! dr run      Build and run the current project
//! dr test     Run the project's test suite
//! dr bench    Run benchmarks
//! dr fmt      Format source code
//! dr lint     Run the static linter
//! dr check    Type-check without producing a binary
//! dr doc      Generate documentation
//! dr lsp      Run the language server over stdio
//! ```

mod commands;
mod dependency_cache;
mod manifest;
mod terminal;
mod workspace;

use std::process;

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let exit_code = run(&args);
    process::exit(exit_code);
}

fn run(args: &[String]) -> i32 {
    if args.len() < 2 {
        print_help();
        return 0;
    }

    let cmd = args[1].as_str();
    let rest = &args[2..];

    match cmd {
        "new" => commands::new::run(rest),
        "init" => commands::init::run(rest),
        "add" => commands::add::run(rest),
        "remove" => commands::remove::run(rest),
        "install" => commands::install::run(rest),
        "build" => commands::build::run(rest),
        "run" => commands::run_cmd::run(rest),
        "test" => commands::test::run(rest),
        "__exec-mir" => commands::exec_mir::run_internal(rest),
        "bench" => commands::bench::run(rest),
        "fmt" => commands::fmt::run(rest),
        "lint" => commands::lint::run(rest),
        "check" => commands::check::run(rest),
        "doc" => commands::doc::run(rest),
        "lsp" => commands::lsp::run(rest),
        "-h" | "--help" | "help" => {
            print_help();
            0
        }
        "-V" | "--version" | "version" => {
            print_version();
            0
        }
        other => {
            terminal::error(&format!("unknown command: `{}`", other));
            eprintln!();
            eprintln!("Run `dr help` to see available commands.");
            1
        }
    }
}

fn print_help() {
    println!("dr — The Daram Language CLI");
    println!();
    println!("Usage: dr <command> [options]");
    println!();
    println!("Commands:");
    println!("  new      <name>     Create a new Daram project in a new directory");
    println!("  init                Initialise the current directory as a Daram project");
    println!("  add      <pkg>      Add a dependency to daram.toml");
    println!("  remove   <pkg>      Remove a dependency from daram.toml");
    println!("  install             Install all dependencies");
    println!("  build               Compile the current project");
    println!("  run      [args]     Build and run the current project");
    println!("  test     [filter]   Run the test suite");
    println!("  bench    [filter]   Run benchmarks");
    println!("  fmt                 Format all source files (`--check`, `--verbose`, `--migrate-syntax`)");
    println!("  lint                Run the static linter");
    println!("  check               Type-check without emitting a binary");
    println!("  doc                 Generate HTML documentation");
    println!("  lsp                 Run the language server over stdio");
    println!();
    println!("Options:");
    println!("  -h, --help      Print this message");
    println!("  -V, --version   Print version information");
}

fn print_version() {
    println!("dr {}", env!("CARGO_PKG_VERSION"));
}
