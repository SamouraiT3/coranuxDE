use std::{
    cell::RefCell, ffi::OsString, path::PathBuf, process::Stdio, rc::Rc, sync::Arc, time::Instant,
};

use serde::Serialize;
use smithay::{
    backend::renderer::{
        Color32F,
        element::{Element, Id, Kind, solid::SolidColorRenderElement},
        gles::GlesRenderer,
        utils::CommitCounter,
    },
    backend::session::libseat::LibSeatSession,
    desktop::{PopupManager, Space, Window, WindowSurfaceType},
    input::pointer::CursorImageStatus,
    input::{Seat, SeatState},
    reexports::{
        calloop::{EventLoop, Interest, LoopSignal, Mode, PostAction, generic::Generic},
        wayland_server::{
            Display, DisplayHandle,
            backend::{ClientData, ClientId, DisconnectReason},
            protocol::wl_surface::WlSurface,
        },
    },
    utils::{Logical, Physical, Point, Rectangle, Size},
    wayland::{
        compositor::{CompositorClientState, CompositorState, with_states},
        dmabuf::{DmabufGlobal, DmabufState},
        output::OutputManagerState,
        selection::data_device::DataDeviceState,
        shell::xdg::{XdgShellState, XdgToplevelSurfaceData, decoration::XdgDecorationState},
        shm::ShmState,
        socket::ListeningSocketSource,
        xwayland_shell::XWaylandShellState,
    },
    xwayland::{X11Surface, X11Wm, XWayland, XWaylandEvent},
};

use crate::CalloopData;
use crate::grabs::ResizeEdge;

