#![allow(irrefutable_let_patterns)]

#[path = "coranux_wm/handlers/mod.rs"]
mod handlers;

#[path = "coranux_wm/drm.rs"]
mod drm;
#[path = "coranux_wm/grabs/mod.rs"]
mod grabs;
#[path = "coranux_wm/input.rs"]
mod input;
#[path = "coranux_wm/state.rs"]
mod state;

use smithay::reexports::{
    calloop::EventLoop,
    wayland_server::{Display, DisplayHandle},
};
pub use state::Smallvil;

pub struct CalloopData {
    state: Smallvil,
    display_handle: DisplayHandle,
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    if let Ok(env_filter) = tracing_subscriber::EnvFilter::try_from_default_env() {
        tracing_subscriber::fmt().with_env_filter(env_filter).init();
    } else {
        tracing_subscriber::fmt().init();
    }

    let mut event_loop: EventLoop<'static, CalloopData> = EventLoop::try_new()?;

    let display: Display<Smallvil> = Display::new()?;
    let display_handle = display.handle();
    let state = Smallvil::new(&mut event_loop, display)?;

    let mut data = CalloopData {
        state,
        display_handle,
    };

    crate::drm::init_drm(&mut event_loop, &mut data)?;

    let ready_file = std::env::var_os("CORANUX_WAYLAND_READY_FILE");
    let cleanup_ready_file = ready_file.clone();
    let cleanup_xwayland_display_file = data.state.xwayland_display_file.clone();
    let cleanup_desktop_ready_file = std::env::var_os("CORANUX_DESKTOP_READY_FILE");
    let cleanup_panel_ready_file = std::env::var_os("CORANUX_PANEL_READY_FILE");
    if let Some(path) = &ready_file {
        std::fs::write(path, b"ready")
            .map_err(|error| format!("impossible de signaler le démarrage Wayland : {error}"))?;
    }
    let run_result = event_loop.run(None, &mut data, |_| {
        // The Wayland socket and Winit backend are ready before entering the event loop.
    });

    if let Some(path) = cleanup_ready_file {
        let _ = std::fs::remove_file(path);
    }
    if let Some(path) = cleanup_xwayland_display_file {
        let _ = std::fs::remove_file(path);
    }
    if let Some(path) = cleanup_desktop_ready_file {
        let _ = std::fs::remove_file(path);
    }
    if let Some(path) = cleanup_panel_ready_file {
        let _ = std::fs::remove_file(path);
    }

    run_result?;
    Ok(())
}
