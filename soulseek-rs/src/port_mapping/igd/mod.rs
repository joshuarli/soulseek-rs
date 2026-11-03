#![allow(dead_code)]
#![allow(clippy::all, clippy::pedantic, clippy::nursery)]
#![deny(missing_docs)]

//! This library allows you to communicate with an IGD enabled device.
//! Use one of the `search_gateway` functions to obtain a `Gateway` object.
//! You can then communicate with the device via this object.

pub use self::common::SearchOptions;
pub use self::gateway::Gateway;

// search of gateway
pub use self::search::search_gateway;

mod common;
mod errors;
mod gateway;
mod http;
mod search;

use std::fmt;

/// Represents the protocols available for port mapping.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PortMappingProtocol {
    /// TCP protocol
    TCP,
    /// UDP protocol
    UDP,
}

impl fmt::Display for PortMappingProtocol {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(
            f,
            "{}",
            match *self {
                PortMappingProtocol::TCP => "TCP",
                PortMappingProtocol::UDP => "UDP",
            }
        )
    }
}
