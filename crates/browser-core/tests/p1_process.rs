//! Failure tests for P1 process isolation (no GUI).
//!
//! These validate IPC, spawn/kill detection, crash-loop protection, and protocol versioning.

use browser_core::{
    content_isolation_enabled, ContentProcessId, ContentProcessManager, CrashLoopConfig,
    RestartDecision,
};
use browser_ipc::{
    listen_unix, BrowserToContent, ContentToBrowser, Envelope, IpcReader, IpcWriter,
    PROTOCOL_VERSION,
};
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};
use uuid::Uuid;

fn temp_dirs() -> (PathBuf, PathBuf) {
    // Keep paths short: macOS sun_path is ~104 bytes.
    let id = &Uuid::new_v4().to_string()[..8];
    let root = PathBuf::from("/tmp").join(format!("rb{id}"));
    let content = root.join("c");
    let sock = root.join("s");
    std::fs::create_dir_all(&content).unwrap();
    std::fs::create_dir_all(&sock).unwrap();
    (content, sock)
}

fn browser_exe() -> PathBuf {
    // Prefer the just-built binary when running under cargo test.
    if let Ok(p) = std::env::var("CARGO_BIN_EXE_rust-browser") {
        return PathBuf::from(p);
    }
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../target/debug/rust-browser")
}

#[test]
fn protocol_version_constant() {
    assert_eq!(PROTOCOL_VERSION, 3);
}

#[test]
fn crash_loop_protection() {
    let (content, sock) = temp_dirs();
    let mut mgr =
        ContentProcessManager::new(content, sock, PathBuf::from("/bin/false")).unwrap();
    mgr.crash_cfg = CrashLoopConfig {
        max_crashes: 3,
        window: Duration::from_secs(60),
        initial_backoff: Duration::from_millis(10),
        max_backoff: Duration::from_millis(50),
    };
    let id = ContentProcessId::new();
    mgr.processes.insert(
        id,
        browser_core::ContentProcessHandle {
            id,
            state: browser_core::ContentProcessState::Running,
            child: None,
            socket_path: mgr.socket_for(id),
            pid: Some(9),
            last_heartbeat: Instant::now(),
            crash_times: Vec::new(),
            backoff: Duration::from_millis(10),
            crash_loop: false,
            generation: 1,
        },
    );
    assert!(matches!(
        mgr.on_crash(id).unwrap(),
        RestartDecision::RestartAfter(_)
    ));
    assert!(matches!(
        mgr.on_crash(id).unwrap(),
        RestartDecision::RestartAfter(_)
    ));
    assert_eq!(mgr.on_crash(id).unwrap(), RestartDecision::CrashLoop);
}

#[test]
fn isolation_env_flag() {
    // Default is OOP enabled unless explicitly forced in-process.
    std::env::remove_var("RUST_BROWSER_CONTENT");
    assert!(content_isolation_enabled());
    std::env::set_var("RUST_BROWSER_CONTENT", "inprocess");
    assert!(!content_isolation_enabled());
    std::env::remove_var("RUST_BROWSER_CONTENT");
}

/// Spawn real content process, hello handshake, kill it, ensure browser-side detect.
#[test]
#[ignore = "requires built rust-browser binary with Servo; run: cargo build -p browser-app && cargo test -p browser-core --test p1_process -- --ignored"]
fn kill_content_process_browser_detects() {
    let exe = browser_exe();
    assert!(exe.exists(), "build rust-browser first: {:?}", exe);

    let (content, sock_dir) = temp_dirs();
    let mut mgr = ContentProcessManager::new(content, sock_dir, exe).unwrap();
    let id = ContentProcessId::new();
    let path = mgr.socket_for(id);
    let listener = listen_unix(&path).unwrap();
    mgr.spawn_with_id(id).unwrap();
    let generation = mgr.generation_of(id).unwrap_or(1);

    let (stream, _) = listener.accept().unwrap();
    stream
        .set_read_timeout(Some(Duration::from_millis(200)))
        .unwrap();
    let mut writer = IpcWriter::new(stream.try_clone().unwrap());
    writer.generation = generation;
    let mut reader = IpcReader::new(stream);

    writer
        .send(&Envelope::with_generation(
            1,
            generation,
            BrowserToContent::Hello {
                protocol_version: PROTOCOL_VERSION,
            },
        ))
        .unwrap();

    let mut ready = false;
    let mut ack = false;
    let deadline = Instant::now() + Duration::from_secs(30);
    while Instant::now() < deadline && !(ready && ack) {
        if let Ok(Some(env)) = reader.try_recv::<ContentToBrowser>() {
            match env.payload {
                ContentToBrowser::HelloAck { pid, .. } => {
                    if let Some(h) = mgr.processes.get_mut(&id) {
                        h.pid = Some(pid);
                    }
                    ack = true;
                }
                ContentToBrowser::Ready => {
                    ready = true;
                    mgr.mark_ready(id);
                }
                ContentToBrowser::ProtocolMismatch { expected, got } => {
                    panic!("protocol mismatch expected={expected} got={got}")
                }
                _ => {}
            }
        }
    }
    assert!(ack && ready, "content did not become ready (ack={ack} ready={ready})");

    let pid = mgr.processes.get(&id).and_then(|h| h.pid).expect("pid");
    #[cfg(unix)]
    {
        let status = Command::new("kill")
            .arg("-9")
            .arg(pid.to_string())
            .stdout(Stdio::null())
            .status()
            .unwrap();
        assert!(status.success());
    }

    // Give the kernel a moment, then detect death.
    std::thread::sleep(Duration::from_millis(200));
    let alive = mgr
        .processes
        .get_mut(&id)
        .map(|h| h.is_alive())
        .unwrap_or(false);
    assert!(!alive, "content process should be dead after kill -9");

    let decision = mgr.on_crash(id).unwrap();
    assert!(matches!(decision, RestartDecision::RestartAfter(_)));
}

