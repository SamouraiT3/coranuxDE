use crate::{Smallvil, grabs::resize_grab, state::ClientState};
use smithay::{
    backend::allocator::{Buffer, dmabuf::Dmabuf},
    backend::renderer::{ImportDma, utils::on_commit_buffer_handler},
    delegate_compositor, delegate_dmabuf, delegate_shm,
    reexports::wayland_server::{
        Client,
        protocol::{wl_buffer, wl_surface::WlSurface},
    },
    wayland::{
        buffer::BufferHandler,
        compositor::{
            CompositorClientState, CompositorHandler, CompositorState, get_parent,
            is_sync_subsurface,
        },
        dmabuf::{DmabufGlobal, DmabufHandler, DmabufState, ImportNotifier},
        shm::{ShmHandler, ShmState},
    },
    xwayland::XWaylandClientData,
};

use super::xdg_shell;

impl CompositorHandler for Smallvil {
    fn compositor_state(&mut self) -> &mut CompositorState {
        &mut self.compositor_state
    }

    fn client_compositor_state<'a>(&self, client: &'a Client) -> &'a CompositorClientState {
        client
            .get_data::<ClientState>()
            .map(|state| &state.compositor_state)
            .or_else(|| {
                client
                    .get_data::<XWaylandClientData>()
                    .map(|state| &state.compositor_state)
            })
            .expect("Wayland clients must have compositor state")
    }

    fn commit(&mut self, surface: &WlSurface) {
        on_commit_buffer_handler::<Self>(surface);
        if !is_sync_subsurface(surface) {
            let mut root = surface.clone();
            while let Some(parent) = get_parent(&root) {
                root = parent;
            }
            let committed_window = self
                .space
                .elements()
                .find(|window| {
                    self.wl_surface_for_window(window)
                        .is_some_and(|surface| surface == root)
                })
                .cloned();
            if let Some(window) = committed_window {
                window.on_commit();
                if self.is_desktop_surface(&root)
                    && window.bbox().size.w > 0
                    && window.bbox().size.h > 0
                    && !self.desktop_keyboard_focus_set
                {
                    self.desktop_keyboard_focus_set = true;
                    self.focus_window(&window);
                }
                if self.is_desktop_surface(&root) && self.desktop_commit_count < 4 {
                    self.desktop_commit_count += 1;
                }
                if let Some(index) = self
                    .pending_open_animations
                    .iter()
                    .position(|pending| pending == &root)
                {
                    self.pending_open_animations.remove(index);
                    self.animate_window_open(&root);
                    if !self.is_panel_surface(&root) {
                        self.focus_window(&window);
                    }
                }
                let output_geometry = self
                    .space
                    .outputs()
                    .next()
                    .and_then(|output| self.space.output_geometry(output));
                if let Some(output_geometry) = output_geometry {
                    if self.is_desktop_surface(&root) {
                        self.enforce_desktop_geometry(&window, output_geometry);
                    } else if self.is_panel_surface(&root) {
                        self.enforce_panel_geometry(&window, output_geometry);
                    }
                }
            }
        };

        let is_desktop = self.is_desktop_surface(surface);
        xdg_shell::handle_commit(&mut self.popups, &self.space, surface, is_desktop);
        resize_grab::handle_commit(&mut self.space, surface);
    }
}

impl BufferHandler for Smallvil {
    fn buffer_destroyed(&mut self, _buffer: &wl_buffer::WlBuffer) {}
}

impl ShmHandler for Smallvil {
    fn shm_state(&self) -> &ShmState {
        &self.shm_state
    }
}

impl DmabufHandler for Smallvil {
    fn dmabuf_state(&mut self) -> &mut DmabufState {
        &mut self.dmabuf_state
    }

    fn dmabuf_imported(
        &mut self,
        _global: &DmabufGlobal,
        dmabuf: Dmabuf,
        notifier: ImportNotifier,
    ) {
        let Some(renderer) = &self.dmabuf_renderer else {
            notifier.failed();
            return;
        };
        if let Err(error) = (&mut *renderer.borrow_mut()).import_dmabuf(&dmabuf, None) {
            eprintln!(
                "coranux_wm: import DMABUF impossible : {error}; buffer={}x{}, format={:?}",
                dmabuf.width(),
                dmabuf.height(),
                dmabuf.format()
            );
            notifier.failed();
            return;
        }
        if let Err(error) = notifier.successful::<Smallvil>() {
            eprintln!("coranux_wm: le client Wayland a disparu pendant l’import DMABUF : {error}");
        }
    }
}

delegate_compositor!(Smallvil);
delegate_dmabuf!(Smallvil);
delegate_shm!(Smallvil);