pub(crate) const FRAME_BORDER: i32 = 5;
const TITLEBAR_HEIGHT: i32 = 30;
pub(crate) const TITLEBAR_TOTAL_HEIGHT: i32 = TITLEBAR_HEIGHT + FRAME_BORDER;
const CONTROL_SIZE: i32 = 20;
const CONTROL_GAP: i32 = 5;
const PANEL_HEIGHT: i32 = 48;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum WindowMode {
    #[default]
    Free,
    Tiling,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FrameAction {
    Close,
    Minimize,
    Maximize,
    ToggleMode,
}

#[derive(Clone, Debug)]
pub struct FrameControl {
    pub action: FrameAction,
    pub rect: Rectangle<i32, Logical>,
}

#[derive(Clone, Debug)]
pub struct WindowFrame {
    pub surface: WlSurface,
    pub outer: Rectangle<i32, Logical>,
    pub titlebar: Rectangle<i32, Logical>,
    pub controls: Vec<FrameControl>,
    pub focused: bool,
    pub tiled: bool,
    pub fullscreen: bool,
}

#[derive(Clone, Debug)]
pub struct TitlebarDrag {
    pub surface: WlSurface,
    pub start: Point<f64, Logical>,
    pub current: Point<f64, Logical>,
    pub initial_window_location: Point<i32, Logical>,
    pub moved: bool,
    pub restore_on_drag: Option<(Rectangle<i32, Logical>, f64)>,
}

#[derive(Clone)]
pub struct MinimizedWindow {
    pub window: Window,
    pub location: Point<i32, Logical>,
    pub size: Size<i32, Logical>,
    pub mode: WindowMode,
}

#[derive(Clone, Debug)]
pub struct WindowGeometryAnimation {
    pub surface: WlSurface,
    pub from: Rectangle<i32, Logical>,
    pub to: Rectangle<i32, Logical>,
    pub started: Instant,
    pub duration: std::time::Duration,
    pub configure_client: bool,
    pub last_tick: Instant,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct PanelWindowInfo {
    pub id: u64,
    pub title: String,
    pub app_id: Option<String>,
    pub minimized: bool,
    pub focused: bool,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize)]
pub struct PanelSnapshot {
    pub windows: Vec<PanelWindowInfo>,
    #[serde(default)]
    pub keyboard_layout: String,
}

pub struct Smallvil {
    pub start_time: std::time::Instant,
    pub seat_session: Option<LibSeatSession>,
    pub socket_name: OsString,
    pub display_handle: DisplayHandle,

    pub space: Space<Window>,
    pub loop_signal: LoopSignal,
    pub normal_window_geometries: Vec<(WlSurface, Rectangle<i32, Logical>)>,
    pub minimized_windows: Vec<MinimizedWindow>,
    pub next_window_offset: i32,
    pub window_mode: WindowMode,
    pub tiled_windows: Vec<WlSurface>,
    pub desktop_surfaces: Vec<WlSurface>,
    pub floating_surfaces: Vec<WlSurface>,
    pub free_window_geometries: Vec<(WlSurface, Rectangle<i32, Logical>)>,
    pub tile_frames: Vec<(WlSurface, Rectangle<i32, Logical>)>,
    pub tile_layout_overrides: Option<(
        Rectangle<i32, Logical>,
        Vec<(WlSurface, Rectangle<i32, Logical>)>,
    )>,
    pub expanded_tiled_window: Option<WlSurface>,
    pub titlebar_drag: Option<TitlebarDrag>,
    pub last_titlebar_click: Option<(WlSurface, Instant)>,
    pub pointer_location: Point<f64, Logical>,
    pub cursor_image_status: CursorImageStatus,
    pub pending_open_animations: Vec<WlSurface>,
    pub geometry_animations: Vec<WindowGeometryAnimation>,

    // Smithay State
    pub compositor_state: CompositorState,
    pub xdg_shell_state: XdgShellState,
    pub xwayland_shell_state: XWaylandShellState,
    pub xwayland_wm: Option<X11Wm>,
    pub xwayland_display_file: Option<PathBuf>,
    pub xdg_decoration_state: XdgDecorationState,
    pub shm_state: ShmState,
    pub dmabuf_state: DmabufState,
    pub dmabuf_global: Option<DmabufGlobal>,
    pub dmabuf_renderer: Option<Rc<RefCell<GlesRenderer>>>,
    pub output_manager_state: OutputManagerState,
    pub seat_state: SeatState<Smallvil>,
    pub data_device_state: DataDeviceState,
    pub popups: PopupManager,

    pub seat: Seat<Self>,
    keyboard_layouts: Vec<String>,
    keyboard_layout_index: usize,
    pub panel_surfaces: Vec<WlSurface>,
    panel_window_ids: Vec<(WlSurface, u64)>,
    next_panel_window_id: u64,
    pub desktop_commit_count: u8,
    pub desktop_keyboard_focus_set: bool,
}

fn configured_keyboard_layouts() -> Vec<String> {
    let configured = std::env::var("CORANUX_KEYBOARD_LAYOUTS")
        .or_else(|_| std::env::var("XKB_DEFAULT_LAYOUT"))
        .unwrap_or_else(|_| "fr,us".to_owned());
    let mut layouts = Vec::new();
    for layout in configured.split(',').map(str::trim) {
        if layout.is_empty()
            || !layout.chars().all(|character| {
                character.is_ascii_alphanumeric() || character == '_' || character == '-'
            })
        {
            eprintln!("coranux_wm: disposition clavier invalide ignorée : {layout}");
            continue;
        }
        if !layouts.iter().any(|known| known == layout) {
            layouts.push(layout.to_owned());
        }
    }
    if layouts.is_empty() {
        layouts.extend(["fr".to_owned(), "us".to_owned()]);
    }
    layouts
}

impl Smallvil {
    pub fn new(
        event_loop: &mut EventLoop<'static, CalloopData>,
        display: Display<Self>,
    ) -> Result<Self, Box<dyn std::error::Error>> {
        let start_time = std::time::Instant::now();

        let dh = display.handle();

        let compositor_state = CompositorState::new::<Self>(&dh);
        let xdg_shell_state = XdgShellState::new::<Self>(&dh);
        let xwayland_shell_state = XWaylandShellState::new::<Self>(&dh);
        let xdg_decoration_state = XdgDecorationState::new::<Self>(&dh);
        let shm_state = ShmState::new::<Self>(&dh, vec![]);
        let dmabuf_state = DmabufState::new();
        let output_manager_state = OutputManagerState::new_with_xdg_output::<Self>(&dh);
        let mut seat_state = SeatState::new();
        let data_device_state = DataDeviceState::new::<Self>(&dh);
        let popups = PopupManager::default();

        // A seat is a group of keyboards, pointer and touch devices.
        // A seat typically has a pointer and maintains a keyboard focus and a pointer focus.
        let mut seat: Seat<Self> = seat_state.new_wl_seat(&dh, "coranux");

        // Notify clients that we have a keyboard, for the sake of the example we assume that keyboard is always present.
        // You may want to track keyboard hot-plug in real compositor.
        let keyboard_layouts = configured_keyboard_layouts();
        let keyboard_layout_index = 0;
        seat.add_keyboard(
            smithay::input::keyboard::XkbConfig {
                layout: &keyboard_layouts[keyboard_layout_index],
                ..Default::default()
            },
            200,
            25,
        )
        .map_err(|error| format!("impossible d’initialiser le clavier : {error:?}"))?;

        // Notify clients that we have a pointer (mouse)
        // Here we assume that there is always pointer plugged in
        seat.add_pointer();

        // A space represents a two-dimensional plane. Windows and Outputs can be mapped onto it.
        //
        // Windows get a position and stacking order through mapping.
        // Outputs become views of a part of the Space and can be rendered via Space::render_output.
        let space = Space::default();

        let socket_name = Self::init_wayland_listener(display, event_loop)?;

        // Get the loop signal, used to stop the event loop
        let loop_signal = event_loop.get_signal();

        let xwayland_display_file =
            std::env::var_os("CORANUX_XWAYLAND_DISPLAY_FILE").map(PathBuf::from);
        let state = Self {
            start_time,
            seat_session: None,
            display_handle: dh,

            space,
            loop_signal,
            normal_window_geometries: Vec::new(),
            minimized_windows: Vec::new(),
            next_window_offset: 0,
            window_mode: WindowMode::Free,
            tiled_windows: Vec::new(),
            desktop_surfaces: Vec::new(),
            floating_surfaces: Vec::new(),
            free_window_geometries: Vec::new(),
            tile_frames: Vec::new(),
            tile_layout_overrides: None,
            expanded_tiled_window: None,
            titlebar_drag: None,
            last_titlebar_click: None,
            pointer_location: Point::from((0.0, 0.0)),
            cursor_image_status: CursorImageStatus::default_named(),
            pending_open_animations: Vec::new(),
            geometry_animations: Vec::new(),
            socket_name,

            compositor_state,
            xdg_shell_state,
            xwayland_shell_state,
            xwayland_wm: None,
            xwayland_display_file: xwayland_display_file.clone(),
            xdg_decoration_state,
            shm_state,
            dmabuf_state,
            dmabuf_global: None,
            dmabuf_renderer: None,
            output_manager_state,
            seat_state,
            data_device_state,
            popups,
            seat,
            keyboard_layouts,
            keyboard_layout_index,
            panel_surfaces: Vec::new(),
            panel_window_ids: Vec::new(),
            next_panel_window_id: 1,
            desktop_commit_count: 0,
            desktop_keyboard_focus_set: false,
        };

        let (xwayland, client) = XWayland::spawn(
            &state.display_handle,
            None,
            std::iter::empty::<(String, String)>(),
            true,
            Stdio::null(),
            Stdio::null(),
            |_| {},
        )?;
        let handle = event_loop.handle();
        let xwayland_loop_handle = handle.clone();
        handle.insert_source(xwayland, move |event, _, data| match event {
            XWaylandEvent::Ready {
                x11_socket,
                display_number,
            } => match X11Wm::start_wm(xwayland_loop_handle.clone(), x11_socket, client.clone()) {
                Ok(xwm) => {
                    data.state.xwayland_wm = Some(xwm);
                    if let Some(path) = &xwayland_display_file {
                        if let Err(error) = std::fs::write(path, format!(":{display_number}")) {
                            eprintln!(
                                "coranux_wm: impossible de publier l’affichage XWayland : {error}"
                            );
                            data.state.loop_signal.stop();
                        }
                    }
                    eprintln!("coranux_wm: XWayland prêt sur :{display_number}");
                }
                Err(error) => {
                    eprintln!("coranux_wm: impossible d’initialiser le gestionnaire X11 : {error}");
                    data.state.loop_signal.stop();
                }
            },
            XWaylandEvent::Error => {
                eprintln!("coranux_wm: le serveur XWayland s’est arrêté pendant son démarrage");
                data.state.loop_signal.stop();
            }
        })?;

        Ok(state)
    }

    fn init_wayland_listener(
        display: Display<Smallvil>,
        event_loop: &mut EventLoop<CalloopData>,
    ) -> Result<OsString, Box<dyn std::error::Error>> {
        // Creates a new listening socket, automatically choosing the next available `wayland` socket name.
        let listening_socket = match std::env::var("CORANUX_WAYLAND_DISPLAY") {
            Ok(name) => ListeningSocketSource::with_name(&name)?,
            Err(_) => ListeningSocketSource::new_auto()?,
        };

        // Get the name of the listening socket.
        // Clients will connect to this socket.
        let socket_name = listening_socket.socket_name().to_os_string();

        let loop_handle = event_loop.handle();

        loop_handle
            .insert_source(listening_socket, move |client_stream, _, state| {
                // Inside the callback, you should insert the client into the display.
                //
                // You may also associate some data with the client when inserting the client.
                state
                    .display_handle
                    .insert_client(client_stream, Arc::new(ClientState::default()))
                    .unwrap();
            })
            .expect("Failed to init the wayland event source.");

        // You also need to add the display itself to the event loop, so that client events will be processed by wayland-server.
        loop_handle
            .insert_source(
                Generic::new(display, Interest::READ, Mode::Level),
                |_, display, state| {
                    // Safety: we don't drop the display
                    unsafe {
                        display
                            .get_mut()
                            .dispatch_clients(&mut state.state)
                            .unwrap();
                    }
                    Ok(PostAction::Continue)
                },
            )
            .unwrap();

        Ok(socket_name)
    }

    pub fn surface_under(
        &self,
        pos: Point<f64, Logical>,
    ) -> Option<(WlSurface, Point<f64, Logical>)> {
        self.space
            .element_under(pos)
            .and_then(|(window, location)| {
                window
                    .surface_under(pos - location.to_f64(), WindowSurfaceType::ALL)
                    .map(|(s, p)| (s, (p + location).to_f64()))
            })
    }

    pub fn window_for_surface(&self, surface: &WlSurface) -> Option<Window> {
        self.space
            .elements()
            .find(|window| {
                self.wl_surface_for_window(window)
                    .is_some_and(|window_surface| window_surface == *surface)
            })
            .cloned()
            .or_else(|| {
                self.minimized_windows
                    .iter()
                    .find(|entry| {
                        self.wl_surface_for_window(&entry.window)
                            .is_some_and(|window_surface| window_surface == *surface)
                    })
                    .map(|entry| entry.window.clone())
            })
    }

    pub fn wl_surface_for_window(&self, window: &Window) -> Option<WlSurface> {
        window
            .toplevel()
            .map(|toplevel| toplevel.wl_surface().clone())
            .or_else(|| window.x11_surface().and_then(X11Surface::wl_surface))
    }

    pub fn map_x11_window(&mut self, surface: X11Surface) {
        if self.find_x11_window(&surface).is_some() {
            return;
        }

        let output_size = self
            .space
            .outputs()
            .next()
            .and_then(|output| self.space.output_geometry(output))
            .map(|geometry| geometry.size);
        let geometry = surface.geometry();
        let size = if geometry.size.w > 0 && geometry.size.h > 0 {
            geometry.size
        } else {
            output_size
                .map(|size| (size.w * 3 / 4, size.h * 3 / 4).into())
                .unwrap_or_else(|| (960, 640).into())
        };
        let offset = 40 + self.next_window_offset.rem_euclid(240);
        self.next_window_offset = (self.next_window_offset + 36).rem_euclid(240);
        let location = (offset, offset);
        let window = Window::new_x11_window(surface.clone());
        let activate = !surface.is_override_redirect();

        if let Err(error) = surface.configure(Rectangle::new(location.into(), size)) {
            eprintln!("coranux_wm: impossible de placer une fenêtre X11 : {error}");
        }
        self.space.map_element(window.clone(), location, activate);
        if !surface.is_override_redirect() {
            let wl_surface = surface.wl_surface();
            if let Some(wl_surface) = wl_surface {
                self.ensure_panel_window_id(&wl_surface);
                if !self
                    .free_window_geometries
                    .iter()
                    .any(|(known, _)| known == &wl_surface)
                {
                    self.free_window_geometries
                        .push((wl_surface.clone(), Rectangle::new(location.into(), size)));
                }
                if self.window_mode == WindowMode::Tiling {
                    self.tiled_windows.push(wl_surface);
                }
            }
        }
        window.on_commit();
        if activate {
            self.focus_window(&window);
        }
        if self.window_mode == WindowMode::Tiling {
            self.reflow_tiled_windows();
        }
    }

    pub fn find_x11_window(&self, surface: &X11Surface) -> Option<Window> {
        self.space
            .elements()
            .find(|window| {
                window
                    .x11_surface()
                    .is_some_and(|candidate| candidate.window_id() == surface.window_id())
            })
            .cloned()
    }

    pub fn update_x11_window_location(
        &mut self,
        surface: &X11Surface,
        location: Point<i32, Logical>,
    ) {
        if let Some(window) = self.find_x11_window(surface) {
            self.space.map_element(window, location, false);
        }
    }

    pub fn forget_x11_window(&mut self, surface: &X11Surface) {
        if let Some(wl_surface) = surface.wl_surface() {
            self.panel_window_ids
                .retain(|(known, _)| known != &wl_surface);
            self.tiled_windows.retain(|known| known != &wl_surface);
            self.free_window_geometries
                .retain(|(known, _)| known != &wl_surface);
            self.normal_window_geometries
                .retain(|(known, _)| known != &wl_surface);
        }
        if let Some(window) = self.find_x11_window(surface) {
            self.space.unmap_elem(&window);
            self.minimized_windows
                .retain(|entry| entry.window != window);
        }
    }

    pub fn register_toplevel(&mut self, window: &Window, initial_location: Point<i32, Logical>) {
        let Some(toplevel) = window.toplevel() else {
            return;
        };
        let surface = toplevel.wl_surface().clone();
        self.pending_open_animations.push(surface.clone());
        let desktop = is_desktop_toplevel(toplevel);
        let panel = is_panel_toplevel(toplevel);
        let floating = toplevel.parent().is_some();
        self.ensure_panel_window_id(&surface);
        self.set_window_role(&surface, desktop, panel, floating);
        self.set_window_z_index(window, desktop, panel);
        if desktop {
            self.resize_desktop_surfaces();
        } else if panel {
            self.resize_panel_surfaces();
        }

        let default_size = self
            .space
            .outputs()
            .next()
            .and_then(|output| self.space.output_geometry(output))
            .map(|geometry| {
                Size::from((
                    (geometry.size.w * 3 / 4).clamp(320, geometry.size.w.max(320)),
                    (geometry.size.h * 3 / 4).clamp(240, geometry.size.h.max(240)),
                ))
            })
            .unwrap_or_else(|| Size::from((960, 640)));
        if !desktop {
            if !self
                .free_window_geometries
                .iter()
                .any(|(saved, _)| saved == &surface)
            {
                self.free_window_geometries.push((
                    surface.clone(),
                    Rectangle::new(initial_location, default_size),
                ));
            }
            if !floating && !self.tiled_windows.iter().any(|saved| saved == &surface) {
                self.tiled_windows.push(surface);
            }
        }
        if self.window_mode == WindowMode::Tiling {
            self.reflow_tiled_windows();
        }
    }

    pub fn animate_window_open(&mut self, surface: &WlSurface) {
        let Some(window) = self.window_for_surface(surface) else {
            return;
        };
        let Some(location) = self.space.element_location(&window) else {
            return;
        };
        let geometry = window.geometry();
        if geometry.size.w <= 0
            || geometry.size.h <= 0
            || self.is_desktop_surface(surface)
            || self.is_panel_surface(surface)
        {
            return;
        }
        let target = Rectangle::new(location, geometry.size);
        let from = Rectangle::new(
            (location.x + 8, location.y + 14).into(),
            ((geometry.size.w - 16).max(1), (geometry.size.h - 12).max(1)).into(),
        );
        self.start_geometry_animation(surface, from, target, true, 220);
    }

    pub fn start_geometry_animation(
        &mut self,
        surface: &WlSurface,
        from: Rectangle<i32, Logical>,
        to: Rectangle<i32, Logical>,
        configure_client: bool,
        duration_ms: u64,
    ) {
        self.geometry_animations
            .retain(|animation| &animation.surface != surface);
        if from == to {
            return;
        }
        self.geometry_animations.push(WindowGeometryAnimation {
            surface: surface.clone(),
            from,
            to,
            started: Instant::now(),
            duration: std::time::Duration::from_millis(duration_ms),
            configure_client,
            last_tick: Instant::now(),
        });
    }

    pub fn advance_window_animations(&mut self) -> bool {
        let now = Instant::now();
        let mut active = Vec::new();
        let animations = std::mem::take(&mut self.geometry_animations);
        for mut animation in animations {
            if self.minimized_windows.iter().any(|entry| {
                entry
                    .window
                    .toplevel()
                    .is_some_and(|top| top.wl_surface() == &animation.surface)
            }) {
                continue;
            }
            if now.duration_since(animation.last_tick) < std::time::Duration::from_millis(16) {
                active.push(animation);
                continue;
            }
            animation.last_tick = now;
            let duration = animation.duration.as_secs_f32().max(0.001);
            let progress =
                (now.duration_since(animation.started).as_secs_f32() / duration).clamp(0.0, 1.0);
            let eased = progress * progress * (3.0 - 2.0 * progress);
            let interpolate =
                |start: i32, end: i32| (start as f32 + (end - start) as f32 * eased).round() as i32;
            let rect = Rectangle::new(
                (
                    interpolate(animation.from.loc.x, animation.to.loc.x),
                    interpolate(animation.from.loc.y, animation.to.loc.y),
                )
                    .into(),
                (
                    interpolate(animation.from.size.w, animation.to.size.w).max(1),
                    interpolate(animation.from.size.h, animation.to.size.h).max(1),
                )
                    .into(),
            );
            if let Some(window) = self.window_for_surface(&animation.surface) {
                self.space.map_element(window.clone(), rect.loc, false);
                if animation.configure_client
                    && let Some(toplevel) = window.toplevel()
                {
                    toplevel.with_pending_state(|state| state.size = Some(rect.size));
                    toplevel.send_pending_configure();
                }
            }
            if progress < 1.0 {
                active.push(animation);
            }
        }
        self.geometry_animations = active;
        !self.geometry_animations.is_empty()
    }

    pub fn update_window_role(&mut self, surface: &WlSurface) {
        let Some(window) = self.window_for_surface(surface) else {
            return;
        };
        let Some(toplevel) = window.toplevel() else {
            return;
        };
        let desktop = is_desktop_toplevel(toplevel);
        let panel = is_panel_toplevel(toplevel);
        let floating = toplevel.parent().is_some();
        self.set_window_role(surface, desktop, panel, floating);
        self.set_window_z_index(&window, desktop, panel);
        if desktop {
            self.resize_desktop_surfaces();
        } else if panel {
            self.resize_panel_surfaces();
        }
        if self.window_mode == WindowMode::Tiling {
            self.reflow_tiled_windows();
        }
    }

    pub fn resize_desktop_surfaces(&mut self) {
        let Some((output_geometry, windows)) = self
            .space
            .outputs()
            .next()
            .and_then(|output| self.space.output_geometry(output))
            .map(|geometry| (geometry, self.space.elements().cloned().collect::<Vec<_>>()))
        else {
            return;
        };

        for window in windows {
            let Some(toplevel) = window.toplevel().cloned() else {
                continue;
            };
            let surface = toplevel.wl_surface();
            if self.is_desktop_surface(surface) {
                self.enforce_desktop_geometry(&window, output_geometry);
            } else if self.is_panel_surface(surface) {
                self.enforce_panel_geometry(&window, output_geometry);
            }
        }
    }

    pub fn resize_panel_surfaces(&mut self) {
        let Some((output_geometry, windows)) = self
            .space
            .outputs()
            .next()
            .and_then(|output| self.space.output_geometry(output))
            .map(|geometry| (geometry, self.space.elements().cloned().collect::<Vec<_>>()))
        else {
            return;
        };

        for window in windows {
            let Some(toplevel) = window.toplevel().cloned() else {
                continue;
            };
            if self.is_panel_surface(toplevel.wl_surface()) {
                self.enforce_panel_geometry(&window, output_geometry);
            }
        }
    }

    pub fn enforce_panel_geometry(
        &mut self,
        window: &Window,
        output_geometry: Rectangle<i32, Logical>,
    ) {
        let Some(toplevel) = window.toplevel().cloned() else {
            return;
        };
        let surface = toplevel.wl_surface();
        if !self.is_panel_surface(surface) {
            return;
        }

        let (panel_location, panel_size) = panel_geometry(output_geometry);
        if self.space.element_location(window) == Some(panel_location)
            && window.geometry().size == panel_size
        {
            return;
        }

        toplevel.with_pending_state(|state| {
            state.size = Some(panel_size);
        });
        self.space
            .map_element(window.clone(), panel_location, false);
        if toplevel.is_initial_configure_sent() {
            toplevel.send_pending_configure();
        }
    }

    /// Keep the desktop surface flush with the output, including after the
    /// client's first buffer commit (which can arrive after our initial configure).
    pub fn enforce_desktop_geometry(
        &mut self,
        window: &Window,
        output_geometry: Rectangle<i32, Logical>,
    ) {
        let Some(toplevel) = window.toplevel().cloned() else {
            return;
        };
        if !self.is_desktop_surface(toplevel.wl_surface()) {
            return;
        }

        let location_matches = self.space.element_location(window) == Some(output_geometry.loc);
        let size_matches = window.geometry().size == output_geometry.size;
        if location_matches && size_matches {
            return;
        }
        toplevel.with_pending_state(|state| {
            state.states.set(
                smithay::reexports::wayland_protocols::xdg::shell::server::xdg_toplevel::State::Fullscreen,
            );
            state.size = Some(output_geometry.size);
        });
        self.space
            .map_element(window.clone(), output_geometry.loc, false);
        if toplevel.is_initial_configure_sent() {
            toplevel.send_pending_configure();
        }
    }

    fn set_window_z_index(&mut self, window: &Window, desktop: bool, panel: bool) {
        let z_index = if desktop {
            smithay::desktop::space::RenderZindex::Background
        } else if panel {
            smithay::desktop::space::RenderZindex::Overlay
        } else {
            smithay::desktop::space::RenderZindex::Shell
        };
        window.override_z_index(z_index.into());
        if let Some(location) = self.space.element_location(window) {
            // Reinsert after changing z-index so Space sorts this window again.
            self.space.map_element(window.clone(), location, false);
        }
    }

    fn set_window_role(&mut self, surface: &WlSurface, desktop: bool, panel: bool, floating: bool) {
        self.desktop_surfaces.retain(|known| known != surface);
        self.panel_surfaces.retain(|known| known != surface);
        self.floating_surfaces.retain(|known| known != surface);
        if desktop {
            self.desktop_surfaces.push(surface.clone());
        } else if panel {
            self.panel_surfaces.push(surface.clone());
        } else if floating {
            self.floating_surfaces.push(surface.clone());
        }
        if desktop || panel || floating {
            self.tiled_windows.retain(|known| known != surface);
        } else if !self.tiled_windows.iter().any(|known| known == surface) {
            self.tiled_windows.push(surface.clone());
        }
    }

    pub fn is_desktop_surface(&self, surface: &WlSurface) -> bool {
        self.desktop_surfaces.iter().any(|known| known == surface)
            || self
                .window_for_surface(surface)
                .and_then(|window| window.toplevel().cloned())
                .is_some_and(|toplevel| is_desktop_toplevel(&toplevel))
    }

    pub fn is_panel_surface(&self, surface: &WlSurface) -> bool {
        self.panel_surfaces.iter().any(|known| known == surface)
            || self
                .window_for_surface(surface)
                .and_then(|window| window.toplevel().cloned())
                .is_some_and(|toplevel| is_panel_toplevel(&toplevel))
    }

    fn ensure_panel_window_id(&mut self, surface: &WlSurface) -> u64 {
        if let Some((_, id)) = self
            .panel_window_ids
            .iter()
            .find(|(known, _)| known == surface)
        {
            return *id;
        }
        let id = self.next_panel_window_id;
        self.next_panel_window_id = self.next_panel_window_id.wrapping_add(1).max(1);
        self.panel_window_ids.push((surface.clone(), id));
        id
    }

    pub fn panel_snapshot(&mut self) -> PanelSnapshot {
        let focused_surface = self
            .seat
            .get_keyboard()
            .and_then(|keyboard| keyboard.current_focus());
        let windows = self.space.elements().cloned().collect::<Vec<_>>();
        let minimized_windows = self
            .minimized_windows
            .iter()
            .map(|entry| entry.window.clone())
            .collect::<Vec<_>>();
        let mut snapshot = PanelSnapshot {
            keyboard_layout: self.keyboard_layouts[self.keyboard_layout_index].clone(),
            ..Default::default()
        };

        for (window, minimized) in windows
            .into_iter()
            .map(|window| (window, false))
            .chain(minimized_windows.into_iter().map(|window| (window, true)))
        {
            let Some(surface) = self.wl_surface_for_window(&window) else {
                continue;
            };
            if self.is_desktop_surface(&surface) || self.is_panel_surface(&surface) {
                continue;
            }
            let id = self.ensure_panel_window_id(&surface);
            let (title, app_id) = if let Some(toplevel) = window.toplevel() {
                (
                    current_title(toplevel).filter(|title| !title.trim().is_empty()),
                    current_app_id(toplevel),
                )
            } else if let Some(x11_surface) = window.x11_surface() {
                let title = x11_surface.title();
                let class = x11_surface.class();
                (
                    (!title.trim().is_empty()).then_some(title),
                    (!class.trim().is_empty()).then_some(class),
                )
            } else {
                continue;
            };
            let title = title
                .or_else(|| app_id.clone())
                .unwrap_or_else(|| "Fenêtre".to_owned());
            snapshot.windows.push(PanelWindowInfo {
                id,
                title,
                app_id,
                minimized,
                focused: !minimized && focused_surface.as_ref() == Some(&surface),
            });
        }
        snapshot
    }

    pub fn cycle_keyboard_layout(&mut self) {
        if self.keyboard_layouts.len() < 2 {
            return;
        }
        let next_index = (self.keyboard_layout_index + 1) % self.keyboard_layouts.len();
        let layout = self.keyboard_layouts[next_index].clone();
        let Some(keyboard) = self.seat.get_keyboard() else {
            eprintln!("coranux_wm: impossible de changer la disposition sans clavier");
            return;
        };
        if let Err(error) = keyboard.set_xkb_config(
            self,
            smithay::input::keyboard::XkbConfig {
                layout: &layout,
                ..Default::default()
            },
        ) {
            eprintln!("coranux_wm: disposition clavier « {layout} » invalide : {error:?}");
            return;
        }
        self.keyboard_layout_index = next_index;
    }

    pub fn activate_panel_window(&mut self, id: u64) {
        let Some(surface) = self
            .panel_window_ids
            .iter()
            .find_map(|(surface, known_id)| (*known_id == id).then(|| surface.clone()))
        else {
            return;
        };

        if let Some(index) = self.minimized_windows.iter().position(|entry| {
            entry
                .window
                .toplevel()
                .is_some_and(|toplevel| toplevel.wl_surface() == &surface)
        }) {
            let entry = self.minimized_windows.remove(index);
            self.minimized_windows.push(entry);
            self.restore_minimized_window();
            return;
        }

        if let Some(window) = self.window_for_surface(&surface) {
            self.focus_window(&window);
        }
    }

    pub fn is_floating_surface(&self, surface: &WlSurface) -> bool {
        self.floating_surfaces.iter().any(|known| known == surface)
    }

    pub fn is_fullscreen_or_maximized(&self, surface: &WlSurface) -> bool {
        let saved_geometry = self
            .normal_window_geometries
            .iter()
            .any(|(saved, _)| saved == surface);
        let xdg_state = self
            .window_for_surface(surface)
            .and_then(|window| window.toplevel().cloned())
            .is_some_and(|toplevel| {
                let states = &toplevel.current_state().states;
                states.contains(
                    smithay::reexports::wayland_protocols::xdg::shell::server::xdg_toplevel::State::Fullscreen,
                ) || states.contains(
                    smithay::reexports::wayland_protocols::xdg::shell::server::xdg_toplevel::State::Maximized,
                )
            });
        saved_geometry || xdg_state
    }

    pub fn toggle_window_mode(&mut self) {
        match self.window_mode {
            WindowMode::Free => {
                self.tile_layout_overrides = None;
                for window in self.space.elements().cloned().collect::<Vec<_>>() {
                    let Some(surface) = self.wl_surface_for_window(&window) else {
                        continue;
                    };
                    if self.is_desktop_surface(&surface)
                        || self.is_panel_surface(&surface)
                        || self.is_floating_surface(&surface)
                    {
                        continue;
                    }
                    if !self.tiled_windows.iter().any(|known| known == &surface) {
                        self.tiled_windows.push(surface.clone());
                    }
                    let location = self.space.element_location(&window).unwrap_or_default();
                    let restored_geometry = self
                        .normal_window_geometries
                        .iter()
                        .position(|(saved, _)| saved == &surface)
                        .map(|index| self.normal_window_geometries.remove(index).1)
                        .unwrap_or_else(|| Rectangle::new(location, window.geometry().size));
                    if let Some(toplevel) = window.toplevel() {
                        toplevel.with_pending_state(|state| {
                            state.states.unset(
                                smithay::reexports::wayland_protocols::xdg::shell::server::xdg_toplevel::State::Fullscreen,
                            );
                            state.states.unset(
                                smithay::reexports::wayland_protocols::xdg::shell::server::xdg_toplevel::State::Maximized,
                            );
                            state.size = Some(restored_geometry.size);
                        });
                        toplevel.send_pending_configure();
                    } else if let Some(x11_surface) = window.x11_surface()
                        && let Err(error) = x11_surface.configure(restored_geometry)
                    {
                        eprintln!("coranux_wm: impossible de restaurer la fenêtre X11 : {error}");
                    }
                    self.space
                        .map_element(window.clone(), restored_geometry.loc, false);
                    self.free_window_geometries
                        .retain(|(saved, _)| saved != &surface);
                    self.free_window_geometries
                        .push((surface.clone(), restored_geometry));
                }
                self.window_mode = WindowMode::Tiling;
                self.expanded_tiled_window = None;
                self.reflow_tiled_windows();
            }
            WindowMode::Tiling => {
                self.window_mode = WindowMode::Free;
                self.expanded_tiled_window = None;
                self.tile_frames.clear();
                self.tile_layout_overrides = None;
                for window in self.space.elements().cloned().collect::<Vec<_>>() {
                    let Some(surface) = self.wl_surface_for_window(&window) else {
                        continue;
                    };
                    if self.is_desktop_surface(&surface)
                        || self.is_panel_surface(&surface)
                        || self.is_floating_surface(&surface)
                    {
                        continue;
                    }
                    if let Some((_, geometry)) = self
                        .free_window_geometries
                        .iter()
                        .find(|(saved, _)| saved == &surface)
                        .cloned()
                    {
                        if let Some(toplevel) = window.toplevel() {
                            toplevel.with_pending_state(|state| {
                                state.states.unset(smithay::reexports::wayland_protocols::xdg::shell::server::xdg_toplevel::State::Maximized);
                                state.states.unset(smithay::reexports::wayland_protocols::xdg::shell::server::xdg_toplevel::State::Fullscreen);
                                state.size = Some(geometry.size);
                            });
                            toplevel.send_pending_configure();
                        } else if let Some(x11_surface) = window.x11_surface()
                            && let Err(error) = x11_surface.configure(geometry)
                        {
                            eprintln!(
                                "coranux_wm: impossible de restaurer la fenêtre X11 : {error}"
                            );
                        }
                        self.space.map_element(window.clone(), geometry.loc, false);
                    }
                }
            }
        }
    }

    pub fn reflow_tiled_windows(&mut self) {
        if self.window_mode != WindowMode::Tiling {
            return;
        }
        let Some(output) = self.space.outputs().next().cloned() else {
            return;
        };
        let Some(output_geometry) = self.space.output_geometry(&output) else {
            return;
        };
        let bounds = work_area_geometry(output_geometry);

        let visible = self
            .tiled_windows
            .iter()
            .filter_map(|surface| {
                self.space
                    .elements()
                    .find(|window| {
                        self.wl_surface_for_window(window)
                            .is_some_and(|candidate| candidate == *surface)
                    })
                    .cloned()
                    .map(|window| (surface.clone(), window))
            })
            .collect::<Vec<_>>();
        let floating = self
            .space
            .elements()
            .filter_map(|window| {
                let surface = self.wl_surface_for_window(window)?;
                self.is_floating_surface(&surface)
                    .then(|| {
                        self.space
                            .element_location(window)
                            .map(|location| (window.clone(), location))
                    })
                    .flatten()
            })
            .collect::<Vec<_>>();
        let slots = if let Some(expanded_index) = self
            .expanded_tiled_window
            .as_ref()
            .and_then(|surface| visible.iter().position(|(known, _)| known == surface))
            .filter(|_| visible.len() > 1)
        {
            let main_width = (bounds.size.w * 3 / 5).max(1);
            let main = Rectangle::new(bounds.loc, (main_width, bounds.size.h).into());
            let side = Rectangle::new(
                (bounds.loc.x + main_width, bounds.loc.y).into(),
                (bounds.size.w - main_width, bounds.size.h).into(),
            );
            let side_slots = grid_rectangles(side, visible.len() - 1);
            let mut side_index = 0;
            (0..visible.len())
                .map(|index| {
                    if index == expanded_index {
                        main
                    } else {
                        let slot = side_slots[side_index];
                        side_index += 1;
                        slot
                    }
                })
                .collect::<Vec<_>>()
        } else {
            let saved = self.tile_layout_overrides.clone().filter(|(_, frames)| {
                frames.len() == visible.len()
                    && frames
                        .iter()
                        .zip(&visible)
                        .all(|((saved, _), (current, _))| saved == current)
            });
            if let Some((saved_bounds, frames)) = saved {
                frames
                    .into_iter()
                    .map(|(_, rect)| scale_rectangle(rect, saved_bounds, bounds))
                    .collect()
            } else {
                self.tile_layout_overrides = None;
                grid_rectangles(bounds, visible.len())
            }
        };
        self.tile_frames.clear();

        for ((surface, window), outer) in visible.into_iter().zip(slots) {
            let client_size = Size::from((
                outer.size.w.max(1),
                (outer.size.h - TITLEBAR_TOTAL_HEIGHT).max(1),
            ));
            let client_location = Point::from((outer.loc.x, outer.loc.y + TITLEBAR_TOTAL_HEIGHT));
            let is_expanded = self
                .expanded_tiled_window
                .as_ref()
                .is_some_and(|expanded| expanded == &surface);
            if let Some(toplevel) = window.toplevel() {
                toplevel.with_pending_state(|state| {
                    if is_expanded {
                        state.states.set(smithay::reexports::wayland_protocols::xdg::shell::server::xdg_toplevel::State::Maximized);
                    } else {
                        state.states.unset(smithay::reexports::wayland_protocols::xdg::shell::server::xdg_toplevel::State::Maximized);
                    }
                    state.size = Some(client_size);
                });
                toplevel.send_pending_configure();
            } else if let Some(x11_surface) = window.x11_surface()
                && let Err(error) =
                    x11_surface.configure(Rectangle::new(client_location, client_size))
            {
                eprintln!(
                    "coranux_wm: impossible de placer une fenêtre X11 dans la grille : {error}"
                );
            }
            self.space
                .map_element(window.clone(), client_location, false);
            self.tile_frames.push((surface, outer));
        }
        for (window, location) in floating {
            self.space.map_element(window, location, false);
        }
    }

    pub fn window_frames(&self) -> Vec<WindowFrame> {
        let focused = self
            .seat
            .get_keyboard()
            .and_then(|keyboard| keyboard.current_focus());
        let output_geometry = self
            .space
            .outputs()
            .next()
            .and_then(|output| self.space.output_geometry(output));
        let mut frames = self
            .space
            .elements()
            .filter_map(|window| {
                let surface = self.wl_surface_for_window(window)?;
                if self.is_desktop_surface(&surface) || self.is_panel_surface(&surface) {
                    return None;
                }
                let fullscreen = window.toplevel().is_some_and(|toplevel| {
                    toplevel.current_state().states.contains(
                        smithay::reexports::wayland_protocols::xdg::shell::server::xdg_toplevel::State::Fullscreen,
                    )
                });
                let maximized = window.toplevel().is_some_and(|toplevel| {
                    toplevel.current_state().states.contains(
                        smithay::reexports::wayland_protocols::xdg::shell::server::xdg_toplevel::State::Maximized,
                    )
                }) || self
                    .normal_window_geometries
                    .iter()
                    .any(|(saved, _)| saved == &surface);
                let arranged_tiled = window.toplevel().is_some()
                    && self.window_mode == WindowMode::Tiling
                    && !self.is_floating_surface(&surface);
                let tiled_style = arranged_tiled;
                let outer = if fullscreen {
                    output_geometry.unwrap_or_else(|| {
                        let location = self.space.element_location(window).unwrap_or_default();
                        Rectangle::new(location, window.geometry().size)
                    })
                } else if arranged_tiled {
                    self.tile_frames
                        .iter()
                        .find(|(known, _)| known == &surface)
                        .map(|(_, rect)| *rect)?
                } else if maximized {
                    let geometry = if window.toplevel().is_none() {
                        output_geometry.map(work_area_geometry)
                    } else {
                        output_geometry
                    };
                    geometry.unwrap_or_else(|| {
                        let location = self.space.element_location(window).unwrap_or_default();
                        Rectangle::new(location, window.geometry().size)
                    })
                } else {
                    let location = self.space.element_location(window)?;
                    let size = window.geometry().size;
                    Rectangle::new(
                        (
                            location.x - FRAME_BORDER,
                            location.y - TITLEBAR_TOTAL_HEIGHT,
                        )
                            .into(),
                        (
                            size.w + FRAME_BORDER * 2,
                            size.h + TITLEBAR_TOTAL_HEIGHT + FRAME_BORDER,
                        )
                            .into(),
                    )
                };
                let titlebar =
                    Rectangle::new(outer.loc, (outer.size.w, TITLEBAR_TOTAL_HEIGHT).into());
                let actions = [
                    FrameAction::ToggleMode,
                    FrameAction::Maximize,
                    FrameAction::Minimize,
                    FrameAction::Close,
                ];
                let controls_width =
                    actions.len() as i32 * (CONTROL_SIZE + CONTROL_GAP) + CONTROL_GAP;
                let controls = actions
                    .iter()
                    .enumerate()
                    .map(|(index, action)| FrameControl {
                        action: *action,
                        rect: Rectangle::new(
                            (
                                outer.loc.x + outer.size.w - controls_width
                                    + CONTROL_GAP
                                    + index as i32 * (CONTROL_SIZE + CONTROL_GAP),
                                outer.loc.y + (TITLEBAR_HEIGHT - CONTROL_SIZE) / 2,
                            )
                                .into(),
                            (CONTROL_SIZE, CONTROL_SIZE).into(),
                        ),
                    })
                    .collect();
                Some(WindowFrame {
                    surface: surface.clone(),
                    outer,
                    titlebar,
                    controls,
                    focused: focused.as_ref().is_some_and(|focused| focused == &surface),
                    tiled: tiled_style,
                    fullscreen,
                })
            })
            .collect::<Vec<_>>();

        // A fullscreen surface is raised by the protocol request. Keep its WM frame
        // above the decorations of every other window as well.
        frames.sort_by_key(|frame| frame.fullscreen);
        frames
    }

    pub fn decoration_hit(
        &self,
        position: Point<f64, Logical>,
    ) -> Option<(WlSurface, Option<FrameAction>, bool)> {
        let point = position.to_i32_round();
        self.window_frames().into_iter().rev().find_map(|frame| {
            if let Some(control) = frame
                .controls
                .iter()
                .find(|control| control.rect.contains(point))
            {
                return Some((frame.surface, Some(control.action), false));
            }
            frame
                .titlebar
                .contains(point)
                .then_some((frame.surface, None, !frame.fullscreen))
        })
    }

    pub fn resize_hit(&self, position: Point<f64, Logical>) -> Option<(WlSurface, ResizeEdge)> {
        const HIT_SIZE: f64 = 8.0;

        self.window_frames().into_iter().rev().find_map(|frame| {
            if frame.fullscreen || self.is_fullscreen_or_maximized(&frame.surface) {
                return None;
            }
            let left = frame.outer.loc.x as f64;
            let right = (frame.outer.loc.x + frame.outer.size.w) as f64;
            let top = frame.outer.loc.y as f64;
            let bottom = (frame.outer.loc.y + frame.outer.size.h) as f64;
            let x_near = position.x >= left - HIT_SIZE && position.x <= right + HIT_SIZE;
            let y_near = position.y >= top - HIT_SIZE && position.y <= bottom + HIT_SIZE;
            if !x_near || !y_near {
                return None;
            }

            let mut edges = ResizeEdge::empty();
            if (position.x - left).abs() <= HIT_SIZE {
                edges |= ResizeEdge::LEFT;
            } else if (position.x - right).abs() <= HIT_SIZE {
                edges |= ResizeEdge::RIGHT;
            }
            if (position.y - top).abs() <= HIT_SIZE {
                edges |= ResizeEdge::TOP;
            } else if (position.y - bottom).abs() <= HIT_SIZE {
                edges |= ResizeEdge::BOTTOM;
            }
            (!edges.is_empty()).then_some((frame.surface, edges))
        })
    }

    pub fn resized_tile_frames(
        &self,
        surface: &WlSurface,
        edges: ResizeEdge,
        delta: Point<f64, Logical>,
        original: &[(WlSurface, Rectangle<i32, Logical>)],
    ) -> Option<Vec<(WlSurface, Rectangle<i32, Logical>)>> {
        let target = original.iter().position(|(known, _)| known == surface)?;
        let mut resized = original.to_vec();
        let mut changed = false;
        if edges.intersects(ResizeEdge::LEFT | ResizeEdge::RIGHT) {
            changed |= resize_tile_axis(&mut resized, target, delta.x.round() as i32, true, edges);
        }
        if edges.intersects(ResizeEdge::TOP | ResizeEdge::BOTTOM) {
            changed |= resize_tile_axis(&mut resized, target, delta.y.round() as i32, false, edges);
        }
        changed.then_some(resized)
    }

    pub fn apply_tiled_frames(&mut self, frames: Vec<(WlSurface, Rectangle<i32, Logical>)>) {
        let Some(output_geometry) = self
            .space
            .outputs()
            .next()
            .and_then(|output| self.space.output_geometry(output))
        else {
            return;
        };
        let bounds = work_area_geometry(output_geometry);
        self.tile_frames = frames.clone();
        self.tile_layout_overrides = Some((bounds, frames.clone()));

        for (surface, outer) in frames {
            let Some(window) = self.window_for_surface(&surface) else {
                continue;
            };
            let Some(toplevel) = window.toplevel().cloned() else {
                continue;
            };
            let client_size = Size::from((
                outer.size.w.max(1),
                (outer.size.h - TITLEBAR_TOTAL_HEIGHT).max(1),
            ));
            let client_location = Point::from((outer.loc.x, outer.loc.y + TITLEBAR_TOTAL_HEIGHT));
            toplevel.with_pending_state(|state| {
                state.size = Some(client_size);
                if !self
                    .expanded_tiled_window
                    .as_ref()
                    .is_some_and(|expanded| expanded == &surface)
                {
                    state.states.unset(
                        smithay::reexports::wayland_protocols::xdg::shell::server::xdg_toplevel::State::Maximized,
                    );
                }
            });
            self.space.map_element(window, client_location, false);
            toplevel.send_pending_configure();
        }
    }

    pub fn decoration_elements(&self) -> Vec<SolidColorRenderElement> {
        let frames = self.window_frames();
        let mut elements = Vec::new();
        let pulse = (self.start_time.elapsed().as_secs_f32() * 3.0).sin() * 0.5 + 0.5;
        for (frame_index, frame) in frames.iter().cloned().enumerate() {
            let frame_element_start = elements.len();
            let title = if frame.focused {
                [0.035, 0.050, 0.082, 1.0]
            } else {
                [0.060, 0.070, 0.095, 1.0]
            };
            let accent = if frame.tiled {
                [0.30, 0.80, 0.96, 1.0]
            } else {
                [0.59, 0.43, 0.98, 1.0]
            };
            let border = if frame.focused {
                [0.25, 0.34, 0.49, 0.96]
            } else {
                [0.18, 0.22, 0.30, 0.88]
            };
            push_solid(&mut elements, frame.titlebar, title);
            // A thin accent line makes the active window easy to find without
            // surrounding every window with a distracting white outline.
            push_solid(
                &mut elements,
                Rectangle::new(frame.outer.loc, (frame.outer.size.w, 3).into()),
                if frame.focused {
                    [
                        accent[0] * (0.88 + pulse * 0.12),
                        accent[1] * (0.88 + pulse * 0.12),
                        accent[2] * (0.88 + pulse * 0.12),
                        1.0,
                    ]
                } else {
                    accent
                },
            );
            push_solid(
                &mut elements,
                Rectangle::new(
                    (frame.outer.loc.x, frame.outer.loc.y + 3).into(),
                    (frame.outer.size.w, 1).into(),
                ),
                [1.0, 1.0, 1.0, if frame.focused { 0.10 } else { 0.045 }],
            );
            let client_top = frame.outer.loc.y + TITLEBAR_TOTAL_HEIGHT;
            let border_height = (frame.outer.size.h - TITLEBAR_TOTAL_HEIGHT).max(0);
            push_solid(
                &mut elements,
                Rectangle::new(
                    (frame.outer.loc.x, client_top).into(),
                    (2, border_height).into(),
                ),
                border,
            );
            push_solid(
                &mut elements,
                Rectangle::new(
                    (frame.outer.loc.x + frame.outer.size.w - 2, client_top).into(),
                    (2, border_height).into(),
                ),
                border,
            );
            push_solid(
                &mut elements,
                Rectangle::new(
                    (
                        frame.outer.loc.x,
                        frame.outer.loc.y + frame.outer.size.h - 2,
                    )
                        .into(),
                    (frame.outer.size.w, 2).into(),
                ),
                border,
            );

            for control in frame.controls {
                let (base_color, icon_color) = match control.action {
                    FrameAction::Close => ([0.96_f32, 0.32, 0.39, 1.0], [0.28, 0.035, 0.055, 1.0]),
                    FrameAction::Minimize => ([1.0_f32, 0.72, 0.27, 1.0], [0.32, 0.18, 0.025, 1.0]),
                    FrameAction::Maximize => {
                        ([0.32_f32, 0.82, 0.51, 1.0], [0.035, 0.25, 0.10, 1.0])
                    }
                    FrameAction::ToggleMode => {
                        ([0.35_f32, 0.67, 0.98, 1.0], [0.035, 0.16, 0.32, 1.0])
                    }
                };
                // Circular traffic-light controls read clearly at a glance;
                // inset glyphs below provide visual feedback while preserving
                // the existing rectangular hit targets.
                let center_x = control.rect.loc.x + control.rect.size.w / 2;
                let center_y = control.rect.loc.y + control.rect.size.h / 2;
                let hovered = control.rect.contains(self.pointer_location.to_i32_round());
                let diameter = if hovered { 15 } else { 12 };
                let color = if hovered {
                    [
                        (base_color[0] + 0.12).min(1.0),
                        (base_color[1] + 0.12).min(1.0),
                        (base_color[2] + 0.12).min(1.0),
                        1.0,
                    ]
                } else {
                    base_color
                };
                // Build a small stepped circle from horizontal bands; the WM
                // renderer only exposes solid rectangles here.
                let half = diameter / 2;
                for row in 0_i32..diameter {
                    let distance = (row - half).abs();
                    let inset = match distance {
                        0 | 1 => 0,
                        2 | 3 => 1,
                        4 | 5 => 2,
                        _ => 3,
                    };
                    let width = diameter - inset * 2;
                    if width > 0 {
                        push_solid(
                            &mut elements,
                            Rectangle::new(
                                (center_x - width / 2, center_y - half + row).into(),
                                (width, 1).into(),
                            ),
                            color,
                        );
                    }
                }
                match control.action {
                    FrameAction::Close => {
                        for step in 0..7 {
                            push_solid(
                                &mut elements,
                                Rectangle::new(
                                    (center_x - 3 + step, center_y - 3 + step).into(),
                                    (2, 2).into(),
                                ),
                                icon_color,
                            );
                            push_solid(
                                &mut elements,
                                Rectangle::new(
                                    (center_x + 2 - step, center_y - 3 + step).into(),
                                    (2, 2).into(),
                                ),
                                icon_color,
                            );
                        }
                    }
                    FrameAction::Minimize => push_solid(
                        &mut elements,
                        Rectangle::new((center_x - 5, center_y + 3).into(), (10, 2).into()),
                        icon_color,
                    ),
                    FrameAction::Maximize => {
                        push_solid(
                            &mut elements,
                            Rectangle::new((center_x - 5, center_y - 5).into(), (10, 2).into()),
                            icon_color,
                        );
                        push_solid(
                            &mut elements,
                            Rectangle::new((center_x - 5, center_y + 3).into(), (10, 2).into()),
                            icon_color,
                        );
                        push_solid(
                            &mut elements,
                            Rectangle::new((center_x - 5, center_y - 3).into(), (2, 6).into()),
                            icon_color,
                        );
                        push_solid(
                            &mut elements,
                            Rectangle::new((center_x + 3, center_y - 3).into(), (2, 6).into()),
                            icon_color,
                        );
                    }
                    FrameAction::ToggleMode if self.window_mode == WindowMode::Tiling => {
                        for (dx, dy) in [(-4, -4), (2, -4), (-4, 2), (2, 2)] {
                            push_solid(
                                &mut elements,
                                Rectangle::new(
                                    (center_x + dx, center_y + dy).into(),
                                    (3, 3).into(),
                                ),
                                icon_color,
                            );
                        }
                    }
                    FrameAction::ToggleMode => {
                        push_solid(
                            &mut elements,
                            Rectangle::new((center_x - 5, center_y - 5).into(), (10, 10).into()),
                            icon_color,
                        );
                        push_solid(
                            &mut elements,
                            Rectangle::new((center_x - 3, center_y - 3).into(), (6, 6).into()),
                            color,
                        );
                        push_solid(
                            &mut elements,
                            Rectangle::new((center_x - 5, center_y - 5).into(), (10, 2).into()),
                            icon_color,
                        );
                    }
                }
            }

            let frame_elements = elements.split_off(frame_element_start);
            let occluders = frames
                .iter()
                .skip(frame_index + 1)
                .map(|higher| higher.outer)
                .collect::<Vec<_>>();
            for element in frame_elements {
                let geometry = element.geometry(1.0.into());
                let mut visible = vec![Rectangle::new(
                    (geometry.loc.x, geometry.loc.y).into(),
                    (geometry.size.w, geometry.size.h).into(),
                )];
                for occluder in &occluders {
                    visible = visible
                        .into_iter()
                        .flat_map(|rect| subtract_rectangle(rect, *occluder))
                        .collect();
                }
                for rect in visible {
                    push_solid_color(&mut elements, rect, element.color());
                }
            }
        }
        elements
    }

    pub fn start_titlebar_drag(&mut self, surface: WlSurface, position: Point<f64, Logical>) {
        let window = self.window_for_surface(&surface);
        let initial_window_location = window
            .as_ref()
            .and_then(|window| self.space.element_location(window))
            .unwrap_or_default();
        let saved_geometry = self
            .normal_window_geometries
            .iter()
            .find(|(saved, _)| saved == &surface)
            .map(|(_, geometry)| *geometry);
        let restore_on_drag = saved_geometry.and_then(|geometry| {
            let output_geometry = self
                .space
                .outputs()
                .next()
                .and_then(|output| self.space.output_geometry(output))?;
            let relative_x =
                (position.x - output_geometry.loc.x as f64) / output_geometry.size.w.max(1) as f64;
            Some((geometry, relative_x.clamp(0.0, 1.0)))
        });
        self.titlebar_drag = Some(TitlebarDrag {
            surface,
            start: position,
            current: position,
            initial_window_location,
            moved: false,
            restore_on_drag,
        });
    }

    pub fn update_titlebar_drag(&mut self, position: Point<f64, Logical>) -> bool {
        let (surface, start, mut initial, moved, restore_on_drag) = {
            let Some(drag) = self.titlebar_drag.as_mut() else {
                return false;
            };
            drag.current = position;
            let delta = position - drag.start;
            if delta.x.abs() + delta.y.abs() > 7.0 {
                drag.moved = true;
            }
            let restore = if drag.moved {
                drag.restore_on_drag.take()
            } else {
                None
            };
            (
                drag.surface.clone(),
                drag.start,
                drag.initial_window_location,
                drag.moved,
                restore,
            )
        };

        if let Some((geometry, horizontal_anchor)) = restore_on_drag {
            self.set_maximized(&surface, false);
            let output_geometry = self
                .space
                .outputs()
                .next()
                .and_then(|output| self.space.output_geometry(output));
            if let (Some(window), Some(output_geometry)) =
                (self.window_for_surface(&surface), output_geometry)
            {
                let restored_frame_width = geometry.size.w + FRAME_BORDER * 2;
                let frame_x =
                    (position.x - horizontal_anchor * restored_frame_width as f64).round() as i32;
                let client_x = frame_x + FRAME_BORDER;
                let client_y = output_geometry.loc.y + TITLEBAR_TOTAL_HEIGHT;
                initial = (client_x, client_y).into();
                self.space.map_element(window, initial, true);
            }
            if let Some(drag) = self.titlebar_drag.as_mut() {
                drag.start = position;
                drag.initial_window_location = initial;
            }
        }

        if moved && (self.window_mode == WindowMode::Free || self.is_floating_surface(&surface)) {
            if let Some(window) = self.window_for_surface(&surface) {
                let effective_start = self.titlebar_drag.as_ref().map_or(start, |drag| drag.start);
                let effective_initial = self
                    .titlebar_drag
                    .as_ref()
                    .map_or(initial, |drag| drag.initial_window_location);
                self.space.map_element(
                    window,
                    (effective_initial.to_f64() + (position - effective_start)).to_i32_round(),
                    true,
                );
            }
        }
        moved
    }

    pub fn finish_titlebar_drag(&mut self, position: Point<f64, Logical>) {
        let Some(mut drag) = self.titlebar_drag.take() else {
            return;
        };
        drag.current = position;
        let delta = position - drag.start;
        drag.moved |= delta.x.abs() + delta.y.abs() > 7.0;
        if drag.moved {
            if self.window_mode == WindowMode::Tiling && !self.is_floating_surface(&drag.surface) {
                self.insert_tiled_window_at(&drag.surface, position);
                self.reflow_tiled_windows();
            }
            self.last_titlebar_click = None;
            return;
        }

        let now = Instant::now();
        if self
            .last_titlebar_click
            .as_ref()
            .is_some_and(|(surface, time)| {
                surface == &drag.surface && now.duration_since(*time).as_millis() < 450
            })
        {
            if !self.is_floating_surface(&drag.surface) {
                self.toggle_expanded_window(&drag.surface);
            }
            self.last_titlebar_click = None;
        } else {
            self.last_titlebar_click = Some((drag.surface, now));
        }
    }

    fn toggle_expanded_window(&mut self, surface: &WlSurface) {
        if self.window_mode != WindowMode::Tiling {
            self.toggle_maximized(surface);
            return;
        }
        self.tile_layout_overrides = None;
        if self
            .expanded_tiled_window
            .as_ref()
            .is_some_and(|expanded| expanded == surface)
        {
            self.expanded_tiled_window = None;
        } else {
            self.expanded_tiled_window = Some(surface.clone());
        }
        self.reflow_tiled_windows();
    }

    fn insert_tiled_window_at(&mut self, surface: &WlSurface, position: Point<f64, Logical>) {
        let Some((target, target_rect)) = self
            .tile_frames
            .iter()
            .min_by_key(|(_, rect)| {
                let center: Point<i32, Logical> =
                    Point::from((rect.loc.x + rect.size.w / 2, rect.loc.y + rect.size.h / 2));
                let dx = center.x as i64 - position.x as i64;
                let dy = center.y as i64 - position.y as i64;
                dx * dx + dy * dy
            })
            .map(|(target, rect)| (target.clone(), *rect))
        else {
            return;
        };
        if &target == surface {
            return;
        }
        let before_target = if target_rect.size.w >= target_rect.size.h {
            position.x < (target_rect.loc.x + target_rect.size.w / 2) as f64
        } else {
            position.y < (target_rect.loc.y + target_rect.size.h / 2) as f64
        };

        self.tiled_windows.retain(|known| known != surface);
        let target_index = self
            .tiled_windows
            .iter()
            .position(|known| known == &target)
            .unwrap_or(self.tiled_windows.len());
        let insert_index = if before_target {
            target_index
        } else {
            target_index + 1
        }
        .min(self.tiled_windows.len());
        self.tiled_windows.insert(insert_index, surface.clone());
        self.expanded_tiled_window = None;
        self.tile_layout_overrides = None;
    }

    pub fn set_maximized(&mut self, surface: &WlSurface, maximized: bool) {
        if self.is_panel_surface(surface) {
            return;
        }
        if self.window_mode == WindowMode::Tiling {
            if self.is_floating_surface(surface) {
                return;
            }
            self.tile_layout_overrides = None;
            if maximized {
                self.expanded_tiled_window = Some(surface.clone());
            } else if self
                .expanded_tiled_window
                .as_ref()
                .is_some_and(|expanded| expanded == surface)
            {
                self.expanded_tiled_window = None;
            }
            if let Some(window) = self.window_for_surface(surface)
                && let Some(toplevel) = window.toplevel()
            {
                toplevel.with_pending_state(|state| {
                    if maximized {
                        state.states.set(smithay::reexports::wayland_protocols::xdg::shell::server::xdg_toplevel::State::Maximized);
                    } else {
                        state.states.unset(smithay::reexports::wayland_protocols::xdg::shell::server::xdg_toplevel::State::Maximized);
                    }
                });
                toplevel.send_pending_configure();
            }
            self.reflow_tiled_windows();
            return;
        }
        let Some(window) = self.window_for_surface(surface) else {
            return;
        };
        if let Some(x11_surface) = window.x11_surface() {
            if maximized {
                if !self
                    .normal_window_geometries
                    .iter()
                    .any(|(saved_surface, _)| saved_surface == surface)
                    && let Some(location) = self.space.element_location(&window)
                {
                    self.normal_window_geometries.push((
                        surface.clone(),
                        Rectangle::new(location, window.geometry().size),
                    ));
                }
                let Some(output_geometry) = self
                    .space
                    .outputs()
                    .next()
                    .and_then(|output| self.space.output_geometry(output))
                else {
                    return;
                };
                let work_area = work_area_geometry(output_geometry);
                let geometry = Rectangle::new(
                    (
                        work_area.loc.x + FRAME_BORDER,
                        work_area.loc.y + TITLEBAR_TOTAL_HEIGHT,
                    )
                        .into(),
                    (
                        (work_area.size.w - FRAME_BORDER * 2).max(1),
                        (work_area.size.h - TITLEBAR_TOTAL_HEIGHT - FRAME_BORDER).max(1),
                    )
                        .into(),
                );
                let from = self
                    .space
                    .element_location(&window)
                    .map(|location| Rectangle::new(location, window.geometry().size));
                self.space.raise_element(&window, true);
                if let Err(error) = x11_surface.configure(geometry) {
                    eprintln!("coranux_wm: impossible d’agrandir la fenêtre X11 : {error}");
                    return;
                }
                self.space.map_element(window, geometry.loc, true);
                if let Some(from) = from {
                    self.start_geometry_animation(surface, from, geometry, true, 240);
                }
            } else if let Some(index) = self
                .normal_window_geometries
                .iter()
                .position(|(saved_surface, _)| saved_surface == surface)
            {
                let (_, geometry) = self.normal_window_geometries.remove(index);
                if let Err(error) = x11_surface.configure(geometry) {
                    eprintln!("coranux_wm: impossible de restaurer la fenêtre X11 : {error}");
                    return;
                }
                self.space.map_element(window, geometry.loc, true);
            }
            return;
        }
        let Some(toplevel) = window.toplevel().cloned() else {
            return;
        };

        if maximized {
            if !self
                .normal_window_geometries
                .iter()
                .any(|(saved_surface, _)| saved_surface == surface)
            {
                if let Some(location) = self.space.element_location(&window) {
                    self.normal_window_geometries.push((
                        surface.clone(),
                        Rectangle::new(location, window.geometry().size),
                    ));
                }
            }
            let Some(output) = self.space.outputs().next().cloned() else {
                return;
            };
            let Some(output_geometry) = self.space.output_geometry(&output) else {
                return;
            };
            let geometry = work_area_geometry(output_geometry);
            let client_size = Size::from((
                (geometry.size.w - FRAME_BORDER * 2).max(1),
                (geometry.size.h - TITLEBAR_TOTAL_HEIGHT - FRAME_BORDER).max(1),
            ));
            let client_location = Point::from((
                geometry.loc.x + FRAME_BORDER,
                geometry.loc.y + TITLEBAR_TOTAL_HEIGHT,
            ));
            toplevel.with_pending_state(|state| {
                state.states.set(smithay::reexports::wayland_protocols::xdg::shell::server::xdg_toplevel::State::Maximized);
                state.size = Some(client_size);
            });
            let from = self
                .space
                .element_location(&window)
                .map(|location| Rectangle::new(location, window.geometry().size))
                .unwrap_or_else(|| Rectangle::new(client_location, client_size));
            let to = Rectangle::new(client_location, client_size);
            self.space.raise_element(&window, true);
            self.start_geometry_animation(surface, from, to, true, 240);
        } else {
            toplevel.with_pending_state(|state| {
                state.states.unset(smithay::reexports::wayland_protocols::xdg::shell::server::xdg_toplevel::State::Maximized);
            });
            if let Some(index) = self
                .normal_window_geometries
                .iter()
                .position(|(saved_surface, _)| saved_surface == surface)
            {
                let (_, geometry) = self.normal_window_geometries.remove(index);
                toplevel.with_pending_state(|state| state.size = Some(geometry.size));
                if !self
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
                    self.start_geometry_animation(surface, from, geometry, true, 240);
                }
            }
        }
        self.send_configures_to_windows();
    }

    pub fn toggle_maximized(&mut self, surface: &WlSurface) {
        if self.is_panel_surface(surface) {
            return;
        }
        if let Some(window) = self.window_for_surface(surface)
            && let Some(toplevel) = window.toplevel().cloned()
            && toplevel.current_state().states.contains(
                smithay::reexports::wayland_protocols::xdg::shell::server::xdg_toplevel::State::Fullscreen,
            )
        {
            let restore = self
                .normal_window_geometries
                .iter()
                .position(|(saved, _)| saved == surface)
                .map(|index| self.normal_window_geometries.remove(index).1)
                .or_else(|| {
                    self.free_window_geometries
                        .iter()
                        .find(|(saved, _)| saved == surface)
                        .map(|(_, rect)| *rect)
                });
            toplevel.with_pending_state(|state| {
                state.states.unset(
                    smithay::reexports::wayland_protocols::xdg::shell::server::xdg_toplevel::State::Fullscreen,
                );
                if let Some(rect) = restore {
                    state.size = Some(rect.size);
                }
            });
            if let Some(rect) = restore {
                self.space.map_element(window, rect.loc, true);
            }
            toplevel.send_pending_configure();
            self.send_configures_to_windows();
            return;
        }
        if self.window_mode == WindowMode::Tiling {
            self.toggle_expanded_window(surface);
            return;
        }
        let is_maximized = self
            .normal_window_geometries
            .iter()
            .any(|(saved_surface, _)| saved_surface == surface);
        self.set_maximized(surface, !is_maximized);
    }

    pub fn minimize_surface(&mut self, surface: &WlSurface) {
        if self.is_panel_surface(surface) {
            return;
        }
        let Some(window) = self.window_for_surface(surface) else {
            return;
        };
        let Some(location) = self.space.element_location(&window) else {
            return;
        };
        window.set_activated(false);
        if let Some(toplevel) = window.toplevel() {
            toplevel.send_pending_configure();
        }
        self.space.unmap_elem(&window);
        if !self
            .minimized_windows
            .iter()
            .any(|entry| entry.window == window)
        {
            self.minimized_windows.push(MinimizedWindow {
                size: window.geometry().size,
                window,
                location,
                mode: self.window_mode,
            });
        }
        if self.window_mode == WindowMode::Tiling {
            if self
                .expanded_tiled_window
                .as_ref()
                .is_some_and(|expanded| expanded == surface)
            {
                self.expanded_tiled_window = None;
            }
            self.reflow_tiled_windows();
        }
        self.focus_top_window();
    }

    pub fn restore_minimized_window(&mut self) -> bool {
        let Some(entry) = self.minimized_windows.pop() else {
            return false;
        };
        let window = entry.window;
        let location = entry.location;
        if let Some(toplevel) = window.toplevel().cloned() {
            let surface = toplevel.wl_surface().clone();
            let saved_free_geometry = self
                .free_window_geometries
                .iter()
                .find(|(saved, _)| saved == &surface)
                .map(|(_, geometry)| *geometry);
            let restore_geometry =
                if self.window_mode == WindowMode::Free && entry.mode == WindowMode::Tiling {
                    saved_free_geometry
                } else {
                    Some(Rectangle::new(location, entry.size))
                };
            let target_location = restore_geometry.map_or(location, |geometry| geometry.loc);
            self.space.map_element(
                window.clone(),
                target_location,
                self.window_mode != WindowMode::Tiling,
            );
            if self.window_mode == WindowMode::Free {
                if let Some(geometry) = restore_geometry {
                    toplevel.with_pending_state(|state| state.size = Some(geometry.size));
                }
            }
            toplevel.send_pending_configure();
            if self.window_mode == WindowMode::Tiling {
                self.reflow_tiled_windows();
            } else {
                self.send_configures_to_windows();
            }
            self.focus_window(&window);
            true
        } else {
            false
        }
    }

    pub fn focus_next_window(&mut self) {
        if self.restore_minimized_window() {
            return;
        }
        let windows = self.space.elements().cloned().collect::<Vec<_>>();
        if windows.is_empty() {
            return;
        }
        let current = self
            .seat
            .get_keyboard()
            .and_then(|keyboard| keyboard.current_focus());
        let index = current
            .as_ref()
            .and_then(|surface| {
                windows.iter().position(|window| {
                    self.wl_surface_for_window(window)
                        .is_some_and(|window_surface| window_surface == *surface)
                })
            })
            .unwrap_or(windows.len() - 1);
        let next = if index == 0 {
            windows.len() - 1
        } else {
            index - 1
        };
        self.focus_window(&windows[next].clone());
    }

    pub fn close_focused_window(&mut self) {
        if let Some(surface) = self
            .seat
            .get_keyboard()
            .and_then(|keyboard| keyboard.current_focus())
        {
            if let Some(window) = self.window_for_surface(&surface) {
                if let Some(toplevel) = window.toplevel() {
                    toplevel.send_close();
                } else if let Some(x11_surface) = window.x11_surface()
                    && let Err(error) = x11_surface.close()
                {
                    eprintln!("coranux_wm: impossible de fermer la fenêtre X11 : {error}");
                }
            }
        }
    }

    pub fn focus_window(&mut self, window: &Window) {
        let Some(surface) = self.wl_surface_for_window(window) else {
            return;
        };
        self.space.raise_element(window, true);
        if let Some(keyboard) = self.seat.get_keyboard() {
            keyboard.set_focus(
                self,
                Some(surface),
                smithay::utils::SERIAL_COUNTER.next_serial(),
            );
        }
        self.send_configures_to_windows();
    }

    fn focus_top_window(&mut self) {
        let top = self.space.elements().next_back().cloned();
        if let Some(window) = top {
            self.focus_window(&window);
        } else if let Some(keyboard) = self.seat.get_keyboard() {
            keyboard.set_focus(
                self,
                Option::<WlSurface>::None,
                smithay::utils::SERIAL_COUNTER.next_serial(),
            );
        }
    }

    fn send_configures_to_windows(&self) {
        for window in self.space.elements() {
            if let Some(toplevel) = window.toplevel() {
                toplevel.send_pending_configure();
            }
        }
    }

    pub fn forget_toplevel(&mut self, surface: &WlSurface) {
        self.tiled_windows.retain(|known| known != surface);
        self.desktop_surfaces.retain(|known| known != surface);
        self.panel_surfaces.retain(|known| known != surface);
        self.floating_surfaces.retain(|known| known != surface);
        self.panel_window_ids.retain(|(known, _)| known != surface);
        self.free_window_geometries
            .retain(|(known, _)| known != surface);
        self.tile_frames.retain(|(known, _)| known != surface);
        if self
            .tile_layout_overrides
            .as_ref()
            .is_some_and(|(_, frames)| frames.iter().any(|(known, _)| known == surface))
        {
            self.tile_layout_overrides = None;
        }
        if self
            .titlebar_drag
            .as_ref()
            .is_some_and(|drag| &drag.surface == surface)
        {
            self.titlebar_drag = None;
        }
        if self
            .last_titlebar_click
            .as_ref()
            .is_some_and(|(clicked, _)| clicked == surface)
        {
            self.last_titlebar_click = None;
        }
        if self
            .expanded_tiled_window
            .as_ref()
            .is_some_and(|expanded| expanded == surface)
        {
            self.expanded_tiled_window = None;
        }
        if self.window_mode == WindowMode::Tiling {
            self.reflow_tiled_windows();
        }
    }
}

