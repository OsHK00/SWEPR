pub mod autostart;
mod hotkey_field;
mod theme;
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
    #[serde(default)]
    pub panel_mode: PanelMode,
    #[serde(default)]
    pub thumb_loading: ThumbLoading,
    /// Cambiar el fondo con el fundido nativo de Windows.
    #[serde(default = "default_wallpaper_fade")]
    pub wallpaper_fade: bool,
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
fn default_wallpaper_fade() -> bool { true }
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

/// Estilo del panel de miniaturas.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum PanelMode {
    /// Tira de miniaturas en un borde: lo más ligero.
    #[default]
    LowPower,
    /// Carrusel de tarjetas inclinadas en el centro de la pantalla.
    Pro,
}

/// Cuándo se generan las miniaturas pequeñas de la galería.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum ThumbLoading {
    /// Todas al escanear y se guardan siempre: el panel nunca muestra huecos.
    #[default]
    Preload,
    /// Solo las cercanas a lo visible, con un máximo en memoria.
    OnDemand,
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

pub fn virtual_key_code(name: &str) -> Option<u32> {
    let normalized = name.trim().to_ascii_uppercase();
    if normalized.len() == 1 {
        let byte = normalized.as_bytes()[0];
        if byte.is_ascii_alphanumeric() {
            return Some(byte as u32);
        }
    }
    match normalized.as_str() {
        "LEFT" => Some(0x25),
        "UP" => Some(0x26),
        "RIGHT" => Some(0x27),
        "DOWN" => Some(0x28),
        "SPACE" => Some(0x20),
        "ENTER" => Some(0x0D),
        "TAB" => Some(0x09),
        "ESC" | "ESCAPE" => Some(0x1B),
        "BACKSPACE" => Some(0x08),
        "HOME" => Some(0x24),
        "END" => Some(0x23),
        "PAGEUP" => Some(0x21),
        "PAGEDOWN" => Some(0x22),
        "INSERT" => Some(0x2D),
        "DELETE" => Some(0x2E),
        value if value.starts_with('F') => value[1..].parse::<u32>().ok()
            .filter(|number| (1..=24).contains(number))
            .map(|number| 0x70 + number - 1),
        value if value.starts_with("0X") => u32::from_str_radix(&value[2..], 16).ok().filter(|code| (1..=0xFE).contains(code)),
        _ => None,
    }
}

/// Nombre con el que se guarda una tecla virtual (inverso de `virtual_key_code`).
fn virtual_key_name(code: u32) -> String {
    match code {
        0x30..=0x39 | 0x41..=0x5A => char::from(code as u8).to_string(),
        0x25 => "LEFT".into(),
        0x26 => "UP".into(),
        0x27 => "RIGHT".into(),
        0x28 => "DOWN".into(),
        0x20 => "SPACE".into(),
        0x0D => "ENTER".into(),
        0x09 => "TAB".into(),
        0x1B => "ESC".into(),
        0x08 => "BACKSPACE".into(),
        0x24 => "HOME".into(),
        0x23 => "END".into(),
        0x21 => "PAGEUP".into(),
        0x22 => "PAGEDOWN".into(),
        0x2D => "INSERT".into(),
        0x2E => "DELETE".into(),
        0x70..=0x87 => format!("F{}", code - 0x70 + 1),
        other => format!("0x{other:02X}"),
    }
}

// Formato del control de atajos de Windows (HKM_GETHOTKEY): tecla en el byte bajo y
// modificadores en el alto.
pub(crate) const HOTKEYF_SHIFT: u16 = 0x01;
pub(crate) const HOTKEYF_CONTROL: u16 = 0x02;
pub(crate) const HOTKEYF_ALT: u16 = 0x04;
pub(crate) const HOTKEYF_EXT: u16 = 0x08;
/// Propio de SWEPR (el control de Windows no admite la tecla Windows).
pub(crate) const HOTKEYF_WIN: u16 = 0x10;

impl HotkeyBinding {
    /// Valor para el campo de atajos, o 0 si no hay tecla.
    pub fn to_hotkey_value(&self) -> u16 {
        let Some(code) = virtual_key_code(&self.key) else { return 0; };
        let mut flags = 0;
        for modifier in &self.modifiers {
            flags |= match modifier.trim().to_ascii_uppercase().as_str() {
                "CTRL" | "CONTROL" => HOTKEYF_CONTROL,
                "ALT" => HOTKEYF_ALT,
                "SHIFT" => HOTKEYF_SHIFT,
                "WIN" | "WINDOWS" => HOTKEYF_WIN,
                _ => 0,
            };
        }
        // Flechas y bloque Inicio/Fin: sin la marca extendida el control las muestra como teclas del teclado numérico.
        if matches!(code, 0x21..=0x28 | 0x2D | 0x2E) {
            flags |= HOTKEYF_EXT;
        }
        (flags << 8) | code as u16
    }

