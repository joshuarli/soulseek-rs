use std::io::{self, Read};
use std::{collections::VecDeque, net::TcpStream};

use crate::message::Message;

// Soulseek messages are length-prefixed (u32 LE size, then payload). TCP gives us
// arbitrary-sized chunks, so we accumulate into a buffer and only emit a Message
// once size + 4 bytes are available.

/// Size of the length prefix that travels with every frame.
pub const LENGTH_PREFIX_BYTES: usize = 4;

/// Largest payload we accept from a peer. Real protocol frames are far smaller;
/// this leaves generous headroom for a large share list while keeping a stalled
/// or hostile peer finite.
pub const MAX_PAYLOAD_BYTES: usize = 16 * 1024 * 1024;

/// Buffer budget for a single frame: its payload plus the length prefix.
pub const MAX_BUFFERED_BYTES: usize =
    MAX_PAYLOAD_BYTES.saturating_add(LENGTH_PREFIX_BYTES);

pub struct MessageReader {
    buffer: VecDeque<u8>,
    max_payload_bytes: usize,
    max_buffered_bytes: usize,
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
            max_payload_bytes: MAX_PAYLOAD_BYTES,
            max_buffered_bytes: MAX_BUFFERED_BYTES,
        }
    }

    #[cfg(test)]
    #[must_use]
    pub const fn new_with_payload_limit(max_payload_bytes: usize) -> Self {
        Self {
            buffer: VecDeque::new(),
            max_payload_bytes,
            max_buffered_bytes: max_payload_bytes
                .saturating_add(LENGTH_PREFIX_BYTES),
        }
    }

    #[cfg(test)]
    #[must_use]
    pub fn new_with_buffer(buffer: Vec<u8>) -> Self {
        Self {
            buffer: buffer.into(),
            max_payload_bytes: MAX_PAYLOAD_BYTES,
            max_buffered_bytes: MAX_BUFFERED_BYTES,
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
        let max_buffered = self.max_buffered_bytes;
        if self.buffer.len().saturating_add(bytes.len()) > max_buffered {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!(
                    "peer exceeded the {max_buffered}-byte frame buffer \
                     (payload limit {} plus {LENGTH_PREFIX_BYTES}-byte header)",
                    self.max_payload_bytes
                ),
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
        if bytes_read < LENGTH_PREFIX_BYTES {
            return Ok(None);
        }

        let message_size = u32::from_le_bytes([
            self.buffer[0],
            self.buffer[1],
            self.buffer[2],
            self.buffer[3],
        ]) as usize;

        if message_size > self.max_payload_bytes {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!(
                    "peer announced a {message_size}-byte frame, above the \
                     {}-byte payload limit",
                    self.max_payload_bytes
                ),
            ));
        }
        let total_size = message_size + LENGTH_PREFIX_BYTES;

        if bytes_read < total_size {
            return Ok(None);
        }

        let message_buffer: Vec<u8> = self.buffer.drain(..total_size).collect();
        Ok(Some(Message::new_with_data(message_buffer)))
    }
}

#[cfg(test)]
mod tests {
    use super::{
        LENGTH_PREFIX_BYTES, MAX_BUFFERED_BYTES, MAX_PAYLOAD_BYTES,
        MessageReader,
    };
    use std::io::{ErrorKind, Write};
    use std::net::{TcpListener, TcpStream};
    use std::time::Duration;

    const TEST_LIMIT: usize = 32;

