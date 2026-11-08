//! Entry point. Everything it does lives in the library crate, so the same
//! code is what integration tests drive.

use std::process::ExitCode;

fn main() -> ExitCode {
    soulseek_rs_tui::run::run(std::env::args_os())
}