    pub fn from_hotkey_value(value: u16) -> Self {
        let code = (value & 0xFF) as u32;
        if code == 0 {
            return HotkeyBinding { modifiers: Vec::new(), key: String::new() };
        }
        let flags = value >> 8;
        let mut modifiers = Vec::new();
        if flags & HOTKEYF_CONTROL != 0 { modifiers.push("CTRL".to_string()); }
        if flags & HOTKEYF_ALT != 0 { modifiers.push("ALT".to_string()); }
        if flags & HOTKEYF_SHIFT != 0 { modifiers.push("SHIFT".to_string()); }
        if flags & HOTKEYF_WIN != 0 { modifiers.push("WIN".to_string()); }
        HotkeyBinding { modifiers, key: virtual_key_name(code) }
    }

    pub fn is_empty(&self) -> bool {
        self.key.trim().is_empty()
    }

    /// Mismo atajo aunque cambie el orden o las mayúsculas.
    pub fn same_as(&self, other: &HotkeyBinding) -> bool {
        let normalize = |binding: &HotkeyBinding| {
            let mut modifiers: Vec<String> = binding.modifiers.iter().map(|m| match m.trim().to_ascii_uppercase().as_str() {
                "CONTROL" => "CTRL".to_string(),
                "WINDOWS" => "WIN".to_string(),
                other => other.to_string(),
            }).collect();
            modifiers.sort();
            (modifiers, virtual_key_code(&binding.key))
        };
        normalize(self) == normalize(other)
    }
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
            panel_mode: PanelMode::LowPower,
            thumb_loading: ThumbLoading::Preload,
            wallpaper_fade: default_wallpaper_fade(),
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
                    let has_panel_mode = serde_json::from_str::<serde_json::Value>(&contents)
                        .ok()
                        .and_then(|value| value.get("panel_mode").cloned())
                        .is_some();
                    let has_thumb_loading = serde_json::from_str::<serde_json::Value>(&contents)
                        .ok()
                        .and_then(|value| value.get("thumb_loading").cloned())
                        .is_some();
                    let has_wallpaper_fade = serde_json::from_str::<serde_json::Value>(&contents)
                        .ok()
                        .and_then(|value| value.get("wallpaper_fade").cloned())
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
                    if !has_all_hotkeys || !has_panel_side || !has_panel_mode || !has_thumb_loading || !has_wallpaper_fade || !has_show_scrollbar || !has_panel_margin || !has_language || has_legacy_acrylic || zones_changed {
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

#[cfg(test)]
mod hotkey_tests {
    use super::*;

    fn binding(modifiers: &[&str], key: &str) -> HotkeyBinding {
        HotkeyBinding { modifiers: modifiers.iter().map(|m| m.to_string()).collect(), key: key.into() }
    }

    #[test]
    fn hotkey_value_roundtrips() {
        for original in [binding(&["CTRL", "ALT"], "P"), binding(&["CTRL", "ALT"], "LEFT"), binding(&["SHIFT", "ALT"], "F5"), binding(&["CTRL"], "0x6B"), binding(&["WIN", "SHIFT"], "W")] {
            let back = HotkeyBinding::from_hotkey_value(original.to_hotkey_value());
            assert!(back.same_as(&original), "{original:?} -> {back:?}");
        }
        // Las flechas llevan la marca extendida para no mostrarse como teclado numérico.
        assert_eq!(binding(&["CTRL", "ALT"], "RIGHT").to_hotkey_value(), ((0x02 | 0x04 | 0x08) << 8) | 0x27);
        assert!(HotkeyBinding::from_hotkey_value(0).is_empty());
    }

    #[test]
    fn same_as_ignores_order_and_case() {
        assert!(binding(&["alt", "Control"], "p").same_as(&binding(&["CTRL", "ALT"], "P")));
        assert!(!binding(&["CTRL", "ALT"], "P").same_as(&binding(&["CTRL", "WIN"], "P")));
    }
}
