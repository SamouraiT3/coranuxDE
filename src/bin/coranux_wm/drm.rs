use std::cell::{Cell, RefCell};
use std::path::Path;
use std::rc::Rc;
use std::time::Duration;

use smithay::{
    backend::{
        allocator::{
            Fourcc,
            gbm::{GbmAllocator, GbmBufferFlags, GbmDevice},
        },
        drm::{
            DrmDevice, DrmDeviceFd, DrmEvent, DrmNode, NodeType,
            compositor::FrameFlags,
            exporter::gbm::GbmFramebufferExporter,
            output::{DrmOutputManager, DrmOutputRenderElements},
        },
        egl::{EGLContext, EGLDisplay},
        input::InputEvent,
        libinput::{LibinputInputBackend, LibinputSessionInterface},
        renderer::{
            ImportDma, Renderer,
            element::{
                Kind, Wrap,
                memory::{MemoryRenderBuffer, MemoryRenderBufferRenderElement},
                render_elements,
                solid::SolidColorRenderElement,
                surface::{WaylandSurfaceRenderElement, render_elements_from_surface_tree},
            },
            gles::GlesRenderer,
        },
        session::{Event as SessionEvent, Session, libseat::LibSeatSession},
        udev::{all_gpus, primary_gpu},
    },
    desktop::space::{SpaceRenderElements, space_render_elements},
    output::{Mode as WlMode, Output, PhysicalProperties, Scale as OutputScale, Subpixel},
    reexports::{
        calloop::{
            EventLoop,
            timer::{TimeoutAction, Timer},
        },
        drm::control::{
            Device as DrmControlDevice, ModeTypeFlags, connector::State as ConnectorState,
        },
        input::{DeviceCapability, Libinput},
        rustix::fs::OFlags,
    },
    utils::{DeviceFd, Transform},
    wayland::dmabuf::DmabufFeedbackBuilder,
};

use crate::{CalloopData, Smallvil};

render_elements! {
    CoranuxOutputRenderElements<=GlesRenderer>;
    Space=SpaceRenderElements<GlesRenderer, WaylandSurfaceRenderElement<GlesRenderer>>,
    Decoration=Wrap<SolidColorRenderElement>,
    CursorSurface=WaylandSurfaceRenderElement<GlesRenderer>,
    CursorBitmap=MemoryRenderBufferRenderElement<GlesRenderer>,
}

const FRAME_INTERVAL: Duration = Duration::from_millis(16);
const COLOR_FORMATS: &[Fourcc] = &[
    Fourcc::Abgr2101010,
    Fourcc::Argb2101010,
    Fourcc::Abgr8888,
    Fourcc::Argb8888,
];
const DEFAULT_CURSOR: [&str; 24] = [
    "b...............",
    "bb..............",
    "bwb.............",
    "bwwb............",
    "bwwwb...........",
    "bwwwwb..........",
    "bwwwwwb.........",
    "bwwwwwwb........",
    "bwwwwwwwb.......",
    "bwwwwwwwwb......",
    "bwwwwwwwwwb.....",
    "bwwwwwwwwwwb....",
    "bwwwwwwwwwwwb...",
    "bwwwwwwwwwwwwb..",
    "bwwwwwwbbbbbbb..",
    "bwwwbwbbb.......",
    "bwwbwwb.........",
    "bwbbwwb.........",
    "bb..bwwb........",
    "....bwwb........",
    "....bbwwb.......",
    ".....bwwb.......",
    ".....bbwwb......",
    "......bbbb......",
];

