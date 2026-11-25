//! Builder for the UploadDenied reply (peer code 50).

use crate::message::Message;

/// Tell a peer its queued request was refused: code 50 carries the filename
/// and a human-readable reason. `reason` must not be `Queued`, which receivers
/// treat as an acknowledgement rather than a failure.
#[must_use]
pub fn build_upload_denied(filename: &str, reason: &str) -> Message {
    Message::new()
        .write_int32(50)
        .write_string(filename)
        .write_string(reason)
        .clone()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::message::peer::UploadDeniedHandler;
    use crate::message::MessageHandler;

    #[test]
    fn an_upload_denial_carries_the_code_filename_and_reason() {
        let filename = "@@share\\f.mp3";
        let reason = "Upload queue full";
        let message = build_upload_denied(filename, reason);

        let mut expected = 50u32.to_le_bytes().to_vec();
        expected.extend_from_slice(&(filename.len() as u32).to_le_bytes());
        expected.extend_from_slice(filename.as_bytes());
        expected.extend_from_slice(&(reason.len() as u32).to_le_bytes());
        expected.extend_from_slice(reason.as_bytes());

        assert_eq!(&message.get_data()[..4], &50u32.to_le_bytes());
        assert_eq!(message.get_data(), expected);
        let mut length = (expected.len() as u32).to_le_bytes().to_vec();
        length.extend(expected);
        assert_eq!(message.get_buffer(), length);
    }

    #[test]
    fn the_reason_makes_the_downloader_see_a_failure() {
        let (tx, rx) = std::sync::mpsc::channel();
        let mut message =
            Message::new_with_data(build_upload_denied("f.mp3", "Upload queue full").get_buffer());
        message.set_pointer(8);

        UploadDeniedHandler.handle(&mut message, tx);
        match rx.try_recv() {
            Ok(crate::peer::PeerMessage::UploadFailed(_, filename)) => {
                assert_eq!(filename, "f.mp3");
            }
            other => panic!("a full queue must fail the download, got {other:?}"),
        }
    }
}
