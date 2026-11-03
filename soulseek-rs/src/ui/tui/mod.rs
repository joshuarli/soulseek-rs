//! A small terminal renderer built on rustix termios and ANSI output.

pub mod event;
pub mod layout;
pub mod style;
pub mod text;
pub mod widgets;

mod buffer;
mod terminal;

pub mod backend {
    pub use super::terminal::TestBackend;
}

pub use terminal::{DefaultTerminal, Frame, Terminal, restore_terminal};
