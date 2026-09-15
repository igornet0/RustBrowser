//! GL surface for egui in the Browser Process without embedding Servo.

use browser_core::{BrowserError, BrowserResult};
use servo::{RenderingContext, WindowRenderingContext};
use std::rc::Rc;
use std::sync::Arc;
use winit::dpi::PhysicalSize;
use winit::raw_window_handle::{DisplayHandle, WindowHandle};

pub struct UiSurface {
    window_context: Rc<WindowRenderingContext>,
}

impl UiSurface {
    pub fn new(
        display_handle: DisplayHandle<'_>,
        window_handle: WindowHandle<'_>,
        window_size: PhysicalSize<u32>,
    ) -> BrowserResult<Self> {
        let window_context = Rc::new(
            WindowRenderingContext::new(display_handle, window_handle, window_size)
                .map_err(|e| BrowserError::engine(format!("UiSurface: {e:?}")))?,
        );
        let _ = window_context.make_current();
        Ok(Self { window_context })
    }

    pub fn glow_context(&self) -> Arc<glow::Context> {
        self.window_context.glow_gl_api()
    }

    pub fn resize_window(&self, size: PhysicalSize<u32>) {
        let size = PhysicalSize::new(size.width.max(1), size.height.max(1));
        self.window_context.resize(size);
    }

    pub fn prepare_for_rendering(&self) {
        let _ = self.window_context.make_current();
        self.window_context.prepare_for_rendering();
    }

    pub fn present(&self) {
        self.window_context.present();
    }
}
