use crate::{
    grabs::{MoveSurfaceGrab, ResizeEdge, ResizeSurfaceGrab},
    state::Smallvil,
};
use smithay::{
    backend::input::{
        AbsolutePositionEvent, Axis, AxisSource, ButtonState, Event, InputBackend, InputEvent,
        KeyState, KeyboardKeyEvent, PointerAxisEvent, PointerButtonEvent, PointerMotionEvent,
    },
    input::{
        keyboard::{FilterResult, keysyms},
        pointer::{AxisFrame, ButtonEvent, MotionEvent},
    },
    reexports::wayland_server::protocol::wl_surface::WlSurface,
    utils::{Rectangle, SERIAL_COUNTER},
};

impl Smallvil {
    pub fn release_pressed_keys(&mut self) {
        let Some(keyboard) = self.seat.get_keyboard() else {
            return;
        };
        let pressed_keys = keyboard.pressed_keys();
        let time = self.start_time.elapsed().as_millis() as u32;

        for key_code in pressed_keys {
            keyboard.input::<(), _>(
                self,
                key_code,
                KeyState::Released,
                SERIAL_COUNTER.next_serial(),
                time,
                |_, _, _| FilterResult::Forward,
            );
        }
    }

    pub fn process_input_event<I: InputBackend>(&mut self, event: InputEvent<I>) {
        match event {
            InputEvent::Keyboard { event, .. } => {
                let serial = SERIAL_COUNTER.next_serial();
                let time = Event::time_msec(&event);
                let key_state = event.state();

                self.seat.get_keyboard().unwrap().input::<(), _>(
                    self,
                    event.key_code(),
                    key_state,
                    serial,
                    time,
                    |state, modifiers, key| {
                        let keysym = key.modified_sym();
                        if key_state == KeyState::Pressed {
                            if modifiers.logo
                                && modifiers.shift
                                && keysym == keysyms::KEY_Escape.into()
                            {
                                state.loop_signal.stop();
                                return FilterResult::Intercept(());
                            }
                            if modifiers.ctrl && modifiers.alt {
                                let vt_switch_keys = [
                                    (keysyms::KEY_XF86Switch_VT_1.into(), 1),
                                    (keysyms::KEY_XF86Switch_VT_2.into(), 2),
                                    (keysyms::KEY_XF86Switch_VT_3.into(), 3),
                                    (keysyms::KEY_XF86Switch_VT_4.into(), 4),
                                    (keysyms::KEY_XF86Switch_VT_5.into(), 5),
                                    (keysyms::KEY_XF86Switch_VT_6.into(), 6),
                                    (keysyms::KEY_XF86Switch_VT_7.into(), 7),
                                    (keysyms::KEY_XF86Switch_VT_8.into(), 8),
                                    (keysyms::KEY_XF86Switch_VT_9.into(), 9),
                                    (keysyms::KEY_XF86Switch_VT_10.into(), 10),
                                    (keysyms::KEY_XF86Switch_VT_11.into(), 11),
                                    (keysyms::KEY_XF86Switch_VT_12.into(), 12),
                                    (keysyms::KEY_F1.into(), 1),
                                    (keysyms::KEY_F2.into(), 2),
                                    (keysyms::KEY_F3.into(), 3),
                                    (keysyms::KEY_F4.into(), 4),
                                    (keysyms::KEY_F5.into(), 5),
                                    (keysyms::KEY_F6.into(), 6),
                                    (keysyms::KEY_F7.into(), 7),
                                    (keysyms::KEY_F8.into(), 8),
                                    (keysyms::KEY_F9.into(), 9),
                                    (keysyms::KEY_F10.into(), 10),
                                    (keysyms::KEY_F11.into(), 11),
                                    (keysyms::KEY_F12.into(), 12),
                                ];
                                let virtual_terminal = vt_switch_keys
                                    .into_iter()
                                    .find_map(|(switch_key, vt)| {
                                        (keysym == switch_key).then_some(vt)
                                    })
                                    .or_else(|| {
                                        key.raw_syms().into_iter().find_map(|raw_key| {
                                            vt_switch_keys.iter().find_map(|(switch_key, vt)| {
                                                (raw_key == *switch_key && *vt <= 12).then_some(*vt)
                                            })
                                        })
                                    });
                                if let Some(virtual_terminal) = virtual_terminal {
                                    state.switch_virtual_terminal(virtual_terminal);
                                    return FilterResult::Intercept(());
                                }
                            }
                            if modifiers.alt && keysym == keysyms::KEY_Tab.into() {
                                state.focus_next_window();
                                return FilterResult::Intercept(());
                            }
                            if modifiers.logo && keysym == keysyms::KEY_Up.into() {
                                if let Some(surface) = state
                                    .seat
                                    .get_keyboard()
                                    .and_then(|keyboard| keyboard.current_focus())
                                {
                                    state.set_maximized(&surface, true);
                                }
                                return FilterResult::Intercept(());
                            }
                            if modifiers.logo && keysym == keysyms::KEY_Down.into() {
                                if let Some(surface) = state
                                    .seat
                                    .get_keyboard()
                                    .and_then(|keyboard| keyboard.current_focus())
                                {
                                    let is_maximized = state
                                        .normal_window_geometries
                                        .iter()
                                        .any(|(saved, _)| saved == &surface);
                                    let is_expanded_tile = state
                                        .expanded_tiled_window
                                        .as_ref()
                                        .is_some_and(|expanded| expanded == &surface);
                                    if is_maximized || is_expanded_tile {
                                        state.set_maximized(&surface, false);
                                    } else {
                                        state.minimize_surface(&surface);
                                    }
                                }

                                return FilterResult::Intercept(());
                            }
                            if modifiers.logo && keysym == keysyms::KEY_h.into() {
                                if let Some(surface) = state
                                    .seat
                                    .get_keyboard()
                                    .and_then(|keyboard| keyboard.current_focus())
                                {
                                    state.minimize_surface(&surface);
                                }
                                return FilterResult::Intercept(());
                            }
                            if modifiers.logo && keysym == keysyms::KEY_Tab.into() {
                                state.focus_next_window();
                                return FilterResult::Intercept(());
                            }
                        }

                        FilterResult::Forward
                    },
                );
            }
            InputEvent::PointerMotion { event, .. } => {
                let position = self
                    .seat
                    .get_pointer()
                    .map(|pointer| pointer.current_location())
                    .unwrap_or(self.pointer_location)
                    + event.delta();
                self.process_pointer_motion(position, event.time_msec());
            }
            InputEvent::PointerMotionAbsolute { event, .. } => {
                let output = self.space.outputs().next().unwrap();
                let output_geo = self.space.output_geometry(output).unwrap();
                let pos = event.position_transformed(output_geo.size) + output_geo.loc.to_f64();
                self.process_pointer_motion(pos, event.time_msec());
            }
            InputEvent::PointerButton { event, .. } => {
                let pointer = self.seat.get_pointer().unwrap();
                let keyboard = self.seat.get_keyboard().unwrap();

                let serial = SERIAL_COUNTER.next_serial();

                let button = event.button_code();

                let button_state = event.state();
                let modifiers = keyboard.modifier_state();
                let mut compositor_grab = None;

                const BTN_LEFT: u32 = 0x110;
                if button == BTN_LEFT && button_state == ButtonState::Released {
                    if self.titlebar_drag.is_some() {
                        self.finish_titlebar_drag(pointer.current_location());
                        pointer.frame(self);
                        return;
                    }
                }

                if button == BTN_LEFT && button_state == ButtonState::Pressed {
                    if let Some((surface, edges)) = self.resize_hit(pointer.current_location())
                        && let Some(window) = self.window_for_surface(&surface)
                    {
                        let location = self.space.element_location(&window).unwrap_or_default();
                        let rect = Rectangle::new(location, window.geometry().size);
                        let tiled_resize = window.toplevel().is_some()
                            && self.window_mode == crate::state::WindowMode::Tiling
                            && !self.is_floating_surface(&surface);
                        let tile_frames = tiled_resize.then(|| self.tile_frames.clone());

                        self.space.raise_element(&window, true);
                        keyboard.set_focus(self, Some(surface.clone()), serial);
                        let pointer_location = pointer.current_location();
                        pointer.motion(
                            self,
                            None,
                            &MotionEvent {
                                location: pointer_location,
                                serial,
                                time: event.time_msec(),
                            },
                        );
                        pointer.button(
                            self,
                            &ButtonEvent {
                                button,
                                state: button_state,
                                serial,
                                time: event.time_msec(),
                            },
                        );
                        if let Some(start_data) = pointer.grab_start_data() {
                            if let Some(toplevel) = window.toplevel() {
                                toplevel.with_pending_state(|state| {
                                    state.states.set(smithay::reexports::wayland_protocols::xdg::shell::server::xdg_toplevel::State::Resizing);
                                });
                                toplevel.send_pending_configure();
                            }
                            pointer.set_grab(
                                self,
                                ResizeSurfaceGrab::start(
                                    start_data,
                                    window,
                                    edges,
                                    rect,
                                    tile_frames,
                                ),
                                serial,
                                smithay::input::pointer::Focus::Clear,
                            );
                        }
                        pointer.frame(self);
                        return;
                    }

                    if let Some((surface, action, is_titlebar)) =
                        self.decoration_hit(pointer.current_location())
                    {
                        if let Some(window) = self.window_for_surface(&surface) {
                            self.focus_window(&window);
                        }
                        if let Some(action) = action {
                            match action {
                                crate::state::FrameAction::Close => {
                                    if let Some(window) = self.window_for_surface(&surface) {
                                        if let Some(toplevel) = window.toplevel() {
                                            toplevel.send_close();
                                        } else if let Some(x11_surface) = window.x11_surface()
                                            && let Err(error) = x11_surface.close()
                                        {
                                            eprintln!(
                                                "coranux_wm: impossible de fermer la fenêtre X11 : {error}"
                                            );
                                        }
                                    }
                                }
                                crate::state::FrameAction::Minimize => {
                                    self.minimize_surface(&surface);
                                }
                                crate::state::FrameAction::Maximize => {
                                    self.toggle_maximized(&surface);
                                }
                                crate::state::FrameAction::ToggleMode => {
                                    self.toggle_window_mode();
                                }
                            }
                        } else if is_titlebar {
                            self.start_titlebar_drag(surface, pointer.current_location());
                        }
                        pointer.frame(self);
                        return;
                    }
                }

                if ButtonState::Pressed == button_state && !pointer.is_grabbed() {
                    if let Some((window, _loc)) = self
                        .space
                        .element_under(pointer.current_location())
                        .map(|(w, l)| (w.clone(), l))
                    {
                        self.space.raise_element(&window, true);
                        let Some(surface) = self.wl_surface_for_window(&window) else {
                            pointer.frame(self);
                            return;
                        };
                        let is_panel = self.is_panel_surface(&surface);
                        if !is_panel {
                            keyboard.set_focus(self, Some(surface.clone()), serial);
                        }
                        self.space.elements().for_each(|window| {
                            if let Some(toplevel) = window.toplevel() {
                                toplevel.send_pending_configure();
                            }
                        });

                        const BTN_RIGHT: u32 = 0x111;
                        let can_manually_move = !is_panel
                            && (self.window_mode == crate::state::WindowMode::Free
                                || self.is_floating_surface(&surface));
                        let can_manually_resize = !self.is_fullscreen_or_maximized(&surface);
                        if can_manually_move
                            && modifiers.logo
                            && (button == BTN_LEFT || (button == BTN_RIGHT && can_manually_resize))
                        {
                            if let Some(location) = self.space.element_location(&window) {
                                compositor_grab = Some((window, location));
                            }
                        }
                    } else {
                        self.space.elements().for_each(|window| {
                            window.set_activated(false);
                            if let Some(toplevel) = window.toplevel() {
                                toplevel.send_pending_configure();
                            }
                        });
                        keyboard.set_focus(self, Option::<WlSurface>::None, serial);
                    }
                };

                pointer.button(
                    self,
                    &ButtonEvent {
                        button,
                        state: button_state,
                        serial,
                        time: event.time_msec(),
                    },
                );

                if ButtonState::Pressed == button_state && modifiers.logo {
                    if let Some((window, location)) = compositor_grab {
                        if let Some(start_data) = pointer.grab_start_data() {
                            const BTN_RIGHT: u32 = 0x111;
                            if button == BTN_LEFT {
                                pointer.set_grab(
                                    self,
                                    MoveSurfaceGrab {
                                        start_data,
                                        window,
                                        initial_window_location: location,
                                    },
                                    serial,
                                    smithay::input::pointer::Focus::Clear,
                                );
                            } else if button == BTN_RIGHT {
                                let geometry = window.geometry();
                                let rect = Rectangle::new(location, geometry.size);
                                let pointer_location = pointer.current_location();
                                let horizontal_edge = if pointer_location.x
                                    < (location.x + geometry.size.w / 2) as f64
                                {
                                    ResizeEdge::LEFT
                                } else {
                                    ResizeEdge::RIGHT
                                };
                                let vertical_edge = if pointer_location.y
                                    < (location.y + geometry.size.h / 2) as f64
                                {
                                    ResizeEdge::TOP
                                } else {
                                    ResizeEdge::BOTTOM
                                };
                                let resize_edges = horizontal_edge | vertical_edge;
                                if let Some(toplevel) = window.toplevel() {
                                    toplevel.with_pending_state(|state| {
                                        state.states.set(smithay::reexports::wayland_protocols::xdg::shell::server::xdg_toplevel::State::Resizing);
                                    });
                                    toplevel.send_pending_configure();
                                    pointer.set_grab(
                                        self,
                                        ResizeSurfaceGrab::start(
                                            start_data,
                                            window,
                                            resize_edges,
                                            rect,
                                            None,
                                        ),
                                        serial,
                                        smithay::input::pointer::Focus::Clear,
                                    );
                                }
                            }
                        }
                    }
                }
                pointer.frame(self);
            }
            InputEvent::PointerAxis { event, .. } => {
                let source = event.source();

                let horizontal_amount = event.amount(Axis::Horizontal).unwrap_or_else(|| {
                    event.amount_v120(Axis::Horizontal).unwrap_or(0.0) * 15.0 / 120.
                });
                let vertical_amount = event.amount(Axis::Vertical).unwrap_or_else(|| {
                    event.amount_v120(Axis::Vertical).unwrap_or(0.0) * 15.0 / 120.
                });
                let horizontal_amount_discrete = event.amount_v120(Axis::Horizontal);
                let vertical_amount_discrete = event.amount_v120(Axis::Vertical);

                let mut frame = AxisFrame::new(event.time_msec()).source(source);
                if horizontal_amount != 0.0 {
                    frame = frame.value(Axis::Horizontal, horizontal_amount);
                    if let Some(discrete) = horizontal_amount_discrete {
                        frame = frame.v120(Axis::Horizontal, discrete as i32);
                    }
                }
                if vertical_amount != 0.0 {
                    frame = frame.value(Axis::Vertical, vertical_amount);
                    if let Some(discrete) = vertical_amount_discrete {
                        frame = frame.v120(Axis::Vertical, discrete as i32);
                    }
                }

                if source == AxisSource::Finger {
                    if event.amount(Axis::Horizontal) == Some(0.0) {
                        frame = frame.stop(Axis::Horizontal);
                    }
                    if event.amount(Axis::Vertical) == Some(0.0) {
                        frame = frame.stop(Axis::Vertical);
                    }
                }

                let pointer = self.seat.get_pointer().unwrap();
                pointer.axis(self, frame);
                pointer.frame(self);
            }
            _ => {}
        }
    }