fn resize_tile_axis(
    frames: &mut [(WlSurface, Rectangle<i32, Logical>)],
    target_index: usize,
    requested_delta: i32,
    horizontal: bool,
    edges: ResizeEdge,
) -> bool {
    const MIN_TILE_EXTENT: i32 = 120;

    let target = frames[target_index].1;
    let resizing_leading_edge = if horizontal {
        edges.contains(ResizeEdge::LEFT)
    } else {
        edges.contains(ResizeEdge::TOP)
    };
    let target_start = if horizontal {
        target.loc.x
    } else {
        target.loc.y
    };
    let target_end = target_start
        + if horizontal {
            target.size.w
        } else {
            target.size.h
        };
    let target_cross_start = if horizontal {
        target.loc.y
    } else {
        target.loc.x
    };
    let target_cross_end = target_cross_start
        + if horizontal {
            target.size.h
        } else {
            target.size.w
        };

    let (same_side, opposite_side) = frames
        .iter()
        .enumerate()
        .filter_map(|(index, (_, rect))| {
            if index == target_index {
                return None;
            }
            let other_start = if horizontal { rect.loc.x } else { rect.loc.y };
            let other_extent = if horizontal { rect.size.w } else { rect.size.h };
            let other_end = other_start + other_extent;
            let other_cross_start = if horizontal { rect.loc.y } else { rect.loc.x };
            let other_cross_end =
                other_cross_start + if horizontal { rect.size.h } else { rect.size.w };
            let on_same_side = if resizing_leading_edge {
                (other_start - target_start).abs() <= 1
            } else {
                (other_end - target_end).abs() <= 1
            };
            let adjacent_on_opposite_side = if resizing_leading_edge {
                (other_end - target_start).abs() <= 1
            } else {
                (other_start - target_end).abs() <= 1
            };
            let cross_touches =
                other_cross_start <= target_cross_end && target_cross_start <= other_cross_end;
            if on_same_side && cross_touches {
                Some((true, index, other_extent))
            } else if adjacent_on_opposite_side && cross_touches {
                Some((false, index, other_extent))
            } else {
                None
            }
        })
        .fold(
            (Vec::new(), Vec::new()),
            |(mut same, mut opposite), entry| {
                let (on_same_side, index, extent) = entry;
                if on_same_side {
                    same.push((index, extent));
                } else {
                    opposite.push((index, extent));
                }
                (same, opposite)
            },
        );
    if opposite_side.is_empty() {
        return false;
    }

    let target_extent = if horizontal {
        target.size.w
    } else {
        target.size.h
    };
    let (min_delta, max_delta) = if resizing_leading_edge {
        let min_delta = opposite_side
            .iter()
            .map(|(_, extent)| MIN_TILE_EXTENT - extent)
            .max()
            .unwrap_or(i32::MIN);
        let max_delta = same_side
            .iter()
            .map(|(_, extent)| extent - MIN_TILE_EXTENT)
            .chain(std::iter::once(target_extent - MIN_TILE_EXTENT))
            .min()
            .unwrap_or(i32::MAX);
        (min_delta, max_delta)
    } else {
        let min_delta = same_side
            .iter()
            .map(|(_, extent)| MIN_TILE_EXTENT - extent)
            .chain(std::iter::once(MIN_TILE_EXTENT - target_extent))
            .max()
            .unwrap_or(i32::MIN);
        (
            min_delta,
            opposite_side
                .iter()
                .map(|(_, extent)| extent - MIN_TILE_EXTENT)
                .min()
                .unwrap_or(i32::MAX),
        )
    };
    if min_delta > max_delta {
        return false;
    }
    let delta = requested_delta.clamp(min_delta, max_delta);
    if delta == 0 {
        return false;
    }

    let update_rect = |rect: &mut Rectangle<i32, Logical>, same_side: bool| {
        let (location, extent) = if horizontal {
            (&mut rect.loc.x, &mut rect.size.w)
        } else {
            (&mut rect.loc.y, &mut rect.size.h)
        };
        if same_side {
            if resizing_leading_edge {
                *location += delta;
                *extent -= delta;
            } else {
                *extent += delta;
            }
        } else if resizing_leading_edge {
            *extent += delta;
        } else {
            *location += delta;
            *extent -= delta;
        }
    };
    update_rect(&mut frames[target_index].1, true);
    for (index, _) in same_side {
        update_rect(&mut frames[index].1, true);
    }
    for (index, _) in opposite_side {
        update_rect(&mut frames[index].1, false);
    }
    true
}