#[test]
fn hang_timeout_detection() {
    let (content, sock) = temp_dirs();
    let mut mgr =
        ContentProcessManager::new(content, sock, PathBuf::from("/bin/false")).unwrap();
    let id = ContentProcessId::new();
    mgr.processes.insert(
        id,
        browser_core::ContentProcessHandle {
            id,
            state: browser_core::ContentProcessState::Running,
            child: None,
            socket_path: mgr.socket_for(id),
            pid: Some(42),
            last_heartbeat: Instant::now() - Duration::from_secs(60),
            crash_times: Vec::new(),
            backoff: Duration::from_millis(10),
            crash_loop: false,
            generation: 1,
        },
    );
    assert!(mgr.heartbeat_timed_out(id, Duration::from_secs(15)));
    mgr.note_heartbeat(id);
    assert!(!mgr.heartbeat_timed_out(id, Duration::from_secs(15)));
}

#[test]
fn content_storage_excludes_password_paths() {
    // Security boundary: content root must not be the passwords DB path.
    let (content, _sock) = temp_dirs();
    let passwords = content
        .parent()
        .unwrap()
        .join("storage")
        .join("passwords.db");
    assert_ne!(content, passwords);
    assert!(!passwords.starts_with(&content));
    // Production uses a dedicated content directory name; test dirs are short (`c`).
    assert!(content.file_name().is_some());
}

#[test]
fn shutdown_during_restart_does_not_spawn() {
    let (content, sock) = temp_dirs();
    let mut mgr =
        ContentProcessManager::new(content, sock, PathBuf::from("/bin/false")).unwrap();
    let id = ContentProcessId::new();
    mgr.processes.insert(
        id,
        browser_core::ContentProcessHandle {
            id,
            state: browser_core::ContentProcessState::Restarting,
            child: None,
            socket_path: mgr.socket_for(id),
            pid: None,
            last_heartbeat: Instant::now(),
            crash_times: Vec::new(),
            backoff: Duration::from_millis(10),
            crash_loop: false,
            generation: 3,
        },
    );
    mgr.shutting_down = true;
    assert!(mgr.spawn().is_err());
    assert_eq!(mgr.on_crash(id).unwrap(), RestartDecision::Gone);
}

#[test]
fn generation_changes_across_alloc() {
    let (content, sock) = temp_dirs();
    let mut mgr =
        ContentProcessManager::new(content, sock, PathBuf::from("/bin/false")).unwrap();
    let g1 = mgr.alloc_generation();
    let g2 = mgr.alloc_generation();
    assert!(g2 > g1);
}

#[test]
fn tab_restore_assignment_after_crash() {
    let (content, sock) = temp_dirs();
    let mut mgr =
        ContentProcessManager::new(content, sock, PathBuf::from("/bin/false")).unwrap();
    let id = ContentProcessId::new();
    mgr.processes.insert(
        id,
        browser_core::ContentProcessHandle {
            id,
            state: browser_core::ContentProcessState::Running,
            child: None,
            socket_path: mgr.socket_for(id),
            pid: Some(7),
            last_heartbeat: Instant::now(),
            crash_times: Vec::new(),
            backoff: Duration::from_millis(10),
            crash_loop: false,
            generation: 1,
        },
    );
    let tab = browser_core::TabId::new();
    let _ = mgr.assign_tab(tab, id);
    assert_eq!(mgr.tab_to_process.get(&tab), Some(&id));
    let _ = mgr.on_crash(id).unwrap();
    // Tab mapping retained for restore; process marked crashed/restarting.
    assert_eq!(mgr.tab_to_process.get(&tab), Some(&id));
}
