//! Raw CoreGraphics bindings.
//!
//! CGPoint is re-exported from screencapturekit so the `#[repr(C)]` layout is
//! shared with the crate we hand these values to.

use screencapturekit::cg::CGPoint;

pub type CGDirectDisplayID = u32;

#[link(name = "CoreGraphics", kind = "framework")]
unsafe extern "C" {
    pub fn CGPreflightScreenCaptureAccess() -> bool;
    pub fn CGRequestScreenCaptureAccess() -> bool;
    pub fn CGMainDisplayID() -> CGDirectDisplayID;
    pub fn CGGetDisplaysWithPoint(
        point: CGPoint,
        max_displays: u32,
        displays: *mut CGDirectDisplayID,
        matching_display_count: *mut u32,
    ) -> i32;
    fn CGEventCreate(source: *const std::ffi::c_void) -> *mut std::ffi::c_void;
    fn CGEventGetLocation(event: *mut std::ffi::c_void) -> CGPoint;
}

#[link(name = "CoreFoundation", kind = "framework")]
unsafe extern "C" {
    fn CFRelease(cf: *const std::ffi::c_void);
}

/// Cursor position in CoreGraphics global display space (top-left origin).
///
/// Deliberately not `NSEvent.mouseLocation`, which is bottom-left origin and
/// would select the wrong display on multi-monitor setups.
pub fn cursor_location() -> Option<CGPoint> {
    unsafe {
        let event = CGEventCreate(std::ptr::null());
        if event.is_null() {
            return None;
        }
        let point = CGEventGetLocation(event);
        CFRelease(event.cast());
        Some(point)
    }
}

/// The display currently under the mouse cursor, falling back to the main
/// display when the cursor sits in a dead zone between displays.
pub fn display_under_cursor() -> CGDirectDisplayID {
    let Some(point) = cursor_location() else {
        return unsafe { CGMainDisplayID() };
    };

    let mut ids = [0 as CGDirectDisplayID; 8];
    let mut count: u32 = 0;
    let err =
        unsafe { CGGetDisplaysWithPoint(point, ids.len() as u32, ids.as_mut_ptr(), &mut count) };

    if err == 0 && count > 0 {
        ids[0]
    } else {
        unsafe { CGMainDisplayID() }
    }
}
