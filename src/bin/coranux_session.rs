use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Child, Command};
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;
use std::time::Duration;

static SHUTDOWN_REQUESTED: AtomicBool = AtomicBool::new(false);

struct Component {
    name: &'static str,
    executable: &'static str,
    required: bool,
    provides_wayland: bool,
    wayland_client: bool,
}

struct RunningComponent {
    name: &'static str,
    required: bool,
    child: Option<Child>,
}

extern "C" fn request_shutdown(_: libc::c_int) {
    SHUTDOWN_REQUESTED.store(true, Ordering::Relaxed);
}

fn main() {
    match run_session() {
        Ok(code) => std::process::exit(code),
        Err(error) => {
            eprintln!("coranux_session: {error}");
            std::process::exit(1);
        }
    }
}

fn run_session() -> Result<i32, Box<dyn std::error::Error>> {
    install_signal_handlers();

    let already_in_wayland_session = env::var_os("WAYLAND_DISPLAY").is_some();
    let x11_session = env::var("XDG_SESSION_TYPE").as_deref() == Ok("x11");
    let display_without_wayland_session = env::var_os("DISPLAY").is_some()
        && env::var("XDG_SESSION_TYPE").as_deref() != Ok("wayland");
    if already_in_wayland_session || x11_session || display_without_wayland_session {
        return Err(
            "une session graphique incompatible est déjà active ; quitte le compositeur courant et démarre Coranux depuis un TTY ou un gestionnaire de connexion compatible Wayland"
                .into(),
        );
    }

    // The WM owns the display backend and Wayland socket. The desktop and panel
    // connect to that private socket.
    let components = [
        Component {
            name: "Coranux WM",
            executable: "coranux_wm",
            required: true,
            provides_wayland: true,
            wayland_client: false,
        },
        Component {
            name: "Coranux Desktop",
            executable: "coranux_desktop",
            required: true,
            provides_wayland: false,
            wayland_client: true,
        },
        Component {
            name: "Coranux Panel",
            executable: "coranux_panel",
            required: true,
            provides_wayland: false,
            wayland_client: true,
        },
    ];
    let runtime_directory = env::var_os("XDG_RUNTIME_DIR").map(PathBuf::from).ok_or(
        "XDG_RUNTIME_DIR est absent ; lance la session depuis un TTY ou une session utilisateur",
    )?;
    let wayland_display = format!("coranux-wayland-{}", std::process::id());
    let ready_file = runtime_directory.join(format!("{wayland_display}.ready"));
    let xwayland_display_file =
        runtime_directory.join(format!("{wayland_display}.xwayland-display"));
    let desktop_ready_file = runtime_directory.join(format!("{wayland_display}.desktop-ready"));
    let panel_ready_file = runtime_directory.join(format!("{wayland_display}.panel-ready"));
    let socket_file = runtime_directory.join(&wayland_display);
    let panel_state_file = runtime_directory.join(format!("{wayland_display}.panel.json"));
    let panel_command_file = runtime_directory.join(format!("{wayland_display}.panel-command"));
    let _ = fs::remove_file(&ready_file);
    let _ = fs::remove_file(&xwayland_display_file);
    let _ = fs::remove_file(&desktop_ready_file);
    let _ = fs::remove_file(&panel_ready_file);
    let _ = fs::remove_file(&socket_file);
    let _ = fs::remove_file(&panel_state_file);
    let _ = fs::remove_file(&panel_command_file);
    let session_directory = env::current_exe()?
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or_else(|| PathBuf::from("."));
    let mut running = Vec::new();
    let mut xwayland_display = None;

    for component in components {
        let executable = match component_path(&session_directory, component.executable) {
            Ok(executable) => executable,
            Err(error) if component.required => {
                stop_components(&mut running);
                let _ = fs::remove_file(&ready_file);
                let _ = fs::remove_file(&xwayland_display_file);
                let _ = fs::remove_file(&desktop_ready_file);
                let _ = fs::remove_file(&panel_ready_file);
                return Err(format!("impossible de préparer {}: {error}", component.name).into());
            }
            Err(error) => {
                eprintln!(
                    "coranux_session: composant facultatif {} indisponible : {error}",
                    component.name
                );
                continue;
            }
        };
        let mut command = Command::new(&executable);
        command
            .env_remove("DISPLAY")
            .env("CORANUX_SESSION", "1")
            .env("CORANUX_SESSION_PID", std::process::id().to_string())
            .env("CORANUX_PANEL_STATE_FILE", &panel_state_file)
            .env("CORANUX_PANEL_COMMAND_FILE", &panel_command_file)
            .env("XDG_CURRENT_DESKTOP", "Coranux")
            .env("XDG_SESSION_DESKTOP", "coranux")
            .env("XDG_SESSION_TYPE", "wayland")
            .env("DESKTOP_SESSION", "coranux");
        if let Some(display) = &xwayland_display {
            command.env("DISPLAY", display);
        }
        if component.provides_wayland {
            command
                .env("CORANUX_WAYLAND_DISPLAY", &wayland_display)
                .env("CORANUX_WAYLAND_READY_FILE", &ready_file)
                .env("CORANUX_XWAYLAND_DISPLAY_FILE", &xwayland_display_file)
                .env("CORANUX_DESKTOP_READY_FILE", &desktop_ready_file)
                .env("CORANUX_PANEL_READY_FILE", &panel_ready_file);
        }
        if component.wayland_client {
            command.env("WAYLAND_DISPLAY", &wayland_display);
        }

        match command.spawn() {
            Ok(child) => {
                eprintln!(
                    "coranux_session: démarré {} (pid {})",
                    component.name,
                    child.id()
                );
                running.push(RunningComponent {
                    name: component.name,
                    required: component.required,
                    child: Some(child),
                });
                if component.provides_wayland {
                    if let Err(error) = wait_for_wayland_ready(
                        running.last_mut().expect("WM vient d’être ajouté"),
                        &ready_file,
                        &socket_file,
                    ) {
                        stop_components(&mut running);
                        let _ = fs::remove_file(&ready_file);
                        let _ = fs::remove_file(&xwayland_display_file);
                        let _ = fs::remove_file(&desktop_ready_file);
                        let _ = fs::remove_file(&panel_ready_file);
                        return Err(
                            format!("Coranux WM n’a pas initialisé Wayland : {error}").into()
                        );
                    }
                    xwayland_display = match wait_for_xwayland_ready(
                        running.last_mut().expect("WM vient d’être ajouté"),
                        &xwayland_display_file,
                    ) {
                        Ok(display) => Some(display),
                        Err(error) => {
                            stop_components(&mut running);
                            let _ = fs::remove_file(&ready_file);
                            let _ = fs::remove_file(&xwayland_display_file);
                            let _ = fs::remove_file(&desktop_ready_file);
                            let _ = fs::remove_file(&panel_ready_file);
                            return Err(format!(
                                "Coranux WM n’a pas initialisé XWayland : {error}"
                            )
                            .into());
                        }
                    };
                }
                if component.executable == "coranux_desktop" {
                    if let Err(error) = wait_for_component_ready(
                        running.last_mut().expect("bureau vient d’être ajouté"),
                        &desktop_ready_file,
                        "bureau",
                    ) {
                        stop_components(&mut running);
                        let _ = fs::remove_file(&ready_file);
                        let _ = fs::remove_file(&xwayland_display_file);
                        let _ = fs::remove_file(&desktop_ready_file);
                        let _ = fs::remove_file(&panel_ready_file);
                        return Err(format!(
                            "Coranux Desktop n’a pas affiché son bureau : {error}"
                        )
                        .into());
                    }
                    eprintln!("coranux_session: bureau affiché");
                }
                if component.executable == "coranux_panel" {
                    if let Err(error) = wait_for_component_ready(
                        running.last_mut().expect("panneau vient d’être ajouté"),
                        &panel_ready_file,
                        "panneau",
                    ) {
                        stop_components(&mut running);
                        let _ = fs::remove_file(&ready_file);
                        let _ = fs::remove_file(&xwayland_display_file);
                        let _ = fs::remove_file(&desktop_ready_file);
                        let _ = fs::remove_file(&panel_ready_file);
                        return Err(
                            format!("Coranux Panel n’a pas affiché son panneau : {error}").into(),
                        );
                    }
                    eprintln!("coranux_session: panneau affiché");
                }
            }
            Err(error) if component.required => {
                stop_components(&mut running);
                let _ = fs::remove_file(&ready_file);
                let _ = fs::remove_file(&xwayland_display_file);
                let _ = fs::remove_file(&desktop_ready_file);
                let _ = fs::remove_file(&panel_ready_file);
                return Err(format!(
                    "impossible de démarrer {} ({}): {error}",
                    component.name,
                    executable.display()
                )
                .into());
            }
            Err(error) => eprintln!(
                "coranux_session: composant facultatif {} non démarré : {error}",
                component.name
            ),
        }
    }

    if !running.iter().any(|component| component.required) {
        return Err("aucun composant principal n’a démarré".into());
    }

    loop {
        if SHUTDOWN_REQUESTED.load(Ordering::Relaxed) {
            eprintln!("coranux_session: arrêt demandé");
            stop_components(&mut running);
            let _ = fs::remove_file(&ready_file);
            let _ = fs::remove_file(&xwayland_display_file);
            let _ = fs::remove_file(&desktop_ready_file);
            let _ = fs::remove_file(&panel_ready_file);
            return Ok(0);
        }

        let mut required_exit = None;
        for component in &mut running {
            let Some(child) = component.child.as_mut() else {
                continue;
            };
            if let Some(status) = child.try_wait()? {
                eprintln!(
                    "coranux_session: {} s’est arrêté ({status})",
                    component.name
                );
                component.child = None;
                if component.required {
                    required_exit = Some(status.code().unwrap_or(1));
                    break;
                }
            }
        }

        if let Some(code) = required_exit {
            stop_components(&mut running);
            let _ = fs::remove_file(&ready_file);
            let _ = fs::remove_file(&xwayland_display_file);
            let _ = fs::remove_file(&desktop_ready_file);
            let _ = fs::remove_file(&panel_ready_file);
            return Ok(code);
        }
        thread::sleep(Duration::from_millis(150));
    }
}

