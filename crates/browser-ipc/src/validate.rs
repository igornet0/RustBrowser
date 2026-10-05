//! IPC limits and fail-closed validation.

use crate::protocol::{
    BrowserToContent, ContentToBrowser, Envelope, FrameBuffer, InputEventMsg, PROTOCOL_VERSION,
};
use crate::IpcError;

/// Maximum framed message body (JSON). Large enough for 1920×1080 RGBA (~8 MiB)
/// plus JSON overhead; not unlimited.
pub const MAX_MESSAGE_SIZE: usize = 16 * 1024 * 1024;

/// Maximum accepted raw frame byte length (`width * height * 4`).
pub const MAX_FRAME_BYTES: usize = 1920 * 1080 * 4 * 2; // ~16 MiB headroom (2× FHD)

pub const MAX_FRAME_WIDTH: u32 = 7680;
pub const MAX_FRAME_HEIGHT: u32 = 4320;

/// Safe pixel budget for JSON-encoded `FrameBuffer` (`Vec<u8>` → number array ≈ 3–4×).
/// Keep raw RGBA under ~¼ of [`MAX_MESSAGE_SIZE`] so encode+send does not stall Content.
pub const MAX_IPC_FRAME_PIXELS: u32 = (MAX_MESSAGE_SIZE / 4 / 4) as u32; // 1_048_576

/// Scale `width`×`height` down uniformly so `w*h <= MAX_IPC_FRAME_PIXELS`.
pub fn clamp_ipc_frame_size(width: u32, height: u32) -> (u32, u32) {
    let w = width.max(1).min(MAX_FRAME_WIDTH);
    let h = height.max(1).min(MAX_FRAME_HEIGHT);
    let pixels = (w as u64).saturating_mul(h as u64);
    if pixels <= u64::from(MAX_IPC_FRAME_PIXELS) {
        return (w, h);
    }
    let scale = (f64::from(MAX_IPC_FRAME_PIXELS) / pixels as f64).sqrt();
    let nw = ((f64::from(w) * scale).floor() as u32).max(1);
    let nh = ((f64::from(h) * scale).floor() as u32).max(1);
    (nw, nh)
}

pub const MAX_URL_LENGTH: usize = 8 * 1024;
pub const MAX_STRING_LENGTH: usize = 16 * 1024;
pub const MAX_TITLE_LENGTH: usize = 2 * 1024;
pub const MAX_CONSOLE_LENGTH: usize = 8 * 1024;
pub const MAX_KEY_LENGTH: usize = 128;
pub const MAX_TEXT_INPUT_LENGTH: usize = 4 * 1024;

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum ValidationError {
    #[error("protocol version mismatch: expected {expected}, got {got}")]
    VersionMismatch { expected: u32, got: u32 },
    #[error("generation mismatch: expected {expected}, got {got}")]
    GenerationMismatch { expected: u64, got: u64 },
    #[error("stale sequence: got {got}, last {last}")]
    StaleSequence { got: u64, last: u64 },
    #[error("message too large: {0} bytes")]
    TooLarge(usize),
    #[error("invalid frame: {0}")]
    InvalidFrame(String),
    #[error("invalid string: {0}")]
    InvalidString(String),
    #[error("invalid dimensions: {0}")]
    InvalidDimensions(String),
    #[error("invalid payload: {0}")]
    InvalidPayload(String),
}

impl From<ValidationError> for IpcError {
    fn from(value: ValidationError) -> Self {
        match value {
            ValidationError::VersionMismatch { expected, got } => {
                IpcError::VersionMismatch { expected, got }
            }
            ValidationError::TooLarge(n) => IpcError::TooLarge(n),
            other => IpcError::Validation(other.to_string()),
        }
    }
}

pub fn checked_frame_bytes(width: u32, height: u32) -> Result<usize, ValidationError> {
    if width == 0 || height == 0 {
        return Err(ValidationError::InvalidDimensions(
            "width/height must be non-zero".into(),
        ));
    }
    if width > MAX_FRAME_WIDTH || height > MAX_FRAME_HEIGHT {
        return Err(ValidationError::InvalidDimensions(format!(
            "{width}x{height} exceeds {MAX_FRAME_WIDTH}x{MAX_FRAME_HEIGHT}"
        )));
    }
    let pixels = (width as u64)
        .checked_mul(height as u64)
        .ok_or_else(|| ValidationError::InvalidFrame("pixel count overflow".into()))?;
    let bytes = pixels
        .checked_mul(4)
        .ok_or_else(|| ValidationError::InvalidFrame("byte count overflow".into()))?;
    if bytes > MAX_FRAME_BYTES as u64 {
        return Err(ValidationError::InvalidFrame(format!(
            "frame {bytes} bytes exceeds MAX_FRAME_BYTES"
        )));
    }
    Ok(bytes as usize)
}

