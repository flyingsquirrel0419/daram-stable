//! Terminal output helpers.

/// Print a success message with a green "✓" prefix.
pub fn success(msg: &str) {
    eprintln!("\x1b[32m✓\x1b[0m {}", msg);
}

/// Print an informational message with a blue "→" prefix.
pub fn info(msg: &str) {
    eprintln!("\x1b[34m→\x1b[0m {}", msg);
}

/// Print a warning with a yellow "!" prefix.
pub fn warn(msg: &str) {
    eprintln!("\x1b[33m!\x1b[0m {}", msg);
}

/// Print an error with a red "✗" prefix to stderr.
pub fn error(msg: &str) {
    eprintln!("\x1b[31m✗\x1b[0m {}", msg);
}

/// Print a step (bold "•") prefix.
pub fn step(msg: &str) {
    eprintln!("\x1b[1m•\x1b[0m {}", msg);
}
