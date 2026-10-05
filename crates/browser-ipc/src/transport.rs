use crate::protocol::{Envelope, PROTOCOL_VERSION};
use crate::validate::{
    validate_browser_to_content, validate_content_to_browser, validate_envelope_meta,
    MAX_MESSAGE_SIZE,
};
use crate::BrowserToContent;
use crate::ContentToBrowser;
use serde::{de::DeserializeOwned, Serialize};
use std::io::{Read, Write};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::Path;
use std::time::Duration;
use thiserror::Error;
use tracing::{error, info};

#[derive(Debug, Error)]
pub enum IpcError {
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    #[error("serialize: {0}")]
    Serde(#[from] serde_json::Error),
    #[error("protocol version mismatch: expected {expected}, got {got}")]
    VersionMismatch { expected: u32, got: u32 },
    #[error("message too large: {0} bytes")]
    TooLarge(usize),
    #[error("validation: {0}")]
    Validation(String),
    #[error("disconnected")]
    Disconnected,
}

pub fn listen_unix(path: &Path) -> Result<UnixListener, IpcError> {
    // Darwin `sockaddr_un.sun_path` is 104 bytes (incl. NUL). Fail early with context.
    const MAX_UNIX_PATH: usize = 103;
    let path_bytes = path.as_os_str().as_encoded_bytes();
    if path_bytes.len() > MAX_UNIX_PATH {
        return Err(IpcError::Validation(format!(
            "unix socket path too long ({} > {MAX_UNIX_PATH}): {}",
            path_bytes.len(),
            path.display()
        )));
    }
    if path.exists() {
        let _ = std::fs::remove_file(path);
    }
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let listener = UnixListener::bind(path)?;
    info!(path = %path.display(), "ipc listen");
    Ok(listener)
}

pub fn connect_unix(path: &Path) -> Result<UnixStream, IpcError> {
    let stream = UnixStream::connect(path)?;
    stream.set_read_timeout(Some(Duration::from_millis(250)))?;
    stream.set_write_timeout(Some(Duration::from_secs(5)))?;
    info!(path = %path.display(), "ipc connected");
    Ok(stream)
}

pub struct IpcWriter {
    stream: UnixStream,
    pub generation: u64,
}

pub struct IpcReader {
    stream: UnixStream,
    /// When set, inbound envelopes must match this generation.
    pub expected_generation: Option<u64>,
}

impl IpcWriter {
    pub fn new(stream: UnixStream) -> Self {
        Self {
            stream,
            generation: 0,
        }
    }

    pub fn send<T: Serialize>(&mut self, env: &Envelope<T>) -> Result<(), IpcError> {
        if env.version != PROTOCOL_VERSION {
            return Err(IpcError::VersionMismatch {
                expected: PROTOCOL_VERSION,
                got: env.version,
            });
        }
        let bytes = serde_json::to_vec(env)?;
        if bytes.len() > MAX_MESSAGE_SIZE {
            return Err(IpcError::TooLarge(bytes.len()));
        }
        let len = (bytes.len() as u32).to_le_bytes();
        self.stream.write_all(&len)?;
        self.stream.write_all(&bytes)?;
        self.stream.flush()?;
        Ok(())
    }

    pub fn send_payload<T: Serialize>(
        &mut self,
        request_id: u64,
        sequence: u64,
        payload: T,
    ) -> Result<(), IpcError> {
        self.send(&Envelope::full(
            request_id,
            self.generation,
            sequence,
            payload,
        ))
    }

    pub fn try_clone_reader(&self) -> Result<IpcReader, IpcError> {
        Ok(IpcReader {
            stream: self.stream.try_clone()?,
            expected_generation: Some(self.generation).filter(|g| *g > 0),
        })
    }
}

impl IpcReader {
    pub fn new(stream: UnixStream) -> Self {
        Self {
            stream,
            expected_generation: None,
        }
    }

