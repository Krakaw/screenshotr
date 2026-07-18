//! Display capture via ScreenCaptureKit.

use screencapturekit::prelude::*;
use screencapturekit::screenshot_manager::{CGImageExt, SCScreenshotManager};

use crate::sys::{CGDirectDisplayID, display_under_cursor};

/// The canvas between tiles in an all-display composite. Mid-grey reads as
/// deliberate padding rather than as screen content that failed to capture.
const GUTTER: [u8; 4] = [0x1d, 0x21, 0x16, 0xff]; // BGRA

pub struct Frame {
    /// Packed BGRA, 4 bytes per pixel, `width * height * 4` long.
    pub bgra: Vec<u8>,
    pub width: u16,
    pub height: u16,
}

/// A display as advertised to clients, so they can pick one by id.
pub struct DisplayInfo {
    pub id: CGDirectDisplayID,
    /// Native pixel dimensions — what a capture of this display returns.
    pub width: u32,
    pub height: u32,
    /// Position in the global arrangement, in points. Two displays never
    /// overlap here, so this is what tells us their left-to-right order.
    pub x: i32,
    pub y: i32,
    /// True for the display under the mouse cursor: the one `display=active`
    /// (and every pre-multi-display client) resolves to.
    pub active: bool,
}

/// Which display(s) a capture request is asking for.
pub enum Target {
    /// The display under the mouse cursor. The default, and what the service
    /// did exclusively before multi-display support.
    Active,
    Id(CGDirectDisplayID),
    All,
}

#[derive(Debug)]
pub enum CaptureError {
    /// No displays were shareable, which in practice means TCC denied us.
    PermissionDenied,
    /// A `display=<id>` request named a display that is not attached.
    NoSuchDisplay(CGDirectDisplayID),
    /// The composite of every display would exceed the dimensions a JPEG
    /// frame can describe.
    CompositeTooLarge { width: u64, height: u64 },
    Sck(SCError),
}

impl std::fmt::Display for CaptureError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::PermissionDenied => write!(f, "screen recording permission denied"),
            Self::NoSuchDisplay(id) => write!(f, "no display with id {id}"),
            Self::CompositeTooLarge { width, height } => {
                write!(f, "composite {width}x{height} exceeds the maximum frame size")
            }
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

/// Fetch the shareable display list, mapping the "denied" case onto a real error.
///
/// ScreenCaptureKit reports an empty display list rather than a permission
/// error when the grant is missing. A Mac always has at least one display, so
/// empty means denied — never report it as "no displays".
fn shareable_displays() -> Result<Vec<SCDisplay>, CaptureError> {
    let displays = SCShareableContent::get()?.displays();
    if displays.is_empty() {
        return Err(CaptureError::PermissionDenied);
    }
    Ok(displays)
}

/// Every attached display, ordered left-to-right then top-to-bottom by their
/// position in the desktop arrangement — the same order `Target::All` tiles
/// them in, so a client's list matches what it sees in the composite.
pub fn list_displays() -> Result<Vec<DisplayInfo>, CaptureError> {
    let active = display_under_cursor();
    let mut infos: Vec<DisplayInfo> = shareable_displays()?
        .iter()
        .map(|d| {
            let frame = d.frame();
            let (width, height) = native_size(d);
            DisplayInfo {
                id: d.display_id(),
                width,
                height,
                x: frame.origin.x.round() as i32,
                y: frame.origin.y.round() as i32,
                active: d.display_id() == active,
            }
        })
        .collect();
    infos.sort_by_key(|d| (d.x, d.y));
    Ok(infos)
}

/// Capture one display, all displays, or whichever is under the cursor.
pub fn capture(target: Target) -> Result<Frame, CaptureError> {
    let displays = shareable_displays()?;

    match target {
        Target::All => capture_composite(&displays),
        Target::Id(id) => {
            let display = displays
                .iter()
                .find(|d| d.display_id() == id)
                .ok_or(CaptureError::NoSuchDisplay(id))?;
            capture_one(display)
        }
        Target::Active => {
            // Falling back to the first display keeps a capture succeeding if
            // the cursor sits somewhere ScreenCaptureKit does not share.
            let wanted = display_under_cursor();
            let display = displays
                .iter()
                .find(|d| d.display_id() == wanted)
                .unwrap_or(&displays[0]);
            capture_one(display)
        }
    }
}

/// A display's dimensions in native pixels.
///
/// SCDisplay dimensions are in points; point_pixel_scale is the backing scale
/// factor (2.0 on Retina). Multiplying yields native pixels — without this the
/// capture silently comes back at half resolution.
fn native_size(display: &SCDisplay) -> (u32, u32) {
    let filter = SCContentFilter::create()
        .with_display(display)
        .with_excluding_windows(&[])
        .build();
    let scale = f64::from(filter.point_pixel_scale());
    (
        (f64::from(display.width()) * scale).round() as u32,
        (f64::from(display.height()) * scale).round() as u32,
    )
}