pub fn init_drm(
    event_loop: &mut EventLoop<CalloopData>,
    data: &mut CalloopData,
) -> Result<(), Box<dyn std::error::Error>> {
    let (mut session, session_notifier) = LibSeatSession::new().map_err(|error| {
        std::io::Error::other(format!(
            "libseat n’a pas pu ouvrir la session graphique ({error}). Vérifie que systemd-logind est actif ou démarre seatd avec `sudo systemctl enable --now seatd`; si tu utilises seatd, ajoute ton utilisateur au groupe `seat` puis reconnecte-toi"
        ))
    })?;
    let seat_name = session.seat();
    if !session.is_active() {
        return Err(format!(
            "la session du siège {seat_name} est inactive ; bascule sur son TTY avec Ctrl+Alt+F<n>, puis relance Coranux"
        )
        .into());
    }
    data.state.seat_session = Some(session.clone());
    let mut gpu_paths = all_gpus(&seat_name)?;
    let primary_gpu = primary_gpu(&seat_name)?;
    if let Some(primary_gpu) = primary_gpu {
        gpu_paths.retain(|path| path != &primary_gpu);
        gpu_paths.insert(0, primary_gpu);
    }

    let mut selected_gpu = None;
    let mut gpu_errors = Vec::new();
    for gpu_path in gpu_paths {
        let device = match session.open(
            &gpu_path,
            OFlags::RDWR | OFlags::CLOEXEC | OFlags::NOCTTY | OFlags::NONBLOCK,
        ) {
            Ok(device) => DrmDeviceFd::new(DeviceFd::from(device)),
            Err(error) => {
                gpu_errors.push(format!("{} : {error}", gpu_path.display()));
                continue;
            }
        };
        let (drm_device, drm_notifier) = match DrmDevice::new(device.clone(), true) {
            Ok(result) => result,
            Err(error) => {
                gpu_errors.push(format!("{} : {error}", gpu_path.display()));
                continue;
            }
        };
        match find_output(&drm_device) {
            Ok(output) => {
                selected_gpu = Some((gpu_path, device, drm_device, drm_notifier, output));
                break;
            }
            Err(error) => gpu_errors.push(format!("{} : {error}", gpu_path.display())),
        }
    }
    let (gpu_path, device_fd, drm_device, drm_notifier, (connector, mode, crtc)) = selected_gpu
        .ok_or_else(|| {
            if gpu_errors.is_empty() {
                "aucun périphérique DRM disponible pour le siège".to_owned()
            } else {
                format!(
                    "aucun GPU du siège {seat_name} ne pilote un écran connecté : {}",
                    gpu_errors.join("; ")
                )
            }
        })?;
    eprintln!(
        "coranux_wm: écran {} trouvé sur {}",
        connector,
        gpu_path.display()
    );
    let gbm = GbmDevice::new(device_fd.clone())?;
    let egl_display = unsafe { EGLDisplay::new(gbm.clone())? };
    let egl_context = EGLContext::new(&egl_display)?;
    let renderer = unsafe { GlesRenderer::new(egl_context)? };
    let render_formats = renderer
        .egl_context()
        .dmabuf_render_formats()
        .iter()
        .copied()
        .collect::<Vec<_>>();
    let dmabuf_formats = renderer
        .dmabuf_formats()
        .iter()
        .copied()
        .collect::<Vec<_>>();
    if dmabuf_formats.is_empty() {
        return Err("le moteur EGL n’annonce aucun format DMABUF importable".into());
    }
    let drm_node = DrmNode::from_file(device_fd.device_fd())?;
    let main_device = drm_node
        .node_with_type(NodeType::Render)
        .and_then(Result::ok)
        .unwrap_or(drm_node);
    let dmabuf_feedback =
        DmabufFeedbackBuilder::new(main_device.dev_id(), dmabuf_formats).build()?;
    eprintln!("coranux_wm: linux-dmabuf activé pour le périphérique {main_device}");
    let renderer = Rc::new(RefCell::new(renderer));
    data.state.dmabuf_renderer = Some(Rc::clone(&renderer));
    data.state.dmabuf_global = Some(
        data.state
            .dmabuf_state
            .create_global_with_default_feedback::<Smallvil>(
                &data.display_handle,
                &dmabuf_feedback,
            ),
    );
    let mode_wl = WlMode::from(mode);
    let (width_mm, height_mm) = connector.size().unwrap_or((0, 0));
    eprintln!(
        "coranux_wm: géométrie sortie: mode={}x{}@{}Hz, taille physique={}x{}mm, échelle logique=1",
        mode.size().0,
        mode.size().1,
        mode.vrefresh(),
        width_mm,
        height_mm
    );
    let output = Output::new(
        "Coranux".to_owned(),
        PhysicalProperties {
            size: (width_mm as i32, height_mm as i32).into(),
            make: "Coranux".into(),
            model: "DRM".into(),
            subpixel: Subpixel::Unknown,
        },
    );
    output.set_preferred(mode_wl);
    output.change_current_state(
        Some(mode_wl),
        Some(Transform::Normal),
        Some(OutputScale::Integer(1)),
        Some((0, 0).into()),
    );
    output.create_global::<Smallvil>(&data.display_handle);
    data.state.space.map_output(&output, (0, 0));

    let allocator = GbmAllocator::new(
        gbm.clone(),
        GbmBufferFlags::RENDERING | GbmBufferFlags::SCANOUT,
    );
    let exporter = GbmFramebufferExporter::new(gbm.clone(), None);
    let mut output_manager = DrmOutputManager::new(
        drm_device,
        allocator,
        exporter,
        Some(gbm),
        COLOR_FORMATS.iter().copied(),
        render_formats,
    );
    let initial_render_elements =
        DrmOutputRenderElements::<GlesRenderer, CoranuxOutputRenderElements>::default();
    let drm_output = output_manager.initialize_output::<_, CoranuxOutputRenderElements>(
        crtc,
        mode,
        &[connector.handle()],
        &output,
        None,
        &mut *renderer.borrow_mut(),
        &initial_render_elements,
    )?;
    let drm_output = Rc::new(RefCell::new(drm_output));
    let frame_pending = Rc::new(Cell::new(false));
    let cursor_buffer = default_cursor_buffer();

    let mut libinput_context =
        Libinput::new_with_udev::<LibinputSessionInterface<LibSeatSession>>(session.clone().into());
    if libinput_context.udev_assign_seat(&seat_name).is_err() {
        return Err(format!("impossible d’assigner libinput au siège {seat_name}").into());
    }
    let input_backend = LibinputInputBackend::new(libinput_context.clone());
    let mut keyboard_devices = Vec::new();
    event_loop
        .handle()
        .insert_source(input_backend, move |mut event, _, data| {
            if let InputEvent::DeviceAdded { device } = &mut event {
                if device.has_capability(DeviceCapability::Keyboard) {
                    keyboard_devices.push(device.clone());
                    if let Some(led_state) = data
                        .state
                        .seat
                        .get_keyboard()
                        .map(|keyboard| keyboard.led_state())
                    {
                        device.led_update(led_state.into());
                    }
                }
            }
            if let InputEvent::DeviceRemoved { device } = &event {
                keyboard_devices.retain(|known| known != device);
            }
            let keyboard_event = matches!(&event, InputEvent::Keyboard { .. });
            data.state.process_input_event(event);
            if keyboard_event
                && let Some(led_state) = data
                    .state
                    .seat
                    .get_keyboard()
                    .map(|keyboard| keyboard.led_state())
            {
                for device in &mut keyboard_devices {
                    device.led_update(led_state.into());
                }
            }
        })?;

    let pending_for_session = Rc::clone(&frame_pending);
    event_loop
        .handle()
        .insert_source(session_notifier, move |event, _, _| match event {
            SessionEvent::PauseSession => {
                eprintln!("coranux_wm: siège suspendu, mise en pause de libinput et DRM");
                libinput_context.suspend();
                output_manager.pause();
                pending_for_session.set(false);
            }
            SessionEvent::ActivateSession => {
                if let Err(error) = libinput_context.resume() {
                    eprintln!("coranux_wm: impossible de reprendre libinput : {error:?}");
                }
                if let Err(error) = output_manager.activate(false) {
                    eprintln!("coranux_wm: impossible de reprendre DRM : {error}");
                }
                pending_for_session.set(false);
            }
        })?;

    let desktop_ready_file =
        std::env::var_os("CORANUX_DESKTOP_READY_FILE").map(std::path::PathBuf::from);
    let panel_ready_file =
        std::env::var_os("CORANUX_PANEL_READY_FILE").map(std::path::PathBuf::from);
    let desktop_ready = Rc::new(Cell::new(false));
    let panel_ready = Rc::new(Cell::new(false));
    let desktop_ready_pending = Rc::new(Cell::new(false));
    let panel_ready_pending = Rc::new(Cell::new(false));
    let output_for_vblank = Rc::clone(&drm_output);
    let pending_for_vblank = Rc::clone(&frame_pending);
    let desktop_ready_for_vblank = Rc::clone(&desktop_ready);
    let panel_ready_for_vblank = Rc::clone(&panel_ready);
    let desktop_ready_pending_for_vblank = Rc::clone(&desktop_ready_pending);
    let panel_ready_pending_for_vblank = Rc::clone(&panel_ready_pending);
    event_loop
        .handle()
        .insert_source(drm_notifier, move |event, _, _| match event {
            DrmEvent::VBlank(event_crtc) if event_crtc == crtc => {
                let _ = output_for_vblank.borrow_mut().frame_submitted();
                pending_for_vblank.set(false);
                if desktop_ready_pending_for_vblank.replace(false)
                    && let Some(path) = &desktop_ready_file
                {
                    match write_ready_signal(path) {
                        Ok(()) => {
                            desktop_ready_for_vblank.set(true);
                        }
                        Err(error) => {
                            eprintln!(
                                "coranux_wm: impossible de signaler le rendu du bureau : {error}"
                            );
                        }
                    }
                }
                if panel_ready_pending_for_vblank.replace(false)
                    && let Some(path) = &panel_ready_file
                {
                    match write_ready_signal(path) {
                        Ok(()) => {
                            panel_ready_for_vblank.set(true);
                        }
                        Err(error) => {
                            eprintln!(
                                "coranux_wm: impossible de signaler le rendu du panneau : {error}"
                            );
                        }
                    }
                }
            }
            DrmEvent::VBlank(_) => {}
            DrmEvent::Error(error) => {
                eprintln!("coranux_wm: erreur DRM : {error}");
            }
        })?;

    let panel_state_file =
        std::env::var_os("CORANUX_PANEL_STATE_FILE").map(std::path::PathBuf::from);
    let panel_command_file =
        std::env::var_os("CORANUX_PANEL_COMMAND_FILE").map(std::path::PathBuf::from);
    let mut last_panel_snapshot = None;
    let pending_for_render = Rc::clone(&frame_pending);
    let renderer_for_render = Rc::clone(&renderer);
    event_loop.handle().insert_source(
        Timer::from_duration(Duration::ZERO),
        move |_, _, data| {
            if let Some(path) = &panel_command_file {
                if let Err(error) = process_panel_command(path, &mut data.state) {
                    eprintln!("coranux_wm: commande du panneau ignorée : {error}");
                }
            }
            if let Some(path) = &panel_state_file {
                let snapshot = data.state.panel_snapshot();
                if last_panel_snapshot.as_ref() != Some(&snapshot) {
                    if let Err(error) = write_panel_snapshot(path, &snapshot) {
                        eprintln!("coranux_wm: impossible de publier l’état du panneau : {error}");
                    } else {
                        last_panel_snapshot = Some(snapshot);
                    }
                }
            }
            if pending_for_render.get() {
                return TimeoutAction::ToDuration(FRAME_INTERVAL);
            }

            data.state.advance_window_animations();
            let mut renderer_guard = renderer_for_render.borrow_mut();
            let renderer = &mut *renderer_guard;
            let output_scale = output.current_scale().fractional_scale();
            let cursor_location = data
                .state
                .pointer_location
                .to_physical_precise_round(output_scale);
            let mut elements = Vec::new();
            match data.state.cursor_image_status.clone() {
                smithay::input::pointer::CursorImageStatus::Hidden => {
                }
                smithay::input::pointer::CursorImageStatus::Named(_) => {
                    match MemoryRenderBufferRenderElement::from_buffer(
                        renderer,
                        cursor_location,
                        &cursor_buffer,
                        None,
                        None,
                        None,
                        Kind::Cursor,
                    ) {
                        Ok(cursor) => {
                            elements.push(CoranuxOutputRenderElements::CursorBitmap(cursor));
                        }
                        Err(error) => {
                            eprintln!("coranux_wm: impossible de préparer le pointeur : {error}");
                        }
                    }
                }
                smithay::input::pointer::CursorImageStatus::Surface(surface) => {
                    let hotspot = smithay::wayland::compositor::with_states(&surface, |states| {
                        states
                            .data_map
                            .get::<smithay::input::pointer::CursorImageSurfaceData>()
                            .and_then(|attributes| {
                                attributes.lock().ok().map(|attributes| attributes.hotspot)
                            })
                    })
                    .unwrap_or_default();
                    let location = (data.state.pointer_location - hotspot.to_f64())
                        .to_physical_precise_round(output_scale);
                    let cursor_elements =
                        render_elements_from_surface_tree::<
                            GlesRenderer,
                            WaylandSurfaceRenderElement<GlesRenderer>,
                    >(renderer, &surface, location, output_scale, 1.0, Kind::Cursor);
                    if cursor_elements.is_empty() {
                        match MemoryRenderBufferRenderElement::from_buffer(
                            renderer,
                            cursor_location,
                            &cursor_buffer,
                            None,
                            None,
                            None,
                            Kind::Cursor,
                        ) {
                            Ok(cursor) => {
                                elements.push(CoranuxOutputRenderElements::CursorBitmap(cursor));
                            }
                            Err(error) => {
                                eprintln!(
                                    "coranux_wm: impossible de préparer le pointeur de secours : {error}"
                                );
                            }
                        }
                    } else {
                        elements.extend(
                            cursor_elements
                                .into_iter()
                                .map(CoranuxOutputRenderElements::CursorSurface),
                        );
                    }
                }
            }
            // DRM composition consumes render elements front-to-back. Keep the
            // cursor and decorations before opaque window surfaces so those
            // surfaces cannot hide them during occlusion culling.
            elements.extend(
                data.state
                    .decoration_elements()
                    .into_iter()
                    .rev()
                    .map(|element| CoranuxOutputRenderElements::Decoration(Wrap::from(element))),
            );
            let space_elements =
                match space_render_elements(renderer, [&data.state.space], &output, 1.0) {
                    Ok(elements) => elements
                    .into_iter()
                    .map(CoranuxOutputRenderElements::Space)
                    .collect::<Vec<_>>(),
                    Err(error) => {
                    eprintln!("coranux_wm: impossible de préparer le rendu : {error}");
                    return TimeoutAction::ToDuration(FRAME_INTERVAL);
                    }
                };
            elements.extend(space_elements);
            let desktop_surface_ready = data.state.space.elements().any(|window| {
                window.geometry().size.w > 0
                    && window.geometry().size.h > 0
                    && window.toplevel().is_some_and(|toplevel| {
                        data.state.is_desktop_surface(toplevel.wl_surface())
                    })
            });
            let panel_surface_ready = data.state.space.elements().any(|window| {
                window.geometry().size.w > 0
                    && window.geometry().size.h > 0
                    && window
                        .toplevel()
                        .is_some_and(|toplevel| data.state.is_panel_surface(toplevel.wl_surface()))
            });

            let frame_result = drm_output.borrow_mut().render_frame(
                renderer,
                &elements,
                [0.1, 0.1, 0.1, 1.0],
                FrameFlags::empty(),
            );
            let frame_queued = match frame_result {
                Ok(frame) if !frame.is_empty => {
                    let sync_result = if frame.needs_sync() {
                        match &frame.primary_element {
                            smithay::backend::drm::compositor::PrimaryPlaneElement::Swapchain(
                                primary,
                            ) => renderer.wait(&primary.sync),
                            smithay::backend::drm::compositor::PrimaryPlaneElement::Element(_) => {
                                Ok(())
                            }
                        }
                    } else {
                        Ok(())
                    };
                    drop(frame);
                    match sync_result {
                        Ok(()) => match drm_output.borrow_mut().queue_frame(()) {
                            Ok(()) => true,
                            Err(error) => {
                                eprintln!("coranux_wm: impossible de présenter l’image : {error}");
                                false
                            }
                        },
                        Err(error) => {
                            eprintln!("coranux_wm: synchronisation du rendu impossible : {error}");
                            false
                        }
                    }
                }
                Ok(_) => false,
                Err(_) => false,
            };

            if frame_queued {
                pending_for_render.set(true);
                if desktop_surface_ready && !desktop_ready.get() {
                    desktop_ready_pending.set(true);
                }
                if panel_surface_ready && !panel_ready.get() {
                    panel_ready_pending.set(true);
                }
                data.state.space.elements().for_each(|window| {
                    window.send_frame(
                        &output,
                        data.state.start_time.elapsed(),
                        Some(Duration::ZERO),
                        |_, _| Some(output.clone()),
                    )
                });
            }
            data.state.space.refresh();
            data.state.popups.cleanup();
            if let Err(error) = data.display_handle.flush_clients() {
                eprintln!("coranux_wm: impossible d’envoyer les événements Wayland : {error}");
            }
            TimeoutAction::ToDuration(FRAME_INTERVAL)
        },
    )?;

    Ok(())
}

