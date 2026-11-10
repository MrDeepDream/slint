// Copyright © SixtyFPS GmbH <info@slint.dev>
// SPDX-License-Identifier: GPL-3.0-only OR LicenseRef-Slint-Royalty-free-2.0 OR LicenseRef-Slint-Software-3.0

//! Delegate the rendering to the [`i_slint_renderer_software::SoftwareRenderer`]

use core::num::NonZeroU32;
use core::ops::DerefMut;
use i_slint_core::graphics::Rgb8Pixel;
use i_slint_core::platform::PlatformError;
use i_slint_core::renderer::DrawOutcome;
pub use i_slint_renderer_software::SoftwareRenderer;
use i_slint_renderer_software::{PremultipliedRgbaColor, RepaintBufferType, TargetPixel};
use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;
use winit::event_loop::ActiveEventLoop;

use super::WinitCompatibleRenderer;

pub struct WinitSoftwareRenderer {
    renderer: SoftwareRenderer,
    _context: RefCell<Option<softbuffer::Context<Arc<winit::window::Window>>>>,
    surface: RefCell<
        Option<softbuffer::Surface<Arc<winit::window::Window>, Arc<winit::window::Window>>>,
    >,
}

#[repr(transparent)]
#[derive(Copy, Clone, bytemuck::Pod, bytemuck::Zeroable)]
struct SoftBufferPixel(pub u32);

impl From<SoftBufferPixel> for PremultipliedRgbaColor {
    #[inline]
    fn from(pixel: SoftBufferPixel) -> Self {
        let v = pixel.0;
        PremultipliedRgbaColor {
            red: (v >> 16) as u8,
            green: (v >> 8) as u8,
            blue: v as u8,
            alpha: (v >> 24) as u8,
        }
    }
}

impl From<PremultipliedRgbaColor> for SoftBufferPixel {
    #[inline]
    fn from(pixel: PremultipliedRgbaColor) -> Self {
        Self(
            (pixel.alpha as u32) << 24
                | ((pixel.red as u32) << 16)
                | ((pixel.green as u32) << 8)
                | (pixel.blue as u32),
        )
    }
}

impl TargetPixel for SoftBufferPixel {
    fn blend(&mut self, color: PremultipliedRgbaColor) {
        let mut x = PremultipliedRgbaColor::from(*self);
        x.blend(color);
        *self = x.into();
    }

    fn from_rgb(r: u8, g: u8, b: u8) -> Self {
        Self(0xff000000 | ((r as u32) << 16) | ((g as u32) << 8) | (b as u32))
    }

    fn background() -> Self {
        Self(0)
    }
}

impl WinitSoftwareRenderer {
    pub fn new_suspended(
        _shared_backend_data: &Rc<crate::SharedBackendData>,
    ) -> Result<Box<dyn WinitCompatibleRenderer>, PlatformError> {
        Ok(Box::new(Self {
            renderer: SoftwareRenderer::new(),
            _context: RefCell::new(None),
            surface: RefCell::new(None),
        }))
    }
}