    fn socket_pair() -> (TcpStream, TcpStream) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let ours = TcpStream::connect(addr).unwrap();
        ours.set_read_timeout(Some(Duration::from_secs(2))).unwrap();
        let (theirs, _) = listener.accept().unwrap();
        theirs.set_nodelay(true).unwrap();
        (ours, theirs)
    }

    fn frame(payload: &[u8]) -> Vec<u8> {
        let mut bytes = (payload.len() as u32).to_le_bytes().to_vec();
        bytes.extend_from_slice(payload);
        bytes
    }

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

    #[test]
    fn production_caps_leave_room_for_the_length_prefix() {
        assert_eq!(
            MAX_BUFFERED_BYTES,
            MAX_PAYLOAD_BYTES + LENGTH_PREFIX_BYTES
        );
        assert_eq!(MessageReader::new().max_buffered_bytes, MAX_BUFFERED_BYTES);
    }

    #[test]
    fn a_frame_at_the_limit_fits_with_its_header() {
        let (mut ours, mut theirs) = socket_pair();
        let mut reader = MessageReader::new_with_payload_limit(TEST_LIMIT);
        theirs.write_all(&frame(&[0x5a; TEST_LIMIT])).unwrap();

        reader.read_from_socket(&mut ours).unwrap();
        assert_eq!(reader.buffer_len(), TEST_LIMIT + LENGTH_PREFIX_BYTES);
        let message = reader.extract_message().unwrap().unwrap();
        assert_eq!(message.get_data(), frame(&[0x5a; TEST_LIMIT]));
        assert_eq!(reader.buffer_len(), 0);
    }

    #[test]
    fn one_byte_beyond_the_payload_limit_is_rejected_from_the_header_alone() {
        let (mut ours, mut theirs) = socket_pair();
        let mut reader = MessageReader::new_with_payload_limit(TEST_LIMIT);
        theirs.write_all(&((TEST_LIMIT + 1) as u32).to_le_bytes()).unwrap();

        reader.read_from_socket(&mut ours).unwrap();
        assert_eq!(reader.buffer_len(), LENGTH_PREFIX_BYTES);
        let error = reader.extract_message().unwrap_err();
        assert_eq!(error.kind(), ErrorKind::InvalidData);
        assert_eq!(reader.buffer_len(), LENGTH_PREFIX_BYTES);
    }

    #[test]
    fn one_byte_beyond_the_buffered_limit_is_rejected_by_append() {
        let (mut ours, mut theirs) = socket_pair();
        let mut reader = MessageReader::new_with_payload_limit(TEST_LIMIT);
        theirs.write_all(&[0x7f; TEST_LIMIT + 5]).unwrap();

        let error = reader.read_from_socket(&mut ours).unwrap_err();
        assert_eq!(error.kind(), ErrorKind::InvalidData);
        assert_eq!(reader.buffer_len(), 0);
    }

    #[test]
    fn a_fragmented_header_and_payload_emit_only_the_complete_frame() {
        let (mut ours, mut theirs) = socket_pair();
        let mut reader = MessageReader::new_with_payload_limit(TEST_LIMIT);
        let bytes = frame(b"hello world");

        theirs.write_all(&bytes[..2]).unwrap();
        reader.read_from_socket(&mut ours).unwrap();
        assert_eq!(reader.buffer_len(), 2);
        assert!(reader.extract_message().unwrap().is_none());

        theirs.write_all(&bytes[2..6]).unwrap();
        reader.read_from_socket(&mut ours).unwrap();
        assert_eq!(reader.buffer_len(), 6);
        assert!(reader.extract_message().unwrap().is_none());

        theirs.write_all(&bytes[6..]).unwrap();
        reader.read_from_socket(&mut ours).unwrap();
        assert_eq!(reader.buffer_len(), bytes.len());
        let message = reader.extract_message().unwrap().unwrap();
        assert_eq!(message.get_data(), bytes);
        assert_eq!(reader.buffer_len(), 0);
    }

    #[test]
    fn coalesced_frames_are_extracted_in_order() {
        let (mut ours, mut theirs) = socket_pair();
        let mut reader = MessageReader::new_with_payload_limit(TEST_LIMIT);
        let frames = [frame(b"one"), frame(b"two"), frame(b"three")];
        let wire: Vec<u8> = frames.iter().flatten().copied().collect();
        theirs.write_all(&wire).unwrap();

        reader.read_from_socket(&mut ours).unwrap();
        for expected in &frames {
            let message = reader.extract_message().unwrap().unwrap();
            assert_eq!(message.get_data(), *expected);
        }
        assert!(reader.extract_message().unwrap().is_none());
        assert_eq!(reader.buffer_len(), 0);
    }

    #[test]
    fn a_near_limit_frame_then_another_frame_are_both_emitted() {
        let (mut ours, mut theirs) = socket_pair();
        let mut reader = MessageReader::new_with_payload_limit(TEST_LIMIT);
        let near_limit = frame(&[0x11; TEST_LIMIT - 2]);
        let small = frame(b"tail");

        theirs.write_all(&near_limit).unwrap();
        reader.read_from_socket(&mut ours).unwrap();
        assert_eq!(reader.extract_message().unwrap().unwrap().get_data(), near_limit);
        assert_eq!(reader.buffer_len(), 0);

        theirs.write_all(&small).unwrap();
        reader.read_from_socket(&mut ours).unwrap();
        assert_eq!(reader.extract_message().unwrap().unwrap().get_data(), small);
        assert_eq!(reader.buffer_len(), 0);
    }

    #[test]
    fn a_slow_incomplete_frame_emits_nothing() {
        let (mut ours, mut theirs) = socket_pair();
        let mut reader = MessageReader::new_with_payload_limit(TEST_LIMIT);
        let mut bytes = (20u32).to_le_bytes().to_vec();
        bytes.extend_from_slice(b"partial");

        theirs.write_all(&bytes).unwrap();
        reader.read_from_socket(&mut ours).unwrap();
        assert_eq!(reader.buffer_len(), bytes.len());
        assert!(reader.extract_message().unwrap().is_none());
        assert_eq!(reader.buffer_len(), bytes.len());
    }

    #[test]
    fn eof_after_a_complete_frame_leaves_the_frame_buffered() {
        let (mut ours, mut theirs) = socket_pair();
        let mut reader = MessageReader::new_with_payload_limit(TEST_LIMIT);
        let bytes = frame(b"reply");
        theirs.write_all(&bytes).unwrap();
        drop(theirs);

        reader.read_from_socket(&mut ours).unwrap();
        let error = reader.read_from_socket(&mut ours).unwrap_err();
        assert_eq!(error.kind(), ErrorKind::UnexpectedEof);
        let message = reader.extract_message().unwrap().unwrap();
        assert_eq!(message.get_data(), bytes);
        assert_eq!(reader.buffer_len(), 0);
    }
}