fn find_output(
    drm_device: &DrmDevice,
) -> Result<
    (
        smithay::reexports::drm::control::connector::Info,
        smithay::reexports::drm::control::Mode,
        smithay::reexports::drm::control::crtc::Handle,
    ),
    Box<dyn std::error::Error>,
> {
    let resources = drm_device.resource_handles()?;
    for connector_handle in resources.connectors() {
        let connector = drm_device.get_connector(*connector_handle, true)?;
        if connector.state() != ConnectorState::Connected {
            continue;
        }
        let Some(mode) = connector
            .modes()
            .iter()
            .find(|mode| mode.mode_type().contains(ModeTypeFlags::PREFERRED))
            .or_else(|| connector.modes().first())
            .copied()
        else {
            continue;
        };
        for encoder_handle in connector.encoders() {
            let encoder = drm_device.get_encoder(*encoder_handle)?;
            if let Some(crtc) = resources
                .filter_crtcs(encoder.possible_crtcs())
                .into_iter()
                .next()
            {
                return Ok((connector, mode, crtc));
            }
        }
    }
    Err("aucun écran connecté avec un mode DRM compatible".into())
}

fn default_cursor_buffer() -> MemoryRenderBuffer {
    let pixels = DEFAULT_CURSOR
        .iter()
        .flat_map(|row| row.bytes())
        .flat_map(|pixel| match pixel {
            b'b' => [0, 0, 0, 255],
            b'w' => [255, 255, 255, 255],
            _ => [0, 0, 0, 0],
        })
        .collect::<Vec<_>>();
    MemoryRenderBuffer::from_slice(
        &pixels,
        Fourcc::Abgr8888,
        (16, 24),
        1,
        Transform::Normal,
        None,
    )
}