impl super::WinitCompatibleRenderer for WinitSoftwareRenderer {
    fn render(&self, window: &i_slint_core::api::Window) -> Result<DrawOutcome, PlatformError> {
        let size = window.size();

        let Some((width, height)) = size.width.try_into().ok().zip(size.height.try_into().ok())
        else {
            // Nothing to render
            return Ok(DrawOutcome::Success);
        };

        let mut borrowed_surface = self.surface.borrow_mut();
        let Some(surface) = borrowed_surface.as_mut() else {
            // Nothing to render
            return Ok(DrawOutcome::Success);
        };

        let winit_window = surface.window().clone();

        surface
            .resize(width, height)
            .map_err(|e| format!("Error resizing softbuffer surface: {e}"))?;

        let mut target_buffer = surface
            .buffer_mut()
            .map_err(|e| format!("Error retrieving softbuffer rendering buffer: {e}"))?;

        let age = target_buffer.age();
        self.renderer.set_repaint_buffer_type(match age {
            1 => RepaintBufferType::ReusedBuffer,
            2 => RepaintBufferType::SwappedBuffers,
            _ => RepaintBufferType::NewBuffer,
        });

        let region = if std::env::var_os("SLINT_LINE_BY_LINE").is_none() {
            let buffer: &mut [SoftBufferPixel] =
                bytemuck::cast_slice_mut(target_buffer.deref_mut());
            self.renderer.render(buffer, width.get() as usize)
        } else {
            // SLINT_LINE_BY_LINE is set and this is a debug mode where we also render in a Rgb565Pixel
            struct FrameBuffer<'a> {
                buffer: &'a mut [u32],
                line: Vec<i_slint_renderer_software::Rgb565Pixel>,
            }
            impl i_slint_renderer_software::LineBufferProvider for FrameBuffer<'_> {
                type TargetPixel = i_slint_renderer_software::Rgb565Pixel;
                fn process_line(
                    &mut self,
                    line: usize,
                    range: core::ops::Range<usize>,
                    render_fn: impl FnOnce(&mut [Self::TargetPixel]),
                ) {
                    let line_begin = line * self.line.len();
                    let sub = &mut self.line[..range.len()];
                    render_fn(sub);
                    for (dst, src) in self.buffer[line_begin..][range].iter_mut().zip(sub) {
                        let p = Rgb8Pixel::from(*src);
                        *dst =
                            0xff000000 | ((p.r as u32) << 16) | ((p.g as u32) << 8) | (p.b as u32);
                    }
                }
            }
            self.renderer.render_by_line(FrameBuffer {
                buffer: &mut target_buffer,
                line: vec![Default::default(); width.get() as usize],
            })
        };

        #[allow(unused_mut)]
        let mut damage = region
            .iter()
            .filter_map(|(pos, size)| {
                Some(softbuffer::Rect {
                    x: pos.x as u32,
                    y: pos.y as u32,
                    width: NonZeroU32::new(size.width)?,
                    height: NonZeroU32::new(size.height)?,
                })
            })
            .collect::<Vec<_>>();
        // Windows can discard window contents on its own, for example the part that was outside
        // every monitor when a frame was presented.
        // It then asks for a repaint of that update region, and winit validates the region only
        // after `RedrawRequested`.
        // The buffer still holds the whole frame, so present that region along with the damage.
        #[cfg(target_os = "windows")]
        damage.extend(win32_update_rect(&winit_window, width, height));
        if !damage.is_empty() {
            winit_window.pre_present_notify();
            target_buffer
                .present_with_damage(&damage)
                .map_err(|e| format!("Error presenting softbuffer buffer: {e}"))?;
        }
        Ok(DrawOutcome::Success)
    }

    fn as_core_renderer(&self) -> &dyn i_slint_core::renderer::Renderer {
        &self.renderer
    }

    fn occluded(&self, _: bool) {
        // On X11 and Windows, the buffer is completely cleared when the window is hidden
        // and the buffer age doesn't respect that, so clean the partial rendering cache
        self.renderer.set_repaint_buffer_type(RepaintBufferType::NewBuffer);
    }

    fn resume(
        &self,
        active_event_loop: &ActiveEventLoop,
        window_attributes: winit::window::WindowAttributes,
        _window_adapter_weak: std::rc::Weak<crate::winitwindowadapter::WinitWindowAdapter>,
    ) -> Result<Arc<winit::window::Window>, PlatformError> {
        let winit_window =
            active_event_loop.create_window(window_attributes).map_err(|winit_os_error| {
                PlatformError::from(format!(
                    "Error creating native window for software rendering: {winit_os_error}"
                ))
            })?;
        let winit_window = Arc::new(winit_window);

        let context = softbuffer::Context::new(winit_window.clone())
            .map_err(|e| format!("Error creating softbuffer context: {e}"))?;

        let surface = softbuffer::Surface::new(&context, winit_window.clone()).map_err(
            |softbuffer_error| format!("Error creating softbuffer surface: {softbuffer_error}"),
        )?;

        *self._context.borrow_mut() = Some(context);
        *self.surface.borrow_mut() = Some(surface);

        Ok(winit_window)
    }

    fn suspend(&self) -> Result<(), PlatformError> {
        drop(self.surface.borrow_mut().take());
        drop(self._context.borrow_mut().take());
        Ok(())
    }
}

