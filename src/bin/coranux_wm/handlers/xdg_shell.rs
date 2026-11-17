use smithay::{
    delegate_xdg_decoration, delegate_xdg_shell,
    desktop::{
        PopupKind, PopupManager, Space, Window, find_popup_root_surface, get_popup_toplevel_coords,
    },
    input::{
        Seat,
        pointer::{Focus, GrabStartData as PointerGrabStartData},
    },
    reexports::{
        wayland_protocols::xdg::shell::server::xdg_toplevel,
        wayland_server::{
            Resource,
            protocol::{wl_output, wl_seat, wl_surface::WlSurface},
        },
    },
    utils::{Rectangle, Serial},
    wayland::{
        compositor::with_states,
        shell::xdg::{
            PopupSurface, PositionerState, ToplevelSurface, XdgShellHandler, XdgShellState,
            XdgToplevelSurfaceData, decoration::XdgDecorationHandler,
        },
    },
};

use crate::{
    Smallvil,
    grabs::{MoveSurfaceGrab, ResizeSurfaceGrab},
    state::{FRAME_BORDER, TITLEBAR_TOTAL_HEIGHT, work_area_geometry},
};

const INITIAL_WINDOW_OFFSET: i32 = 40;

impl XdgShellHandler for Smallvil {
    fn xdg_shell_state(&mut self) -> &mut XdgShellState {
        &mut self.xdg_shell_state
    }

    fn new_toplevel(&mut self, surface: ToplevelSurface) {
        let window = Window::new_wayland_window(surface);
        let output_size = self
            .space
            .outputs()
            .next()
            .and_then(|output| self.space.output_geometry(output))
            .map(|geometry| geometry.size);
        let offset_limit = output_size
            .map(|size| size.w.min(size.h).saturating_sub(180).max(1))
            .unwrap_or(240);
        let offset = INITIAL_WINDOW_OFFSET + self.next_window_offset.rem_euclid(offset_limit);
        self.next_window_offset = (self.next_window_offset + 36).rem_euclid(offset_limit);
        self.space
            .map_element(window.clone(), (offset, offset), false);
        self.register_toplevel(&window, (offset, offset).into());
    }

    fn new_popup(&mut self, surface: PopupSurface, _positioner: PositionerState) {
        self.unconstrain_popup(&surface);
        let _ = self.popups.track_popup(PopupKind::Xdg(surface));
    }

    fn reposition_request(
        &mut self,
        surface: PopupSurface,
        positioner: PositionerState,
        token: u32,
    ) {
        surface.with_pending_state(|state| {
            let geometry = positioner.get_geometry();
            state.geometry = geometry;
            state.positioner = positioner;
        });
        self.unconstrain_popup(&surface);
        surface.send_repositioned(token);
    }

    fn move_request(&mut self, surface: ToplevelSurface, seat: wl_seat::WlSeat, serial: Serial) {
        if self.is_panel_surface(surface.wl_surface())
            || (self.window_mode == crate::state::WindowMode::Tiling
                && !self.is_floating_surface(surface.wl_surface()))
        {
            return;
        }
        let seat = Seat::from_resource(&seat).unwrap();

        let wl_surface = surface.wl_surface();

        if let Some(start_data) = check_grab(&seat, wl_surface, serial) {
            let pointer = seat.get_pointer().unwrap();

            let window = self
                .space
                .elements()
                .find(|window| {
                    window
                        .toplevel()
                        .is_some_and(|toplevel| toplevel.wl_surface() == wl_surface)
                })
                .unwrap()
                .clone();
            let initial_window_location = self.space.element_location(&window).unwrap();

            let grab = MoveSurfaceGrab {
                start_data,
                window,
                initial_window_location,
            };

            pointer.set_grab(self, grab, serial, Focus::Clear);
        }
    }

    fn resize_request(
        &mut self,
        surface: ToplevelSurface,
        seat: wl_seat::WlSeat,
        serial: Serial,
        edges: xdg_toplevel::ResizeEdge,
    ) {
        if self.is_panel_surface(surface.wl_surface())
            || self.is_fullscreen_or_maximized(surface.wl_surface())
        {
            return;
        }
        if self.window_mode == crate::state::WindowMode::Tiling
            && !self.is_floating_surface(surface.wl_surface())
        {
            return;
        }
        let seat = Seat::from_resource(&seat).unwrap();

        let wl_surface = surface.wl_surface();

        if let Some(start_data) = check_grab(&seat, wl_surface, serial) {
            let pointer = seat.get_pointer().unwrap();

            let window = self
                .space
                .elements()
                .find(|window| {
                    window
                        .toplevel()
                        .is_some_and(|toplevel| toplevel.wl_surface() == wl_surface)
                })
                .unwrap()
                .clone();
            let initial_window_location = self.space.element_location(&window).unwrap();
            let initial_window_size = window.geometry().size;

            surface.with_pending_state(|state| {
                state.states.set(xdg_toplevel::State::Resizing);
            });

            surface.send_pending_configure();

            let grab = ResizeSurfaceGrab::start(
                start_data,
                window,
                edges.into(),
                Rectangle::new(initial_window_location, initial_window_size),
                None,
            );

            pointer.set_grab(self, grab, serial, Focus::Clear);
        }
    }

