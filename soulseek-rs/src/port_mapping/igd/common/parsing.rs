use std::collections::HashMap;
use std::net::{IpAddr, SocketAddr};

use xmltree::{self, Element};

use super::super::errors::{AddPortError, GetExternalIpError, RemovePortError, RequestError, SearchError};

/// How deeply a gateway's XML may nest. Real device descriptions nest under ten
/// levels; the parser recurses per level, so an unbounded document from any
/// host on the LAN would overflow the stack and abort the process.
const MAX_XML_DEPTH: usize = 32;

/// Whether `xml` opens more than `limit` elements at once, found without
/// building a tree. Counts conservatively: comments, CDATA, processing
/// instructions and declarations are skipped as the parser skips them, and
/// quotes inside a tag are honoured so an attribute cannot hide a `/>`.
fn nests_deeper_than(xml: &[u8], limit: usize) -> bool {
    fn find(haystack: &[u8], from: usize, needle: &[u8]) -> usize {
        haystack[from..]
            .windows(needle.len())
            .position(|window| window == needle)
            .map_or(haystack.len(), |at| from + at + needle.len())
    }

    let mut depth = 0usize;
    let mut i = 0;
    while let Some(offset) = xml[i..].iter().position(|&byte| byte == b'<') {
        i += offset;
        let rest = &xml[i..];
        if rest.starts_with(b"<!--") {
            i = find(xml, i + 4, b"-->");
        } else if rest.starts_with(b"<![CDATA[") {
            i = find(xml, i + 9, b"]]>");
        } else if rest.starts_with(b"<?") {
            i = find(xml, i + 2, b"?>");
        } else if rest.starts_with(b"<!") {
            i = find(xml, i + 2, b">");
        } else if rest.starts_with(b"</") {
            depth = depth.saturating_sub(1);
            i = find(xml, i + 2, b">");
        } else {
            let mut quote = None;
            let mut j = i + 1;
            let mut self_closing = false;
            while j < xml.len() {
                let byte = xml[j];
                match quote {
                    Some(open) if byte == open => quote = None,
                    Some(_) => {}
                    None if byte == b'"' || byte == b'\'' => quote = Some(byte),
                    None if byte == b'>' => {
                        self_closing = xml[j - 1] == b'/';
                        break;
                    }
                    None => {}
                }
                j += 1;
            }
            if !self_closing {
                depth += 1;
                if depth > limit {
                    return true;
                }
            }
            i = j + 1;
        }
        if i >= xml.len() {
            break;
        }
    }
    false
}

// Parse the result.
pub fn parse_search_result(text: &str) -> Result<(SocketAddr, String), SearchError> {
    use SearchError::InvalidResponse;

    for line in text.lines() {
        let line = line.trim();
        if line.to_ascii_lowercase().starts_with("location:") {
            if let Some(colon) = line.find(':') {
                let url_text = line[colon + 1..].trim();
                return parse_location(url_text).ok_or(InvalidResponse);
            }
        }
    }
    Err(InvalidResponse)
}

fn parse_location(url: &str) -> Option<(SocketAddr, String)> {
    let (scheme, rest) = url.split_once("://")?;
    let default_port = match scheme.to_ascii_lowercase().as_str() {
        "http" | "ws" => 80,
        "https" | "wss" => 443,
        "ftp" => 21,
        _ => return None,
    };
    let authority_end = rest.find(['/', '?', '#']).unwrap_or(rest.len());
    let (authority, rest) = rest.split_at(authority_end);
    let (host, port) = match authority.rsplit_once(':') {
        Some((host, "")) => (host, default_port),
        Some((host, port)) => (host, port.parse().ok()?),
        None => (authority, default_port),
    };
    let addr = SocketAddr::new(host.parse().ok()?, port);
    let path_end = rest.find(['?', '#']).unwrap_or(rest.len());
    let path = match &rest[..path_end] {
        "" => "/".to_owned(),
        path => path.to_owned(),
    };
    Some((addr, path))
}

