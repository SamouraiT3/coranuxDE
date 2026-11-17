use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use eframe::egui::{self, Align2, Area, Color32, FontId, Pos2, Sense, Stroke, Vec2};
use serde::{Deserialize, Serialize};

const APPLICATIONS_PAGE_SIZE: usize = 20;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ViewMode {
    Bureau,
    Applications,
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
enum DockPosition {
    Top,
    #[default]
    Bottom,
    Left,
    Right,
}

#[derive(Clone)]
enum ContextTarget {
    Empty,
    Core,
    Element(PathBuf),
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum TransferOperation {
    Move,
    Copy,
}

#[derive(Deserialize, Serialize)]
struct SavedPosition {
    x: f32,
    y: f32,
}

#[derive(Deserialize, Serialize, Default)]
struct SavedState {
    positions: HashMap<String, SavedPosition>,
    expanded: Vec<String>,
    #[serde(default)]
    camera_x: f32,
    #[serde(default)]
    camera_y: f32,
    #[serde(default = "default_camera_zoom")]
    camera_zoom: f32,
    #[serde(default)]
    icon_theme: Option<String>,
    #[serde(default)]
    favorites: Vec<String>,
    #[serde(default)]
    dock_position: DockPosition,
    #[serde(default = "default_dock_icon_size")]
    dock_icon_size: f32,
    #[serde(default)]
    dock_auto_hide: bool,
}

struct BureauApp {
    bureau_directory: PathBuf,
    positions_file: PathBuf,
    mode: ViewMode,
    message: String,
    positions: HashMap<PathBuf, Vec2>,
    expanded: HashSet<PathBuf>,
    expansion: HashMap<PathBuf, f32>,
    camera_offset: Vec2,
    camera_target: Vec2,
    camera_zoom: f32,
    camera_zoom_target: f32,
    background_press_started: Option<f64>,
    background_press_position: Option<Pos2>,
    long_press_handled: bool,
    debug_mode: bool,
    debug_last_event: String,
    debug_pointer_empty: bool,
    debug_camera_animating: bool,
    context_menu: Option<(ContextTarget, Pos2)>,
    context_input: String,
    context_action: Option<bool>,
    creation_directory: Option<PathBuf>,
    pending_transfer: Option<(PathBuf, TransferOperation)>,
    rename_target: Option<PathBuf>,
    icon_cache: HashMap<(PathBuf, bool), Option<PathBuf>>,
    label_cache: HashMap<PathBuf, String>,
    icon_theme: Option<String>,
    available_icon_themes: Vec<String>,
    applications: Vec<PathBuf>,
    favorites: HashSet<PathBuf>,
    dock_position: DockPosition,
    dock_icon_size: f32,
    dock_auto_hide: bool,
    applications_search: String,
    applications_category: String,
    applications_favorites_only: bool,
    applications_page: usize,
    mode_transition_progress: f32,
    transition_old_mode: Option<ViewMode>,
    transition_old_entries: Vec<(PathBuf, Option<PathBuf>)>,
}

impl BureauApp {
    fn new() -> Self {
        let home = std::env::var_os("HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("."));
        let config_home = std::env::var_os("XDG_CONFIG_HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|| home.join(".config"));
        let bureau_directory = Self::desktop_directory(&home, &config_home);
        let positions_file = config_home.join("coranux_desktop").join("positions.json");
        if !positions_file.exists() {
            for legacy_directory in ["coranux_de", "noyau-bureau"] {
                let legacy_positions_file =
                    config_home.join(legacy_directory).join("positions.json");
                if legacy_positions_file.is_file() {
                    if let Some(parent) = positions_file.parent() {
                        let _ = fs::create_dir_all(parent);
                    }
                    if fs::copy(&legacy_positions_file, &positions_file).is_ok() {
                        break;
                    }
                }
            }
        }
        let (
            positions,
            expanded,
            camera_offset,
            camera_zoom,
            saved_icon_theme,
            favorites,
            dock_position,
            dock_icon_size,
            dock_auto_hide,
        ) = Self::load_state(&positions_file);
        let icon_theme = saved_icon_theme.or_else(detect_icon_theme);
        let available_icon_themes = list_icon_themes();
        let expansion = expanded.iter().map(|path| (path.clone(), 1.0)).collect();

        Self {
            bureau_directory,
            positions_file,
            mode: ViewMode::Bureau,
            message: String::from("Bureau Coranux"),
            positions,
            expanded,
            expansion,
            camera_offset,
            camera_target: camera_offset,
            camera_zoom,
            camera_zoom_target: camera_zoom,
            background_press_started: None,
            background_press_position: None,
            long_press_handled: false,
            debug_mode: false,
            debug_last_event: String::from("aucun"),
            debug_pointer_empty: false,
            debug_camera_animating: false,
            context_menu: None,
            context_input: String::new(),
            context_action: None,
            creation_directory: None,
            pending_transfer: None,
            rename_target: None,
            icon_cache: HashMap::new(),
            label_cache: HashMap::new(),
            icon_theme,
            available_icon_themes,
            applications: list_applications(),
            favorites: favorites.into_iter().collect(),
            dock_position,
            dock_icon_size,
            dock_auto_hide,
            applications_search: String::new(),
            applications_category: String::from("Toutes"),
            applications_favorites_only: false,
            applications_page: 0,
            mode_transition_progress: 1.0,
            transition_old_mode: None,
            transition_old_entries: Vec::new(),
        }
    }

    fn desktop_directory(home: &Path, config_home: &Path) -> PathBuf {
        if let Ok(output) = Command::new("xdg-user-dir").arg("DESKTOP").output() {
            if output.status.success() {
                let path = PathBuf::from(String::from_utf8_lossy(&output.stdout).trim());
                if !path.as_os_str().is_empty() {
                    return path;
                }
            }
        }

        let user_dirs_file = config_home.join("user-dirs.dirs");
        if let Ok(contents) = fs::read_to_string(user_dirs_file) {
            if let Some(value) = contents
                .lines()
                .find_map(|line| line.strip_prefix("XDG_DESKTOP_DIR="))
                .map(str::trim)
                .map(|value| value.trim_matches('"'))
            {
                let home_text = home.to_string_lossy();
                let expanded = value
                    .replace("${HOME}", &home_text)
                    .replace("$HOME", &home_text);
                return PathBuf::from(expanded);
            }
        }

        home.to_path_buf()
    }

    fn load_state(
        path: &Path,
    ) -> (
        HashMap<PathBuf, Vec2>,
        HashSet<PathBuf>,
        Vec2,
        f32,
        Option<String>,
        Vec<PathBuf>,
        DockPosition,
        f32,
        bool,
    ) {
        let contents = match fs::read_to_string(path) {
            Ok(contents) => contents,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return (
                    HashMap::new(),
                    HashSet::new(),
                    Vec2::ZERO,
                    default_camera_zoom(),
                    None,
                    Vec::new(),
                    DockPosition::default(),
                    default_dock_icon_size(),
                    false,
                );
            }
            Err(error) => {
                eprintln!(
                    "coranux_desktop: impossible de lire {} : {error}",
                    path.display()
                );
                return (
                    HashMap::new(),
                    HashSet::new(),
                    Vec2::ZERO,
                    default_camera_zoom(),
                    None,
                    Vec::new(),
                    DockPosition::default(),
                    default_dock_icon_size(),
                    false,
                );
            }
        };
        let state = match serde_json::from_str::<SavedState>(&contents) {
            Ok(state) => state,
            Err(error) => {
                eprintln!(
                    "coranux_desktop: état invalide dans {} : {error}",
                    path.display()
                );
                let timestamp = SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .map_or(0, |duration| duration.as_nanos());
                let backup_path =
                    path.with_extension(format!("json.corrupt-{}-{timestamp}", std::process::id()));
                match fs::copy(path, &backup_path) {
                    Ok(_) => eprintln!(
                        "coranux_desktop: copie de récupération créée dans {}",
                        backup_path.display()
                    ),
                    Err(error) => eprintln!(
                        "coranux_desktop: impossible de sauvegarder l’état invalide : {error}"
                    ),
                }
                return (
                    HashMap::new(),
                    HashSet::new(),
                    Vec2::ZERO,
                    default_camera_zoom(),
                    None,
                    Vec::new(),
                    DockPosition::default(),
                    default_dock_icon_size(),
                    false,
                );
            }
        };
        let camera_offset = Vec2::new(state.camera_x, state.camera_y);
        let positions = state
            .positions
            .into_iter()
            .map(|(path, position)| (PathBuf::from(path), Vec2::new(position.x, position.y)))
            .collect();
        let expanded = state.expanded.into_iter().map(PathBuf::from).collect();
        let favorites = state.favorites.into_iter().map(PathBuf::from).collect();
        (
            positions,
            expanded,
            camera_offset,
            state.camera_zoom,
            state.icon_theme,
            favorites,
            state.dock_position,
            state.dock_icon_size,
            state.dock_auto_hide,
        )
    }

    fn save_state(&self) {
        let positions = self
            .positions
            .iter()
            .map(|(path, position)| {
                (
                    path.to_string_lossy().into_owned(),
                    SavedPosition {
                        x: position.x,
                        y: position.y,
                    },
                )
            })
            .collect();
        let state = SavedState {
            positions,
            expanded: self
                .expanded
                .iter()
                .map(|path| path.to_string_lossy().into_owned())
                .collect(),
            camera_x: self.camera_offset.x,
            camera_y: self.camera_offset.y,
            camera_zoom: self.camera_zoom,
            icon_theme: self.icon_theme.clone(),
            favorites: self
                .favorites
                .iter()
                .map(|path| path.to_string_lossy().into_owned())
                .collect(),
            dock_position: self.dock_position,
            dock_icon_size: self.dock_icon_size,
            dock_auto_hide: self.dock_auto_hide,
        };
        let contents = match serde_json::to_string_pretty(&state) {
            Ok(contents) => contents,
            Err(error) => {
                eprintln!("coranux_desktop: impossible de sérialiser l’état : {error}");
                return;
            }
        };
        if let Some(parent) = self.positions_file.parent() {
            if let Err(error) = fs::create_dir_all(parent) {
                eprintln!(
                    "coranux_desktop: impossible de créer {} : {error}",
                    parent.display()
                );
                return;
            }
        }
        let temporary_file = self
            .positions_file
            .with_extension(format!("json.tmp-{}", std::process::id()));
        if let Err(error) = fs::write(&temporary_file, contents)
            .and_then(|()| fs::rename(&temporary_file, &self.positions_file))
        {
            let _ = fs::remove_file(&temporary_file);
            eprintln!(
                "coranux_desktop: impossible d’enregistrer {} : {error}",
                self.positions_file.display()
            );
        }
    }

    fn children(path: &Path) -> Vec<PathBuf> {
        let mut entries = fs::read_dir(path)
            .into_iter()
            .flatten()
            .flatten()
            .map(|entry| entry.path())
            .filter(|path| {
                !path
                    .file_name()
                    .is_some_and(|name| name.to_string_lossy().starts_with('.'))
            })
            .collect::<Vec<_>>();
        entries.sort_by_key(|path| {
            (
                !path.is_dir(),
                path.file_name().map(|name| name.to_os_string()),
            )
        });
        entries
    }

    fn visible_entries(&self) -> Vec<(PathBuf, Option<PathBuf>)> {
        if self.mode == ViewMode::Applications {
            let matching = self.filtered_applications();
            let start = self
                .applications_page
                .saturating_mul(APPLICATIONS_PAGE_SIZE);
            return matching
                .into_iter()
                .skip(start)
                .take(APPLICATIONS_PAGE_SIZE)
                .map(|path| (path, None))
                .collect();
        }
        let mut visible = Self::children(&self.bureau_directory)
            .into_iter()
            .map(|path| (path, None))
            .collect::<Vec<_>>();
        let mut index = 0;
        while index < visible.len() {
            let path = visible[index].0.clone();
            let is_animating = self.expansion.get(&path).is_some_and(|progress| {
                let target = if self.expanded.contains(&path) {
                    1.0
                } else {
                    0.0
                };
                (target - progress).abs() > 0.001 || *progress > 0.001
            });
            if (self.expanded.contains(&path) || is_animating) && path.is_dir() {
                visible.extend(
                    Self::children(&path)
                        .into_iter()
                        .map(|child| (child, Some(path.clone()))),
                );
            }
            index += 1;
        }
        visible
    }

    fn filtered_applications(&self) -> Vec<PathBuf> {
        let query = self.applications_search.trim().to_lowercase();
        let mut apps = self
            .applications
            .iter()
            .filter(|path| {
                (!self.applications_favorites_only || self.favorites.contains(*path))
                    && (self.applications_category == "Toutes"
                        || desktop_category(path) == self.applications_category)
                    && (query.is_empty() || display_name(path).to_lowercase().contains(&query))
            })
            .cloned()
            .collect::<Vec<_>>();
        apps.sort_by_key(|path| display_name(path).to_lowercase());
        apps
    }

    fn layout_entries(
        &mut self,
        mode: ViewMode,
        entries: &[(PathBuf, Option<PathBuf>)],
        center: Pos2,
    ) -> Vec<(PathBuf, Option<PathBuf>, Pos2)> {
        let mut nodes = Vec::with_capacity(entries.len());
        for (index, (path, parent)) in entries.iter().enumerate() {
            if mode == ViewMode::Applications {
                let angle = (index as f32 / entries.len().max(1) as f32) * std::f32::consts::TAU
                    - std::f32::consts::FRAC_PI_2;
                nodes.push((
                    path.clone(),
                    None,
                    center + Vec2::angled(angle) * (250.0 * self.camera_zoom),
                ));
                continue;
            }
            let sibling_index = parent
                .as_deref()
                .map_or(index, |parent| Self::child_index(entries, path, parent));
            let sibling_total = parent.as_deref().map_or(
                entries
                    .iter()
                    .filter(|(_, candidate)| candidate.is_none())
                    .count(),
                |parent| Self::child_total(entries, parent),
            );
            let base = self.position_for(
                path,
                parent.as_deref(),
                sibling_index,
                sibling_total,
                center,
            );
            let target = center + (base - center) * self.camera_zoom;
            let position = if let Some(parent) = parent {
                let parent_position = nodes
                    .iter()
                    .find(|(candidate, _, _)| candidate == parent)
                    .map(|(_, _, position)| *position)
                    .unwrap_or(center);
                parent_position + (target - parent_position) * self.ancestor_progress(parent)
            } else {
                target
            };
            nodes.push((path.clone(), parent.clone(), position));
        }
        if mode == ViewMode::Bureau {
            let parent_positions = nodes
                .iter()
                .map(|(path, _, position)| (path.clone(), *position))
                .collect::<HashMap<_, _>>();
            for (path, parent, position) in &mut nodes {
                if let Some(offset) = self.positions.get(path) {
                    let target = center + *offset * self.camera_zoom;
                    *position = if let Some(parent) = parent {
                        let parent_position =
                            parent_positions.get(parent).copied().unwrap_or(center);
                        parent_position
                            + (target - parent_position) * self.ancestor_progress(parent)
                    } else {
                        target
                    };
                }
            }
        }
        nodes
    }

    fn step_expansions(&mut self, delta_time: f32) -> bool {
        let paths = self.expansion.keys().cloned().collect::<Vec<_>>();
        let mut active = false;
        for path in paths {
            let target = if self.expanded.contains(&path) {
                1.0
            } else {
                0.0
            };
            let progress = self.expansion.entry(path.clone()).or_insert(target);
            let difference = target - *progress;
            if difference.abs() > 0.001 {
                let smoothing = 1.0 - (-delta_time * 9.0).exp();
                *progress += difference * smoothing;
                active = true;
            } else {
                *progress = target;
            }
            if *progress <= 0.0 && !self.expanded.contains(&path) {
                self.expansion.remove(&path);
            }
        }
        active
    }

    fn step_camera(&mut self, delta_time: f32) -> bool {
        let difference = self.camera_target - self.camera_offset;
        let zoom_difference = self.camera_zoom_target - self.camera_zoom;
        if difference.length_sq() < 0.25 && zoom_difference.abs() < 0.002 {
            let changed = difference.length_sq() > 0.0 || zoom_difference.abs() > 0.0;
            self.camera_offset = self.camera_target;
            self.camera_zoom = self.camera_zoom_target;
            if changed {
                self.save_state();
            }
            return false;
        }
        let frame_time = delta_time.max(1.0 / 120.0);
        self.camera_offset += difference * (1.0 - (-frame_time * 10.0).exp());
        self.camera_zoom += zoom_difference * (1.0 - (-frame_time * 10.0).exp());
        true
    }

    fn reset_camera(&mut self) {
        self.camera_target = Vec2::ZERO;
        self.camera_zoom_target = default_camera_zoom();
        self.message = String::from("Noyau recentré");
        self.debug_event("recentrage par clic molette");
    }

    fn switch_mode(&mut self) {
        self.transition_old_mode = Some(self.mode);
        self.transition_old_entries = self.visible_entries();
        self.mode = if self.mode == ViewMode::Bureau {
            ViewMode::Applications
        } else {
            ViewMode::Bureau
        };
        self.applications_page = 0;
        self.mode_transition_progress = 0.0;
        self.message = if self.mode == ViewMode::Bureau {
            "Bureau Coranux".into()
        } else {
            "Mode applications".into()
        };
    }

    fn step_mode_transition(&mut self, delta_time: f32) -> bool {
        if self.mode_transition_progress >= 1.0 {
            return false;
        }
        self.mode_transition_progress +=
            (1.0 - self.mode_transition_progress) * (1.0 - (-delta_time * 7.0).exp());
        if self.mode_transition_progress >= 0.998 {
            self.mode_transition_progress = 1.0;
            self.transition_old_mode = None;
            self.transition_old_entries.clear();
            false
        } else {
            true
        }
    }

    fn close_context_menu(&mut self) {
        self.context_menu = None;
        self.context_action = None;
        self.creation_directory = None;
        self.rename_target = None;
        self.context_input.clear();
    }

    fn create_entry(&mut self, folder: bool) {
        let name = self.context_input.trim();
        if name.is_empty() {
            return;
        }
        let directory = self
            .creation_directory
            .as_deref()
            .unwrap_or(&self.bureau_directory);
        let path = directory.join(name);
        let result = if folder {
            fs::create_dir(&path)
        } else {
            fs::File::create(&path).map(|_| ())
        };
        match result {
            Ok(()) => {
                if folder {
                    self.expanded.insert(path.clone());
                    self.expansion.insert(path.clone(), 0.0);
                }
                self.message = format!("Créé : {}", path.display());
                self.save_state();
            }
            Err(error) => self.message = format!("Création impossible : {}", error),
        }
        self.close_context_menu();
    }

    fn transfer_entry(
        &mut self,
        source: &Path,
        destination_directory: &Path,
        operation: TransferOperation,
    ) {
        let Some(name) = source.file_name() else {
            self.message = String::from("Élément source invalide");
            self.pending_transfer = None;
            return;
        };
        let destination = destination_directory.join(name);
        if destination == source {
            self.message = String::from("La source est déjà dans ce dossier");
            self.pending_transfer = None;
            return;
        }
        if source.is_dir() && destination.starts_with(source) {
            self.message =
                String::from("Impossible de copier ou déplacer un dossier dans lui-même");
            self.pending_transfer = None;
            return;
        }

        let result = match operation {
            TransferOperation::Move => fs::rename(source, &destination),
            TransferOperation::Copy => copy_path(source, &destination),
        };
        match result {
            Ok(()) => {
                if operation == TransferOperation::Move {
                    self.remap_subtree_paths(source, &destination);
                }
                self.message = format!(
                    "{} : {} → {}",
                    match operation {
                        TransferOperation::Move => "Déplacé",
                        TransferOperation::Copy => "Copié",
                    },
                    source.display(),
                    destination.display()
                );
                self.save_state();
            }
            Err(error) => self.message = format!("Opération impossible : {}", error),
        }
        self.pending_transfer = None;
    }

    fn remap_subtree_paths(&mut self, old_root: &Path, new_root: &Path) {
        fn remap(path: &Path, old_root: &Path, new_root: &Path) -> Option<PathBuf> {
            Some(new_root.join(path.strip_prefix(old_root).ok()?))
        }

        let positions = std::mem::take(&mut self.positions);
        self.positions = positions
            .into_iter()
            .map(|(path, position)| (remap(&path, old_root, new_root).unwrap_or(path), position))
            .collect();
        self.expanded = std::mem::take(&mut self.expanded)
            .into_iter()
            .map(|path| remap(&path, old_root, new_root).unwrap_or(path))
            .collect();
        self.expansion = std::mem::take(&mut self.expansion)
            .into_iter()
            .map(|(path, progress)| (remap(&path, old_root, new_root).unwrap_or(path), progress))
            .collect();
        self.icon_cache = std::mem::take(&mut self.icon_cache)
            .into_iter()
            .map(|((path, expanded), icon)| {
                (
                    (remap(&path, old_root, new_root).unwrap_or(path), expanded),
                    icon,
                )
            })
            .collect();
        self.label_cache = std::mem::take(&mut self.label_cache)
            .into_iter()
            .map(|(path, label)| (remap(&path, old_root, new_root).unwrap_or(path), label))
            .collect();
    }

    fn destination_for_target(&self, target: &ContextTarget) -> PathBuf {
        match target {
            ContextTarget::Empty | ContextTarget::Core => self.bureau_directory.clone(),
            ContextTarget::Element(path) if path.is_dir() => path.clone(),
            ContextTarget::Element(path) => path
                .parent()
                .unwrap_or(&self.bureau_directory)
                .to_path_buf(),
        }
    }

    fn delete_entry(&mut self, path: &Path) {
        let result = if path.is_dir() {
            fs::remove_dir_all(path)
        } else {
            fs::remove_file(path)
        };
        match result {
            Ok(()) => {
                self.positions.remove(path);
                self.icon_cache
                    .retain(|(cached_path, _), _| cached_path != path);
                self.label_cache.remove(path);
                self.expanded.remove(path);
                self.expansion.remove(path);
                self.message = format!("Supprimé : {}", display_name(path));
                self.save_state();
            }
            Err(error) => self.message = format!("Suppression impossible : {}", error),
        }
        self.close_context_menu();
    }

    fn rename_entry(&mut self, old_path: &Path) {
        let name = self.context_input.trim();
        if name.is_empty() {
            return;
        }
        let new_path = old_path
            .parent()
            .unwrap_or(&self.bureau_directory)
            .join(name);
        match fs::rename(old_path, &new_path) {
            Ok(()) => {
                if let Some(position) = self.positions.remove(old_path) {
                    self.positions.insert(new_path.clone(), position);
                }
                let cached_keys: Vec<_> = self
                    .icon_cache
                    .keys()
                    .filter(|(cached_path, _)| cached_path == old_path)
                    .cloned()
                    .collect();
                for (cached_path, expanded) in cached_keys {
                    if let Some(icon_path) = self.icon_cache.remove(&(cached_path, expanded)) {
                        self.icon_cache
                            .insert((new_path.clone(), expanded), icon_path);
                    }
                }
                if let Some(label) = self.label_cache.remove(old_path) {
                    self.label_cache.insert(new_path.clone(), label);
                }
                if self.expanded.remove(old_path) {
                    self.expanded.insert(new_path.clone());
                }
                if let Some(progress) = self.expansion.remove(old_path) {
                    self.expansion.insert(new_path.clone(), progress);
                }
                self.message = format!("Renommé : {}", display_name(&new_path));
                self.save_state();
            }
            Err(error) => self.message = format!("Renommage impossible : {}", error),
        }
        self.close_context_menu();
    }

    fn position_for(
        &mut self,
        path: &Path,
        parent: Option<&Path>,
        index: usize,
        total: usize,
        center: Pos2,
    ) -> Pos2 {
        if let Some(offset) = self.positions.get(path) {
            return center + *offset;
        }
        let angle = (index as f32 / total.max(1) as f32) * std::f32::consts::TAU
            - std::f32::consts::FRAC_PI_2;
        let offset = if let Some(parent) = parent {
            let parent_position = self.position_for(parent, None, 0, 1, center);
            parent_position - center + Vec2::angled(angle) * 128.0
        } else {
            Vec2::angled(angle) * 250.0
        };
        self.positions.insert(path.to_path_buf(), offset);
        center + offset
    }

    fn toggle_directory(&mut self, path: &Path) {
        if !self.expanded.remove(path) {
            self.expanded.insert(path.to_path_buf());
            self.expansion.entry(path.to_path_buf()).or_insert(0.0);
            self.message = format!("Déploiement : {}", display_name(path));
        } else {
            self.message = format!("Repli : {}", display_name(path));
        }
        self.save_state();
    }

    fn collapse_all(&mut self) {
        for path in self.expanded.iter().cloned().collect::<Vec<_>>() {
            self.expansion.entry(path).or_insert(1.0);
        }
        self.expanded.clear();
        self.message = String::from("Branches repliées");
        self.save_state();
    }

    fn open_file(&mut self, path: &Path) {
        if Command::new("xdg-open").arg(path).spawn().is_err() {
            self.message = format!("Impossible d'ouvrir {}", display_name(path));
        } else {
            self.message = format!("Ouverture : {}", display_name(path));
        }
    }

    fn launch_application(&mut self, path: &Path) {
        let launched = Command::new("gio").arg("launch").arg(path).spawn().is_ok()
            || desktop_id(path)
                .is_some_and(|id| Command::new("gtk-launch").arg(id).spawn().is_ok());
        self.message = if launched {
            format!("Lancement : {}", display_name(path))
        } else {
            format!(
                "Impossible de lancer {} (gio ou gtk-launch requis)",
                display_name(path)
            )
        };
    }

    fn toggle_favorite(&mut self, path: &Path) {
        if !self.favorites.remove(path) {
            self.favorites.insert(path.to_path_buf());
            self.message = format!("Ajouté aux favoris : {}", display_name(path));
        } else {
            self.message = format!("Retiré des favoris : {}", display_name(path));
        }
        self.save_state();
    }

    fn resolve_collisions(
        &mut self,
        moved_path: &Path,
        visible: &[(PathBuf, Option<PathBuf>)],
        center: Pos2,
    ) {
        let Some(offset) = self.positions.get(moved_path).copied() else {
            return;
        };
        let mut moved = center + offset;
        let core_distance = 82.0 + 30.0 + 18.0;
        let from_core = moved - center;
        if from_core.length() < core_distance {
            let direction = if from_core.length() > 0.1 {
                from_core.normalized()
            } else {
                Vec2::new(1.0, 0.0)
            };
            let corrected = center + direction * core_distance;
            moved += (corrected - moved) * 0.38;
        }
        for (other_path, _) in visible {
            if other_path == moved_path {
                continue;
            }
            let Some(other_offset) = self.positions.get(other_path).copied() else {
                continue;
            };
            let other = center + other_offset;
            let difference = moved - other;
            let distance = difference.length();
            let minimum = 30.0 * 2.0 + 16.0;
            if distance < minimum {
                let direction = if distance > 0.1 {
                    difference / distance
                } else {
                    Vec2::new(1.0, 0.0)
                };
                let corrected = other + direction * minimum;
                moved += (corrected - moved) * 0.38;
            }
        }
        self.positions
            .insert(moved_path.to_path_buf(), moved - center);
    }

    fn child_total(visible: &[(PathBuf, Option<PathBuf>)], parent: &Path) -> usize {
        visible
            .iter()
            .filter(|(_, candidate)| candidate.as_deref() == Some(parent))
            .count()
    }

    fn child_index(visible: &[(PathBuf, Option<PathBuf>)], path: &Path, parent: &Path) -> usize {
        visible
            .iter()
            .filter(|(candidate, candidate_parent)| {
                candidate_parent.as_deref() == Some(parent) && candidate <= path
            })
            .count()
            .saturating_sub(1)
    }

    fn expansion_progress(&self, path: &Path) -> f32 {
        self.expansion.get(path).copied().unwrap_or_else(|| {
            if self.expanded.contains(path) {
                1.0
            } else {
                0.0
            }
        })
    }

    fn ancestor_progress(&self, path: &Path) -> f32 {
        let mut progress = 1.0;
        let mut ancestor = Some(path);
        while let Some(current) = ancestor {
            if current == self.bureau_directory {
                break;
            }
            progress *= self.expansion_progress(current);
            ancestor = current.parent();
        }
        progress
    }

    fn node_color(is_directory: bool, is_expanded: bool) -> Color32 {
        match (is_directory, is_expanded) {
            (true, true) => Color32::from_rgba_unmultiplied(242, 151, 97, 72),
            (true, false) => Color32::from_rgba_unmultiplied(72, 190, 177, 58),
            (false, _) => Color32::from_rgba_unmultiplied(85, 143, 190, 48),
        }
    }

    fn icon_path(&mut self, path: &Path, is_expanded: bool) -> Option<PathBuf> {
        let cache_key = (path.to_path_buf(), is_expanded);
        if let Some(icon_path) = self.icon_cache.get(&cache_key) {
            return icon_path.clone();
        }
        let icon_names = if path.is_dir() {
            if is_expanded {
                vec![
                    String::from("folder-open"),
                    String::from("folder-open-symbolic"),
                    String::from("folder"),
                ]
            } else {
                vec![String::from("folder"), String::from("folder-symbolic")]
            }
        } else if path
            .extension()
            .is_some_and(|extension| extension == "desktop")
        {
            vec![
                desktop_icon_name(path).unwrap_or_else(|| String::from("application-x-executable")),
                String::from("application-x-executable"),
            ]
        } else {
            mime_icon_names(path)
        };

        let resolved = icon_names.into_iter().find_map(|icon_name| {
            let icon_path = PathBuf::from(&icon_name);
            if icon_path.is_absolute() && icon_path.is_file() {
                return Some(icon_path);
            }
            let lookup = freedesktop_icons::lookup(&icon_name)
                .with_size(48)
                .with_cache();
            if let Some(theme) = &self.icon_theme {
                lookup.with_theme(theme).find()
            } else {
                lookup.find()
            }
        });
        self.icon_cache.insert(cache_key, resolved.clone());
        resolved
    }

    fn label_for(&mut self, path: &Path) -> String {
        if let Some(label) = self.label_cache.get(path) {
            return label.clone();
        }
        let label = display_name(path);
        self.label_cache.insert(path.to_path_buf(), label.clone());
        label
    }

    fn debug_event(&mut self, event: impl Into<String>) {
        self.debug_last_event = event.into();
    }

    fn connected_segment(
        start: Pos2,
        end: Pos2,
        start_radius: f32,
        end_radius: f32,
    ) -> Option<(Pos2, Pos2)> {
        let difference = end - start;
        let length = difference.length();
        if length <= start_radius + end_radius {
            return None;
        }
        let direction = difference / length;
        Some((
            start + direction * start_radius,
            end - direction * end_radius,
        ))
    }
}

impl eframe::App for BureauApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        if ctx.input(|input| input.key_pressed(egui::Key::F3)) {
            self.debug_mode = !self.debug_mode;
        }
        let screen = ctx.available_rect();
        let center = screen.center();
        let delta_time = ctx.input(|input| input.stable_dt).clamp(1.0 / 240.0, 0.05);
        let expansion_active = self.step_expansions(delta_time);
        let camera_active = self.step_camera(delta_time);
        let mode_transition_active = self.step_mode_transition(delta_time);
        let animation_active = expansion_active || camera_active || mode_transition_active;
        self.debug_camera_animating = (self.camera_offset - self.camera_target).length() > 0.25
            || (self.camera_zoom - self.camera_zoom_target).abs() > 0.002;
        let graph_center = center + self.camera_offset;
        if animation_active {
            ctx.request_repaint();
            ctx.request_repaint_after(Duration::from_millis(16));
        }
        let application_count = self.filtered_applications().len();
        let application_page_count = application_count.div_ceil(APPLICATIONS_PAGE_SIZE).max(1);
        self.applications_page = self.applications_page.min(application_page_count - 1);
        let visible = self.visible_entries();
        let transition = self.mode_transition_progress
            * self.mode_transition_progress
            * (3.0 - 2.0 * self.mode_transition_progress);
        let transition_old_nodes = self
            .transition_old_mode
            .map(|mode| {
                let entries = self.transition_old_entries.clone();
                self.layout_entries(mode, &entries, graph_center)
            })
            .unwrap_or_default();
        let mut application_categories = self
            .applications
            .iter()
            .map(|path| desktop_category(path))
            .collect::<HashSet<_>>()
            .into_iter()
            .collect::<Vec<_>>();
        application_categories.sort_by_key(|category| category.to_lowercase());
        ctx.set_visuals(egui::Visuals {
            dark_mode: true,
            window_fill: Color32::from_rgb(11, 16, 24),
            panel_fill: Color32::from_rgb(11, 16, 24),
            ..egui::Visuals::dark()
        });
        egui_extras::install_image_loaders(ctx);

        egui::CentralPanel::default()
            .frame(egui::Frame::NONE)
            .show(ctx, |ui| {
            let screen = ui.max_rect();
            let painter = ui.painter().clone();
            paint_wallpaper(&painter, screen, graph_center);

            let background = ui.interact(screen, ui.id().with("background"), Sense::drag());
            if background.dragged() && self.camera_offset == self.camera_target {
                                self.camera_offset += background.drag_delta();
                self.camera_target = self.camera_offset;
                self.background_press_started = None;
                self.background_press_position = None;
                self.long_press_handled = true;
                ctx.request_repaint();
            } else if background.drag_stopped() {
                                self.save_state();
            }
            let (pointer_position, middle_clicked, scroll_y) = ctx.input(|input| {
                (
                    input.pointer.interact_pos(),
                    input.pointer.button_clicked(egui::PointerButton::Middle),
                    input.raw_scroll_delta.y,
                )
            });
            if middle_clicked {
                self.reset_camera();
                ctx.request_repaint();
            }
            if scroll_y.abs() > 0.0 {
                self.camera_zoom_target = (self.camera_zoom_target * (scroll_y * 0.0015).exp()).clamp(0.35, 2.5);
                self.message = format!("Zoom : {:.0}%", self.camera_zoom_target * 100.0);
                ctx.request_repaint();
            }

            let mut node_data = Vec::with_capacity(visible.len());
            for (index, (path, parent)) in visible.iter().enumerate() {
                if self.mode == ViewMode::Applications {
                    let angle = (index as f32 / visible.len().max(1) as f32) * std::f32::consts::TAU - std::f32::consts::FRAC_PI_2;
                    let position = graph_center + Vec2::angled(angle) * (250.0 * self.camera_zoom);
                    node_data.push((path.clone(), None, position));
                    continue;
                }
                let sibling_index = parent.as_deref().map_or(index, |parent| Self::child_index(&visible, path, parent));
                let sibling_total = parent.as_deref().map_or(visible.iter().filter(|(_, candidate)| candidate.is_none()).count(), |parent| Self::child_total(&visible, parent));
                let base_position = self.position_for(path, parent.as_deref(), sibling_index, sibling_total, graph_center);
                let final_position = graph_center + (base_position - graph_center) * self.camera_zoom;
                let position = if let Some(parent) = parent {
                    let parent_position = node_data.iter().find(|(candidate, _, _)| candidate == parent).map(|(_, _, position)| *position).unwrap_or(graph_center);
                    let progress = self.ancestor_progress(parent);
                    parent_position + (final_position - parent_position) * progress
                } else {
                    final_position
                };
                node_data.push((path.clone(), parent.clone(), position));
            }
            if !animation_active && self.mode == ViewMode::Bureau {
                for (path, _, _) in node_data.clone() {
                    self.resolve_collisions(&path, &visible, graph_center);
                }
            }
            let parent_positions = node_data.iter().map(|(path, _, position)| (path.clone(), *position)).collect::<HashMap<_, _>>();
            for (path, parent, position) in &mut node_data {
                if self.mode == ViewMode::Applications { continue; }
                if let Some(offset) = self.positions.get(path) {
                    let final_position = graph_center + *offset * self.camera_zoom;
                    *position = if let Some(parent) = parent {
                        let parent_position = parent_positions.get(parent).copied().unwrap_or(graph_center);
                        parent_position + (final_position - parent_position) * self.ancestor_progress(parent)
                    } else {
                        final_position
                    };
                }
            }

            for (_, _, position) in &mut node_data {
                *position = graph_center + (*position - graph_center) * transition;
            }
            let node_opacity = transition.clamp(0.0, 1.0);
            let node_alpha = (node_opacity * 255.0) as u8;

            let pointer_is_empty = pointer_position.is_none_or(|pointer| {
                pointer.distance(graph_center) > 100.0
                    && node_data.iter().all(|(_, _, position)| pointer.distance(*position) > 44.0)
            });
            self.debug_pointer_empty = pointer_is_empty;

            let old_opacity = 1.0 - transition;
            let old_positions = transition_old_nodes.iter().map(|(path, _, position)| {
                (path.clone(), graph_center + (*position - graph_center) * old_opacity)
            }).collect::<HashMap<_, _>>();
            for (_, parent, position) in &transition_old_nodes {
                let collapsed_position = graph_center + (*position - graph_center) * old_opacity;
                let start = parent.as_deref().and_then(|parent| old_positions.get(parent).copied()).unwrap_or(graph_center);
                let start_radius = if parent.is_some() { 30.0 } else { 82.0 };
                if let Some((line_start, line_end)) = Self::connected_segment(start, collapsed_position, start_radius, 30.0) {
                    let color = if parent.is_some() { Color32::from_rgb(71, 120, 126) } else { Color32::from_rgb(91, 82, 91) }.gamma_multiply(old_opacity);
                    painter.line_segment([line_start, line_end], Stroke::new(1.5_f32, color));
                }
            }
            for (path, _, position) in &transition_old_nodes {
                let collapsed_position = graph_center + (*position - graph_center) * old_opacity;
                let is_directory = path.is_dir();
                let is_expanded = self.expanded.contains(path);
                painter.circle_filled(collapsed_position, 38.0, Color32::from_rgba_unmultiplied(76, 170, 203, 17).gamma_multiply(old_opacity));
                painter.circle_filled(collapsed_position, 30.0, Self::node_color(is_directory, is_expanded).gamma_multiply(old_opacity));
                painter.circle_stroke(collapsed_position, 31.0, Stroke::new(1.0_f32, Color32::from_rgba_unmultiplied(145, 208, 218, 190).gamma_multiply(old_opacity)));
                if let Some(icon_path) = self.icon_path(path, is_expanded) {
                    let uri = format!("file://{}", icon_path.to_string_lossy());
                    ui.put(
                        egui::Rect::from_center_size(collapsed_position, Vec2::splat(34.0)),
                        egui::Image::from_uri(uri).fit_to_exact_size(Vec2::splat(30.0)).tint(Color32::from_white_alpha((old_opacity * 255.0) as u8)),
                    );
                }
                painter.text(collapsed_position + Vec2::new(0.0, 42.0), Align2::CENTER_TOP, self.label_for(path), FontId::proportional(14.0), Color32::from_rgb(211, 222, 230).gamma_multiply(old_opacity));
            }

            let position_by_path = node_data.iter().map(|(path, _, position)| (path.clone(), *position)).collect::<HashMap<_, _>>();
            for (_path, parent, position) in &node_data {
                let start = parent.as_deref().and_then(|parent| position_by_path.get(parent).copied()).unwrap_or(graph_center);
                let start_radius = if parent.is_some() { 30.0 } else { 82.0 };
                if let Some((line_start, line_end)) = Self::connected_segment(start, *position, start_radius, 30.0) {
                    let base_color = if parent.is_some() { Color32::from_rgb(66, 149, 160) } else { Color32::from_rgb(117, 104, 135) };
                    painter.line_segment([line_start, line_end], Stroke::new(4.0_f32, base_color.gamma_multiply(node_opacity * 0.12)));
                    let line_color = base_color.gamma_multiply(node_opacity * 0.68);
                    painter.line_segment([line_start, line_end], Stroke::new(1.5_f32, line_color));
                    painter.circle_filled(line_end, 2.5, Color32::from_rgb(113, 202, 205).gamma_multiply(node_opacity * 0.75));
                }
            }

            for (index, (path, _, position)) in node_data.iter().enumerate() {
                let is_directory = path.is_dir();
                let is_expanded = self.expanded.contains(path);
                painter.circle_filled(*position, 38.0, Color32::from_rgba_unmultiplied(76, 170, 203, 17).gamma_multiply(node_opacity));
                painter.circle_filled(*position, 30.0, Self::node_color(is_directory, is_expanded).gamma_multiply(node_opacity));
                painter.circle_stroke(*position, 31.0, Stroke::new(1.0_f32, Color32::from_rgba_unmultiplied(145, 208, 218, 190).gamma_multiply(node_opacity)));
                if let Some(icon_path) = self.icon_path(path, is_expanded) {
                    let icon_uri = format!("file://{}", icon_path.to_string_lossy());
                    ui.put(
                        egui::Rect::from_center_size(*position, Vec2::splat(34.0)),
                        egui::Image::from_uri(icon_uri).fit_to_exact_size(Vec2::splat(30.0)).tint(Color32::from_white_alpha(node_alpha)),
                    );
                } else {
                    painter.text(*position, Align2::CENTER_CENTER, if is_directory { if is_expanded { "◉" } else { "○" } } else { "·" }, FontId::proportional(25.0), Color32::WHITE.gamma_multiply(node_opacity));
                }
                painter.text(*position + Vec2::new(0.0, 42.0), Align2::CENTER_TOP, self.label_for(path), FontId::proportional(14.0), Color32::from_rgb(211, 222, 230).gamma_multiply(node_opacity));

                let hit_rect = egui::Rect::from_center_size(*position, Vec2::splat(76.0));
                let node_sense = if self.mode_transition_progress < 0.998 {
                    Sense::hover()
                } else if self.mode == ViewMode::Applications {
                    Sense::click()
                } else {
                    Sense::click_and_drag()
                };
                let response = ui.interact(hit_rect, ui.id().with(("node", index)), node_sense);
                if response.dragged() {
                    if let Some(offset) = self.positions.get_mut(path) {
                                                *offset += response.drag_delta();
                    }
                    self.resolve_collisions(path, &visible, graph_center);
                    ctx.request_repaint();
                } else if response.drag_stopped() {
                                        self.save_state();
                } else if response.clicked() && self.mode == ViewMode::Applications {
                    self.launch_application(path);
                } else if response.clicked() && self.pending_transfer.is_some() {
                    if is_directory {
                        let (source, operation) = self.pending_transfer.clone().unwrap();
                        self.transfer_entry(&source, path, operation);
                    } else {
                        self.message = String::from("Choisis un dossier de destination ou le noyau");
                    }
                } else if response.clicked() && is_directory {
                    self.toggle_directory(path);
                } else if response.double_clicked() && !is_directory {
                    self.open_file(path);
                }
            }

            let core_rect = egui::Rect::from_center_size(graph_center, Vec2::splat(164.0));
            let core_response = ui.interact(core_rect, ui.id().with("core"), Sense::click());
            let core_accent = if self.mode == ViewMode::Bureau { Color32::from_rgb(255, 155, 99) } else { Color32::from_rgb(108, 177, 255) };
            painter.circle_filled(graph_center, 116.0, Color32::from_rgba_unmultiplied(core_accent.r(), core_accent.g(), core_accent.b(), 10));
            painter.circle_filled(graph_center, 101.0, Color32::from_rgba_unmultiplied(core_accent.r(), core_accent.g(), core_accent.b(), 18));
            painter.circle_stroke(graph_center, 98.0, Stroke::new(1.0_f32, Color32::from_rgba_unmultiplied(core_accent.r(), core_accent.g(), core_accent.b(), 75)));
            painter.circle_stroke(graph_center, 91.0, Stroke::new(1.0_f32, Color32::from_rgba_unmultiplied(143, 194, 217, 42)));
            for index in 0..8 {
                let angle = index as f32 * std::f32::consts::TAU / 8.0;
                let position = graph_center + Vec2::angled(angle) * 98.0;
                let dot_radius = if index % 2 == 0 { 2.6 } else { 1.4 };
                let dot_color = if index % 2 == 0 { core_accent } else { Color32::from_rgb(111, 216, 215) };
                painter.circle_filled(position, dot_radius, dot_color.gamma_multiply(0.86));
            }
            painter.circle_filled(graph_center, 82.0, Color32::from_rgb(13, 23, 36));
            painter.circle_filled(graph_center - Vec2::new(11.0, 13.0), 66.0, Color32::from_rgb(21, 38, 53));
            painter.circle_filled(graph_center + Vec2::new(0.0, 27.0), 56.0, Color32::from_rgba_unmultiplied(4, 11, 20, 95));
            painter.circle_stroke(graph_center, 82.0, Stroke::new(2.0_f32, core_accent.gamma_multiply(0.82)));
            painter.circle_stroke(graph_center, 73.0, Stroke::new(1.0_f32, Color32::from_rgba_unmultiplied(181, 211, 224, 58)));

            let emblem_center = graph_center - Vec2::new(0.0, 12.0);
            painter.circle_filled(emblem_center, 34.0, Color32::from_rgba_unmultiplied(core_accent.r(), core_accent.g(), core_accent.b(), 19));
            painter.circle_stroke(emblem_center, 31.0, Stroke::new(1.0_f32, core_accent.gamma_multiply(0.55)));
            for index in 0..6 {
                let angle = index as f32 * std::f32::consts::TAU / 6.0;
                let start = emblem_center + Vec2::angled(angle) * 38.0;
                let end = emblem_center + Vec2::angled(angle) * 49.0;
                painter.line_segment([start, end], Stroke::new(1.5_f32, core_accent.gamma_multiply(0.7)));
                painter.circle_filled(end, 2.0, Color32::from_rgb(196, 232, 239));
            }
            painter.text(emblem_center, Align2::CENTER_CENTER, "C", FontId::proportional(45.0), Color32::from_rgb(236, 245, 248));
            painter.text(graph_center + Vec2::new(0.0, 43.0), Align2::CENTER_CENTER, "CORANUX", FontId::proportional(12.0), Color32::WHITE);
            painter.text(graph_center + Vec2::new(0.0, 59.0), Align2::CENTER_CENTER, if self.mode == ViewMode::Bureau { "BUREAU" } else { "APPLICATIONS" }, FontId::proportional(9.0), Color32::from_rgb(172, 202, 215));
            if core_response.clicked() && self.mode_transition_progress >= 0.998 {
                if let Some((source, operation)) = self.pending_transfer.clone() {
                    self.transfer_entry(&source, &self.bureau_directory.clone(), operation);
                } else {
                    self.switch_mode();
                }
            }

            if self.mode_transition_progress >= 0.998 && ctx.input(|input| input.pointer.secondary_clicked()) {
                if let Some(pointer) = pointer_position {
                    let target = if pointer.distance(graph_center) <= 100.0 {
                        ContextTarget::Core
                    } else if let Some((path, _, _)) = node_data.iter().find(|(_, _, position)| pointer.distance(*position) <= 44.0) {
                        ContextTarget::Element(path.clone())
                    } else {
                        ContextTarget::Empty
                    };
                    self.context_menu = Some((target, pointer));
                    self.context_action = None;
                    self.creation_directory = None;
                    self.rename_target = None;
                    self.context_input.clear();
                }
            }

            if let Some((target, menu_position)) = self.context_menu.clone() {
                let menu_area = Area::new("context-menu".into())
                    .fixed_pos(menu_position)
                    .order(egui::Order::Foreground)
                    .show(ctx, |ui| {
                        egui::Frame::popup(ui.style()).show(ui, |ui| {
                            ui.set_min_width(190.0);
                            if ui.button("Recentrer").clicked() {
                                self.reset_camera();
                                self.close_context_menu();
                            }
                            if ui.button("Changer de mode").clicked() {
                                self.switch_mode();
                                self.close_context_menu();
                            }
                            ui.separator();
                            match &target {
                                ContextTarget::Empty => {
                                    if self.mode == ViewMode::Bureau && ui.button("Créer un fichier").clicked() {
                                        self.context_action = Some(false);
                                        self.creation_directory = Some(self.destination_for_target(&target));
                                        self.context_input = String::from("Nouveau fichier");
                                    }
                                    if self.mode == ViewMode::Bureau && ui.button("Créer un dossier").clicked() {
                                        self.context_action = Some(true);
                                        self.creation_directory = Some(self.destination_for_target(&target));
                                        self.context_input = String::from("Nouveau dossier");
                                    }
                                }
                                ContextTarget::Core => {
                                    if self.mode == ViewMode::Bureau && ui.button("Créer un fichier").clicked() {
                                        self.context_action = Some(false);
                                        self.creation_directory = Some(self.destination_for_target(&target));
                                        self.context_input = String::from("Nouveau fichier");
                                    }
                                    if self.mode == ViewMode::Bureau && ui.button("Créer un dossier").clicked() {
                                        self.context_action = Some(true);
                                        self.creation_directory = Some(self.destination_for_target(&target));
                                        self.context_input = String::from("Nouveau dossier");
                                    }
                                }
                                ContextTarget::Element(path) => {
                                    if self.mode == ViewMode::Applications {
                                        let label = if self.favorites.contains(path) { "Retirer des favoris" } else { "Ajouter aux favoris" };
                                        if ui.button(label).clicked() {
                                            self.toggle_favorite(path);
                                            self.close_context_menu();
                                        }
                                    } else if path.is_dir() {
                                        if ui.button("Créer un fichier ici").clicked() {
                                            self.context_action = Some(false);
                                            self.creation_directory = Some(path.clone());
                                            self.context_input = String::from("Nouveau fichier");
                                        }
                                        if ui.button("Créer un dossier ici").clicked() {
                                            self.context_action = Some(true);
                                            self.creation_directory = Some(path.clone());
                                            self.context_input = String::from("Nouveau dossier");
                                        }
                                        ui.separator();
                                    }
                                    if self.mode == ViewMode::Bureau && ui.button("Déplacer vers…").clicked() {
                                        self.pending_transfer = Some((path.clone(), TransferOperation::Move));
                                        self.message = format!("Déplacer {} : clique sur le dossier destination ou le noyau", display_name(path));
                                        self.close_context_menu();
                                    }
                                    if self.mode == ViewMode::Bureau && ui.button("Copier vers…").clicked() {
                                        self.pending_transfer = Some((path.clone(), TransferOperation::Copy));
                                        self.message = format!("Copier {} : clique sur le dossier destination ou le noyau", display_name(path));
                                        self.close_context_menu();
                                    }
                                    if self.mode == ViewMode::Bureau && ui.button("Supprimer").clicked() {
                                        self.delete_entry(path);
                                    }
                                    if self.mode == ViewMode::Bureau && ui.button("Renommer").clicked() {
                                        self.rename_target = Some(path.clone());
                                        self.context_action = None;
                                        self.context_input = path.file_name().map(|name| name.to_string_lossy().into_owned()).unwrap_or_default();
                                    }
                                }
                            }
                            if let Some(folder) = self.context_action {
                                ui.separator();
                                ui.label(if folder { "Nom du dossier" } else { "Nom du fichier" });
                                ui.text_edit_singleline(&mut self.context_input);
                                if ui.button("Créer").clicked() {
                                    self.create_entry(folder);
                                }
                            }
                            if let Some(path) = self.rename_target.clone() {
                                ui.separator();
                                ui.label("Nouveau nom");
                                ui.text_edit_singleline(&mut self.context_input);
                                if ui.button("Valider").clicked() {
                                    self.rename_entry(&path);
                                }
                            }
                        });
                    });
                if ctx.input(|input| input.pointer.primary_clicked()) {
                    if let Some(pointer) = pointer_position {
                        if !menu_area.response.rect.contains(pointer) {
                            self.close_context_menu();
                        }
                    }
                }
            }

            Area::new("header".into()).fixed_pos(Pos2::new(28.0, 24.0)).show(ctx, |ui| {
                egui::Frame::new()
                    .fill(Color32::from_rgba_unmultiplied(13, 23, 35, 220))
                    .stroke(Stroke::new(1.0_f32, Color32::from_rgba_unmultiplied(84, 125, 148, 115)))
                    .corner_radius(egui::CornerRadius::same(15))
                    .inner_margin(egui::Margin::symmetric(13, 9))
                    .show(ui, |ui| {
                        ui.horizontal(|ui| {
                            ui.label(egui::RichText::new("◉").size(27.0).strong().color(Color32::from_rgb(245, 164, 108)));
                            ui.vertical(|ui| {
                                ui.label(egui::RichText::new("CORANUX DE").size(16.0).strong().color(Color32::from_rgb(236, 223, 205)));
                                ui.label(egui::RichText::new(&self.message).size(12.0).color(Color32::from_rgb(156, 194, 207)));
                                ui.label(egui::RichText::new(self.bureau_directory.to_string_lossy()).size(10.0).color(Color32::from_rgb(105, 139, 157)));
                            });
                        });
                    });
            });
            if self.mode == ViewMode::Applications {
                Area::new("applications-filter".into()).fixed_pos(Pos2::new((screen.width() - 350.0).max(16.0), 24.0)).show(ctx, |ui| {
                    egui::Frame::new()
                        .fill(Color32::from_rgba_unmultiplied(14, 24, 37, 242))
                        .stroke(Stroke::new(1.0_f32, Color32::from_rgb(65, 112, 139)))
                        .corner_radius(egui::CornerRadius::same(16))
                        .inner_margin(egui::Margin::same(14))
                        .shadow(egui::epaint::Shadow {
                            offset: [0, 7],
                            blur: 20,
                            spread: 0,
                            color: Color32::from_black_alpha(115),
                        })
                        .show(ui, |ui| {
                        ui.set_width(294.0);
                        ui.horizontal(|ui| {
                            ui.label(egui::RichText::new("◉").size(20.0).strong().color(Color32::from_rgb(117, 180, 220)));
                            ui.vertical(|ui| {
                                ui.label(egui::RichText::new("APPLICATIONS").size(15.0).strong().color(Color32::from_rgb(218, 232, 245)));
                                ui.label(egui::RichText::new("CORANUX · LANCEUR").size(10.0).color(Color32::from_rgb(117, 151, 174)));
                            });
                        });
                        ui.add_space(8.0);
                        let response = ui.add(egui::TextEdit::singleline(&mut self.applications_search).hint_text("Rechercher une application…"));
                        if response.changed() {
                            self.applications_page = 0;
                        }
                        ui.add_space(3.0);
                        egui::ComboBox::from_label("Catégorie")
                            .selected_text(&self.applications_category)
                            .show_ui(ui, |ui| {
                                ui.selectable_value(&mut self.applications_category, String::from("Toutes"), "Toutes");
                                for category in &application_categories {
                                    ui.selectable_value(&mut self.applications_category, category.clone(), category);
                                }
                            });
                        if ui.selectable_label(self.applications_favorites_only, egui::RichText::new("★ Favoris uniquement").color(Color32::from_rgb(230, 195, 123))).clicked() {
                            self.applications_favorites_only = !self.applications_favorites_only;
                            self.applications_page = 0;
                        }
                        ui.horizontal(|ui| {
                            if ui.add_enabled(self.applications_page > 0, egui::Button::new("Précédent")).clicked() {
                                self.applications_page -= 1;
                            }
                            ui.label(format!("Page {} / {} · {} application(s)", self.applications_page + 1, application_page_count, application_count));
                            if ui.add_enabled(self.applications_page + 1 < application_page_count, egui::Button::new("Suivante")).clicked() {
                                self.applications_page += 1;
                            }
                        });
                    });
                });
            }
            let footer_width = (screen.width() - 24.0).min(540.0);
            Area::new("footer".into()).fixed_pos(Pos2::new(screen.width() - footer_width - 12.0, screen.height() - 110.0)).show(ctx, |ui| {
                egui::Frame::new()
                    .fill(Color32::from_rgba_unmultiplied(13, 23, 35, 235))
                    .stroke(Stroke::new(1.0_f32, Color32::from_rgba_unmultiplied(75, 120, 145, 135)))
                    .corner_radius(egui::CornerRadius::same(14))
                    .inner_margin(egui::Margin::symmetric(11, 8))
                    .shadow(egui::epaint::Shadow {
                        offset: [0, 5],
                        blur: 16,
                        spread: 0,
                        color: Color32::from_black_alpha(100),
                    })
                    .show(ui, |ui| {
                        ui.set_width(footer_width - 22.0);
                        ui.horizontal(|ui| {
                            ui.label(egui::RichText::new("●").size(11.0).color(Color32::from_rgb(112, 210, 184)));
                            let info = if self.mode == ViewMode::Applications {
                                if application_count == 0 {
                                    format!("0 application · page 1/{}", application_page_count)
                                } else {
                                    format!("{}–{} / {} applications · page {}/{}", self.applications_page * APPLICATIONS_PAGE_SIZE + 1, (self.applications_page * APPLICATIONS_PAGE_SIZE + visible.len()).min(application_count), application_count, self.applications_page + 1, application_page_count)
                                }
                            } else {
                                format!("{} éléments visibles", visible.len())
                            };
                            ui.label(egui::RichText::new(info).size(12.0).color(Color32::from_rgb(177, 202, 216)));
                            ui.add_space(8.0);

                            let collapse_button = egui::Button::new(egui::RichText::new("⌄").size(17.0).strong())
                                .fill(Color32::from_rgb(26, 43, 58))
                                .stroke(Stroke::new(1.0_f32, Color32::from_rgb(68, 104, 128)))
                                .corner_radius(egui::CornerRadius::same(9));
                            if ui.add_enabled(self.mode == ViewMode::Bureau && !self.expanded.is_empty(), collapse_button).on_hover_text("Replier toutes les branches").clicked() {
                                self.collapse_all();
                            }

                            let action_text = if self.pending_transfer.is_some() {
                                "Annuler"
                            } else if self.mode == ViewMode::Bureau {
                                "Applications"
                            } else {
                                "Bureau"
                            };
                            let action_fill = if self.pending_transfer.is_some() { Color32::from_rgb(83, 46, 49) } else { Color32::from_rgb(43, 79, 104) };
                            let action = egui::Button::new(egui::RichText::new(action_text).size(12.0).strong())
                                .fill(action_fill)
                                .stroke(Stroke::new(1.0_f32, Color32::from_rgb(89, 151, 177)))
                                .corner_radius(egui::CornerRadius::same(9));
                            if ui.add_sized(Vec2::new(126.0, 32.0), action).clicked() {
                                if self.pending_transfer.is_some() {
                                    self.pending_transfer = None;
                                    self.message = String::from("Opération annulée");
                                } else {
                                    self.switch_mode();
                                }
                            }
                        });
                    });
            });

            if self.debug_mode {
                let debug_y = if self.mode == ViewMode::Applications { 390.0 } else { 24.0 };
                Area::new("debug-panel".into()).fixed_pos(Pos2::new(screen.width() - 330.0, debug_y)).show(ctx, |ui| {
                    egui::Frame::dark_canvas(ui.style()).show(ui, |ui| {
                        ui.set_width(300.0);
                        ui.label(egui::RichText::new("DEBUG RECENTRAGE").strong().color(Color32::from_rgb(255, 190, 120)));
                        ui.label("clic molette : recentrage");
                        ui.label(format!("zone vide : {}", self.debug_pointer_empty));
                        ui.label(format!("animation caméra : {}", self.debug_camera_animating));
                        ui.label(format!("zoom : {:.0}%", self.camera_zoom * 100.0));
                        ui.label(format!("distance restante : {:.1}px", (self.camera_offset - self.camera_target).length()));
                        ui.label(format!("pointeur : {:?}", pointer_position));
                        ui.label(format!("caméra : ({:.1}, {:.1})", self.camera_offset.x, self.camera_offset.y));
                        ui.label(format!("cible : ({:.1}, {:.1})", self.camera_target.x, self.camera_target.y));
                        ui.label(format!("dernier événement : {}", self.debug_last_event));
                        ui.separator();
                        ui.label(format!("thème d’icônes actif : {}", self.icon_theme.as_deref().unwrap_or("automatique (aucun détecté)")));
                        let mut selected_theme = self.icon_theme.clone().unwrap_or_default();
                        egui::ComboBox::from_label("Thème d’icônes")
                            .selected_text(if selected_theme.is_empty() { "Automatique" } else { &selected_theme })
                            .show_ui(ui, |ui| {
                                ui.selectable_value(&mut selected_theme, String::new(), "Automatique");
                                for theme in &self.available_icon_themes {
                                    ui.selectable_value(&mut selected_theme, theme.clone(), theme);
                                }
                            });
                        let next_theme = if selected_theme.is_empty() { None } else { Some(selected_theme) };
                        if next_theme != self.icon_theme {
                            self.icon_theme = next_theme;
                            self.icon_cache.clear();
                            self.save_state();
                        }
                        ui.separator();
                        if ui.button("Arrêter Coranux Desktop").clicked() {
                            self.save_state();
                            request_session_shutdown();
                            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                        }
                    });
                });
            }
        });
    }
}

fn paint_wallpaper(painter: &egui::Painter, screen: egui::Rect, center: Pos2) {
    painter.rect_filled(screen, 0.0, Color32::from_rgb(7, 12, 20));

    // Soft ambient pools give the desktop depth without competing with icons.
    painter.circle_filled(
        center + Vec2::new(-170.0, -120.0),
        570.0,
        Color32::from_rgba_unmultiplied(9, 72, 91, 15),
    );
    painter.circle_filled(
        center + Vec2::new(260.0, 210.0),
        470.0,
        Color32::from_rgba_unmultiplied(32, 42, 94, 13),
    );
    painter.circle_filled(
        center,
        360.0,
        Color32::from_rgba_unmultiplied(13, 34, 52, 14),
    );

    let columns = (screen.width() / 78.0).ceil() as usize;
    let rows = (screen.height() / 72.0).ceil() as usize;
    for column in 0..columns {
        for row in 0..rows {
            let seed = (column * 37 + row * 71 + column * row * 13) % 17;
            if seed < 8 {
                continue;
            }
            let x = screen.left() + column as f32 * 78.0 + (seed as f32 * 2.7) % 31.0;
            let y = screen.top() + row as f32 * 72.0 + ((seed * 5) as f32 * 1.9) % 29.0;
            let radius = if seed > 14 { 1.5 } else { 0.8 };
            let color = if seed > 14 {
                Color32::from_rgba_unmultiplied(117, 191, 213, 72)
            } else {
                Color32::from_rgba_unmultiplied(101, 144, 172, 34)
            };
            painter.circle_filled(Pos2::new(x, y), radius, color);
        }
    }

    for (radius, alpha) in [(150.0, 21), (245.0, 17), (340.0, 12)] {
        painter.circle_stroke(
            center,
            radius,
            Stroke::new(
                1.0_f32,
                Color32::from_rgba_unmultiplied(70, 127, 150, alpha),
            ),
        );
    }
}

fn display_name(path: &Path) -> String {
    let name = path
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .filter(|name| !name.is_empty())
        .unwrap_or_else(|| path.to_string_lossy().into_owned());
    if path.is_file() && name.ends_with(".desktop") {
        fs::read_to_string(path)
            .ok()
            .and_then(|contents| {
                contents.lines().find_map(|line| {
                    line.strip_prefix("Name=")
                        .map(str::trim)
                        .filter(|value| !value.is_empty())
                        .map(str::to_owned)
                })
            })
            .unwrap_or_else(|| name.trim_end_matches(".desktop").to_owned())
    } else {
        name
    }
}

fn copy_path(source: &Path, destination: &Path) -> std::io::Result<()> {
    let metadata = fs::symlink_metadata(source)?;
    let file_type = metadata.file_type();
    if file_type.is_symlink() {
        #[cfg(unix)]
        {
            use std::os::unix::fs::symlink;
            return symlink(fs::read_link(source)?, destination);
        }
        #[cfg(not(unix))]
        {
            return Err(std::io::Error::new(
                std::io::ErrorKind::Unsupported,
                "copie de lien symbolique non prise en charge",
            ));
        }
    }
    if file_type.is_dir() {
        fs::create_dir(destination)?;
        for entry in fs::read_dir(source)? {
            let entry = entry?;
            copy_path(&entry.path(), &destination.join(entry.file_name()))?;
        }
        Ok(())
    } else {
        fs::copy(source, destination).map(|_| ())
    }
}

fn list_applications() -> Vec<PathBuf> {
    let mut roots = Vec::new();
    if let Some(data_home) = std::env::var_os("XDG_DATA_HOME").map(PathBuf::from) {
        roots.push(data_home.join("applications"));
    } else if let Some(home) = std::env::var_os("HOME").map(PathBuf::from) {
        roots.push(home.join(".local/share/applications"));
    }
    let data_dirs =
        std::env::var_os("XDG_DATA_DIRS").unwrap_or_else(|| "/usr/local/share:/usr/share".into());
    roots.extend(std::env::split_paths(&data_dirs).map(|directory| directory.join("applications")));

    let mut seen_ids = HashSet::new();
    let mut applications = Vec::new();
    for root in roots {
        let mut files = Vec::new();
        collect_desktop_files(&root, &mut files);
        files.sort();
        for path in files {
            if !is_launchable_application(&path) {
                continue;
            }
            let id = path
                .strip_prefix(&root)
                .unwrap_or(&path)
                .to_string_lossy()
                .replace('/', "-");
            if seen_ids.insert(id) {
                applications.push(path);
            }
        }
    }
    applications.sort_by_key(|path| display_name(path).to_lowercase());
    applications
}

fn collect_desktop_files(directory: &Path, output: &mut Vec<PathBuf>) {
    let Ok(entries) = fs::read_dir(directory) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect_desktop_files(&path, output);
        } else if path
            .extension()
            .is_some_and(|extension| extension == "desktop")
        {
            output.push(path);
        }
    }
}

