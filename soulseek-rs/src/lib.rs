//! The program itself. `main.rs` is a thin entry point over this crate, so
//! every module is compiled once and integration tests drive the same code the
//! binary runs.

pub mod api;
pub mod cli;
pub mod commands;
pub mod daemon;
pub(crate) mod directories;
pub mod error;
pub mod models;
pub mod output;
pub mod persist;
pub(crate) mod port_mapping;
pub mod remote;
pub mod run;
pub mod time;
pub(crate) mod ui;

/// The interactive window, reachable so an integration test can drive it
/// with key events, advance it a frame at a time and read the screen back.
/// Hidden from the documented API: it is a harness door, not a contract.
#[doc(hidden)]
pub use ui::MainTui;

/// The deterministic session the PTY fixture binary renders, built only with
/// the `fixtures` feature.
#[cfg(feature = "fixtures")]
#[doc(hidden)]
pub use ui::fixture::FixtureSession;

/// The renderer and event types, for integration tests that drive the window
/// against a test backend.
#[doc(hidden)]
pub use ui::tui::{Terminal, backend::TestBackend};
#[doc(hidden)]
pub use ui::tui::event::{
    KeyCode, KeyEvent, KeyEventKind, KeyModifiers, MouseButton, MouseEvent,
    MouseEventKind,
};