pub fn parse_control_urls(resp: &[u8]) -> Result<(String, String), SearchError> {
    if nests_deeper_than(resp, MAX_XML_DEPTH) {
        return Err(SearchError::InvalidResponse);
    }
    let root = Element::parse(resp)?;

    let mut urls = root.children.iter().filter_map(|child| {
        let child = child.as_element()?;
        if child.name == "device" {
            Some(parse_device(child)?)
        } else {
            None
        }
    });

    urls.next().ok_or(SearchError::InvalidResponse)
}

fn parse_device(device: &Element) -> Option<(String, String)> {
    let services = device.get_child("serviceList").and_then(|service_list| {
        service_list
            .children
            .iter()
            .filter_map(|child| {
                let child = child.as_element()?;
                if child.name == "service" {
                    parse_service(child)
                } else {
                    None
                }
            })
            .next()
    });
    let devices = device.get_child("deviceList").and_then(parse_device_list);
    services.or(devices)
}

fn parse_device_list(device_list: &Element) -> Option<(String, String)> {
    device_list
        .children
        .iter()
        .filter_map(|child| {
            let child = child.as_element()?;
            if child.name == "device" {
                parse_device(child)
            } else {
                None
            }
        })
        .next()
}

fn parse_service(service: &Element) -> Option<(String, String)> {
    let service_type = service.get_child("serviceType")?;
    let service_type = service_type
        .get_text()
        .map(|s| s.into_owned())
        .unwrap_or_else(|| "".into());
    if [
        "urn:schemas-upnp-org:service:WANPPPConnection:1",
        "urn:schemas-upnp-org:service:WANIPConnection:1",
        "urn:schemas-upnp-org:service:WANIPConnection:2",
    ]
    .contains(&service_type.as_str())
    {
        let scpd_url = service.get_child("SCPDURL");
        let control_url = service.get_child("controlURL");
        if let (Some(scpd_url), Some(control_url)) = (scpd_url, control_url) {
            Some((
                scpd_url.get_text().map(|s| s.into_owned()).unwrap_or_else(|| "".into()),
                control_url
                    .get_text()
                    .map(|s| s.into_owned())
                    .unwrap_or_else(|| "".into()),
            ))
        } else {
            None
        }
    } else {
        None
    }
}

pub fn parse_schemas(resp: &[u8]) -> Result<HashMap<String, Vec<String>>, SearchError> {
    if nests_deeper_than(resp, MAX_XML_DEPTH) {
        return Err(SearchError::InvalidResponse);
    }
    let root = Element::parse(resp)?;

    let mut schema = root.children.iter().filter_map(|child| {
        let child = child.as_element()?;
        if child.name == "actionList" {
            parse_action_list(child)
        } else {
            None
        }
    });

    schema.next().ok_or(SearchError::InvalidResponse)
}

fn parse_action_list(action_list: &Element) -> Option<HashMap<String, Vec<String>>> {
    Some(
        action_list
            .children
            .iter()
            .filter_map(|child| {
                let child = child.as_element()?;
                if child.name == "action" {
                    parse_action(child)
                } else {
                    None
                }
            })
            .collect(),
    )
}

fn parse_action(action: &Element) -> Option<(String, Vec<String>)> {
    Some((
        action.get_child("name")?.get_text()?.into_owned(),
        parse_argument_list(action.get_child("argumentList")?)?,
    ))
}

fn parse_argument_list(argument_list: &Element) -> Option<Vec<String>> {
    Some(
        argument_list
            .children
            .iter()
            .filter_map(|child| {
                let child = child.as_element()?;
                if child.name == "argument" {
                    parse_argument(child)
                } else {
                    None
                }
            })
            .collect(),
    )
}

fn parse_argument(action: &Element) -> Option<String> {
    if action.get_child("direction")?.get_text()?.into_owned().as_str() == "in" {
        Some(action.get_child("name")?.get_text()?.into_owned())
    } else {
        None
    }
}

pub struct RequestReponse {
    text: String,
    xml: xmltree::Element,
}

pub type RequestResult = Result<RequestReponse, RequestError>;