fn scale_rectangle(
    rect: Rectangle<i32, Logical>,
    from: Rectangle<i32, Logical>,
    to: Rectangle<i32, Logical>,
) -> Rectangle<i32, Logical> {
    if from.size.w <= 0 || from.size.h <= 0 {
        return rect;
    }
    let scale = |value: i32, old_origin: i32, old_extent: i32, new_origin: i32, new_extent: i32| {
        new_origin + (((value - old_origin) as i64 * new_extent as i64) / old_extent as i64) as i32
    };
    let left = scale(rect.loc.x, from.loc.x, from.size.w, to.loc.x, to.size.w);
    let right = scale(
        rect.loc.x + rect.size.w,
        from.loc.x,
        from.size.w,
        to.loc.x,
        to.size.w,
    );
    let top = scale(rect.loc.y, from.loc.y, from.size.h, to.loc.y, to.size.h);
    let bottom = scale(
        rect.loc.y + rect.size.h,
        from.loc.y,
        from.size.h,
        to.loc.y,
        to.size.h,
    );
    Rectangle::new(
        (left, top).into(),
        ((right - left).max(1), (bottom - top).max(1)).into(),
    )
}

fn push_solid(
    elements: &mut Vec<SolidColorRenderElement>,
    rect: Rectangle<i32, Logical>,
    color: [f32; 4],
) {
    push_solid_color(elements, rect, Color32F::from(color));
}

