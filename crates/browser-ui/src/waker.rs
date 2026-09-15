use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use winit::event_loop::{EventLoop, EventLoopClosed, EventLoopProxy};

#[derive(Debug)]
pub struct WakerEvent;

/// Wakes the winit loop for Servo; ignores calls after [`Self::retire`] or loop shutdown.
#[derive(Clone)]
pub struct Waker {
    proxy: EventLoopProxy<WakerEvent>,
    alive: Arc<AtomicBool>,
}

impl Waker {
    pub fn new(event_loop: &EventLoop<WakerEvent>) -> Self {
        Self {
            proxy: event_loop.create_proxy(),
            alive: Arc::new(AtomicBool::new(true)),
        }
    }

    /// Stop sending wake events (call before tearing down the event loop).
    pub fn retire(&self) {
        self.alive.store(false, Ordering::Release);
    }
}

impl servo::EventLoopWaker for Waker {
    fn clone_box(&self) -> Box<dyn servo::EventLoopWaker> {
        Box::new(self.clone())
    }

    fn wake(&self) {
        if !self.alive.load(Ordering::Acquire) {
            return;
        }
        if let Err(EventLoopClosed(_)) = self.proxy.send_event(WakerEvent) {
            self.alive.store(false, Ordering::Release);
        }
    }
}