pub fn parse_response(text: String, ok: &str) -> RequestResult {
    if nests_deeper_than(text.as_bytes(), MAX_XML_DEPTH) {
        return Err(RequestError::InvalidResponse(text));
    }
    let mut xml = match xmltree::Element::parse(text.as_bytes()) {
        Ok(xml) => xml,
        Err(..) => return Err(RequestError::InvalidResponse(text)),
    };
    let body = match xml.get_mut_child("Body") {
        Some(body) => body,
        None => return Err(RequestError::InvalidResponse(text)),
    };
    if let Some(ok) = body.take_child(ok) {
        return Ok(RequestReponse { text, xml: ok });
    }
    let upnp_error = match body
        .get_child("Fault")
        .and_then(|e| e.get_child("detail"))
        .and_then(|e| e.get_child("UPnPError"))
    {
        Some(upnp_error) => upnp_error,
        None => return Err(RequestError::InvalidResponse(text)),
    };

    match (
        upnp_error.get_child("errorCode"),
        upnp_error.get_child("errorDescription"),
    ) {
        (Some(e), Some(d)) => match (e.get_text().as_ref(), d.get_text().as_ref()) {
            (Some(et), Some(dt)) => match et.parse::<u16>() {
                Ok(en) => Err(RequestError::ErrorCode(en, From::from(&dt[..]))),
                Err(..) => Err(RequestError::InvalidResponse(text)),
            },
            _ => Err(RequestError::InvalidResponse(text)),
        },
        _ => Err(RequestError::InvalidResponse(text)),
    }
}

pub fn parse_get_external_ip_response(result: RequestResult) -> Result<IpAddr, GetExternalIpError> {
    match result {
        Ok(resp) => match resp
            .xml
            .get_child("NewExternalIPAddress")
            .and_then(|e| e.get_text())
            .and_then(|t| t.parse::<IpAddr>().ok())
        {
            Some(ipv4_addr) => Ok(ipv4_addr),
            None => Err(GetExternalIpError::RequestError(RequestError::InvalidResponse(
                resp.text,
            ))),
        },
        Err(RequestError::ErrorCode(606, _)) => Err(GetExternalIpError::ActionNotAuthorized),
        Err(e) => Err(GetExternalIpError::RequestError(e)),
    }
}

pub fn convert_add_port_error(err: RequestError) -> AddPortError {
    match err {
        RequestError::ErrorCode(605, _) => AddPortError::DescriptionTooLong,
        RequestError::ErrorCode(606, _) => AddPortError::ActionNotAuthorized,
        RequestError::ErrorCode(718, _) => AddPortError::PortInUse,
        RequestError::ErrorCode(724, _) => AddPortError::SamePortValuesRequired,
        RequestError::ErrorCode(725, _) => AddPortError::OnlyPermanentLeasesSupported,
        e => AddPortError::RequestError(e),
    }
}

pub fn parse_delete_port_mapping_response(result: RequestResult) -> Result<(), RemovePortError> {
    match result {
        Ok(_) => Ok(()),
        Err(err) => Err(match err {
            RequestError::ErrorCode(606, _) => RemovePortError::ActionNotAuthorized,
            RequestError::ErrorCode(714, _) => RemovePortError::NoSuchPortMapping,
            e => RemovePortError::RequestError(e),
        }),
    }
}

#[test]
fn test_parse_search_result_case_insensitivity() {
    assert!(parse_search_result("location:http://0.0.0.0:0/control_url").is_ok());
    assert!(parse_search_result("LOCATION:http://0.0.0.0:0/control_url").is_ok());
}

#[test]
fn test_parse_search_result_ok() {
    let result = parse_search_result("location:http://0.0.0.0:0/control_url").unwrap();
    assert_eq!(result.0.ip(), IpAddr::V4(std::net::Ipv4Addr::new(0, 0, 0, 0)));
    assert_eq!(result.0.port(), 0);
    assert_eq!(&result.1[..], "/control_url");
}

#[test]
fn test_parse_search_result_fail() {
    assert!(parse_search_result("content-type:http://0.0.0.0:0/control_url").is_err());
}