    pub fn recv<T: DeserializeOwned>(&mut self) -> Result<Envelope<T>, IpcError> {
        let mut len_buf = [0u8; 4];
        read_exact_retry(&mut self.stream, &mut len_buf)?;
        let len = u32::from_le_bytes(len_buf) as usize;
        if len > MAX_MESSAGE_SIZE {
            return Err(IpcError::TooLarge(len));
        }
        if len == 0 {
            return Err(IpcError::Validation("empty message".into()));
        }
        let mut buf = vec![0u8; len];
        read_exact_retry(&mut self.stream, &mut buf)?;
        let env: Envelope<T> = serde_json::from_slice(&buf)?;
        validate_envelope_meta(&env, self.expected_generation)?;
        Ok(env)
    }

    /// Non-blocking-ish recv: returns None on timeout / WouldBlock.
    pub fn try_recv<T: DeserializeOwned>(&mut self) -> Result<Option<Envelope<T>>, IpcError> {
        match self.recv() {
            Ok(env) => Ok(Some(env)),
            Err(IpcError::Io(e))
                if e.kind() == std::io::ErrorKind::WouldBlock
                    || e.kind() == std::io::ErrorKind::TimedOut =>
            {
                Ok(None)
            }
            Err(IpcError::Io(e)) if e.kind() == std::io::ErrorKind::UnexpectedEof => {
                Err(IpcError::Disconnected)
            }
            Err(e) => Err(e),
        }
    }

    pub fn recv_browser_to_content(&mut self) -> Result<Envelope<BrowserToContent>, IpcError> {
        let env = self.recv::<BrowserToContent>()?;
        validate_browser_to_content(&env.payload)?;
        Ok(env)
    }

    pub fn recv_content_to_browser(&mut self) -> Result<Envelope<ContentToBrowser>, IpcError> {
        let env = self.recv::<ContentToBrowser>()?;
        validate_content_to_browser(&env.payload)?;
        Ok(env)
    }

    pub fn try_recv_content_to_browser(
        &mut self,
    ) -> Result<Option<Envelope<ContentToBrowser>>, IpcError> {
        match self.try_recv::<ContentToBrowser>()? {
            Some(env) => {
                validate_content_to_browser(&env.payload)?;
                Ok(Some(env))
            }
            None => Ok(None),
        }
    }
}

fn read_exact_retry(stream: &mut UnixStream, buf: &mut [u8]) -> Result<(), IpcError> {
    match stream.read_exact(buf) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => {
            error!("ipc eof");
            Err(IpcError::Disconnected)
        }
        Err(e) => Err(IpcError::Io(e)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::{BrowserToContent, ContentToBrowser};
    use crate::validate::MAX_MESSAGE_SIZE;
    use tempfile::tempdir;

    #[test]
    fn roundtrip_hello() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("sock");
        let listener = listen_unix(&path).unwrap();
        let client = std::thread::spawn({
            let path = path.clone();
            move || {
                let stream = connect_unix(&path).unwrap();
                let mut w = IpcWriter::new(stream.try_clone().unwrap());
                w.generation = 1;
                let mut r = IpcReader::new(stream);
                r.expected_generation = Some(1);
                w.send(&Envelope::with_generation(
                    1,
                    1,
                    BrowserToContent::Hello {
                        protocol_version: PROTOCOL_VERSION,
                    },
                ))
                .unwrap();
                let env: Envelope<ContentToBrowser> = r.recv().unwrap();
                matches!(env.payload, ContentToBrowser::HelloAck { .. })
            }
        });
        let (stream, _) = listener.accept().unwrap();
        let mut r = IpcReader::new(stream.try_clone().unwrap());
        r.expected_generation = Some(1);
        let mut w = IpcWriter::new(stream);
        w.generation = 1;
        let env: Envelope<BrowserToContent> = r.recv().unwrap();
        assert!(matches!(env.payload, BrowserToContent::Hello { .. }));
        w.send(&Envelope::with_generation(
            env.request_id,
            1,
            ContentToBrowser::HelloAck {
                protocol_version: PROTOCOL_VERSION,
                pid: 1,
            },
        ))
        .unwrap();
        assert!(client.join().unwrap());
    }

    #[test]
    fn rejects_oversized_length_prefix() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("big.sock");
        let listener = listen_unix(&path).unwrap();
        let client = std::thread::spawn({
            let path = path.clone();
            move || {
                let mut stream = connect_unix(&path).unwrap();
                let len = ((MAX_MESSAGE_SIZE as u32) + 1).to_le_bytes();
                stream.write_all(&len).unwrap();
            }
        });
        let (stream, _) = listener.accept().unwrap();
        let mut r = IpcReader::new(stream);
        let err = r.recv::<BrowserToContent>().unwrap_err();
        assert!(matches!(err, IpcError::TooLarge(_)));
        let _ = client.join();
    }

