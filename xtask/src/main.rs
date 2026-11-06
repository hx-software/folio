//! The build tool generates every JavaScript loader; no application JS is authored.
use std::{
    env,
    path::PathBuf,
    process::{Command, ExitCode},
};

fn main() -> ExitCode {
    let args: Vec<_> = env::args().skip(1).collect();
    let command = args.first().map(String::as_str).unwrap_or("serve");
    if !matches!(command, "serve" | "build") {
        eprintln!("Usage: cargo xtask [serve|build] [--release]");
        return ExitCode::FAILURE;
    }
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let mut trunk = Command::new("trunk");
    trunk.current_dir(root.parent().unwrap()).arg(command);
    if args.iter().any(|a| a == "--release") {
        trunk.arg("--release");
    }
    match trunk.status() {
        Ok(status) if status.success() => ExitCode::SUCCESS,
        Ok(status) => {
            eprintln!("Trunk exited with {status}");
            ExitCode::FAILURE
        }
        Err(error) => {
            eprintln!("Cannot run Trunk: {error}. See README.md for prerequisites.");
            ExitCode::FAILURE
        }
    }
}