fn is_launchable_application(path: &Path) -> bool {
    let Ok(contents) = fs::read_to_string(path) else {
        return false;
    };
    let desktop_entry = contents
        .split("[Desktop Entry]")
        .nth(1)
        .unwrap_or_default()
        .split('\n')
        .take_while(|line| !line.trim_start().starts_with('['))
        .collect::<Vec<_>>()
        .join("\n");
    let value = |key: &str| {
        desktop_entry
            .lines()
            .find_map(|line| line.strip_prefix(&format!("{key}=")).map(str::trim))
    };
    if value("Type").is_some_and(|value| value != "Application")
        || value("Hidden").is_some_and(|value| value.eq_ignore_ascii_case("true"))
        || value("NoDisplay").is_some_and(|value| value.eq_ignore_ascii_case("true"))
        || value("Name").is_none()
        || value("Exec").is_none()
    {
        return false;
    }
    if let Some(try_exec) = value("TryExec") {
        let candidate = Path::new(try_exec.split_whitespace().next().unwrap_or_default());
        let available = if candidate.components().count() > 1 {
            candidate.is_file()
        } else {
            std::env::var_os("PATH")
                .map(|paths| std::env::split_paths(&paths).collect::<Vec<_>>())
                .unwrap_or_default()
                .into_iter()
                .any(|directory| directory.join(candidate).is_file())
        };
        if !available {
            return false;
        }
    }
    true
}

