pub mod autostart;
pub mod window;

use std::fs::{create_dir_all, File};
use std::io::{Read, Write};
use std::path::PathBuf;
use serde::{Deserialize, Serialize};
use crate::detector::zones::{ZoneConfig, ZoneType};
use crate::log_info;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppConfig {
    pub wallpaper_dir: PathBuf,
    pub detection_enabled: bool,
    pub autostart: bool,
    pub scan_subfolders: bool,
    pub sort_by: String, // "Name" | "Date"
    pub fit_mode: String, // "Fill" | "Fit" | "Stretch" | "Center" | "Tile"
    pub thumb_size: u32, // Tamaño en px de las miniaturas generadas (64..512, recomendado 128)
    pub zones: ZoneConfig,
    #[serde(default)]
    pub panel_side: PanelSide,
    #[serde(default = "default_show_scrollbar")]
    pub show_scrollbar: bool,
    #[serde(default = "default_panel_margin")]
    pub panel_margin: u32,
    #[serde(default = "default_language")]
    pub language: String,
    #[serde(default)]
    pub hotkeys: HotkeyConfig,
}

fn default_show_scrollbar() -> bool { true }
fn default_panel_margin() -> u32 { 16 }
fn default_language() -> String { "auto".to_string() }

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PanelSide {
    Top,
    Left,
    Right,
}

impl Default for PanelSide {
    fn default() -> Self { Self::Top }
}

impl PanelSide {
    pub fn active_zones(self) -> Vec<ZoneType> {
        match self {
            Self::Top => vec![ZoneType::TopLeft, ZoneType::Top, ZoneType::TopRight],
            Self::Left => vec![ZoneType::TopLeft, ZoneType::Left, ZoneType::BottomLeft],
            Self::Right => vec![ZoneType::TopRight, ZoneType::Right, ZoneType::BottomRight],
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HotkeyConfig {
    #[serde(default = "default_open_panel_hotkey")]
    pub open_panel: HotkeyBinding,
    #[serde(default = "default_previous_wallpaper_hotkey")]
    pub previous_wallpaper: HotkeyBinding,
    #[serde(default = "default_next_wallpaper_hotkey")]
    pub next_wallpaper: HotkeyBinding,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HotkeyBinding {
    pub modifiers: Vec<String>,
    pub key: String,
}

fn default_open_panel_hotkey() -> HotkeyBinding {
    HotkeyBinding { modifiers: vec!["CTRL".into(), "ALT".into()], key: "P".into() }
}

fn default_previous_wallpaper_hotkey() -> HotkeyBinding {
    HotkeyBinding { modifiers: vec!["CTRL".into(), "ALT".into()], key: "LEFT".into() }
}

fn default_next_wallpaper_hotkey() -> HotkeyBinding {
    HotkeyBinding { modifiers: vec!["CTRL".into(), "ALT".into()], key: "RIGHT".into() }
}

impl Default for HotkeyConfig {
    fn default() -> Self {
        Self {
            open_panel: default_open_panel_hotkey(),
            previous_wallpaper: default_previous_wallpaper_hotkey(),
            next_wallpaper: default_next_wallpaper_hotkey(),
        }
    }
}

impl Default for AppConfig {
    fn default() -> Self {
        let wallpaper_dir = match std::env::var("USERPROFILE") {
            Ok(profile) => PathBuf::from(profile).join("Pictures"),
            Err(_) => PathBuf::from("."),
        };

        Self {
            wallpaper_dir,
            detection_enabled: true,
            autostart: false,
            scan_subfolders: true,
            sort_by: "Name".to_string(),
            fit_mode: "Fill".to_string(),
            thumb_size: 128,
            zones: ZoneConfig::default(),
            panel_side: PanelSide::Top,
            show_scrollbar: true,
            panel_margin: default_panel_margin(),
            language: default_language(),
            hotkeys: HotkeyConfig::default(),
        }
    }
}

impl AppConfig {
    /// Devuelve el tamaño de miniatura configurado, con valores dentro del rango seguro.
    pub fn thumb_size_value(&self) -> u32 {
        const MIN: u32 = 64;
        const MAX: u32 = 512;
        self.thumb_size.clamp(MIN, MAX)
    }
    pub fn config_path() -> PathBuf {
        match std::env::var("LOCALAPPDATA") {
            Ok(local_app) => PathBuf::from(local_app).join("swepr").join("config.json"),
            Err(_) => PathBuf::from("config.json"),
        }
    }

    /// Carga la configuración desde %LOCALAPPDATA%\swepr\config.json o crea una por defecto.
    pub fn load_or_default() -> Self {
        let path = Self::config_path();

        if let Ok(mut file) = File::open(&path) {
            let mut contents = String::new();
            if file.read_to_string(&mut contents).is_ok() {
                if let Ok(mut config) = serde_json::from_str::<AppConfig>(&contents) {
                    let zones_changed = config.zones.active_zones != config.panel_side.active_zones();
                    config.zones.active_zones = config.panel_side.active_zones();
                    let has_all_hotkeys = serde_json::from_str::<serde_json::Value>(&contents)
                        .ok()
                        .and_then(|value| value.get("hotkeys").cloned())
                        .map(|hotkeys| ["open_panel", "previous_wallpaper", "next_wallpaper"]
                            .iter()
                            .all(|name| hotkeys.get(name).is_some()))
                        .unwrap_or(false);
                    let has_panel_side = serde_json::from_str::<serde_json::Value>(&contents)
                        .ok()
                        .and_then(|value| value.get("panel_side").cloned())
                        .is_some();
                    let has_show_scrollbar = serde_json::from_str::<serde_json::Value>(&contents)
                        .ok()
                        .and_then(|value| value.get("show_scrollbar").cloned())
                        .is_some();
                    let has_panel_margin = serde_json::from_str::<serde_json::Value>(&contents)
                        .ok()
                        .and_then(|value| value.get("panel_margin").cloned())
                        .is_some();
                    let has_language = serde_json::from_str::<serde_json::Value>(&contents)
                        .ok()
                        .and_then(|value| value.get("language").cloned())
                        .is_some();
                    let has_legacy_acrylic = serde_json::from_str::<serde_json::Value>(&contents)
                        .ok()
                        .map(|value| value.get("acrylic_enabled").is_some() || value.get("acrylic_opacity").is_some())
                        .unwrap_or(false);
                    if !has_all_hotkeys || !has_panel_side || !has_show_scrollbar || !has_panel_margin || !has_language || has_legacy_acrylic || zones_changed {
                        let _ = config.save();
                    }
                    log_info!("Configuración general cargada desde {:?}", path);
                    return config;
                }
            }
        }

        let default_config = Self::default();
        let _ = default_config.save();
        default_config
    }

    /// Guarda la configuración actual a disco en formato JSON legible.
    pub fn save(&self) -> Result<(), String> {
        let path = Self::config_path();
        if let Some(parent) = path.parent() {
            let _ = create_dir_all(parent);
        }

        let serialized = serde_json::to_string_pretty(self)
            .map_err(|e| format!("Error al serializar configuración: {e}"))?;

        let mut file = File::create(&path)
            .map_err(|e| format!("Error al crear {:?}: {e}", path))?;

        file.write_all(serialized.as_bytes())
            .map_err(|e| format!("Error al escribir {:?}: {e}", path))?;

        log_info!("Configuración persistida en {:?}", path);
        Ok(())
    }
}
