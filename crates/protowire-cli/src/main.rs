//! Thin wrapper that hands argv to [`protowire_cli::run`] and streams the
//! result to stdout/stderr.

use std::io::Write as _;
use std::process::ExitCode;

fn main() -> ExitCode {
    let argv: Vec<String> = std::env::args().skip(1).collect();
    let argv_refs: Vec<&str> = argv.iter().map(String::as_str).collect();

    let result = protowire_cli::run(&argv_refs, |path| std::fs::read(path));

    let _ = std::io::stdout().write_all(&result.stdout);
    if !result.stderr.is_empty() {
        let _ = std::io::stderr().write_all(result.stderr.as_bytes());
    }
    ExitCode::from(result.exit.clamp(0, 255) as u8)
}