fn desktop_id(path: &Path) -> Option<String> {
    path.file_stem()
        .map(|stem| stem.to_string_lossy().into_owned())
}

fn desktop_category(path: &Path) -> String {
    let Ok(contents) = fs::read_to_string(path) else {
        return String::from("Autres");
    };
    let categories = contents
        .lines()
        .find_map(|line| line.strip_prefix("Categories="))
        .unwrap_or_default();
    let known = [
        "AudioVideo",
        "Development",
        "Education",
        "Game",
        "Graphics",
        "Network",
        "Office",
        "Science",
        "Settings",
        "System",
        "Utility",
    ];
    let category = categories
        .split(';')
        .find(|category| known.contains(category))
        .or_else(|| categories.split(';').find(|category| !category.is_empty()));
    match category.unwrap_or("Autres") {
        "AudioVideo" => String::from("Audio et vidéo"),
        "Development" => String::from("Développement"),
        "Education" => String::from("Éducation"),
        "Game" => String::from("Jeux"),
        "Graphics" => String::from("Graphisme"),
        "Network" => String::from("Internet"),
        "Office" => String::from("Bureautique"),
        "Science" => String::from("Sciences"),
        "Settings" => String::from("Paramètres"),
        "System" => String::from("Système"),
        "Utility" => String::from("Accessoires"),
        other => other.to_owned(),
    }
}