#[cfg(target_os = "windows")]
fn win32_update_rect(
    winit_window: &winit::window::Window,
    width: NonZeroU32,
    height: NonZeroU32,
) -> Option<softbuffer::Rect> {
    use windows::Win32::Foundation::{HWND, RECT};
    use windows::Win32::Graphics::Gdi::GetUpdateRect;
    use winit::raw_window_handle::{HasWindowHandle, RawWindowHandle};

    let RawWindowHandle::Win32(handle) = winit_window.window_handle().ok()?.as_raw() else {
        return None;
    };
    let hwnd = HWND(handle.hwnd.get() as *mut core::ffi::c_void);
    let mut rect = RECT::default();
    // SAFETY: `winit_window` keeps the window behind `hwnd` alive, and `rect` outlives the call.
    if !unsafe { GetUpdateRect(hwnd, Some(&mut rect), false) }.as_bool() {
        return None;
    }
    clip_to_buffer(rect.left, rect.top, rect.right, rect.bottom, width, height)
}

#[cfg(target_os = "windows")]
fn clip_to_buffer(
    left: i32,
    top: i32,
    right: i32,
    bottom: i32,
    width: NonZeroU32,
    height: NonZeroU32,
) -> Option<softbuffer::Rect> {
    let clip = |edge: i32, size: NonZeroU32| u32::try_from(edge).unwrap_or(0).min(size.get());
    let (x, y) = (clip(left, width), clip(top, height));
    Some(softbuffer::Rect {
        x,
        y,
        width: NonZeroU32::new(clip(right, width).checked_sub(x)?)?,
        height: NonZeroU32::new(clip(bottom, height).checked_sub(y)?)?,
    })
}

#[cfg(all(test, target_os = "windows"))]
mod tests {
    use super::clip_to_buffer;
    use core::num::NonZeroU32;

    fn clip(left: i32, top: i32, right: i32, bottom: i32) -> Option<(u32, u32, u32, u32)> {
        let (width, height) = (NonZeroU32::new(800).unwrap(), NonZeroU32::new(600).unwrap());
        clip_to_buffer(left, top, right, bottom, width, height)
            .map(|rect| (rect.x, rect.y, rect.width.get(), rect.height.get()))
    }

    #[test]
    fn keeps_rect_inside_buffer() {
        assert_eq!(clip(10, 20, 110, 70), Some((10, 20, 100, 50)));
        assert_eq!(clip(0, 0, 800, 600), Some((0, 0, 800, 600)));
    }

    #[test]
    fn clips_rect_partly_outside_buffer() {
        assert_eq!(clip(-50, -400, 300, 200), Some((0, 0, 300, 200)));
        assert_eq!(clip(700, 500, 1000, 900), Some((700, 500, 100, 100)));
        assert_eq!(clip(i32::MIN, i32::MIN, i32::MAX, i32::MAX), Some((0, 0, 800, 600)));
    }

    #[test]
    fn drops_empty_rect() {
        assert_eq!(clip(0, 0, 0, 0), None);
        assert_eq!(clip(10, 20, 10, 70), None);
        assert_eq!(clip(10, 20, 110, 20), None);
        assert_eq!(clip(110, 70, 10, 20), None);
    }

    #[test]
    fn drops_rect_outside_buffer() {
        assert_eq!(clip(-300, 10, -100, 50), None);
        assert_eq!(clip(800, 10, 900, 50), None);
        assert_eq!(clip(10, -500, 110, 0), None);
        assert_eq!(clip(10, 600, 110, 700), None);
    }
}