fn wait_for_component_ready(
    component: &mut RunningComponent,
    ready_file: &Path,
    component_label: &str,
) -> std::io::Result<()> {
    let deadline = std::time::Instant::now() + Duration::from_secs(30);
    loop {
        if ready_file.is_file() {
            return Ok(());
        }
        if SHUTDOWN_REQUESTED.load(Ordering::Relaxed) {
            return Err(std::io::Error::other("arrêt demandé"));
        }
        if let Some(child) = component.child.as_mut() {
            if let Some(status) = child.try_wait()? {
                component.child = None;
                return Err(std::io::Error::other(format!(
                    "processus du {component_label} terminé avant le premier rendu ({status})"
                )));
            }
        }
        if std::time::Instant::now() >= deadline {
            return Err(std::io::Error::new(
                std::io::ErrorKind::TimedOut,
                format!("délai dépassé en attendant le premier rendu du {component_label}"),
            ));
        }
        thread::sleep(Duration::from_millis(50));
    }
}

fn wait_for_wayland_ready(
    component: &mut RunningComponent,
    ready_file: &Path,
    socket_file: &Path,
) -> std::io::Result<()> {
    let deadline = std::time::Instant::now() + Duration::from_secs(15);
    loop {
        if ready_file.is_file() && socket_file.exists() {
            return Ok(());
        }
        if SHUTDOWN_REQUESTED.load(Ordering::Relaxed) {
            return Err(std::io::Error::other("arrêt demandé"));
        }
        if let Some(child) = component.child.as_mut() {
            if let Some(status) = child.try_wait()? {
                component.child = None;
                return Err(std::io::Error::other(format!(
                    "WM terminé avant le démarrage du bureau ({status})"
                )));
            }
        }
        if std::time::Instant::now() >= deadline {
            return Err(std::io::Error::new(
                std::io::ErrorKind::TimedOut,
                "délai dépassé en attendant le socket Wayland",
            ));
        }
        thread::sleep(Duration::from_millis(30));
    }
}

