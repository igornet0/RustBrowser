//! In-process hang detection.
//!
//! A background thread watches heartbeats from the UI/engine loop.
//! This is **not** process isolation — if the whole process freezes, recovery
//! is limited to exit + session restore. See `docs/P0_LIMITATIONS.md`.

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::thread::{self, JoinHandle};
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tracing::{error, warn};

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

#[derive(Debug, Clone)]
pub struct WatchdogConfig {
    /// No heartbeat for this long → suspected hang (log / flag).
    pub suspected_after: Duration,
    /// No heartbeat for this long → confirmed hang.
    pub confirmed_after: Duration,
    /// After confirmed hang, exit process so session restore can run (last resort).
    pub exit_after_confirmed: Duration,
    pub poll_interval: Duration,
}

impl Default for WatchdogConfig {
    fn default() -> Self {
        Self {
            // Conservative: slow sites / cold start should not trip this.
            suspected_after: Duration::from_secs(20),
            confirmed_after: Duration::from_secs(60),
            exit_after_confirmed: Duration::from_secs(15),
            poll_interval: Duration::from_secs(1),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WatchdogStatus {
    Healthy,
    SuspectedHang,
    ConfirmedHang,
}

pub struct Watchdog {
    last_beat_ms: Arc<AtomicU64>,
    suspected: Arc<AtomicBool>,
    confirmed: Arc<AtomicBool>,
    stop: Arc<AtomicBool>,
    config: WatchdogConfig,
    handle: Option<JoinHandle<()>>,
}

impl Watchdog {
    pub fn start(config: WatchdogConfig) -> Self {
        let last_beat_ms = Arc::new(AtomicU64::new(now_ms()));
        let suspected = Arc::new(AtomicBool::new(false));
        let confirmed = Arc::new(AtomicBool::new(false));
        let stop = Arc::new(AtomicBool::new(false));

        let last_c = last_beat_ms.clone();
        let sus_c = suspected.clone();
        let conf_c = confirmed.clone();
        let stop_c = stop.clone();
        let cfg = config.clone();

        let handle = thread::Builder::new()
            .name("browser-watchdog".into())
            .spawn(move || {
                let mut confirmed_at: Option<std::time::Instant> = None;
                while !stop_c.load(Ordering::Relaxed) {
                    thread::sleep(cfg.poll_interval);
                    let last = last_c.load(Ordering::Relaxed);
                    let age_ms = now_ms().saturating_sub(last);
                    let age = Duration::from_millis(age_ms);

                    if age >= cfg.confirmed_after {
                        if !conf_c.swap(true, Ordering::SeqCst) {
                            error!(
                                age_ms,
                                "watchdog confirmed hang (in-process; not process isolation)"
                            );
                            confirmed_at = Some(std::time::Instant::now());
                        }
                        if let Some(at) = confirmed_at {
                            if at.elapsed() >= cfg.exit_after_confirmed {
                                error!(
                                    "watchdog forcing process exit after confirmed hang — rely on session checkpoint"
                                );
                                // Hard last resort: main loop cannot recover if blocked.
                                std::process::exit(78);
                            }
                        }
                    } else if age >= cfg.suspected_after {
                        if !sus_c.swap(true, Ordering::SeqCst) {
                            warn!(age_ms, "watchdog suspected hang");
                        }
                    } else {
                        sus_c.store(false, Ordering::Relaxed);
                        conf_c.store(false, Ordering::Relaxed);
                        confirmed_at = None;
                    }
                }
            })
            .ok();

        Self {
            last_beat_ms,
            suspected,
            confirmed,
            stop,
            config,
            handle,
        }
    }

    pub fn beat(&self) {
        self.last_beat_ms.store(now_ms(), Ordering::Relaxed);
    }

    pub fn status(&self) -> WatchdogStatus {
        if self.confirmed.load(Ordering::Relaxed) {
            WatchdogStatus::ConfirmedHang
        } else if self.suspected.load(Ordering::Relaxed) {
            WatchdogStatus::SuspectedHang
        } else {
            WatchdogStatus::Healthy
        }
    }

    pub fn config(&self) -> &WatchdogConfig {
        &self.config
    }

    pub fn stop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
    }
}

impl Drop for Watchdog {
    fn drop(&mut self) {
        self.stop();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn beat_keeps_healthy() {
        let wd = Watchdog::start(WatchdogConfig {
            suspected_after: Duration::from_secs(30),
            confirmed_after: Duration::from_secs(60),
            exit_after_confirmed: Duration::from_secs(3600),
            poll_interval: Duration::from_millis(50),
        });
        wd.beat();
        assert_eq!(wd.status(), WatchdogStatus::Healthy);
    }
}
