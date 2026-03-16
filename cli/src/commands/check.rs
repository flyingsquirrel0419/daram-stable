//! `dr check` — type-check the project without producing a binary.

use crate::{commands::build, terminal};

pub fn run(args: &[String]) -> i32 {
    let mut check_args: Vec<String> = vec!["--check".to_string()];
    check_args.extend_from_slice(args);
    let result = build::run(&check_args);
    if result == 0 {
        terminal::success("type check passed — no errors");
    }
    result
}