    fn process_pointer_motion(
        &mut self,
        position: smithay::utils::Point<f64, smithay::utils::Logical>,
        time: u32,
    ) {
        let Some(output) = self.space.outputs().next() else {
            return;
        };
        let Some(output_geometry) = self.space.output_geometry(output) else {
            return;
        };
        let max_x = (output_geometry.loc.x + output_geometry.size.w - 1).max(output_geometry.loc.x);
        let max_y = (output_geometry.loc.y + output_geometry.size.h - 1).max(output_geometry.loc.y);
        let position = (
            position.x.clamp(output_geometry.loc.x as f64, max_x as f64),
            position.y.clamp(output_geometry.loc.y as f64, max_y as f64),
        )
            .into();
        self.pointer_location = position;

        if self.titlebar_drag.is_some()
            || self
                .seat
                .get_pointer()
                .is_some_and(|pointer| pointer.is_grabbed())
        {}

        let has_titlebar_drag = self.titlebar_drag.is_some();
        self.update_titlebar_drag(position);
        let serial = SERIAL_COUNTER.next_serial();
        let pointer = self.seat.get_pointer().unwrap();
        let under = if has_titlebar_drag {
            None
        } else {
            self.surface_under(position)
        };
        pointer.motion(
            self,
            under,
            &MotionEvent {
                location: position,
                serial,
                time,
            },
        );
        pointer.frame(self);
    }
}

impl Smallvil {
    #[cfg(target_os = "linux")]
    fn switch_virtual_terminal(&mut self, virtual_terminal: i32) {
        use smithay::backend::session::Session;

        let Some(session) = self.seat_session.as_mut() else {
            eprintln!("coranux_wm: session libseat indisponible pour changer de TTY");
            return;
        };
        if let Err(error) = session.change_vt(virtual_terminal) {
            eprintln!(
                "coranux_wm: impossible de basculer vers le TTY {virtual_terminal} via libseat : {error}"
            );
        }
    }

    #[cfg(not(target_os = "linux"))]
    fn switch_virtual_terminal(&mut self, virtual_terminal: i32) {
        eprintln!(
            "coranux_wm: changement vers le TTY {virtual_terminal} indisponible sur cette plateforme"
        );
    }
}