    fn maximize_request(&mut self, surface: ToplevelSurface) {
        self.set_maximized(surface.wl_surface(), true);
    }

    fn unmaximize_request(&mut self, surface: ToplevelSurface) {
        self.set_maximized(surface.wl_surface(), false);
    }

    fn fullscreen_request(
        &mut self,
        surface: ToplevelSurface,
        _output: Option<wl_output::WlOutput>,
    ) {
        let wl_surface = surface.wl_surface().clone();
        if self.is_panel_surface(&wl_surface) {
            self.resize_panel_surfaces();
            return;
        }
        let desktop = self.is_desktop_surface(&wl_surface);
        if self.window_mode == crate::state::WindowMode::Tiling && !desktop {
            if self.is_floating_surface(&wl_surface) {
                surface.send_configure();
            } else {
                self.set_maximized(&wl_surface, true);
            }
            return;
        }
        let Some(output) = self.space.outputs().next().cloned() else {
            surface.send_configure();
            return;
        };
        let Some(output_geometry) = self.space.output_geometry(&output) else {
            surface.send_configure();
            return;
        };
        let geometry = if desktop {
            output_geometry
        } else {
            work_area_geometry(output_geometry)
        };

        let client_location = if desktop {
            geometry.loc
        } else {
            (
                geometry.loc.x + FRAME_BORDER,
                geometry.loc.y + TITLEBAR_TOTAL_HEIGHT,
            )
                .into()
        };
        let client_size = if desktop {
            geometry.size
        } else {
            (
                (geometry.size.w - FRAME_BORDER * 2).max(1),
                (geometry.size.h - TITLEBAR_TOTAL_HEIGHT - FRAME_BORDER).max(1),
            )
                .into()
        };

        surface.with_pending_state(|state| {
            state.states.set(xdg_toplevel::State::Fullscreen);
            state.size = Some(client_size);
        });
        if let Some(window) = self.window_for_surface(surface.wl_surface()) {
            if !desktop
                && !self
                    .normal_window_geometries
                    .iter()
                    .any(|(saved, _)| saved == &wl_surface)
                && let Some(location) = self.space.element_location(&window)
            {
                self.normal_window_geometries.push((
                    wl_surface.clone(),
                    Rectangle::new(location, window.geometry().size),
                ));
            }
            if desktop {
                self.space.map_element(window, client_location, true);
                surface.send_configure();
            } else {
                let from = self
                    .space
                    .element_location(&window)
                    .map(|location| Rectangle::new(location, window.geometry().size))
                    .unwrap_or_else(|| Rectangle::new(client_location, client_size));
                self.space.raise_element(&window, true);
                self.start_geometry_animation(
                    &wl_surface,
                    from,
                    Rectangle::new(client_location, client_size),
                    true,
                    260,
                );
            }
        } else {
            surface.send_configure();
        }
    }

    fn unfullscreen_request(&mut self, surface: ToplevelSurface) {
        let wl_surface = surface.wl_surface().clone();
        if self.window_mode == crate::state::WindowMode::Tiling
            && !self.is_desktop_surface(&wl_surface)
            && !self.is_floating_surface(&wl_surface)
        {
            self.set_maximized(&wl_surface, false);
            return;
        }
        if self.is_desktop_surface(&wl_surface) {
            surface.send_configure();
            return;
        }

        surface.with_pending_state(|state| {
            state.states.unset(xdg_toplevel::State::Fullscreen);
        });
        if let Some(index) = self
            .normal_window_geometries
            .iter()
            .position(|(saved, _)| saved == &wl_surface)
        {
            let (_, geometry) = self.normal_window_geometries.remove(index);
            surface.with_pending_state(|state| state.size = Some(geometry.size));
            if let Some(window) = self.window_for_surface(&wl_surface)
                && !self
                    .minimized_windows
                    .iter()
                    .any(|entry| entry.window == window)
            {
                let from = self
                    .space
                    .element_location(&window)
                    .map(|location| Rectangle::new(location, window.geometry().size))
                    .unwrap_or(geometry);
                self.space.raise_element(&window, true);
                self.start_geometry_animation(&wl_surface, from, geometry, true, 260);
            } else {
                surface.send_pending_configure();
            }
        } else {
            surface.send_pending_configure();
        }
    }

    fn minimize_request(&mut self, surface: ToplevelSurface) {
        self.minimize_surface(surface.wl_surface());
    }