pub fn validate_frame(frame: &FrameBuffer) -> Result<(), ValidationError> {
    let expected = checked_frame_bytes(frame.width, frame.height)?;
    if frame.rgba.is_empty() {
        // Placeholder frames allowed (no pixels yet).
        return Ok(());
    }
    if frame.rgba.len() != expected {
        return Err(ValidationError::InvalidFrame(format!(
            "rgba len {} != width*height*4 ({expected})",
            frame.rgba.len()
        )));
    }
    Ok(())
}

fn check_url(url: &url::Url) -> Result<(), ValidationError> {
    let s = url.as_str();
    if s.len() > MAX_URL_LENGTH {
        return Err(ValidationError::InvalidString(format!(
            "url length {} > {MAX_URL_LENGTH}",
            s.len()
        )));
    }
    Ok(())
}

fn check_str(label: &str, s: &str, max: usize) -> Result<(), ValidationError> {
    if s.len() > max {
        return Err(ValidationError::InvalidString(format!(
            "{label} length {} > {max}",
            s.len()
        )));
    }
    Ok(())
}

pub fn validate_envelope_meta<T>(
    env: &Envelope<T>,
    expected_generation: Option<u64>,
) -> Result<(), ValidationError> {
    if env.version != PROTOCOL_VERSION {
        return Err(ValidationError::VersionMismatch {
            expected: PROTOCOL_VERSION,
            got: env.version,
        });
    }
    if let Some(gen) = expected_generation {
        if env.generation != gen {
            return Err(ValidationError::GenerationMismatch {
                expected: gen,
                got: env.generation,
            });
        }
    }
    Ok(())
}

pub fn validate_browser_to_content(msg: &BrowserToContent) -> Result<(), ValidationError> {
    match msg {
        BrowserToContent::Hello { protocol_version } => {
            if *protocol_version != PROTOCOL_VERSION {
                return Err(ValidationError::VersionMismatch {
                    expected: PROTOCOL_VERSION,
                    got: *protocol_version,
                });
            }
        }
        BrowserToContent::CreateTab { url, .. } => {
            if let Some(u) = url {
                check_url(u)?;
            }
        }
        BrowserToContent::Navigate { url, .. } => check_url(url)?,
        BrowserToContent::Resize {
            width,
            height,
            scale_factor,
        }
        | BrowserToContent::SetViewport {
            width,
            height,
            scale_factor,
            ..
        } => {
            let _ = checked_frame_bytes((*width).max(1), (*height).max(1))?;
            if !scale_factor.is_finite() || *scale_factor <= 0.0 || *scale_factor > 8.0 {
                return Err(ValidationError::InvalidDimensions(
                    "scale_factor out of range".into(),
                ));
            }
        }
        BrowserToContent::Input { event, .. } => validate_input(event)?,
        BrowserToContent::SetNetworkRoute { mode } => match mode {
            crate::protocol::NetworkRouteMsg::Direct => {}
            crate::protocol::NetworkRouteMsg::Proxy {
                scheme,
                host,
                ..
            } => {
                check_str("proxy scheme", scheme, 32)?;
                check_str("proxy host", host, 256)?;
            }
            crate::protocol::NetworkRouteMsg::Unavailable { reason } => {
                check_str("unavailable reason", reason, MAX_STRING_LENGTH)?;
            }
        },
        _ => {}
    }
    Ok(())
}

pub fn validate_content_to_browser(msg: &ContentToBrowser) -> Result<(), ValidationError> {
    match msg {
        ContentToBrowser::HelloAck {
            protocol_version, ..
        } => {
            if *protocol_version != PROTOCOL_VERSION {
                return Err(ValidationError::VersionMismatch {
                    expected: PROTOCOL_VERSION,
                    got: *protocol_version,
                });
            }
        }
        ContentToBrowser::NavigationStarted { url, .. }
        | ContentToBrowser::NavigationFinished { url, .. }
        | ContentToBrowser::UrlChanged { url, .. } => check_url(url)?,
        ContentToBrowser::TitleChanged { title, .. } => {
            check_str("title", title, MAX_TITLE_LENGTH)?
        }
        ContentToBrowser::ConsoleMessage {
            level, message, ..
        } => {
            check_str("console level", level, 32)?;
            check_str("console message", message, MAX_CONSOLE_LENGTH)?;
        }
        ContentToBrowser::TabCrashed { reason, .. } | ContentToBrowser::Error { message: reason, .. } => {
            check_str("error", reason, MAX_STRING_LENGTH)?;
        }
        ContentToBrowser::Frame { frame, .. } => validate_frame(frame)?,
        ContentToBrowser::LoadProgress { progress, .. } => {
            if !(0.0..=1.0).contains(progress) && !progress.is_nan() {
                // Allow slightly out-of-range from engines; reject absurd values.
                if !progress.is_finite() || *progress < -0.1 || *progress > 1.1 {
                    return Err(ValidationError::InvalidPayload(
                        "load progress out of range".into(),
                    ));
                }
            }
        }
        _ => {}
    }
    Ok(())
}

