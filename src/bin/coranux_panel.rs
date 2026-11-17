use std::path::PathBuf;
use std::process::Command;
use std::time::{Duration, Instant};

use eframe::egui::{self, Align, Color32, Layout, RichText, ScrollArea, Stroke};
use serde::{Deserialize, Serialize};

const PANEL_HEIGHT: f32 = 48.0;
const SNAPSHOT_POLL_INTERVAL: Duration = Duration::from_millis(300);
const PLUGIN_RESCAN_INTERVAL: Duration = Duration::from_secs(5);

#[derive(Clone, Deserialize, PartialEq, Eq)]
struct PanelWindow {
    id: u64,
    title: String,
    app_id: Option<String>,
    minimized: bool,
    focused: bool,
}

#[derive(Default, Deserialize, PartialEq, Eq)]
struct PanelSnapshot {
    windows: Vec<PanelWindow>,
    #[serde(default)]
    keyboard_layout: String,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
struct PluginManifest {
    name: String,
    label: String,
    command: Vec<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
struct PinnedLauncher {
    label: String,
    command: Vec<String>,
}

#[derive(Debug, Default, Deserialize, Serialize)]
struct PinnedLaunchersConfig {
    #[serde(default)]
    launchers: Vec<PinnedLauncher>,
}

struct CoranuxPanel {
    state_file: PathBuf,
    command_file: PathBuf,
    launchers_file: PathBuf,
    snapshot: PanelSnapshot,
    launchers: Vec<PinnedLauncher>,
    plugins: Vec<Box<dyn PanelPlugin>>,
    plugins_scanned_at: Instant,
}

trait PanelPlugin {
    fn show(&mut self, ui: &mut egui::Ui, context: &egui::Context);
}

struct ClockPlugin {
    clock: String,
    last_updated: Instant,
}

impl PanelPlugin for ClockPlugin {
    fn show(&mut self, ui: &mut egui::Ui, context: &egui::Context) {
        if self.last_updated.elapsed() >= Duration::from_secs(1) {
            self.clock = current_time();
            self.last_updated = Instant::now();
        }
        ui.label(
            RichText::new(&self.clock)
                .size(14.0)
                .strong()
                .color(Color32::from_rgb(232, 239, 249)),
        );
        context.request_repaint_after(Duration::from_secs(1));
    }
}

struct SessionPlugin;

impl PanelPlugin for SessionPlugin {
    fn show(&mut self, ui: &mut egui::Ui, context: &egui::Context) {
        let quit = ui
            .add_sized(
                [40.0, 36.0],
                egui::Button::new(
                    RichText::new("⏻")
                        .size(19.0)
                        .color(Color32::from_rgb(255, 145, 150)),
                )
                .fill(Color32::from_rgb(55, 31, 41))
                .stroke(Stroke::new(1.0_f32, Color32::from_rgb(109, 54, 65))),
            )
            .on_hover_text("Quitter Coranux");
        if quit.clicked() {
            request_session_shutdown();
            context.send_viewport_cmd(egui::ViewportCommand::Close);
        }
    }
}

struct AudioPlugin {
    backend: AudioBackend,
    volume: u8,
    muted: bool,
    last_updated: Instant,
}

#[derive(Clone, Copy)]
enum AudioBackend {
    Wpctl,
    Pactl,
}

impl PanelPlugin for AudioPlugin {
    fn show(&mut self, ui: &mut egui::Ui, context: &egui::Context) {
        if self.last_updated.elapsed() >= Duration::from_secs(1) {
            self.refresh();
        }
        let backend = match self.backend {
            AudioBackend::Wpctl => "PipeWire (wpctl)",
            AudioBackend::Pactl => "PulseAudio (pactl)",
        };
        let icon = if self.muted || self.volume == 0 {
            "Muet"
        } else {
            "Audio"
        };
        let mute = ui
            .small_button(format!("{icon} {}%", self.volume))
            .on_hover_text(if self.muted {
                format!("{backend} — muet, cliquer pour réactiver")
            } else {
                format!("{backend} — cliquer pour couper/rétablir le son")
            });
        if mute.clicked() {
            self.toggle_mute();
            self.refresh();
        }
        if ui.small_button("−").clicked() {
            self.adjust_volume(false);
            self.refresh();
        }
        if ui.small_button("+").clicked() {
            self.adjust_volume(true);
            self.refresh();
        }
        context.request_repaint_after(Duration::from_secs(1));
    }
}

impl AudioPlugin {
    fn refresh(&mut self) {
        let output = match self.backend {
            AudioBackend::Wpctl => Command::new("wpctl")
                .args(["get-volume", "@DEFAULT_AUDIO_SINK@"])
                .output(),
            AudioBackend::Pactl => Command::new("pactl")
                .args(["get-sink-volume", "@DEFAULT_SINK@"])
                .output(),
        };
        match output {
            Ok(output) if output.status.success() => {
                let text = String::from_utf8_lossy(&output.stdout);
                match self.backend {
                    AudioBackend::Wpctl => {
                        self.volume = parse_wpctl_volume(&text).unwrap_or(self.volume);
                        self.muted = text.contains("[MUTED]");
                    }
                    AudioBackend::Pactl => {
                        self.volume = parse_pactl_volume(&text).unwrap_or(self.volume);
                        match Command::new("pactl")
                            .args(["get-sink-mute", "@DEFAULT_SINK@"])
                            .output()
                        {
                            Ok(output) if output.status.success() => {
                                self.muted = String::from_utf8_lossy(&output.stdout)
                                    .to_ascii_lowercase()
                                    .contains("yes");
                            }
                            Ok(output) => eprintln!(
                                "coranux_panel: pactl get-sink-mute a quitté avec {}",
                                output.status
                            ),
                            Err(error) => {
                                eprintln!("coranux_panel: impossible de lire l’état muet : {error}")
                            }
                        }
                    }
                }
            }
            Ok(output) => eprintln!(
                "coranux_panel: lecture audio a quitté avec {}",
                output.status
            ),
            Err(error) => eprintln!("coranux_panel: impossible de lire le volume : {error}"),
        }
        self.last_updated = Instant::now();
    }