fn desktop_icon_name(path: &Path) -> Option<String> {
    fs::read_to_string(path).ok()?.lines().find_map(|line| {
        line.strip_prefix("Icon=")
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_owned)
    })
}

fn mime_icon_names(path: &Path) -> Vec<String> {
    let mime_type = Command::new("xdg-mime")
        .arg("query")
        .arg("filetype")
        .arg(path)
        .output()
        .ok()
        .filter(|output| output.status.success())
        .map(|output| String::from_utf8_lossy(&output.stdout).trim().to_owned())
        .unwrap_or_default();
    let mut names = Vec::new();
    if !mime_type.is_empty() {
        // Les thèmes freedesktop utilisent souvent le nom MIME complet, avec
        // des replis x-generic pour les catégories.
        names.push(format!("mime-{}", mime_type.replace('/', "-")));
        names.push(mime_type.replace('/', "-"));
        if let Some((category, _)) = mime_type.split_once('/') {
            names.push(format!("{category}-x-generic"));
        }
    }
    names.extend([
        String::from("text-x-generic"),
        String::from("application-octet-stream"),
    ]);
    names.dedup();
    names
}

fn detect_icon_theme() -> Option<String> {
    // XDG_CURRENT_DESKTOP settings are commonly exposed by desktop portals and
    // gsettings; the GTK settings files below cover GTK based environments.
    if let Ok(value) = std::env::var("GTK_THEME") {
        let name = value.split(':').next().unwrap_or(&value).trim();
        if !name.is_empty() {
            return Some(name.to_owned());
        }
    }
    let home = std::env::var_os("HOME").map(PathBuf::from);
    let config_home = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .or_else(|| home.as_ref().map(|home| home.join(".config")));
    for path in [
        config_home
            .as_ref()
            .map(|base| base.join("gtk-3.0/settings.ini")),
        config_home
            .as_ref()
            .map(|base| base.join("gtk-4.0/settings.ini")),
        home.as_ref().map(|base| base.join(".gtkrc-2.0")),
        Some(PathBuf::from("/etc/gtk-3.0/settings.ini")),
    ]
    .into_iter()
    .flatten()
    {
        if let Ok(contents) = fs::read_to_string(path) {
            if let Some(theme) = extract_ini_icon_theme(&contents) {
                return Some(theme);
            }
        }
    }
    if let Ok(output) = Command::new("gsettings")
        .args(["get", "org.gnome.desktop.interface", "icon-theme"])
        .output()
    {
        if output.status.success() {
            let value = String::from_utf8_lossy(&output.stdout)
                .trim()
                .trim_matches('\'')
                .to_owned();
            if !value.is_empty() {
                return Some(value);
            }
        }
    }
    freedesktop_icons::default_theme_gtk()
}