#[test]
fn test_parse_search_result_default_port() {
    let (addr, path) = parse_search_result("location:http://192.168.1.1/rootDesc.xml").unwrap();
    assert_eq!(addr, SocketAddr::from(([192, 168, 1, 1], 80)));
    assert_eq!(path, "/rootDesc.xml");
}

#[test]
fn test_parse_search_result_drops_query_and_fragment() {
    let (_, path) = parse_search_result("location:http://0.0.0.0:0/control_url?a=b#c").unwrap();
    assert_eq!(path, "/control_url");
}

#[test]
fn test_parse_search_result_without_host_fails() {
    assert!(parse_search_result("location:http:///control_url").is_err());
}

#[test]
fn test_parse_device1() {
    let text = r#"<?xml version="1.0" encoding="UTF-8"?>
<root xmlns="urn:schemas-upnp-org:device-1-0">
   <specVersion>
      <major>1</major>
      <minor>0</minor>
   </specVersion>
   <device>
      <deviceType>urn:schemas-upnp-org:device:InternetGatewayDevice:1</deviceType>
      <friendlyName></friendlyName>
      <manufacturer></manufacturer>
      <manufacturerURL></manufacturerURL>
      <modelDescription></modelDescription>
      <modelName></modelName>
      <modelNumber>1</modelNumber>
      <serialNumber>00000000</serialNumber>
      <UDN></UDN>
      <serviceList>
         <service>
            <serviceType>urn:schemas-upnp-org:service:Layer3Forwarding:1</serviceType>
            <serviceId>urn:upnp-org:serviceId:Layer3Forwarding1</serviceId>
            <controlURL>/ctl/L3F</controlURL>
            <eventSubURL>/evt/L3F</eventSubURL>
            <SCPDURL>/L3F.xml</SCPDURL>
         </service>
      </serviceList>
      <deviceList>
         <device>
            <deviceType>urn:schemas-upnp-org:device:WANDevice:1</deviceType>
            <friendlyName>WANDevice</friendlyName>
            <manufacturer>MiniUPnP</manufacturer>
            <manufacturerURL>http://miniupnp.free.fr/</manufacturerURL>
            <modelDescription>WAN Device</modelDescription>
            <modelName>WAN Device</modelName>
            <modelNumber>20180615</modelNumber>
            <modelURL>http://miniupnp.free.fr/</modelURL>
            <serialNumber>00000000</serialNumber>
            <UDN>uuid:804e2e56-7bfe-4733-bae0-04bf6d569692</UDN>
            <UPC>MINIUPNPD</UPC>
            <serviceList>
               <service>
                  <serviceType>urn:schemas-upnp-org:service:WANCommonInterfaceConfig:1</serviceType>
                  <serviceId>urn:upnp-org:serviceId:WANCommonIFC1</serviceId>
                  <controlURL>/ctl/CmnIfCfg</controlURL>
                  <eventSubURL>/evt/CmnIfCfg</eventSubURL>
                  <SCPDURL>/WANCfg.xml</SCPDURL>
               </service>
            </serviceList>
            <deviceList>
               <device>
                  <deviceType>urn:schemas-upnp-org:device:WANConnectionDevice:1</deviceType>
                  <friendlyName>WANConnectionDevice</friendlyName>
                  <manufacturer>MiniUPnP</manufacturer>
                  <manufacturerURL>http://miniupnp.free.fr/</manufacturerURL>
                  <modelDescription>MiniUPnP daemon</modelDescription>
                  <modelName>MiniUPnPd</modelName>
                  <modelNumber>20180615</modelNumber>
                  <modelURL>http://miniupnp.free.fr/</modelURL>
                  <serialNumber>00000000</serialNumber>
                  <UDN>uuid:804e2e56-7bfe-4733-bae0-04bf6d569692</UDN>
                  <UPC>MINIUPNPD</UPC>
                  <serviceList>
                     <service>
                        <serviceType>urn:schemas-upnp-org:service:WANIPConnection:1</serviceType>
                        <serviceId>urn:upnp-org:serviceId:WANIPConn1</serviceId>
                        <controlURL>/ctl/IPConn</controlURL>
                        <eventSubURL>/evt/IPConn</eventSubURL>
                        <SCPDURL>/WANIPCn.xml</SCPDURL>
                     </service>
                  </serviceList>
               </device>
            </deviceList>
         </device>
      </deviceList>
      <presentationURL>http://192.168.0.1/</presentationURL>
   </device>
</root>"#;

    let (control_schema_url, control_url) = parse_control_urls(text.as_bytes()).unwrap();
    assert_eq!(control_url, "/ctl/IPConn");
    assert_eq!(control_schema_url, "/WANIPCn.xml");
}

