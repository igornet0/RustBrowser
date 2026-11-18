//! Framed, versioned IPC between Browser Process and Content Process.
//!
//! Transport: Unix domain stream (macOS/Linux). Messages are length-prefixed JSON.
//! Do not send Servo/`WebView`/`Rc` types — only serializable protocol messages.

mod protocol;
pub mod session;
mod transport;
mod validate;

/// Base64 (de)serialization for raw frame bytes on the JSON wire.
pub(crate) mod bytes_b64 {
    use base64::{engine::general_purpose::STANDARD, Engine};
    use serde::{Deserialize, Deserializer, Serializer};

    pub fn serialize<S: Serializer>(bytes: &Vec<u8>, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&STANDARD.encode(bytes))
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(deserializer: D) -> Result<Vec<u8>, D::Error> {
        let encoded = String::deserialize(deserializer)?;
        STANDARD.decode(encoded).map_err(serde::de::Error::custom)
    }
}

pub use protocol::{
    content_msg_tab, BrowserToContent, ContentToBrowser, CookieInfo, Envelope, FrameBuffer,
    InputEventMsg, NetworkRequestMsg, NetworkRouteMsg, PROTOCOL_VERSION,
};
pub use session::RemoteContentSession;
pub use transport::{connect_unix, listen_unix, IpcError, IpcReader, IpcWriter};
pub use validate::{
    checked_frame_bytes, clamp_ipc_frame_size, validate_browser_to_content, validate_content_to_browser,
    validate_cookies, validate_envelope_meta, validate_frame, InboundOrder, ValidationError,
    MAX_COOKIES, MAX_COOKIE_DOMAIN, MAX_COOKIE_NAME, MAX_COOKIE_PATH, MAX_COOKIE_SAME_SITE,
    MAX_COOKIE_VALUE, MAX_FRAME_BYTES, MAX_FRAME_HEIGHT, MAX_FRAME_WIDTH, MAX_IPC_FRAME_PIXELS,
    MAX_MESSAGE_SIZE, MAX_NETWORK_LOG, MAX_SCRIPT_LENGTH, MAX_STRING_LENGTH, MAX_URL_LENGTH,
};

#[cfg(test)]
mod tests_protocol;