    #[test]
    fn rejects_version_mismatch() {
        use std::io::Write;
        let mut env = Envelope::new(1, BrowserToContent::Heartbeat);
        env.version = PROTOCOL_VERSION + 99;
        let dir = tempdir().unwrap();
        let path = dir.path().join("ver.sock");
        let listener = listen_unix(&path).unwrap();
        let client = std::thread::spawn({
            let path = path.clone();
            let env = env.clone();
            move || {
                let mut stream = connect_unix(&path).unwrap();
                let bytes = serde_json::to_vec(&env).unwrap();
                let len = (bytes.len() as u32).to_le_bytes();
                stream.write_all(&len).unwrap();
                stream.write_all(&bytes).unwrap();
                stream.flush().unwrap();
            }
        });
        let (stream, _) = listener.accept().unwrap();
        let mut r = IpcReader::new(stream);
        let err = r.recv::<BrowserToContent>().unwrap_err();
        assert!(matches!(err, IpcError::VersionMismatch { .. }));
        let _ = client.join();
    }

    #[test]
    fn heartbeat_roundtrip_latency_sample() {
        use std::time::Instant;
        let dir = tempdir().unwrap();
        let path = dir.path().join("ipc.sock");
        let listener = listen_unix(&path).unwrap();
        let client = std::thread::spawn({
            let path = path.clone();
            move || {
                let stream = connect_unix(&path).unwrap();
                let mut w = IpcWriter::new(stream.try_clone().unwrap());
                w.generation = 1;
                let mut r = IpcReader::new(stream);
                r.expected_generation = Some(1);
                let mut samples = Vec::new();
                for i in 0..50u64 {
                    let t0 = Instant::now();
                    w.send(&Envelope::with_generation(i, 1, BrowserToContent::Heartbeat))
                        .unwrap();
                    let env: Envelope<ContentToBrowser> = r.recv().unwrap();
                    assert!(matches!(env.payload, ContentToBrowser::HeartbeatAck));
                    samples.push(t0.elapsed());
                }
                samples
            }
        });
        let (stream, _) = listener.accept().unwrap();
        let mut r = IpcReader::new(stream.try_clone().unwrap());
        r.expected_generation = Some(1);
        let mut w = IpcWriter::new(stream);
        w.generation = 1;
        for _ in 0..50 {
            let env: Envelope<BrowserToContent> = r.recv().unwrap();
            assert!(matches!(env.payload, BrowserToContent::Heartbeat));
            w.send(&Envelope::with_generation(
                env.request_id,
                1,
                ContentToBrowser::HeartbeatAck,
            ))
            .unwrap();
        }
        let samples = client.join().unwrap();
        let sum: std::time::Duration = samples.iter().copied().sum();
        let avg = sum / samples.len() as u32;
        let max = samples.iter().max().copied().unwrap();
        eprintln!(
            "IPC Heartbeat RTT n={} avg={:?} max={:?}",
            samples.len(),
            avg,
            max
        );
        assert!(avg < std::time::Duration::from_millis(50));
    }