    fn adjust_volume(&self, increase: bool) {
        let (program, arguments) = match self.backend {
            AudioBackend::Wpctl => (
                "wpctl",
                vec![
                    "set-volume",
                    "@DEFAULT_AUDIO_SINK@",
                    if increase { "5%+" } else { "5%-" },
                ],
            ),
            AudioBackend::Pactl => (
                "pactl",
                vec![
                    "set-sink-volume",
                    "@DEFAULT_SINK@",
                    if increase { "+5%" } else { "-5%" },
                ],
            ),
        };
        run_audio_command(program, &arguments, "ajuster le volume");
    }

    fn toggle_mute(&self) {
        let (program, arguments) = match self.backend {
            AudioBackend::Wpctl => ("wpctl", vec!["set-mute", "@DEFAULT_AUDIO_SINK@", "toggle"]),
            AudioBackend::Pactl => ("pactl", vec!["set-sink-mute", "@DEFAULT_SINK@", "toggle"]),
        };
        run_audio_command(program, &arguments, "changer l’état audio");
    }
}

fn run_audio_command(program: &str, arguments: &[&str], action: &str) {
    match Command::new(program).args(arguments).status() {
        Ok(status) if status.success() => {}
        Ok(status) => eprintln!("coranux_panel: {action} a échoué ({status})"),
        Err(error) => eprintln!("coranux_panel: impossible de {action} : {error}"),
    }
}

struct ExternalPlugin {
    manifest: PluginManifest,
}

impl PanelPlugin for ExternalPlugin {
    fn show(&mut self, ui: &mut egui::Ui, _context: &egui::Context) {
        if ui.small_button(&self.manifest.label).clicked() {
            let Some((program, arguments)) = self.manifest.command.split_first() else {
                return;
            };
            if let Err(error) = Command::new(program).args(arguments).spawn() {
                eprintln!(
                    "coranux_panel: impossible de lancer le greffon {} : {error}",
                    self.manifest.name
                );
            }
        }
    }
}

impl CoranuxPanel {
    fn new() -> Self {
        let state_file = std::env::var_os("CORANUX_PANEL_STATE_FILE")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("/nonexistent/coranux-panel-state.json"));
        let command_file = std::env::var_os("CORANUX_PANEL_COMMAND_FILE")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("/nonexistent/coranux-panel-command"));
        let launchers_file = launchers_config_path();

