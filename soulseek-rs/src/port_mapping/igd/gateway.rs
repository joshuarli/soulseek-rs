use std::collections::HashMap;
use std::fmt;
use std::net::{IpAddr, SocketAddr};

use super::common::{messages, parsing, parsing::RequestResult};
use super::http;
use super::errors::{AddPortError, GetExternalIpError, RemovePortError, RequestError};
use super::PortMappingProtocol;

/// This structure represents a gateway found by the search functions.
#[derive(Clone, Debug)]
pub struct Gateway {
    /// Socket address of the gateway
    pub addr: SocketAddr,
    /// Control url of the device
    pub control_url: String,
    /// Control schema for all actions
    pub control_schema: HashMap<String, Vec<String>>,
}

impl Gateway {
    fn perform_request(&self, header: &str, body: &str, ok: &str) -> RequestResult {
        let url = format!("http://{}{}", self.addr, self.control_url);

        let response = http::post(
            &url,
            &[("SOAPAction", header), ("Content-Type", "text/xml")],
            body.as_bytes().to_vec(),
        )?;
        let text = String::from_utf8(response)
            .map_err(|error| RequestError::InvalidResponse(error.to_string()))?;

        parsing::parse_response(text, ok)
    }

    /// Get the external IP address of the gateway.
    pub fn get_external_ip(&self) -> Result<IpAddr, GetExternalIpError> {
        parsing::parse_get_external_ip_response(self.perform_request(
            messages::GET_EXTERNAL_IP_HEADER,
            &messages::format_get_external_ip_message(),
            "GetExternalIPAddressResponse",
        ))
    }

    fn add_port_mapping(
        &self,
        protocol: PortMappingProtocol,
        external_port: u16,
        local_addr: SocketAddr,
        lease_duration: u32,
        description: &str,
    ) -> Result<(), RequestError> {
        self.perform_request(
            messages::ADD_PORT_MAPPING_HEADER,
            &messages::format_add_port_mapping_message(
                self.control_schema
                    .get("AddPortMapping")
                    .ok_or_else(|| RequestError::UnsupportedAction("AddPortMapping".to_string()))?,
                protocol,
                external_port,
                local_addr,
                lease_duration,
                description,
            ),
            "AddPortMappingResponse",
        )?;

        Ok(())
    }

    /// Add a port mapping.
    ///
    /// The local_addr is the address where the traffic is sent to.
    /// The lease_duration parameter is in seconds. A value of 0 is infinite.
    pub fn add_port(
        &self,
        protocol: PortMappingProtocol,
        external_port: u16,
        local_addr: SocketAddr,
        lease_duration: u32,
        description: &str,
    ) -> Result<(), AddPortError> {
        if external_port == 0 {
            return Err(AddPortError::ExternalPortZeroInvalid);
        }
        if local_addr.port() == 0 {
            return Err(AddPortError::InternalPortZeroInvalid);
        }

        self.add_port_mapping(protocol, external_port, local_addr, lease_duration, description)
            .map_err(parsing::convert_add_port_error)
    }

    /// Remove a port mapping.
    pub fn remove_port(&self, protocol: PortMappingProtocol, external_port: u16) -> Result<(), RemovePortError> {
        parsing::parse_delete_port_mapping_response(self.perform_request(
            messages::DELETE_PORT_MAPPING_HEADER,
            &messages::format_delete_port_message(
                self.control_schema.get("DeletePortMapping").ok_or_else(|| {
                    RemovePortError::RequestError(RequestError::UnsupportedAction("DeletePortMapping".to_string()))
                })?,
                protocol,
                external_port,
            ),
            "DeletePortMappingResponse",
        ))
    }
}

impl fmt::Display for Gateway {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "http://{}{}", self.addr, self.control_url)
    }
}