fn wait_for_xwayland_ready(
    component: &mut RunningComponent,
    display_file: &Path,
) -> std::io::Result<String> {
    let deadline = std::time::Instant::now() + Duration::from_secs(15);
    loop {
        if display_file.is_file() {
            let display = fs::read_to_string(display_file)?;
            let display = display.trim();
            let display_number = display.strip_prefix(':').unwrap_or_default();
            if !display_number.is_empty()
                && display_number.bytes().all(|byte| byte.is_ascii_digit())
            {
                return Ok(display.to_owned());
            }
            if display.is_empty() {
                thread::sleep(Duration::from_millis(20));
                continue;
            }
            return Err(std::io::Error::other(
                "numéro d’affichage XWayland invalide",
            ));
        }
        if SHUTDOWN_REQUESTED.load(Ordering::Relaxed) {
            return Err(std::io::Error::other("arrêt demandé"));
        }
        if let Some(child) = component.child.as_mut() {
            if let Some(status) = child.try_wait()? {
                component.child = None;
                return Err(std::io::Error::other(format!(
                    "compositeur terminé avant le démarrage de XWayland ({status})"
                )));
            }
        }
        if std::time::Instant::now() >= deadline {
            return Err(std::io::Error::new(
                std::io::ErrorKind::TimedOut,
                "délai dépassé en attendant XWayland",
            ));
        }
        thread::sleep(Duration::from_millis(50));
    }
}

