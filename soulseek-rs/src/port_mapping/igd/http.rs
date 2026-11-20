use std::error::Error as StdError;
use std::fmt;
use std::io::Read;
use std::time::Duration;

use h12tiny::client_sync::http::header::{HeaderName, HeaderValue, InvalidHeaderValue};
use h12tiny::client_sync::http::uri::{InvalidUri, Uri};
use h12tiny::client_sync::http::{Method, Request};
use h12tiny::client_sync::{Client, Error as ClientError};

const DEFAULT_TIMEOUT: Duration = Duration::from_secs(30);

/// The largest response body taken from a gateway. Device descriptions and
/// action lists are tens of kilobytes; any LAN host can answer discovery, so
/// the read must not follow a body without end.
pub(crate) const MAX_BODY: u64 = 1 << 20;

/// A synchronous HTTP failure raised while discovering or contacting a
/// gateway.
#[derive(Debug)]
pub enum HttpError {
    InvalidUri(InvalidUri),
    InvalidHeaderName(String),
    InvalidHeader(InvalidHeaderValue),
    Client(ClientError),
    Io(std::io::Error),
    /// The body was longer than [`MAX_BODY`].
    BodyTooLarge,
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
            Self::BodyTooLarge => {
                write!(formatter, "response body over {MAX_BODY} bytes")
            }
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
            Self::BodyTooLarge => None,
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
    response
        .into_body()
        .take(MAX_BODY + 1)
        .read_to_end(&mut body)?;
    if body.len() as u64 > MAX_BODY {
        return Err(HttpError::BodyTooLarge);
    }
    Ok(body)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{BufRead, BufReader, Write};
    use std::net::{SocketAddr, TcpListener, TcpStream};

    /// A one-shot HTTP server on a port the OS picked: reads the request head,
    /// then hands the socket to `respond`.
    fn serve(respond: impl FnOnce(TcpStream) + Send + 'static) -> SocketAddr {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
        let address = listener.local_addr().expect("address");
        std::thread::spawn(move || {
            let (stream, _) = listener.accept().expect("accept");
            let mut reader = BufReader::new(stream.try_clone().expect("clone"));
            let mut line = String::new();
            while reader.read_line(&mut line).is_ok_and(|n| n > 0) && line != "\r\n" {
                line.clear();
            }
            respond(stream);
        });
        address
    }

    fn with_length(length: u64) -> impl FnOnce(TcpStream) + Send + 'static {
        move |mut stream| {
            let _ = write!(stream, "HTTP/1.1 200 OK\r\nContent-Length: {length}\r\n\r\n");
            let chunk = [b'x'; 65_536];
            let mut left = length;
            while left > 0 {
                let n = left.min(chunk.len() as u64) as usize;
                if stream.write_all(&chunk[..n]).is_err() {
                    return;
                }
                left -= n as u64;
            }
        }
    }

    fn fetch(address: SocketAddr) -> Result<Vec<u8>, HttpError> {
        get(&format!("http://{address}/desc.xml"), Duration::from_secs(10))
    }

    #[test]
    fn a_body_at_the_limit_is_returned_whole() {
        let body = fetch(serve(with_length(MAX_BODY))).expect("fetch");
        assert_eq!(body.len() as u64, MAX_BODY);
    }

    #[test]
    fn a_body_one_byte_over_the_limit_is_refused() {
        let result = fetch(serve(with_length(MAX_BODY + 1)));
        assert!(matches!(result, Err(HttpError::BodyTooLarge)), "{result:?}");
    }

    #[test]
    fn a_body_without_end_is_cut_off_rather_than_buffered() {
        // No length: the body runs until the server's write fails, which it
        // would not for 64 MiB if the client kept reading.
        let (sent, written) = std::sync::mpsc::channel();
        let address = serve(move |mut stream| {
            let _ = stream.write_all(b"HTTP/1.1 200 OK\r\nConnection: close\r\n\r\n");
            let chunk = [b'x'; 65_536];
            let mut total = 0u64;
            for _ in 0..1024 {
                if stream.write_all(&chunk).is_err() {
                    break;
                }
                total += chunk.len() as u64;
            }
            let _ = sent.send(total);
        });
        let result = fetch(address);
        assert!(matches!(result, Err(HttpError::BodyTooLarge)), "{result:?}");
        let total = written
            .recv_timeout(Duration::from_secs(10))
            .expect("the server saw the client stop reading");
        assert!(
            total < 32 * 1024 * 1024,
            "the client kept reading an endless body: {total} bytes sent"
        );
    }
}
