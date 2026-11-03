//! Render the real window over a deterministic session, for PTY capture.
//!
//! Built only with the `fixtures` feature. The terminal lifecycle is the same
//! one the program uses, so the fixture exercises raw mode, the alternate
//! screen and mouse reporting rather than a test backend.

use soulseek_rs_tui::{FixtureSession, MainTui, Terminal};
use std::sync::Arc;
use std::time::Duration;

fn main() -> std::process::ExitCode {
    let session = Arc::new(FixtureSession::sample());
    let tui = MainTui::new(
        session,
        "/home/tester/Music".to_string(),
        false,
        Duration::from_secs(60),
        None,
        None,
    );

    let terminal = match Terminal::enter() {
        Ok(terminal) => terminal,
        Err(error) => {
            eprintln!("tui fixture: {error}");
            return std::process::ExitCode::FAILURE;
        }
    };
    match tui.run(terminal) {
        Ok(_) => std::process::ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("tui fixture: {error}");
            std::process::ExitCode::FAILURE
        }
    }
}
