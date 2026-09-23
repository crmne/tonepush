//! Whether frames may wait for vsync.
//!
//! On Wayland a hidden window gets no frame callbacks, and a vsync wait in
//! `swap_buffers` then blocks the whole event loop. The patched winit reports
//! the xdg-shell v6 `suspended` state as `Occluded`, so eframe runs only the
//! app's `logic` for a hidden window instead of painting it. Compositors older
//! than that version give no such warning, so vsync stays off there.

/// Decided once, at startup.
pub fn vsync() -> bool {
    static VSYNC: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *VSYNC.get_or_init(|| {
        #[cfg(target_os = "linux")]
        let wayland = wayland::reports_suspended();
        #[cfg(not(target_os = "linux"))]
        let wayland = None;
        vsync_is_safe(wayland)
    })
}

/// `wayland` is `None` off Wayland, otherwise whether the compositor reports
/// hidden windows as suspended.
fn vsync_is_safe(wayland: Option<bool>) -> bool {
    wayland.unwrap_or(true)
}

#[cfg(target_os = "linux")]
mod wayland {
    use wayland_client::globals::{registry_queue_init, GlobalListContents};
    use wayland_client::protocol::wl_registry::{self, WlRegistry};
    use wayland_client::{Connection, Dispatch, QueueHandle};

    /// The first xdg_wm_base version with the `suspended` toplevel state.
    const SUSPENDED_SINCE: u32 = 6;

    struct Globals;

    impl Dispatch<WlRegistry, GlobalListContents> for Globals {
        fn event(
            _: &mut Self,
            _: &WlRegistry,
            _: wl_registry::Event,
            _: &GlobalListContents,
            _: &Connection,
            _: &QueueHandle<Self>,
        ) {
        }
    }

    /// `None` when this is not a Wayland session, as winit then uses X11.
    pub(super) fn reports_suspended() -> Option<bool> {
        let connection = Connection::connect_to_env().ok()?;
        let (globals, _queue) = registry_queue_init::<Globals>(&connection).ok()?;
        Some(globals.contents().with_list(|list| {
            list.iter().any(|global| {
                global.interface == "xdg_wm_base" && global.version >= SUSPENDED_SINCE
            })
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::vsync_is_safe;

    #[test]
    fn vsync_waits_only_where_a_hidden_window_cannot_block() {
        // macOS, Windows, Android and X11.
        assert!(vsync_is_safe(None));
        // Wayland compositors that suspend hidden windows.
        assert!(vsync_is_safe(Some(true)));
        // Older Wayland compositors give no warning before they stop callbacks.
        assert!(!vsync_is_safe(Some(false)));
    }
}
