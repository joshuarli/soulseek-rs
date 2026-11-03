use std::error::Error as StdError;
use std::fmt;
use std::io::Read;
use std::time::Duration;

use h12tiny::client_sync::http::header::{HeaderName, HeaderValue, InvalidHeaderValue};
use h12tiny::client_sync::http::uri::{InvalidUri, Uri};
use h12tiny::client_sync::http::{Method, Request};
use h12tiny::client_sync::{Client, Error as ClientError};

const DEFAULT_TIMEOUT: Duration = Duration::from_secs(30);

/// A synchronous HTTP failure raised while discovering or contacting a
/// gateway.
#[derive(Debug)]
pub enum HttpError {
    InvalidUri(InvalidUri),
    InvalidHeaderName(String),
    InvalidHeader(InvalidHeaderValue),
    Client(ClientError),
    Io(std::io::Error),
}

impl fmt::Display for HttpError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidUri(error) => write!(formatter, "invalid URL: {error}"),
            Self::InvalidHeaderName(name) => {
                write!(formatter, "invalid header name: {name}")
            }
            Self::InvalidHeader(error) => {
                write!(formatter, "invalid header value: {error}")
            }
            Self::Client(error) => error.fmt(formatter),
            Self::Io(error) => error.fmt(formatter),
        }
    }
}

impl StdError for HttpError {
    fn source(&self) -> Option<&(dyn StdError + 'static)> {
        match self {
            Self::InvalidUri(error) => Some(error),
            Self::InvalidHeaderName(_) => None,
            Self::InvalidHeader(error) => Some(error),
            Self::Client(error) => Some(error),
            Self::Io(error) => Some(error),
        }
    }
}

impl From<ClientError> for HttpError {
    fn from(error: ClientError) -> Self {
        Self::Client(error)
    }
}

impl From<std::io::Error> for HttpError {
    fn from(error: std::io::Error) -> Self {
        Self::Io(error)
    }
}

pub(crate) fn get(url: &str, timeout: Duration) -> Result<Vec<u8>, HttpError> {
    send(Method::GET, url, &[], Vec::new(), Some(timeout))
}

pub(crate) fn post(
    url: &str,
    headers: &[(&str, &str)],
    body: Vec<u8>,
) -> Result<Vec<u8>, HttpError> {

    send(Method::POST, url, headers, body, None)
}

fn send(
    method: Method,
    url: &str,
    headers: &[(&str, &str)],
    body: Vec<u8>,
    timeout: Option<Duration>,
) -> Result<Vec<u8>, HttpError> {
    let mut request = Request::new(body);
    *request.method_mut() = method;
    *request.uri_mut() = url
        .parse::<Uri>()
        .map_err(HttpError::InvalidUri)?;
    for (name, value) in headers {
        let name = HeaderName::from_bytes(name.as_bytes())
            .map_err(|_| HttpError::InvalidHeaderName((*name).to_string()))?;
        let value =
            HeaderValue::from_str(value).map_err(HttpError::InvalidHeader)?;
        request.headers_mut().insert(name, value);
    }

    let timeout = timeout
        .unwrap_or(DEFAULT_TIMEOUT)
        .max(Duration::from_millis(1));
    let client = Client::builder().connect_timeout(timeout)?.build();
    let mut response = client.request_with_timeout(request, Some(timeout))?;
    response.body_mut().set_read_timeout(Some(timeout))?;
    let mut body = Vec::new();
    response.into_body().read_to_end(&mut body)?;
    Ok(body)
}
