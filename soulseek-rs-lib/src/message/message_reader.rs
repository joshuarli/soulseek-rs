use std::io::{self, Read};
use std::{collections::VecDeque, net::TcpStream};

use crate::message::Message;

// Soulseek messages are length-prefixed (u32 LE size, then payload). TCP gives us
// arbitrary-sized chunks, so we accumulate into a buffer and only emit a Message
// once size + 4 bytes are available.

/// A malformed length prefix must not make a peer grow our buffer without
/// bound. Real protocol frames are far smaller; this leaves generous headroom
/// for a large share list while keeping a stalled or hostile peer finite.
pub const MAX_BUFFERED_BYTES: usize = 16 * 1024 * 1024;

pub struct MessageReader {
    buffer: VecDeque<u8>,
}

impl Default for MessageReader {
    fn default() -> Self {
        Self::new()
    }
}

impl MessageReader {
    #[must_use]
    pub const fn new() -> Self {
        Self {
            buffer: VecDeque::new(),
        }
    }

    #[cfg(test)]
    #[must_use]
    pub fn new_with_buffer(buffer: Vec<u8>) -> Self {
        Self {
            buffer: buffer.into(),
        }
    }

    pub fn read_from_socket(
        &mut self,
        stream: &mut TcpStream,
    ) -> io::Result<()> {
        let mut temp_buffer = [0; 1024]; // Temporary buffer for reading from the socket
        let bytes_read = stream.read(&mut temp_buffer)?;
        // On a TCP socket a zero-byte read is the other end hanging up. Passing
        // that back as success left the caller polling a dead socket forever,
        // which is how a server that closes cleanly went unnoticed.
        if bytes_read == 0 {
            return Err(io::Error::new(
                io::ErrorKind::UnexpectedEof,
                "the remote end closed the connection",
            ));
        }

        self.append(&temp_buffer[..bytes_read])
    }

    fn append(&mut self, bytes: &[u8]) -> io::Result<()> {
        if self.buffer.len().saturating_add(bytes.len()) > MAX_BUFFERED_BYTES {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!("peer exceeded the {MAX_BUFFERED_BYTES}-byte frame buffer"),
            ));
        }
        self.buffer.extend(bytes);
        Ok(())
    }

    #[must_use]
    pub fn buffer_len(&self) -> usize {
        self.buffer.len()
    }

    pub fn get_buffer(&mut self) -> Vec<u8> {
        self.buffer.drain(..).collect()
    }

    pub fn extract_message(&mut self) -> io::Result<Option<Message>> {
        let bytes_read = self.buffer.len();
        if bytes_read < 4 {
            return Ok(None);
        }

        let message_size = u32::from_le_bytes([
            self.buffer[0],
            self.buffer[1],
            self.buffer[2],
            self.buffer[3],
        ]) as usize;

        if message_size > MAX_BUFFERED_BYTES {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!(
                    "peer announced a {message_size}-byte frame, above the \
                     {MAX_BUFFERED_BYTES}-byte limit"
                ),
            ));
        }
        let total_size = message_size + 4;

        if bytes_read < total_size {
            return Ok(None);
        }

        let message_buffer: Vec<u8> = self.buffer.drain(..total_size).collect();
        Ok(Some(Message::new_with_data(message_buffer)))
    }
}

#[cfg(test)]
mod tests {
    use crate::message::MessageReader;
    use super::MAX_BUFFERED_BYTES;

    #[test]
    fn test_extract_message() {
        let buffer: Vec<u8> = [
            8, 0, 0, 0, 117, 115, 101, 114, 110, 97, 109, 101, 8, 0, 0, 0, 112,
            97, 115, 115, 119, 111, 114, 100, 160, 0, 0, 0, 32, 0, 0, 0, 100,
            53, 49, 99, 57, 97, 55, 101, 57, 51, 53, 51, 55, 52, 54, 97, 54,
            48, 50, 48, 102, 57, 54, 48, 50, 100, 52, 53, 50, 57, 50, 57, 17,
            0, 0, 0,
        ]
        .to_vec();
        let mut buffered_reader = MessageReader::new_with_buffer(buffer);
        let mut message = buffered_reader.extract_message().unwrap().unwrap();
        assert_eq!(
            message.get_data(),
            vec![8, 0, 0, 0, 117, 115, 101, 114, 110, 97, 109, 101]
        );
        assert_eq!(message.read_string(), "username");
    }
    #[test]
    fn test_extract_message_incomplete_message() {
        let incomplete_buffer = vec![1, 2, 3];
        let mut buffered_reader =
            MessageReader::new_with_buffer(incomplete_buffer);

        let result = buffered_reader.extract_message();
        assert_eq!(None, result.unwrap());

        let rest: Vec<u8> = buffered_reader
            .buffer
            .drain(..buffered_reader.buffer.len())
            .collect();

        assert!(buffered_reader.buffer.is_empty());
        assert_eq!(vec![1, 2, 3], rest);
    }

    #[test]
    fn a_huge_declared_frame_is_rejected_without_waiting_for_its_bytes() {
        let size = (MAX_BUFFERED_BYTES as u32).saturating_add(1);
        let mut reader = MessageReader::new_with_buffer(size.to_le_bytes().to_vec());
        let error = reader.extract_message().unwrap_err();
        assert_eq!(error.kind(), std::io::ErrorKind::InvalidData);
    }

    #[test]
    fn the_buffered_byte_cap_cannot_be_exceeded_by_streaming() {
        let mut reader = MessageReader::new();
        let chunk = vec![0u8; 1024];
        for _ in 0..MAX_BUFFERED_BYTES / 1024 {
            reader.append(&chunk).unwrap();
        }
        let error = reader.append(&chunk).unwrap_err();
        assert_eq!(error.kind(), std::io::ErrorKind::InvalidData);
    }

    /// A hangup has to reach the caller as an error. Reported as success it
    /// leaves the actors polling a socket that will never speak again, which is
    /// how a server that closed cleanly went unnoticed.
    #[test]
    fn a_closed_connection_reads_as_an_error_not_as_success() {
        use std::io::Write;
        use std::net::{TcpListener, TcpStream};

        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let mut ours = TcpStream::connect(addr).unwrap();
        let (mut theirs, _) = listener.accept().unwrap();

        theirs.write_all(&[1, 2, 3]).unwrap();
        theirs.flush().unwrap();
        drop(theirs);

        let mut reader = MessageReader::new();
        // Whatever was sent before the close still arrives.
        while reader.buffer_len() < 3 {
            reader.read_from_socket(&mut ours).unwrap();
        }
        let error = reader
            .read_from_socket(&mut ours)
            .expect_err("a closed connection is not a successful read");
        assert_eq!(error.kind(), std::io::ErrorKind::UnexpectedEof);
    }
}
