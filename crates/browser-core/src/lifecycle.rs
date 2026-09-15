//! Formal Content Process lifecycle state machine (Browser-side).

use serde::{Deserialize, Serialize};

/// Content process lifecycle. Invalid transitions return `Err`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum LifecycleState {
    Starting,
    Ready,
    Running,
    Suspending,
    Suspended,
    Restarting,
    Dead,
    CrashLoopLocked,
    Shutdown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LifecycleEvent {
    Spawned,
    HelloOk,
    MarkRunning,
    Suspend,
    SuspendedAck,
    Resume,
    Crash,
    RestartScheduled,
    RestartBegin,
    CrashLoop,
    Shutdown,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TransitionError {
    pub from: LifecycleState,
    pub event: &'static str,
}

impl std::fmt::Display for TransitionError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "invalid lifecycle transition from {:?} on {}",
            self.from, self.event
        )
    }
}

impl std::error::Error for TransitionError {}

impl LifecycleState {
    pub fn transition(self, event: LifecycleEvent) -> Result<Self, TransitionError> {
        use LifecycleEvent as E;
        use LifecycleState as S;
        let next = match (self, event) {
            (S::Starting, E::HelloOk) => S::Ready,
            (S::Starting, E::Crash) => S::Dead,
            (S::Starting, E::Shutdown) => S::Shutdown,

            (S::Ready, E::MarkRunning) => S::Running,
            (S::Ready, E::Crash) => S::Dead,
            (S::Ready, E::Shutdown) => S::Shutdown,
            (S::Ready, E::Suspend) => S::Suspending,

            (S::Running, E::Suspend) => S::Suspending,
            (S::Running, E::Crash) => S::Dead,
            (S::Running, E::Shutdown) => S::Shutdown,

            (S::Suspending, E::SuspendedAck) => S::Suspended,
            (S::Suspending, E::Crash) => S::Dead,
            (S::Suspending, E::Shutdown) => S::Shutdown,

            (S::Suspended, E::Resume) => S::Running,
            (S::Suspended, E::Crash) => S::Dead,
            (S::Suspended, E::Shutdown) => S::Shutdown,

            (S::Dead, E::RestartScheduled) => S::Restarting,
            (S::Dead, E::CrashLoop) => S::CrashLoopLocked,
            (S::Dead, E::Shutdown) => S::Shutdown,

            (S::Restarting, E::Spawned) => S::Starting,
            (S::Restarting, E::CrashLoop) => S::CrashLoopLocked,
            (S::Restarting, E::Shutdown) => S::Shutdown,

            (S::CrashLoopLocked, E::RestartBegin) => S::Restarting,
            (S::CrashLoopLocked, E::Shutdown) => S::Shutdown,

            (S::Shutdown, _) => {
                return Err(TransitionError {
                    from: self,
                    event: "after Shutdown",
                })
            }

            _ => {
                return Err(TransitionError {
                    from: self,
                    event: match event {
                        E::Spawned => "Spawned",
                        E::HelloOk => "HelloOk",
                        E::MarkRunning => "MarkRunning",
                        E::Suspend => "Suspend",
                        E::SuspendedAck => "SuspendedAck",
                        E::Resume => "Resume",
                        E::Crash => "Crash",
                        E::RestartScheduled => "RestartScheduled",
                        E::RestartBegin => "RestartBegin",
                        E::CrashLoop => "CrashLoop",
                        E::Shutdown => "Shutdown",
                    },
                })
            }
        };
        Ok(next)
    }

    pub fn is_terminal(self) -> bool {
        matches!(self, Self::Shutdown | Self::CrashLoopLocked)
    }

    pub fn allows_ipc(self) -> bool {
        matches!(
            self,
            Self::Ready | Self::Running | Self::Suspending | Self::Suspended
        )
    }
}

/// Soft resource limits for Content (application-level; OS sandbox still absent).
#[derive(Debug, Clone)]
pub struct ContentResourceLimits {
    pub max_tabs_per_content: usize,
    pub max_frame_width: u32,
    pub max_frame_height: u32,
    pub max_ipc_message_bytes: usize,
    pub max_restarts_in_window: u32,
    pub restart_window: std::time::Duration,
}

impl Default for ContentResourceLimits {
    fn default() -> Self {
        Self {
            max_tabs_per_content: 64,
            max_frame_width: 7680,
            max_frame_height: 4320,
            max_ipc_message_bytes: 16 * 1024 * 1024,
            max_restarts_in_window: 5,
            restart_window: std::time::Duration::from_secs(60),
        }
    }
}

impl ContentResourceLimits {
    pub fn os_memory_policy_implemented(&self) -> bool {
        false
    }

    pub fn os_cpu_policy_implemented(&self) -> bool {
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn happy_path_running() {
        let mut s = LifecycleState::Starting;
        s = s.transition(LifecycleEvent::HelloOk).unwrap();
        s = s.transition(LifecycleEvent::MarkRunning).unwrap();
        assert_eq!(s, LifecycleState::Running);
    }

    #[test]
    fn crash_restart_lockout() {
        let mut s = LifecycleState::Running;
        s = s.transition(LifecycleEvent::Crash).unwrap();
        s = s.transition(LifecycleEvent::RestartScheduled).unwrap();
        s = s.transition(LifecycleEvent::Spawned).unwrap();
        assert_eq!(s, LifecycleState::Starting);
        s = s.transition(LifecycleEvent::HelloOk).unwrap();
        s = s.transition(LifecycleEvent::MarkRunning).unwrap();
        s = s.transition(LifecycleEvent::Crash).unwrap();
        s = s.transition(LifecycleEvent::CrashLoop).unwrap();
        assert_eq!(s, LifecycleState::CrashLoopLocked);
    }

    #[test]
    fn invalid_transition_from_shutdown() {
        let s = LifecycleState::Shutdown;
        assert!(s.transition(LifecycleEvent::HelloOk).is_err());
    }

    #[test]
    fn cannot_mark_running_from_dead() {
        assert!(LifecycleState::Dead
            .transition(LifecycleEvent::MarkRunning)
            .is_err());
    }

    #[test]
    fn shutdown_during_restarting() {
        let mut s = LifecycleState::Restarting;
        s = s.transition(LifecycleEvent::Shutdown).unwrap();
        assert_eq!(s, LifecycleState::Shutdown);
        assert!(s.transition(LifecycleEvent::Spawned).is_err());
    }
}
