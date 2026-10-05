use browser_core::BrowserResult;
use browser_engine::{run_content_process, ContentOptions};
use std::path::PathBuf;

fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| {
                "info,browser_core=debug,browser_engine=debug,browser_ui=debug,browser_profile=debug,browser_ipc=debug"
                    .into()
            }),
        )
        .init();

    // Required by Servo's rustls networking stack.
    if let Err(err) = rustls::crypto::aws_lc_rs::default_provider().install_default() {
        tracing::error!(?err, "failed to install rustls crypto provider");
        std::process::exit(1);
    }

    let args: Vec<String> = std::env::args().collect();
    if let Some(socket) = content_process_socket(&args) {
        let storage = content_storage(&args);
        let host_file = content_host_file(&args);
        let options = ContentOptions {
            automation: args
                .iter()
                .any(|a| a == browser_automation::AUTOMATION_CONTENT_FLAG),
        };
        tracing::info!(?socket, ?storage, ?host_file, ?options, "content process entry");
        if let Err(err) = run_content_process(&socket, storage, host_file, options) {
            tracing::error!(error = %err, "content process exited with error");
            std::process::exit(1);
        }
        return;
    }

    if let Some(addr) = arg_value(&args, "--automation-server") {
        if let Err(err) = run_automation(&args, &addr) {
            tracing::error!(error = %err, "automation server exited with error");
            std::process::exit(1);
        }
        return;
    }

    tracing::info!("browser process startup");
    if let Err(err) = run_browser() {
        tracing::error!(error = %err, "browser exited with error");
        std::process::exit(1);
    }
}

fn run_browser() -> BrowserResult<()> {
    browser_ui::run()
}

/// `--automation-server <addr> [--automation-token <t>] [--automation-max-tabs <n>]
/// [--automation-data <dir>] [--content-host-file <path>]`: headless HTTP API, no window.
fn run_automation(args: &[String], addr: &str) -> Result<(), String> {
    let addr = addr
        .parse()
        .map_err(|e| format!("--automation-server {addr}: {e}"))?;
    let token = arg_value(args, "--automation-token")
        .or_else(|| std::env::var("RUST_BROWSER_AUTOMATION_TOKEN").ok())
        .filter(|t| !t.trim().is_empty());
    let max_tabs = arg_value(args, "--automation-max-tabs")
        .and_then(|v| v.parse().ok())
        .unwrap_or(4);
    let pid = std::process::id();
    let data_dir = arg_value(args, "--automation-data")
        .map(PathBuf::from)
        .unwrap_or_else(|| std::env::temp_dir().join(format!("rust-browser-automation-{pid}")));
    let exe = std::env::current_exe().map_err(|e| format!("current_exe: {e}"))?;
    browser_automation::run(browser_automation::AutomationConfig {
        addr,
        token,
        exe,
        data_dir,
        // Unix socket paths are limited to ~104 bytes on macOS.
        socket_dir: PathBuf::from("/tmp").join(format!("rb-auto-{pid}")),
        host_file: content_host_file(args),
        max_tabs,
        max_queue: 64,
    })
}

fn arg_value(args: &[String], flag: &str) -> Option<String> {
    args.windows(2)
        .find_map(|w| (w[0] == flag).then(|| w[1].clone()))
}

fn content_process_socket(args: &[String]) -> Option<PathBuf> {
    args.windows(2).find_map(|w| {
        if w[0] == "--content-process" {
            Some(PathBuf::from(&w[1]))
        } else {
            None
        }
    })
}

fn content_storage(args: &[String]) -> Option<PathBuf> {
    args.windows(2).find_map(|w| {
        if w[0] == "--content-storage" {
            Some(PathBuf::from(&w[1]))
        } else {
            None
        }
    })
}

fn content_host_file(args: &[String]) -> Option<PathBuf> {
    args.windows(2).find_map(|w| {
        if w[0] == "--content-host-file" {
            Some(PathBuf::from(&w[1]))
        } else {
            None
        }
    })
}