    #[test]
    fn frame_ipc_throughput_sample() {
        use crate::protocol::FrameBuffer;
        use browser_core::TabId;
        use std::time::Instant;

        let sizes = [(800u32, 600u32), (1280, 720), (1920, 1080)];
        for (w, h) in sizes {
            let pixels = (w as usize) * (h as usize) * 4;
            let frame = FrameBuffer {
                width: w,
                height: h,
                rgba: vec![0x7F; pixels],
            };
            let payload = ContentToBrowser::Frame {
                tab_id: TabId::new(),
                frame: frame.clone(),
            };
            let env = Envelope::full(1, 1, 1, payload);
            let encoded = serde_json::to_vec(&env).unwrap();
            eprintln!(
                "Frame encode {w}x{h}: raw={} JSON={} ({:.1}x)",
                pixels,
                encoded.len(),
                encoded.len() as f64 / pixels as f64
            );
            if encoded.len() > MAX_MESSAGE_SIZE {
                eprintln!(
                    "Frame IPC {w}x{h}: SKIP send — JSON {} > MAX_MESSAGE_SIZE {MAX_MESSAGE_SIZE}",
                    encoded.len()
                );
                continue;
            }

            let dir = tempdir().unwrap();
            let path = dir.path().join(format!("f{w}x{h}.sock"));
            let listener = listen_unix(&path).unwrap();
            let n = 5u32;
            let client = std::thread::spawn({
                let path = path.clone();
                let frame = frame.clone();
                move || {
                    let stream = connect_unix(&path).unwrap();
                    stream.set_read_timeout(Some(Duration::from_secs(30))).unwrap();
                    stream.set_write_timeout(Some(Duration::from_secs(30))).unwrap();
                    let mut wri = IpcWriter::new(stream.try_clone().unwrap());
                    wri.generation = 1;
                    let mut r = IpcReader::new(stream);
                    r.expected_generation = Some(1);
                    let mut latencies = Vec::new();
                    let t_all = Instant::now();
                    for i in 0..n {
                        let t0 = Instant::now();
                        wri.send(&Envelope::full(
                            i as u64,
                            1,
                            i as u64 + 1,
                            ContentToBrowser::Frame {
                                tab_id: TabId::new(),
                                frame: frame.clone(),
                            },
                        ))
                        .unwrap();
                        let env: Envelope<BrowserToContent> = r.recv().unwrap();
                        assert!(matches!(env.payload, BrowserToContent::Heartbeat));
                        latencies.push(t0.elapsed());
                    }
                    (t_all.elapsed(), latencies, pixels)
                }
            });
            let (stream, _) = listener.accept().unwrap();
            stream.set_read_timeout(Some(Duration::from_secs(30))).unwrap();
            stream.set_write_timeout(Some(Duration::from_secs(30))).unwrap();
            let mut r = IpcReader::new(stream.try_clone().unwrap());
            r.expected_generation = Some(1);
            let mut wr = IpcWriter::new(stream);
            wr.generation = 1;
            for _ in 0..n {
                let env: Envelope<ContentToBrowser> = r.recv().unwrap();
                assert!(matches!(env.payload, ContentToBrowser::Frame { .. }));
                wr.send(&Envelope::with_generation(
                    env.request_id,
                    1,
                    BrowserToContent::Heartbeat,
                ))
                .unwrap();
            }
            let (total, latencies, pixels) = client.join().unwrap();
            let mut sorted = latencies.clone();
            sorted.sort();
            let p95 = sorted[sorted.len().saturating_sub(1).min((sorted.len() as f32 * 0.95) as usize)];
            let avg: std::time::Duration =
                latencies.iter().copied().sum::<std::time::Duration>() / n;
            let bytes_total = pixels * n as usize;
            let secs = total.as_secs_f64().max(1e-9);
            let mbps = (bytes_total as f64 / (1024.0 * 1024.0)) / secs;
            let fps = n as f64 / secs;
            eprintln!(
                "Frame IPC {w}x{h}: bytes/frame={pixels} fps≈{fps:.2} MB/s≈{mbps:.1} avg={avg:?} p95={p95:?} copies=JSON serde+socket"
            );
        }
    }

}