#[test]
fn test_parse_device2() {
    let text = r#"<?xml version="1.0" encoding="UTF-8"?>
    <root xmlns="urn:schemas-upnp-org:device-1-0">
        <specVersion>
            <major>1</major>
            <minor>0</minor>
        </specVersion>
        <device>
            <deviceType>urn:schemas-upnp-org:device:InternetGatewayDevice:1</deviceType>
            <friendlyName>FRITZ!Box 7430</friendlyName>
            <manufacturer>AVM Berlin</manufacturer>
            <manufacturerURL>http://www.avm.de</manufacturerURL>
            <modelDescription>FRITZ!Box 7430</modelDescription>
            <modelName>FRITZ!Box 7430</modelName>
            <modelNumber>avm</modelNumber>
            <modelURL>http://www.avm.de</modelURL>
            <UDN>uuid:00000000-0000-0000-0000-000000000000</UDN>
            <iconList>
                <icon>
                    <mimetype>image/gif</mimetype>
                    <width>118</width>
                    <height>119</height>
                    <depth>8</depth>
                    <url>/ligd.gif</url>
                </icon>
            </iconList>
            <serviceList>
                <service>
                    <serviceType>urn:schemas-any-com:service:Any:1</serviceType>
                    <serviceId>urn:any-com:serviceId:any1</serviceId>
                    <controlURL>/igdupnp/control/any</controlURL>
                    <eventSubURL>/igdupnp/control/any</eventSubURL>
                    <SCPDURL>/any.xml</SCPDURL>
                </service>
            </serviceList>
            <deviceList>
                <device>
                    <deviceType>urn:schemas-upnp-org:device:WANDevice:1</deviceType>
                    <friendlyName>WANDevice - FRITZ!Box 7430</friendlyName>
                    <manufacturer>AVM Berlin</manufacturer>
                    <manufacturerURL>www.avm.de</manufacturerURL>
                    <modelDescription>WANDevice - FRITZ!Box 7430</modelDescription>
                    <modelName>WANDevice - FRITZ!Box 7430</modelName>
                    <modelNumber>avm</modelNumber>
                    <modelURL>www.avm.de</modelURL>
                    <UDN>uuid:00000000-0000-0000-0000-000000000000</UDN>
                    <UPC>AVM IGD</UPC>
                    <serviceList>
                        <service>
                            <serviceType>urn:schemas-upnp-org:service:WANCommonInterfaceConfig:1</serviceType>
                            <serviceId>urn:upnp-org:serviceId:WANCommonIFC1</serviceId>
                            <controlURL>/igdupnp/control/WANCommonIFC1</controlURL>
                            <eventSubURL>/igdupnp/control/WANCommonIFC1</eventSubURL>
                            <SCPDURL>/igdicfgSCPD.xml</SCPDURL>
                        </service>
                    </serviceList>
                    <deviceList>
                        <device>
                            <deviceType>urn:schemas-upnp-org:device:WANConnectionDevice:1</deviceType>
                            <friendlyName>WANConnectionDevice - FRITZ!Box 7430</friendlyName>
                            <manufacturer>AVM Berlin</manufacturer>
                            <manufacturerURL>www.avm.de</manufacturerURL>
                            <modelDescription>WANConnectionDevice - FRITZ!Box 7430</modelDescription>
                            <modelName>WANConnectionDevice - FRITZ!Box 7430</modelName>
                            <modelNumber>avm</modelNumber>
                            <modelURL>www.avm.de</modelURL>
                            <UDN>uuid:00000000-0000-0000-0000-000000000000</UDN>
                            <UPC>AVM IGD</UPC>
                            <serviceList>
                                <service>
                                    <serviceType>urn:schemas-upnp-org:service:WANDSLLinkConfig:1</serviceType>
                                    <serviceId>urn:upnp-org:serviceId:WANDSLLinkC1</serviceId>
                                    <controlURL>/igdupnp/control/WANDSLLinkC1</controlURL>
                                    <eventSubURL>/igdupnp/control/WANDSLLinkC1</eventSubURL>
                                    <SCPDURL>/igddslSCPD.xml</SCPDURL>
                                </service>
                                <service>
                                    <serviceType>urn:schemas-upnp-org:service:WANIPConnection:1</serviceType>
                                    <serviceId>urn:upnp-org:serviceId:WANIPConn1</serviceId>
                                    <controlURL>/igdupnp/control/WANIPConn1</controlURL>
                                    <eventSubURL>/igdupnp/control/WANIPConn1</eventSubURL>
                                    <SCPDURL>/igdconnSCPD.xml</SCPDURL>
                                </service>
                                <service>
                                    <serviceType>urn:schemas-upnp-org:service:WANIPv6FirewallControl:1</serviceType>
                                    <serviceId>urn:upnp-org:serviceId:WANIPv6Firewall1</serviceId>
                                    <controlURL>/igd2upnp/control/WANIPv6Firewall1</controlURL>
                                    <eventSubURL>/igd2upnp/control/WANIPv6Firewall1</eventSubURL>
                                    <SCPDURL>/igd2ipv6fwcSCPD.xml</SCPDURL>
                                </service>
                            </serviceList>
                        </device>
                    </deviceList>
                </device>
            </deviceList>
            <presentationURL>http://fritz.box</presentationURL>
        </device>
    </root>
    "#;
    let result = parse_control_urls(text.as_bytes());
    assert!(result.is_ok());
    let (control_schema_url, control_url) = result.unwrap();
    assert_eq!(control_url, "/igdupnp/control/WANIPConn1");
    assert_eq!(control_schema_url, "/igdconnSCPD.xml");
}