fn validate_input(event: &InputEventMsg) -> Result<(), ValidationError> {
    match event {
        InputEventMsg::Key { key, .. } => check_str("key", key, MAX_KEY_LENGTH)?,
        InputEventMsg::Text { text } => check_str("text", text, MAX_TEXT_INPUT_LENGTH)?,
        InputEventMsg::MouseMove { .. }
        | InputEventMsg::MouseButton { .. }
        | InputEventMsg::MouseWheel { .. } => {}
    }
    Ok(())
}

/// Tracks content generation + per-tab sequences for Content→Browser ordering.
#[derive(Debug, Default)]
pub struct InboundOrder {
    pub generation: u64,
    /// Last accepted sequence per tab (0 = none).
    last_seq: std::collections::HashMap<browser_core::TabId, u64>,
    /// Global last sequence for messages without tab (Ready, HeartbeatAck, …).
    last_global_seq: u64,
}

impl InboundOrder {
    pub fn new(generation: u64) -> Self {
        Self {
            generation,
            last_seq: Default::default(),
            last_global_seq: 0,
        }
    }

    pub fn reset(&mut self, generation: u64) {
        self.generation = generation;
        self.last_seq.clear();
        self.last_global_seq = 0;
    }

    /// Returns Ok(true) if message should be applied; Ok(false) if silently dropped (duplicate/stale).
    pub fn accept(
        &mut self,
        env_generation: u64,
        sequence: u64,
        tab_id: Option<browser_core::TabId>,
    ) -> Result<bool, ValidationError> {
        if env_generation != self.generation {
            return Err(ValidationError::GenerationMismatch {
                expected: self.generation,
                got: env_generation,
            });
        }
        if sequence == 0 {
            // Unordered control messages (heartbeat, etc.).
            return Ok(true);
        }
        let last = match tab_id {
            Some(tab) => self.last_seq.entry(tab).or_insert(0),
            None => &mut self.last_global_seq,
        };
        if sequence <= *last {
            return Ok(false); // duplicate / stale
        }
        *last = sequence;
        Ok(true)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use browser_core::TabId;

    #[test]
    fn rejects_overflow_dimensions() {
        assert!(checked_frame_bytes(u32::MAX, u32::MAX).is_err());
    }

    #[test]
    fn rejects_mismatched_rgba_len() {
        let frame = FrameBuffer {
            width: 2,
            height: 2,
            rgba: vec![0; 7],
        };
        assert!(validate_frame(&frame).is_err());
    }

    #[test]
    fn accepts_placeholder_empty_rgba() {
        let frame = FrameBuffer::placeholder(100, 100);
        assert!(validate_frame(&frame).is_ok());
    }

    #[test]
    fn clamp_ipc_frame_keeps_small_sizes() {
        assert_eq!(clamp_ipc_frame_size(1280, 720), (1280, 720));
    }

    #[test]
    fn clamp_ipc_frame_caps_retina() {
        let (w, h) = clamp_ipc_frame_size(3024, 1800);
        assert!(u64::from(w) * u64::from(h) <= u64::from(MAX_IPC_FRAME_PIXELS));
        assert!(w < 3024);
        assert!(h < 1800);
        // Aspect roughly preserved.
        let aspect_in = 3024.0 / 1800.0;
        let aspect_out = f64::from(w) / f64::from(h);
        assert!((aspect_in - aspect_out).abs() < 0.05);
    }

    #[test]
    fn stale_generation_rejected() {
        let mut order = InboundOrder::new(7);
        let tab = TabId::new();
        assert!(order.accept(7, 1, Some(tab)).unwrap());
        assert!(matches!(
            order.accept(6, 2, Some(tab)),
            Err(ValidationError::GenerationMismatch { .. })
        ));
    }

    #[test]
    fn duplicate_sequence_dropped() {
        let mut order = InboundOrder::new(1);
        let tab = TabId::new();
        assert_eq!(order.accept(1, 5, Some(tab)).unwrap(), true);
        assert_eq!(order.accept(1, 5, Some(tab)).unwrap(), false);
        assert_eq!(order.accept(1, 4, Some(tab)).unwrap(), false);
        assert_eq!(order.accept(1, 6, Some(tab)).unwrap(), true);
    }

    #[test]
    fn reset_clears_sequences() {
        let mut order = InboundOrder::new(1);
        let tab = TabId::new();
        assert!(order.accept(1, 10, Some(tab)).unwrap());
        order.reset(2);
        assert!(order.accept(2, 1, Some(tab)).unwrap());
    }
}