fn list_icon_themes() -> Vec<String> {
    let mut roots = Vec::new();
    if let Some(home) = std::env::var_os("HOME").map(PathBuf::from) {
        roots.push(home.join(".icons"));
    }
    if let Some(data_home) = std::env::var_os("XDG_DATA_HOME").map(PathBuf::from) {
        roots.push(data_home.join("icons"));
    } else if let Some(home) = std::env::var_os("HOME").map(PathBuf::from) {
        roots.push(home.join(".local/share/icons"));
    }
    let data_dirs =
        std::env::var_os("XDG_DATA_DIRS").unwrap_or_else(|| "/usr/local/share:/usr/share".into());
    for directory in std::env::split_paths(&data_dirs) {
        roots.push(directory.join("icons"));
    }
    roots.push(PathBuf::from("/usr/share/pixmaps"));

    let mut themes = HashSet::new();
    for root in roots {
        let Ok(entries) = fs::read_dir(root) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if !path.is_dir() {
                continue;
            }
            let name = entry.file_name().to_string_lossy().into_owned();
            if path.join("index.theme").is_file() || name == "hicolor" {
                themes.insert(name);
            }
        }
    }
    let mut themes = themes.into_iter().collect::<Vec<_>>();
    themes.sort_by_key(|theme| theme.to_lowercase());
    themes
}