    fn toplevel_destroyed(&mut self, surface: ToplevelSurface) {
        let wl_surface = surface.wl_surface().clone();
        if let Some(window) = self.window_for_surface(&wl_surface) {
            self.space.unmap_elem(&window);
        }
        self.minimized_windows.retain(|entry| {
            !entry
                .window
                .toplevel()
                .is_some_and(|toplevel| toplevel.wl_surface() == &wl_surface)
        });
        self.normal_window_geometries
            .retain(|(saved_surface, _)| saved_surface != &wl_surface);
        self.forget_toplevel(&wl_surface);
    }

    fn app_id_changed(&mut self, surface: ToplevelSurface) {
        self.update_window_role(surface.wl_surface());
    }

    fn title_changed(&mut self, surface: ToplevelSurface) {
        self.update_window_role(surface.wl_surface());
    }

    fn parent_changed(&mut self, surface: ToplevelSurface) {
        self.update_window_role(surface.wl_surface());
    }

    fn grab(&mut self, _surface: PopupSurface, _seat: wl_seat::WlSeat, _serial: Serial) {
        // TODO popup grabs
    }
}

impl XdgDecorationHandler for Smallvil {
    fn new_decoration(&mut self, toplevel: ToplevelSurface) {
        set_server_decoration(&toplevel);
    }

    fn request_mode(
        &mut self,
        toplevel: ToplevelSurface,
        _mode: smithay::reexports::wayland_protocols::xdg::decoration::zv1::server::zxdg_toplevel_decoration_v1::Mode,
    ) {
        set_server_decoration(&toplevel);
    }

    fn unset_mode(&mut self, toplevel: ToplevelSurface) {
        set_server_decoration(&toplevel);
    }
}

fn set_server_decoration(toplevel: &ToplevelSurface) {
    use smithay::reexports::wayland_protocols::xdg::decoration::zv1::server::zxdg_toplevel_decoration_v1::Mode;
    toplevel.with_pending_state(|state| state.decoration_mode = Some(Mode::ServerSide));
    toplevel.send_pending_configure();
}

// Xdg Shell
delegate_xdg_shell!(Smallvil);
delegate_xdg_decoration!(Smallvil);

fn check_grab(
    seat: &Seat<Smallvil>,
    surface: &WlSurface,
    serial: Serial,
) -> Option<PointerGrabStartData<Smallvil>> {
    let pointer = seat.get_pointer()?;

    // Check that this surface has a click grab.
    if !pointer.has_grab(serial) {
        return None;
    }

    let start_data = pointer.grab_start_data()?;

    let (focus, _) = start_data.focus.as_ref()?;
    // If the focus was for a different surface, ignore the request.
    if !focus.id().same_client_as(&surface.id()) {
        return None;
    }

    Some(start_data)
}

/// Should be called on `WlSurface::commit`
pub fn handle_commit(
    popups: &mut PopupManager,
    space: &Space<Window>,
    surface: &WlSurface,
    _is_desktop: bool,
) {
    // Handle toplevel commits.
    if let Some(window) = space
        .elements()
        .find(|window| {
            window
                .toplevel()
                .is_some_and(|toplevel| toplevel.wl_surface() == surface)
        })
        .cloned()
    {
        let initial_configure_sent = with_states(surface, |states| {
            states
                .data_map
                .get::<XdgToplevelSurfaceData>()
                .unwrap()
                .lock()
                .unwrap()
                .initial_configure_sent
        });

        if !initial_configure_sent {
            window.toplevel().unwrap().send_configure();
        }
    }

    // Handle popup commits.
    popups.commit(surface);
    if let Some(popup) = popups.find_popup(surface) {
        match popup {
            PopupKind::Xdg(ref xdg) => {
                if !xdg.is_initial_configure_sent() {
                    // NOTE: This should never fail as the initial configure is always
                    // allowed.
                    xdg.send_configure().expect("initial configure failed");
                }
            }
            PopupKind::InputMethod(ref _input_method) => {}
        }
    }
}

impl Smallvil {
    fn unconstrain_popup(&self, popup: &PopupSurface) {
        let Ok(root) = find_popup_root_surface(&PopupKind::Xdg(popup.clone())) else {
            return;
        };
        let Some(window) = self.space.elements().find(|window| {
            window
                .toplevel()
                .is_some_and(|toplevel| toplevel.wl_surface() == &root)
        }) else {
            return;
        };

        let output = self.space.outputs().next().unwrap();
        let output_geo = self.space.output_geometry(output).unwrap();
        let window_geo = self.space.element_geometry(window).unwrap();

        // The target geometry for the positioner should be relative to its parent's geometry, so
        // we will compute that here.
        let mut target = output_geo;
        target.loc -= get_popup_toplevel_coords(&PopupKind::Xdg(popup.clone()));
        target.loc -= window_geo.loc;

        popup.with_pending_state(|state| {
            state.geometry = state.positioner.get_unconstrained_geometry(target);
        });
    }
}