        Self {
            state_file,
            command_file,
            launchers: read_pinned_launchers(&launchers_file),
            launchers_file,
            snapshot: PanelSnapshot::default(),
            plugins: discover_plugins(),
            plugins_scanned_at: Instant::now(),
        }
    }

    fn refresh_snapshot(&mut self) {
        match std::fs::read(&self.state_file) {
            Ok(contents) => match serde_json::from_slice::<PanelSnapshot>(&contents) {
                Ok(snapshot) => self.snapshot = snapshot,
                Err(error) => eprintln!("coranux_panel: état du panneau invalide : {error}"),
            },
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => eprintln!(
                "coranux_panel: impossible de lire {} : {error}",
                self.state_file.display()
            ),
        }
    }

    fn refresh_plugins(&mut self) {
        if self.plugins_scanned_at.elapsed() >= PLUGIN_RESCAN_INTERVAL {
            self.plugins = discover_plugins();
            self.launchers = read_pinned_launchers(&self.launchers_file);
            self.plugins_scanned_at = Instant::now();
        }
    }

    fn activate_window(&self, id: u64) {
        self.send_compositor_command(&format!("activate {id}"));
    }

    fn cycle_keyboard_layout(&self) {
        self.send_compositor_command("layout-next");
    }

    fn send_compositor_command(&self, command: &str) {
        let temporary = self
            .command_file
            .with_extension(format!("tmp-{}", std::process::id()));
        let result = std::fs::write(&temporary, format!("{command}\n"))
            .and_then(|()| std::fs::rename(&temporary, &self.command_file));
        if let Err(error) = result {
            let _ = std::fs::remove_file(temporary);
            eprintln!("coranux_panel: impossible d’envoyer la commande au compositeur : {error}");
        }
    }
}

fn discover_plugins() -> Vec<Box<dyn PanelPlugin>> {
    let mut plugins: Vec<Box<dyn PanelPlugin>> = vec![Box::new(ClockPlugin {
        clock: current_time(),
        last_updated: Instant::now(),
    })];

    let audio_backend = if command_available("wpctl") {
        Some(AudioBackend::Wpctl)
    } else if command_available("pactl") {
        Some(AudioBackend::Pactl)
    } else {
        None
    };
    if let Some(backend) = audio_backend {
        plugins.push(Box::new(AudioPlugin {
            backend,
            volume: 0,
            muted: false,
            last_updated: Instant::now() - Duration::from_secs(2),
        }));
    }

    let mut known_plugins = std::collections::HashSet::new();
    for directory in plugin_directories() {
        let entries = match std::fs::read_dir(&directory) {
            Ok(entries) => entries,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
            Err(error) => {
                eprintln!(
                    "coranux_panel: impossible de lire les greffons dans {} : {error}",
                    directory.display()
                );
                continue;
            }
        };

        let mut manifests = entries
            .flatten()
            .map(|entry| entry.path())
            .filter(|path| {
                path.extension()
                    .is_some_and(|extension| extension == "json")
            })
            .collect::<Vec<_>>();
        manifests.sort();
        for path in manifests {
            let contents = match std::fs::read(&path) {
                Ok(contents) => contents,
                Err(error) => {
                    eprintln!(
                        "coranux_panel: impossible de lire le manifeste {} : {error}",
                        path.display()
                    );
                    continue;
                }
            };
            let manifest = match serde_json::from_slice::<PluginManifest>(&contents) {
                Ok(manifest) => manifest,
                Err(error) => {
                    eprintln!(
                        "coranux_panel: manifeste invalide {} : {error}",
                        path.display()
                    );
                    continue;
                }
            };
            let Some((program, _)) = manifest.command.split_first() else {
                eprintln!(
                    "coranux_panel: le greffon {} ne définit aucune commande",
                    path.display()
                );
                continue;
            };
            if manifest.name.trim().is_empty()
                || manifest.label.trim().is_empty()
                || !command_available(program)
            {
                continue;
            }
            if known_plugins.insert(manifest.name.clone()) {
                plugins.push(Box::new(ExternalPlugin { manifest }));
            }
        }
    }

    plugins
}

