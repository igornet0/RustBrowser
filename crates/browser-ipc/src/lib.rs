//! Framed, versioned IPC between Browser Process and Content Process.
//!
//! Transport: Unix domain stream (macOS/Linux). Messages are length-prefixed JSON.
//! Do not send Servo/`WebView`/`Rc` types — only serializable protocol messages.

mod protocol;
mod transport;
mod validate;

pub use protocol::{
    content_msg_tab, BrowserToContent, ContentToBrowser, Envelope, FrameBuffer, InputEventMsg,
    NetworkRouteMsg, PROTOCOL_VERSION,
};
pub use transport::{connect_unix, listen_unix, IpcError, IpcReader, IpcWriter};
pub use validate::{
    checked_frame_bytes, validate_browser_to_content, validate_content_to_browser,
    clamp_ipc_frame_size, validate_envelope_meta, validate_frame, InboundOrder, ValidationError,
    MAX_FRAME_BYTES, MAX_FRAME_HEIGHT, MAX_FRAME_WIDTH, MAX_IPC_FRAME_PIXELS, MAX_MESSAGE_SIZE,
    MAX_STRING_LENGTH, MAX_URL_LENGTH,
};

#[cfg(test)]
mod tests_protocol;