#[test]
fn test_parse_device3() {
    let text = r#"<?xml version="1.0" encoding="UTF-8"?>
<root xmlns="urn:schemas-upnp-org:device-1-0">
<specVersion>
    <major>1</major>
    <minor>0</minor>
</specVersion>
<device xmlns="urn:schemas-upnp-org:device-1-0">
   <deviceType>urn:schemas-upnp-org:device:InternetGatewayDevice:1</deviceType>
   <friendlyName></friendlyName>
   <manufacturer></manufacturer>
   <manufacturerURL></manufacturerURL>
   <modelDescription></modelDescription>
   <modelName></modelName>
   <modelNumber></modelNumber>
   <serialNumber></serialNumber>
   <presentationURL>http://192.168.1.1</presentationURL>
   <UDN>uuid:00000000-0000-0000-0000-000000000000</UDN>
   <UPC>999999999001</UPC>
   <iconList>
      <icon>
         <mimetype>image/png</mimetype>
         <width>16</width>
         <height>16</height>
         <depth>8</depth>
         <url>/ligd.png</url>
      </icon>
   </iconList>
   <deviceList>
      <device>
         <deviceType>urn:schemas-upnp-org:device:WANDevice:1</deviceType>
         <friendlyName></friendlyName>
         <manufacturer></manufacturer>
         <manufacturerURL></manufacturerURL>
         <modelDescription></modelDescription>
         <modelName></modelName>
         <modelNumber></modelNumber>
         <modelURL></modelURL>
         <serialNumber></serialNumber>
         <presentationURL>http://192.168.1.254</presentationURL>
         <UDN>uuid:00000000-0000-0000-0000-000000000000</UDN>
         <UPC>999999999001</UPC>
         <serviceList>
            <service>
               <serviceType>urn:schemas-upnp-org:service:WANCommonInterfaceConfig:1</serviceType>
               <serviceId>urn:upnp-org:serviceId:WANCommonIFC1</serviceId>
               <controlURL>/upnp/control/WANCommonIFC1</controlURL>
               <eventSubURL>/upnp/control/WANCommonIFC1</eventSubURL>
               <SCPDURL>/332b484d/wancomicfgSCPD.xml</SCPDURL>
            </service>
         </serviceList>
         <deviceList>
            <device>
               <deviceType>urn:schemas-upnp-org:device:WANConnectionDevice:1</deviceType>
               <friendlyName></friendlyName>
               <manufacturer></manufacturer>
               <manufacturerURL></manufacturerURL>
               <modelDescription></modelDescription>
               <modelName></modelName>
               <modelNumber></modelNumber>
               <modelURL></modelURL>
               <serialNumber></serialNumber>
               <presentationURL>http://192.168.1.254</presentationURL>
               <UDN>uuid:00000000-0000-0000-0000-000000000000</UDN>
               <UPC>999999999001</UPC>
               <serviceList>
                  <service>
                     <serviceType>urn:schemas-upnp-org:service:WANIPConnection:1</serviceType>
                     <serviceId>urn:upnp-org:serviceId:WANIPConn1</serviceId>
                     <controlURL>/upnp/control/WANIPConn1</controlURL>
                     <eventSubURL>/upnp/control/WANIPConn1</eventSubURL>
                     <SCPDURL>/332b484d/wanipconnSCPD.xml</SCPDURL>
                  </service>
               </serviceList>
            </device>
         </deviceList>
      </device>
   </deviceList>
</device>
</root>"#;

    let (control_schema_url, control_url) = parse_control_urls(text.as_bytes()).unwrap();
    assert_eq!(control_url, "/upnp/control/WANIPConn1");
    assert_eq!(control_schema_url, "/332b484d/wanipconnSCPD.xml");
}

