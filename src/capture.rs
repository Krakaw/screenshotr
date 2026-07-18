//! Active-display capture via ScreenCaptureKit.

use screencapturekit::prelude::*;
use screencapturekit::screenshot_manager::{CGImageExt, SCScreenshotManager};

use crate::sys::display_under_cursor;

pub struct Frame {
    /// Packed BGRA, 4 bytes per pixel, `width * height * 4` long.
    pub bgra: Vec<u8>,
    pub width: u16,
    pub height: u16,
}

#[derive(Debug)]
pub enum CaptureError {
    /// No displays were shareable, which in practice means TCC denied us.
    PermissionDenied,
    Sck(SCError),
}

impl std::fmt::Display for CaptureError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::PermissionDenied => write!(f, "screen recording permission denied"),
            Self::Sck(e) => write!(f, "screencapturekit error: {e:?}"),
        }
    }
}

impl From<SCError> for CaptureError {
    fn from(e: SCError) -> Self {
        Self::Sck(e)
    }
}

/// Ground-truth capability check: can ScreenCaptureKit actually enumerate
/// displays right now?
///
/// This is stricter than `CGPreflightScreenCaptureAccess`, which can report
/// true while ScreenCaptureKit still returns nothing — notably after the app
/// bundle is replaced on disk and the grant goes stale. Startup gates on this,
/// not on preflight, so a lying preflight can't leave us serving while every
/// capture fails.
pub fn can_capture() -> bool {
    SCShareableContent::get()
        .map(|c| !c.displays().is_empty())
        .unwrap_or(false)
}

/// Capture the display under the mouse cursor at its native pixel resolution.
pub fn capture_active_display() -> Result<Frame, CaptureError> {
    let target = display_under_cursor();
    let content = SCShareableContent::get()?;
    let displays = content.displays();

    // ScreenCaptureKit reports an empty display list rather than a permission
    // error when the grant is missing. A Mac always has at least one display,
    // so empty means denied — never report it as "no displays".
    if displays.is_empty() {
        return Err(CaptureError::PermissionDenied);
    }

    let display = displays
        .iter()
        .find(|d| d.display_id() == target)
        .unwrap_or(&displays[0]);

    log::debug!(
        "capturing display {} ({}x{} pt)",
        display.display_id(),
        display.width(),
        display.height()
    );

    let filter = SCContentFilter::create()
        .with_display(display)
        .with_excluding_windows(&[])
        .build();

    // SCDisplay dimensions are in points; point_pixel_scale is the backing
    // scale factor (2.0 on Retina). Multiplying yields native pixels — without
    // this the capture silently comes back at half resolution.
    let scale = f64::from(filter.point_pixel_scale());
    let width = (f64::from(display.width()) * scale).round() as u32;
    let height = (f64::from(display.height()) * scale).round() as u32;

    let config = SCStreamConfiguration::new()
        .with_width(width)
        .with_height(height)
        .with_pixel_format(PixelFormat::BGRA)
        .with_shows_cursor(true);

    let image = SCScreenshotManager::capture_image(&filter, &config)?;
    let bgra = image.bgra_data()?;

    Ok(Frame {
        bgra,
        width: width as u16,
        height: height as u16,
    })
}
