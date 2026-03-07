use anyhow::Result;
use core_foundation::base::TCFType;

use crate::context::ScreenContext;

pub struct MacOSScreenReader;

impl MacOSScreenReader {
    pub fn new() -> Result<Self> {
        Ok(Self)
    }

    pub fn capture_context(&self) -> Result<ScreenContext> {
        // TODO: Full implementation reads kCGWindowOwnerName, kCGWindowName,
        // kCGWindowBounds from each dict entry. For now, returns empty context.
        tracing::warn!("MacOSScreenReader::capture_context is not yet implemented");

        let windows = unsafe {
            let window_list = core_graphics::window::CGWindowListCopyWindowInfo(
                core_graphics::window::kCGWindowListOptionOnScreenOnly
                    | core_graphics::window::kCGWindowListExcludeDesktopElements,
                core_graphics::window::kCGNullWindowID,
            );

            let infos = Vec::new();
            if !window_list.is_null() {
                let _list = core_foundation::array::CFArray::<
                    core_foundation::dictionary::CFDictionary,
                >::wrap_under_create_rule(window_list as _);

                // Placeholder: full dict parsing will be added in a follow-up task
            }
            infos
        };

        Ok(ScreenContext::with_windows(windows, None))
    }
}