fn extract_ini_icon_theme(contents: &str) -> Option<String> {
    contents.lines().find_map(|line| {
        let (key, value) = line.trim().split_once('=')?;
        (key.trim() == "gtk-icon-theme-name")
            .then(|| value.trim().trim_matches('"').trim_matches('\'').to_owned())
            .filter(|value| !value.is_empty())
    })
}

fn default_camera_zoom() -> f32 {
    1.0
}

fn default_dock_icon_size() -> f32 {
    48.0
}

fn request_session_shutdown() {
    let Some(pid) = std::env::var("CORANUX_SESSION_PID")
        .ok()
        .and_then(|pid| pid.parse::<libc::pid_t>().ok())
    else {
        return;
    };
    if pid <= 1 {
        eprintln!("coranux_desktop: identifiant de session invalide ({pid})");
        return;
    }
    let result = unsafe { libc::kill(pid, libc::SIGTERM) };
    if result != 0 {
        eprintln!(
            "coranux_desktop: impossible de demander l’arrêt de la session {pid} : {}",
            std::io::Error::last_os_error()
        );
    }
}

fn main() -> eframe::Result {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("Coranux Desktop")
            .with_app_id("coranux_desktop")
            .with_fullscreen(true)
            .with_inner_size([1100.0, 760.0])
            .with_min_inner_size([720.0, 520.0]),
        ..Default::default()
    };
    eframe::run_native(
        "Coranux Desktop",
        options,
        Box::new(|_creation_context| {
            eprintln!("coranux_desktop: initialisation du bureau commencée");
            let started = std::time::Instant::now();
            let app = BureauApp::new();
            eprintln!(
                "coranux_desktop: initialisation terminée en {:.2}s",
                started.elapsed().as_secs_f32()
            );
            Ok(Box::new(app))
        }),
    )
}
