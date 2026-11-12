#[cfg(any(test, feature = "fixtures"))]
#[cfg_attr(
    not(feature = "fixtures"),
    allow(dead_code, reason = "unit tests use part of the fixture; the sample session is built only for the fixtures binary")
)]
pub mod fixture;
pub mod login;
mod main_tui;
mod paging;
mod panes;
mod styles;
pub mod tui;
mod utils;

pub use main_tui::{MainTui, launch_main_tui};
pub use paging::*;
pub use styles::*;
pub use utils::*;