fn process_panel_command(
    path: &Path,
    state: &mut Smallvil,
) -> Result<(), Box<dyn std::error::Error>> {
    let command = match std::fs::read_to_string(path) {
        Ok(command) => command,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error.into()),
    };
    std::fs::remove_file(path)?;
    let mut fields = command.split_whitespace();
    match (fields.next(), fields.next(), fields.next()) {
        (Some("activate"), Some(id), None) => state.activate_panel_window(id.parse::<u64>()?),
        (Some("layout-next"), None, None) => state.cycle_keyboard_layout(),
        _ => eprintln!("coranux_wm: commande de panneau inconnue"),
    }
    Ok(())
}

fn write_panel_snapshot(
    path: &Path,
    snapshot: &crate::state::PanelSnapshot,
) -> Result<(), Box<dyn std::error::Error>> {
    let contents = serde_json::to_vec(snapshot)?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let temporary = path.with_extension(format!("tmp-{}", std::process::id()));
    if let Err(error) =
        std::fs::write(&temporary, contents).and_then(|()| std::fs::rename(&temporary, path))
    {
        let _ = std::fs::remove_file(&temporary);
        return Err(error.into());
    }
    Ok(())
}

fn write_ready_signal(path: &Path) -> Result<(), std::io::Error> {
    let temporary = path.with_extension(format!("tmp-{}", std::process::id()));
    if let Err(error) =
        std::fs::write(&temporary, b"ready").and_then(|()| std::fs::rename(&temporary, path))
    {
        let _ = std::fs::remove_file(&temporary);
        return Err(error);
    }
    Ok(())
}