/// Capture a single display at its native pixel resolution.
fn capture_one(display: &SCDisplay) -> Result<Frame, CaptureError> {
    let (width, height) = native_size(display);

    log::debug!(
        "capturing display {} ({}x{} px)",
        display.display_id(),
        width,
        height
    );

    let filter = SCContentFilter::create()
        .with_display(display)
        .with_excluding_windows(&[])
        .build();

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

/// Capture every display and tile them into one frame, left-to-right in
/// arrangement order and top-aligned.
///
/// Tiles keep their native resolution rather than being placed at their true
/// global coordinates. Displays with different backing scales share no common
/// pixel grid, so honouring the real geometry would mean resampling every tile
/// — and an arrangement with a stacked or offset display would leave most of a
/// huge canvas as empty gutter. Adjacency is preserved; exact geometry is not.
fn capture_composite(displays: &[SCDisplay]) -> Result<Frame, CaptureError> {
    let mut ordered: Vec<&SCDisplay> = displays.iter().collect();
    ordered.sort_by_key(|d| {
        let frame = d.frame();
        (frame.origin.x.round() as i32, frame.origin.y.round() as i32)
    });

    let tiles = ordered
        .iter()
        .map(|d| capture_one(d))
        .collect::<Result<Vec<Frame>, _>>()?;

    let composite = tile_horizontally(tiles)?;
    log::debug!(
        "composited {} displays into {}x{}",
        ordered.len(),
        composite.width,
        composite.height
    );
    Ok(composite)
}

/// Lay frames out left to right, top-aligned, padding the ragged bottom edge
/// left by shorter tiles with [`GUTTER`].
fn tile_horizontally(tiles: Vec<Frame>) -> Result<Frame, CaptureError> {
    // A lone tile needs no canvas; hand it back untouched.
    if tiles.len() == 1 {
        return Ok(tiles.into_iter().next().expect("length checked"));
    }

    // Widen to u64 before summing: enough 4K displays would overflow the u16
    // that Frame (and the JPEG encoder) uses for dimensions.
    let total_width: u64 = tiles.iter().map(|t| u64::from(t.width)).sum();
    let max_height: u64 = tiles.iter().map(|t| u64::from(t.height)).max().unwrap_or(0);
    if total_width > u64::from(u16::MAX) || max_height > u64::from(u16::MAX) {
        return Err(CaptureError::CompositeTooLarge {
            width: total_width,
            height: max_height,
        });
    }
    let (canvas_w, canvas_h) = (total_width as usize, max_height as usize);

    let mut canvas = GUTTER.repeat(canvas_w * canvas_h);

    let mut x_offset = 0usize;
    for tile in &tiles {
        let (tw, th) = (tile.width as usize, tile.height as usize);
        for row in 0..th {
            let src = row * tw * 4;
            let dst = (row * canvas_w + x_offset) * 4;
            canvas[dst..dst + tw * 4].copy_from_slice(&tile.bgra[src..src + tw * 4]);
        }
        x_offset += tw;
    }

    Ok(Frame {
        bgra: canvas,
        width: canvas_w as u16,
        height: canvas_h as u16,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A `w`x`h` frame filled with a single identifying byte value.
    fn solid(w: u16, h: u16, fill: u8) -> Frame {
        Frame {
            bgra: vec![fill; w as usize * h as usize * 4],
            width: w,
            height: h,
        }
    }

    /// The BGRA pixel at (x, y) in a frame.
    fn pixel(frame: &Frame, x: usize, y: usize) -> [u8; 4] {
        let i = (y * frame.width as usize + x) * 4;
        frame.bgra[i..i + 4].try_into().expect("4 bytes")
    }

    #[test]
    fn single_tile_passes_through_unchanged() {
        let out = tile_horizontally(vec![solid(3, 2, 0xAB)]).unwrap();
        assert_eq!((out.width, out.height), (3, 2));
        assert!(out.bgra.iter().all(|&b| b == 0xAB));
    }

    #[test]
    fn equal_height_tiles_sit_side_by_side_in_order() {
        let out = tile_horizontally(vec![solid(2, 2, 0x11), solid(3, 2, 0x22)]).unwrap();
        assert_eq!((out.width, out.height), (5, 2));

        // Left tile occupies x 0..2, right tile x 2..5, on every row.
        for y in 0..2 {
            for x in 0..2 {
                assert_eq!(pixel(&out, x, y), [0x11; 4], "left tile at ({x},{y})");
            }
            for x in 2..5 {
                assert_eq!(pixel(&out, x, y), [0x22; 4], "right tile at ({x},{y})");
            }
        }
    }

    #[test]
    fn shorter_tile_is_top_aligned_and_gutter_fills_below() {
        // A 1x1 tile beside a 1x3: rows 1 and 2 of the short column are gutter.
        let out = tile_horizontally(vec![solid(1, 1, 0x11), solid(1, 3, 0x22)]).unwrap();
        assert_eq!((out.width, out.height), (2, 3));

        assert_eq!(pixel(&out, 0, 0), [0x11; 4], "short tile occupies the top row");
        assert_eq!(pixel(&out, 0, 1), GUTTER, "gutter below the short tile");
        assert_eq!(pixel(&out, 0, 2), GUTTER, "gutter below the short tile");
        for y in 0..3 {
            assert_eq!(pixel(&out, 1, y), [0x22; 4], "tall tile at row {y}");
        }
    }

    #[test]
    fn oversized_composite_is_rejected_rather_than_truncated() {
        let tiles = vec![solid(u16::MAX, 1, 0), solid(u16::MAX, 1, 0)];
        assert!(matches!(
            tile_horizontally(tiles),
            Err(CaptureError::CompositeTooLarge { .. })
        ));
    }
}