fn push_solid_color(
    elements: &mut Vec<SolidColorRenderElement>,
    rect: Rectangle<i32, Logical>,
    color: Color32F,
) {
    let physical = Rectangle::<i32, Physical>::new(
        (rect.loc.x, rect.loc.y).into(),
        (rect.size.w, rect.size.h).into(),
    );
    elements.push(SolidColorRenderElement::new(
        Id::new(),
        physical,
        CommitCounter::default(),
        color,
        Kind::Unspecified,
    ));
}

fn subtract_rectangle(
    rect: Rectangle<i32, Logical>,
    occluder: Rectangle<i32, Logical>,
) -> Vec<Rectangle<i32, Logical>> {
    let Some(intersection) = rect.intersection(occluder) else {
        return vec![rect];
    };

    let rect_right = rect.loc.x + rect.size.w;
    let rect_bottom = rect.loc.y + rect.size.h;
    let intersection_right = intersection.loc.x + intersection.size.w;
    let intersection_bottom = intersection.loc.y + intersection.size.h;
    let mut visible = Vec::with_capacity(4);

    if intersection.loc.y > rect.loc.y {
        visible.push(Rectangle::new(
            rect.loc,
            (rect.size.w, intersection.loc.y - rect.loc.y).into(),
        ));
    }
    if intersection_bottom < rect_bottom {
        visible.push(Rectangle::new(
            (rect.loc.x, intersection_bottom).into(),
            (rect.size.w, rect_bottom - intersection_bottom).into(),
        ));
    }
    if intersection.loc.x > rect.loc.x {
        visible.push(Rectangle::new(
            (rect.loc.x, intersection.loc.y).into(),
            (intersection.loc.x - rect.loc.x, intersection.size.h).into(),
        ));
    }
    if intersection_right < rect_right {
        visible.push(Rectangle::new(
            (intersection_right, intersection.loc.y).into(),
            (rect_right - intersection_right, intersection.size.h).into(),
        ));
    }
    visible
}