fn plugin_directories() -> Vec<PathBuf> {
    let mut directories = Vec::new();
    if let Some(config_home) = std::env::var_os("XDG_CONFIG_HOME").map(PathBuf::from) {
        directories.push(config_home.join("coranux/panel-plugins"));
    } else if let Some(home) = std::env::var_os("HOME").map(PathBuf::from) {
        directories.push(home.join(".config/coranux/panel-plugins"));
    }

    if let Some(data_home) = std::env::var_os("XDG_DATA_HOME").map(PathBuf::from) {
        directories.push(data_home.join("coranux/panel-plugins"));
    } else if let Some(home) = std::env::var_os("HOME").map(PathBuf::from) {
        directories.push(home.join(".local/share/coranux/panel-plugins"));
    }

    let data_dirs =
        std::env::var_os("XDG_DATA_DIRS").unwrap_or_else(|| "/usr/local/share:/usr/share".into());
    directories.extend(
        std::env::split_paths(&data_dirs).map(|directory| directory.join("coranux/panel-plugins")),
    );
    directories
}

fn launchers_config_path() -> PathBuf {
    if let Some(config_home) = std::env::var_os("XDG_CONFIG_HOME").map(PathBuf::from) {
        config_home.join("coranux/panel-launchers.json")
    } else if let Some(home) = std::env::var_os("HOME").map(PathBuf::from) {
        home.join(".config/coranux/panel-launchers.json")
    } else {
        PathBuf::from("/nonexistent/coranux-panel-launchers.json")
    }
}

fn read_pinned_launchers(path: &std::path::Path) -> Vec<PinnedLauncher> {
    let contents = match std::fs::read(path) {
        Ok(contents) => contents,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Vec::new(),
        Err(error) => {
            eprintln!(
                "coranux_panel: impossible de lire {} : {error}",
                path.display()
            );
            return Vec::new();
        }
    };
    let config = match serde_json::from_slice::<PinnedLaunchersConfig>(&contents) {
        Ok(config) => config,
        Err(error) => {
            eprintln!(
                "coranux_panel: configuration de raccourcis invalide {} : {error}",
                path.display()
            );
            return Vec::new();
        }
    };

    config
        .launchers
        .into_iter()
        .filter(|launcher| {
            let Some((program, _)) = launcher.command.split_first() else {
                eprintln!(
                    "coranux_panel: raccourci « {} » sans commande ignoré",
                    launcher.label
                );
                return false;
            };
            if launcher.label.trim().is_empty() {
                eprintln!("coranux_panel: raccourci avec un libellé vide ignoré");
                return false;
            }
            if !command_available(program) {
                eprintln!(
                    "coranux_panel: exécutable « {program} » introuvable pour le raccourci « {} »",
                    launcher.label
                );
                return false;
            }
            true
        })
        .collect()
}

fn command_available(command: &str) -> bool {
    let path = PathBuf::from(command);
    if path.components().count() > 1 {
        return path.is_file();
    }
    let Some(search_path) = std::env::var_os("PATH") else {
        return false;
    };
    std::env::split_paths(&search_path).any(|directory| directory.join(command).is_file())
}

fn parse_wpctl_volume(output: &str) -> Option<u8> {
    output
        .split_whitespace()
        .find_map(|field| field.parse::<f32>().ok())
        .map(|volume| (volume * 100.0).round().clamp(0.0, 150.0) as u8)
}

