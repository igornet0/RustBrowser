use browser_core::BrowserResult;
use browser_engine::run_content_process;
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
        tracing::info!(?socket, ?storage, "content process entry");
        if let Err(err) = run_content_process(&socket, storage) {
            tracing::error!(error = %err, "content process exited with error");
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