fn current_app_id(toplevel: &smithay::wayland::shell::xdg::ToplevelSurface) -> Option<String> {
    with_states(toplevel.wl_surface(), |states| {
        states
            .data_map
            .get::<XdgToplevelSurfaceData>()
            .and_then(|attributes| {
                attributes
                    .lock()
                    .ok()
                    .and_then(|attributes| attributes.app_id.clone())
            })
    })
}

fn current_title(toplevel: &smithay::wayland::shell::xdg::ToplevelSurface) -> Option<String> {
    with_states(toplevel.wl_surface(), |states| {
        states
            .data_map
            .get::<XdgToplevelSurfaceData>()
            .and_then(|attributes| {
                attributes
                    .lock()
                    .ok()
                    .and_then(|attributes| attributes.title.clone())
            })
    })
}

fn is_desktop_toplevel(toplevel: &smithay::wayland::shell::xdg::ToplevelSurface) -> bool {
    current_app_id(toplevel).as_deref() == Some("coranux_desktop")
        || current_title(toplevel).as_deref() == Some("Coranux Desktop")
}

fn is_panel_toplevel(toplevel: &smithay::wayland::shell::xdg::ToplevelSurface) -> bool {
    current_app_id(toplevel).as_deref() == Some("coranux_panel")
        || current_title(toplevel).as_deref() == Some("Coranux Panel")
}