fn parse_pactl_volume(output: &str) -> Option<u8> {
    output.split('/').find_map(|field| {
        field
            .trim()
            .strip_suffix('%')
            .and_then(|value| value.parse::<u8>().ok())
    })
}

impl eframe::App for CoranuxPanel {
    fn update(&mut self, context: &egui::Context, _frame: &mut eframe::Frame) {
        self.refresh_snapshot();
        self.refresh_plugins();
        context.request_repaint_after(SNAPSHOT_POLL_INTERVAL);

        egui::CentralPanel::default()
            .frame(
                egui::Frame::new()
                    .fill(Color32::from_rgb(13, 18, 29))
                    .stroke(Stroke::new(1.0_f32, Color32::from_rgb(39, 53, 73)))
                    .corner_radius(egui::CornerRadius::same(10))
                    .inner_margin(egui::Margin::symmetric(10, 5)),
            )
            .show(context, |ui| {
                ui.set_height(PANEL_HEIGHT - 10.0);
                ui.with_layout(Layout::left_to_right(Align::Center), |ui| {
                    egui::Frame::new()
                        .fill(Color32::from_rgb(28, 49, 68))
                        .corner_radius(egui::CornerRadius::same(7))
                        .inner_margin(egui::Margin::symmetric(9, 4))
                        .show(ui, |ui| {
                            ui.label(
                                RichText::new("C")
                                    .size(14.0)
                                    .strong()
                                    .color(Color32::from_rgb(116, 218, 245)),
                            );
                        });
                    ui.add_space(4.0);
                    ui.label(
                        RichText::new("Coranux")
                            .size(14.0)
                            .strong()
                            .color(Color32::from_rgb(221, 233, 247)),
                    );
                    for launcher in &self.launchers {
                        let Some((program, arguments)) = launcher.command.split_first() else {
                            continue;
                        };
                        if ui
                            .add_sized(
                                [92.0, 34.0],
                                egui::Button::new(
                                    RichText::new(&launcher.label)
                                        .size(12.0)
                                        .color(Color32::from_rgb(220, 232, 246)),
                                )
                                .fill(Color32::from_rgb(24, 33, 49))
                                .stroke(Stroke::new(1.0_f32, Color32::from_rgb(43, 57, 76))),
                            )
                            .on_hover_text(format!("Lancer {}", launcher.label))
                            .clicked()
                        {
                            if let Err(error) = Command::new(program).args(arguments).spawn() {
                                eprintln!(
                                    "coranux_panel: impossible de lancer {} : {error}",
                                    launcher.label
                                );
                            }
                        }
                    }
                    ui.separator();

                    ScrollArea::horizontal()
                        .id_salt("panel-windows")
                        .auto_shrink([false, true])
                        .show(ui, |ui| {
                            ui.horizontal(|ui| {
                                let windows = self.snapshot.windows.clone();
                                for window in windows {
                                    let color = if window.minimized {
                                        Color32::from_rgb(150, 164, 183)
                                    } else if window.focused {
                                        Color32::from_rgb(142, 221, 244)
                                    } else {
                                        Color32::from_rgb(231, 237, 246)
                                    };
                                    let fill = if window.focused && !window.minimized {
                                        Color32::from_rgb(38, 61, 81)
                                    } else {
                                        Color32::from_rgb(24, 33, 49)
                                    };
                                    let button = ui
                                        .add_sized(
                                            [148.0, 34.0],
                                            egui::Button::new(
                                                RichText::new(&window.title)
                                                    .size(12.5)
                                                    .color(color),
                                            )
                                            .fill(fill)
                                            .stroke(
                                                Stroke::new(1.0_f32, Color32::from_rgb(43, 57, 76)),
                                            ),
                                        )
                                        .on_hover_text(
                                            window.app_id.as_deref().unwrap_or("Fenêtre ouverte"),
                                        );
                                    if button.clicked() {
                                        self.activate_window(window.id);
                                    }
                                }
                            });
                        });

                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        SessionPlugin.show(ui, context);
                        let layout_label = if self.snapshot.keyboard_layout.is_empty() {
                            "Clavier".to_owned()
                        } else {
                            self.snapshot.keyboard_layout.to_uppercase()
                        };
                        if ui
                            .small_button(layout_label)
                            .on_hover_text("Changer la disposition du clavier")
                            .clicked()
                        {
                            self.cycle_keyboard_layout();
                        }
                        for plugin in &mut self.plugins {
                            plugin.show(ui, context);
                        }
                    });
                });
            });
    }
}