#[cfg(test)]
mod limits {
    use super::*;

    fn nested(open: &str, close: &str, depth: usize) -> String {
        format!("<root>{}{}</root>", open.repeat(depth), close.repeat(depth))
    }

    #[test]
    fn a_deeply_nested_document_is_refused_instead_of_overflowing_the_stack() {
        // 100,000 levels aborted the process with a stack overflow before the
        // depth limit existed.
        let xml = nested("<a>", "</a>", 100_000);
        assert!(matches!(parse_control_urls(xml.as_bytes()), Err(SearchError::InvalidResponse)));
        assert!(matches!(parse_schemas(xml.as_bytes()), Err(SearchError::InvalidResponse)));
        assert!(matches!(
            parse_response(xml, "AddPortMappingResponse"),
            Err(RequestError::InvalidResponse(_))
        ));
    }

    #[test]
    fn the_depth_limit_is_exact() {
        let at_limit = nested("<a>", "</a>", MAX_XML_DEPTH - 1);
        let over = nested("<a>", "</a>", MAX_XML_DEPTH);
        assert!(!nests_deeper_than(at_limit.as_bytes(), MAX_XML_DEPTH));
        assert!(nests_deeper_than(over.as_bytes(), MAX_XML_DEPTH));
    }

    #[test]
    fn markup_that_looks_like_nesting_cannot_hide_or_fake_it() {
        let deep = |open: &str| nested(open, "</a>", 1000);
        // An attribute value holding `/>` does not close the tag.
        assert!(nests_deeper_than(deep(r#"<a b="/>">"#).as_bytes(), 32));
        assert!(nests_deeper_than(deep("<a b='\"/>'>").as_bytes(), 32));
        // Real self-closing elements do not nest.
        let flat = format!("<root>{}</root>", "<a/>".repeat(1000));
        assert!(!nests_deeper_than(flat.as_bytes(), 32));
        // Comments and CDATA are text, not elements.
        let text = format!("<root><!--{}--><![CDATA[{}]]></root>", "<a>".repeat(1000), "<a>".repeat(1000));
        assert!(!nests_deeper_than(text.as_bytes(), 32));
        // An unterminated tag or comment ends the scan without panicking.
        for xml in ["<a", "<a b=\"", "<!--", "<![CDATA[", "<?", "</", "<"] {
            let _ = nests_deeper_than(xml.as_bytes(), 32);
        }
    }
}