fn component_path(session_directory: &Path, executable: &str) -> std::io::Result<PathBuf> {
    let sibling = session_directory.join(executable);
    if sibling.is_file() {
        return Ok(sibling);
    }

    Err(std::io::Error::new(
        std::io::ErrorKind::NotFound,
        format!(
            "{} introuvable ; construis les composants avec `cargo build --bins`",
            sibling.display()
        ),
    ))
}

fn stop_components(components: &mut [RunningComponent]) {
    for component in components.iter_mut() {
        if let Some(child) = component.child.as_mut() {
            if unsafe { libc::kill(child.id() as libc::pid_t, libc::SIGTERM) } == -1 {
                let error = std::io::Error::last_os_error();
                if error.raw_os_error() != Some(libc::ESRCH) {
                    eprintln!(
                        "coranux_session: impossible d’arrêter {} proprement : {error}",
                        component.name
                    );
                }
            }
        }
    }

    let deadline = std::time::Instant::now() + Duration::from_secs(2);
    while std::time::Instant::now() < deadline {
        let mut still_running = false;
        for component in components.iter_mut() {
            let Some(child) = component.child.as_mut() else {
                continue;
            };
            match child.try_wait() {
                Ok(Some(_)) => component.child = None,
                Ok(None) => still_running = true,
                Err(error) => {
                    eprintln!(
                        "coranux_session: impossible de vérifier l’arrêt de {} : {error}",
                        component.name
                    );
                    still_running = true;
                }
            }
        }
        if !still_running {
            return;
        }
        thread::sleep(Duration::from_millis(20));
    }

    for component in components.iter_mut() {
        if let Some(mut child) = component.child.take() {
            if let Err(error) = child.kill() {
                eprintln!(
                    "coranux_session: impossible de forcer l’arrêt de {} : {error}",
                    component.name
                );
            }
            let _ = child.wait();
        }
    }
}

fn install_signal_handlers() {
    unsafe {
        let handler = request_shutdown as *const () as libc::sighandler_t;
        libc::signal(libc::SIGINT, handler);
        libc::signal(libc::SIGTERM, handler);
    }
}