fn panel_geometry(output: Rectangle<i32, Logical>) -> (Point<i32, Logical>, Size<i32, Logical>) {
    let size = Size::from((output.size.w, PANEL_HEIGHT.min(output.size.h)));
    let location = Point::from((output.loc.x, output.loc.y + output.size.h - size.h));
    (location, size)
}

pub(crate) fn work_area_geometry(output: Rectangle<i32, Logical>) -> Rectangle<i32, Logical> {
    Rectangle::new(
        output.loc,
        (
            output.size.w,
            output.size.h - PANEL_HEIGHT.min(output.size.h),
        )
            .into(),
    )
}

fn grid_rectangles(bounds: Rectangle<i32, Logical>, count: usize) -> Vec<Rectangle<i32, Logical>> {
    if count == 0 {
        return Vec::new();
    }

    if count == 1 {
        return vec![bounds];
    }
    if count == 2 {
        let left_width = bounds.size.w / 2;
        return vec![
            Rectangle::new(bounds.loc, (left_width, bounds.size.h).into()),
            Rectangle::new(
                (bounds.loc.x + left_width, bounds.loc.y).into(),
                (bounds.size.w - left_width, bounds.size.h).into(),
            ),
        ];
    }
    if count == 3 {
        let top_height = bounds.size.h * 2 / 3;
        let left_width = bounds.size.w / 2;
        return vec![
            Rectangle::new(bounds.loc, (left_width, top_height).into()),
            Rectangle::new(
                (bounds.loc.x + left_width, bounds.loc.y).into(),
                (bounds.size.w - left_width, top_height).into(),
            ),
            Rectangle::new(
                (bounds.loc.x, bounds.loc.y + top_height).into(),
                (bounds.size.w, bounds.size.h - top_height).into(),
            ),
        ];
    }
    if count == 4 {
        let left_width = bounds.size.w / 2;
        let top_height = bounds.size.h / 2;
        return vec![
            Rectangle::new(bounds.loc, (left_width, top_height).into()),
            Rectangle::new(
                (bounds.loc.x + left_width, bounds.loc.y).into(),
                (bounds.size.w - left_width, top_height).into(),
            ),
            Rectangle::new(
                (bounds.loc.x, bounds.loc.y + top_height).into(),
                (left_width, bounds.size.h - top_height).into(),
            ),
            Rectangle::new(
                (bounds.loc.x + left_width, bounds.loc.y + top_height).into(),
                (bounds.size.w - left_width, bounds.size.h - top_height).into(),
            ),
        ];
    }

    let aspect = bounds.size.w.max(1) as f64 / bounds.size.h.max(1) as f64;
    let columns = (2..=count)
        .min_by(|left, right| {
            grid_aspect_cost(count, *left, aspect)
                .total_cmp(&grid_aspect_cost(count, *right, aspect))
        })
        .unwrap_or(2);
    let row_count = count.div_ceil(columns);
    let mut rects = Vec::with_capacity(count);
    let mut consumed = 0;
    let mut y = bounds.loc.y;
    for row in 0..row_count {
        let in_row = columns.min(count - consumed);
        let height = if row + 1 == row_count {
            bounds.loc.y + bounds.size.h - y
        } else {
            (bounds.size.h * in_row as i32 / count as i32).max(1)
        };
        let mut x = bounds.loc.x;
        for column in 0..in_row {
            let width = if column + 1 == in_row {
                bounds.loc.x + bounds.size.w - x
            } else {
                bounds.size.w / in_row as i32
            };
            rects.push(Rectangle::new((x, y).into(), (width, height).into()));
            x += width;
            consumed += 1;
        }
        y += height;
    }
    rects
}

