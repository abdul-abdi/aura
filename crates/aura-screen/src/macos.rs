#[cfg(target_os = "macos")]
use anyhow::Result;

#[cfg(target_os = "macos")]
use core_foundation::base::TCFType;

#[cfg(target_os = "macos")]
use crate::context::ScreenContext;

#[cfg(target_os = "macos")]
pub struct MacOSScreenReader;

#[cfg(target_os = "macos")]
impl MacOSScreenReader {
    pub fn new() -> Result<Self> {
        Ok(Self)
    }

    pub fn capture_context(&self) -> Result<ScreenContext> {
        let mut ctx = ScreenContext::new();

        // Use CGWindowListCopyWindowInfo to enumerate windows
        let windows = unsafe {
            core_graphics::display::CGDisplay::main();
            let window_list = core_graphics::window::CGWindowListCopyWindowInfo(
                core_graphics::window::kCGWindowListOptionOnScreenOnly
                    | core_graphics::window::kCGWindowListExcludeDesktopElements,
                core_graphics::window::kCGNullWindowID,
            );

            let infos = Vec::new();
            if !window_list.is_null() {
                let list = core_foundation::array::CFArray::<
                    core_foundation::dictionary::CFDictionary,
                >::wrap_under_create_rule(window_list as _);

                let count = list.len();

                // Basic window enumeration — full accessibility tree
                // reading will be added in a follow-up task
                for _i in 0..count {
                    // Placeholder: full implementation reads kCGWindowOwnerName,
                    // kCGWindowName, kCGWindowBounds from each dict entry
                }
            }
            infos
        };

        ctx.update(windows, None);
        Ok(ctx)
    }
}