fn current_time() -> String {
    match Command::new("date").arg("+%H:%M").output() {
        Ok(output) if output.status.success() => {
            String::from_utf8_lossy(&output.stdout).trim().to_owned()
        }
        Ok(output) => {
            eprintln!(
                "coranux_panel: date a quitté avec le statut {}",
                output.status
            );
            "--:--".to_owned()
        }
        Err(error) => {
            eprintln!("coranux_panel: impossible de lire l’heure : {error}");
            "--:--".to_owned()
        }
    }
}

fn request_session_shutdown() {
    let Some(pid) = std::env::var("CORANUX_SESSION_PID")
        .ok()
        .and_then(|pid| pid.parse::<libc::pid_t>().ok())
    else {
        return;
    };
    if pid <= 1 {
        eprintln!("coranux_panel: identifiant de session invalide ({pid})");
        return;
    }
    if unsafe { libc::kill(pid, libc::SIGTERM) } != 0 {
        eprintln!(
            "coranux_panel: impossible d’arrêter la session {pid} : {}",
            std::io::Error::last_os_error()
        );
    }
}

fn main() -> eframe::Result {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("Coranux Panel")
            .with_app_id("coranux_panel")
            .with_decorations(false)
            .with_fullscreen(true)
            .with_inner_size([1280.0, PANEL_HEIGHT])
            .with_min_inner_size([480.0, PANEL_HEIGHT]),
        ..Default::default()
    };
    eframe::run_native(
        "Coranux Panel",
        options,
        Box::new(|_| Ok(Box::new(CoranuxPanel::new()))),
    )
}

#[cfg(test)]
mod tests {
    use super::{
        PanelSnapshot, PinnedLaunchersConfig, PluginManifest, parse_pactl_volume,
        parse_wpctl_volume,
    };

    #[test]
    fn deserializes_open_and_minimized_windows() {
        let snapshot: PanelSnapshot = serde_json::from_str(
            r#"{"windows":[{"id":4,"title":"Terminal","app_id":"terminal","minimized":false,"focused":true},{"id":9,"title":"Files","app_id":null,"minimized":true,"focused":false}]}"#,
        )
        .unwrap();

        assert_eq!(snapshot.windows.len(), 2);
        assert!(snapshot.windows[1].minimized);
        assert!(!snapshot.windows[1].focused);
    }

    #[test]
    fn parses_supported_external_plugin_manifest() {
        let plugin: PluginManifest = serde_json::from_str(
            r#"{"name":"calendar","label":"Calendar","command":["calendar-app","--panel"]}"#,
        )
        .unwrap();

        assert_eq!(plugin.name, "calendar");
        assert_eq!(plugin.command, ["calendar-app", "--panel"]);
    }

    #[test]
    fn parses_pinned_application_launchers() {
        let config: PinnedLaunchersConfig = serde_json::from_str(
            r#"{"launchers":[{"label":"Navigateur","command":["firefox","--new-window"]}]}"#,
        )
        .unwrap();

        assert_eq!(config.launchers.len(), 1);
        assert_eq!(config.launchers[0].label, "Navigateur");
        assert_eq!(config.launchers[0].command, ["firefox", "--new-window"]);
    }

    #[test]
    fn parses_pipewire_and_pulseaudio_volume_output() {
        assert_eq!(parse_wpctl_volume("Volume: 0.47 [MUTED]"), Some(47));
        assert_eq!(
            parse_pactl_volume("Volume: front-left: 65536 / 82% / 0.00 dB"),
            Some(82)
        );
        assert_eq!(parse_wpctl_volume("No audio device"), None);
    }
}