fn grid_aspect_cost(count: usize, columns: usize, output_aspect: f64) -> f64 {
    let rows = count.div_ceil(columns);
    let mut consumed = 0;
    let mut cost = 0.0;
    for _ in 0..rows {
        let in_row = columns.min(count - consumed);
        let tile_aspect = output_aspect * count as f64 / (in_row * in_row) as f64;
        cost += tile_aspect.ln().powi(2) * in_row as f64;
        consumed += in_row;
    }
    cost
}

#[derive(Default)]
pub struct ClientState {
    pub compositor_state: CompositorClientState,
}

impl ClientData for ClientState {
    fn initialized(&self, _client_id: ClientId) {}
    fn disconnected(&self, _client_id: ClientId, _reason: DisconnectReason) {}
}

#[cfg(test)]
mod tests {
    use super::{panel_geometry, work_area_geometry};
    use smithay::utils::{Logical, Point, Rectangle, Size};

    #[test]
    fn panel_starts_full_width_at_the_bottom_of_the_output() {
        let output = Rectangle::<i32, Logical>::new((10, 20).into(), (1920, 1080).into());
        let (location, size) = panel_geometry(output);

        assert_eq!(location, Point::from((10, 1052)));
        assert_eq!(size, Size::from((1920, 48)));
    }

    #[test]
    fn work_area_ends_above_the_panel() {
        let output = Rectangle::<i32, Logical>::new((10, 20).into(), (1920, 1080).into());

        assert_eq!(
            work_area_geometry(output),
            Rectangle::new((10, 20).into(), (1920, 1032).into())
        );
    }
}
