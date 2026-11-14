pub mod autostart;
mod hidden;
mod hotkey_field;
mod theme;
pub mod window;

use std::fs::create_dir_all;
use std::io::Write;
use std::path::{Path, PathBuf};
use serde::{Deserialize, Serialize};
use crate::detector::zones::{ZoneConfig, ZoneType};
use crate::{log_error, log_info};

/// Configuración de SWEPR. Los campos que falten en el JSON toman el valor por defecto.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct AppConfig {
    pub wallpaper_dir: PathBuf,
    pub detection_enabled: bool,
    pub autostart: bool,
    pub scan_subfolders: bool,
    pub sort_by: SortOrder,
    pub fit_mode: FitMode,
    pub thumb_size: u32, // Tamaño en px de las miniaturas generadas (64..512, recomendado 128)
    pub zones: ZoneConfig,
    pub panel_side: PanelSide,
    pub panel_mode: PanelMode,
    pub pro_priority: ProPriority,
    pub thumb_loading: ThumbLoading,
    /// Cambiar el fondo con el fundido nativo de Windows.
    pub wallpaper_fade: bool,
    pub show_scrollbar: bool,
    pub panel_margin: u32,
    pub language: String,
    pub hotkeys: HotkeyConfig,
    /// Cambio automático de fondo cada N minutos (0 = desactivado).
    pub rotation_minutes: u32,
    /// "Siguiente" (atajo, bandeja y rotación) elige una imagen al azar.
    pub shuffle: bool,
    /// La rotación automática solo usa las imágenes favoritas (si hay alguna).
    pub rotation_favorites_only: bool,
    /// Imágenes marcadas como favoritas desde el panel.
    pub favorites: Vec<PathBuf>,
    /// Imágenes quitadas de la galería (el archivo no se borra; se pueden restaurar en Configuración).
    pub excluded: Vec<PathBuf>,
    /// Con varios monitores, "siguiente", aleatorio y la rotación ponen una imagen distinta en cada uno,
    /// y al elegir en el panel se cambia solo el monitor donde está el panel.
    pub per_monitor: bool,
    /// Cómo se cambia el fondo: con Windows o con el lienzo de SWEPR.
    pub apply_mode: ApplyMode,
    /// Duración de la transición del lienzo, en milisegundos.
    pub transition_ms: u32,
    /// Efecto de la transición del lienzo.
    pub transition: Transition,
}

/// Efecto con el que el lienzo pasa de un fondo al siguiente.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum Transition {
    /// Fundido cruzado.
    #[default]
    Fade,
    /// La imagen nueva entra desde la derecha empujando a la anterior.
    Slide,
    /// La imagen nueva aparece acercándose suavemente.
    Zoom,
    /// Barrido de izquierda a derecha.
    Wipe,
    /// Círculo que se abre desde el centro.
    Circle,
    /// Uno distinto cada vez.
    Random,
}

impl Transition {
    pub const ALL: [Transition; 6] = [Transition::Fade, Transition::Slide, Transition::Zoom, Transition::Wipe, Transition::Circle, Transition::Random];
    /// Efectos concretos (sin "aleatorio").
    pub const EFFECTS: [Transition; 5] = [Transition::Fade, Transition::Slide, Transition::Zoom, Transition::Wipe, Transition::Circle];

    /// El efecto que se usará ahora: "aleatorio" elige uno de los demás.
    pub fn resolve(self, random: impl FnOnce(usize) -> usize) -> Transition {
        match self {
            Transition::Random => Self::EFFECTS[random(Self::EFFECTS.len()) % Self::EFFECTS.len()],
            other => other,
        }
    }
}

/// Forma de aplicar los fondos.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum ApplyMode {
    /// Windows cambia el fondo (con o sin su fundido, según `wallpaper_fade`).
    #[default]
    Windows,
    /// SWEPR lo dibuja detrás de los iconos, al instante y con transición propia.
    Canvas,
}

fn default_show_scrollbar() -> bool { true }
fn default_wallpaper_fade() -> bool { true }
fn default_panel_margin() -> u32 { 16 }
fn default_language() -> String { "auto".to_string() }

/// Orden de la galería.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum SortOrder {
    #[default]
    #[serde(alias = "Name", alias = "NAME")]
    Name,
    /// Más recientes primero.
    #[serde(alias = "Date", alias = "DATE")]
    Date,
}

/// Cómo se ajusta la imagen a la pantalla.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum FitMode {
    #[default]
    #[serde(alias = "Fill")]
    Fill,
    #[serde(alias = "Fit")]
    Fit,
    #[serde(alias = "Stretch")]
    Stretch,
    #[serde(alias = "Center")]
    Center,
    #[serde(alias = "Tile")]
    Tile,
    /// Una sola imagen extendida sobre todos los monitores.
    #[serde(alias = "Span")]
    Span,
}

impl FitMode {
    pub const ALL: [FitMode; 6] = [FitMode::Fill, FitMode::Fit, FitMode::Stretch, FitMode::Center, FitMode::Tile, FitMode::Span];
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum PanelSide {
    #[default]
    Top,
    Left,
    Right,
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

/// Prioridad de carga del carrusel Pro.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum ProPriority {
    /// Precarga las tarjetas grandes de toda la galería para evitar esperas al navegar.
    Smooth,
    /// Conserva en memoria solo las tarjetas cercanas al área visible.
    #[default]
    Performance,
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

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct HotkeyConfig {
    #[serde(default = "default_open_panel_hotkey")]
    pub open_panel: HotkeyBinding,
    #[serde(default = "default_previous_wallpaper_hotkey")]
    pub previous_wallpaper: HotkeyBinding,
    #[serde(default = "default_next_wallpaper_hotkey")]
    pub next_wallpaper: HotkeyBinding,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
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
            sort_by: SortOrder::Name,
            fit_mode: FitMode::Fill,
            thumb_size: 128,
            zones: ZoneConfig::default(),
            panel_side: PanelSide::Top,
            panel_mode: PanelMode::LowPower,
            pro_priority: ProPriority::Performance,
            thumb_loading: ThumbLoading::Preload,
            wallpaper_fade: default_wallpaper_fade(),
            show_scrollbar: default_show_scrollbar(),
            panel_margin: default_panel_margin(),
            language: default_language(),
            hotkeys: HotkeyConfig::default(),
            rotation_minutes: 0,
            shuffle: false,
            rotation_favorites_only: false,
            favorites: Vec::new(),
            excluded: Vec::new(),
            per_monitor: false,
            apply_mode: ApplyMode::Windows,
            transition_ms: 600,
            transition: Transition::Fade,
        }
    }
}

/// Escribe un archivo de forma atómica: primero a `<archivo>.tmp` y luego lo renombra,
/// así un corte a mitad de escritura nunca deja el archivo original vacío o a medias.
pub fn write_atomically(path: &Path, contents: &[u8]) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        let _ = create_dir_all(parent);
    }
    let mut temp_name = path.file_name().map(|name| name.to_os_string()).unwrap_or_default();
    temp_name.push(".tmp");
    let temp_path = path.with_file_name(temp_name);
    {
        let mut file = std::fs::File::create(&temp_path).map_err(|e| format!("Error al crear {:?}: {e}", temp_path))?;
        file.write_all(contents).map_err(|e| format!("Error al escribir {:?}: {e}", temp_path))?;
        file.sync_all().map_err(|e| format!("Error al volcar {:?}: {e}", temp_path))?;
    }
    std::fs::rename(&temp_path, path).map_err(|e| {
        let _ = std::fs::remove_file(&temp_path);
        format!("Error al reemplazar {:?}: {e}", path)
    })
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
    /// Si el archivo está dañado se conserva una copia en `config.corrupt.json` antes de
    /// volver a los valores por defecto, para no perder la configuración del usuario.
    pub fn load_or_default() -> Self {
        let path = Self::config_path();

        match std::fs::read_to_string(&path) {
            Ok(contents) => match serde_json::from_str::<AppConfig>(&contents) {
                Ok(mut config) => {
                    config.zones.active_zones = config.panel_side.active_zones();
                    // Reescribir si faltaban campos, sobraban (opciones antiguas) o cambió algo.
                    let normalized = serde_json::to_string_pretty(&config).unwrap_or_default();
                    if normalized.trim_end() != contents.trim_end() {
                        let _ = config.save();
                    }
                    log_info!("Configuración general cargada desde {:?}", path);
                    return config;
                }
                Err(e) => {
                    let backup = path.with_file_name("config.corrupt.json");
                    log_error!("Configuración dañada ({e}). Se guarda una copia en {:?} y se usan valores por defecto.", backup);
                    let _ = std::fs::copy(&path, &backup);
                }
            },
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => log_error!("No se pudo leer {:?}: {e}", path),
        }

        let default_config = Self::default();
        let _ = default_config.save();
        default_config
    }

    /// Guarda la configuración actual a disco en formato JSON legible (de forma atómica).
    pub fn save(&self) -> Result<(), String> {
        let path = Self::config_path();
        let serialized = serde_json::to_string_pretty(self)
            .map_err(|e| format!("Error al serializar configuración: {e}"))?;
        write_atomically(&path, serialized.as_bytes())?;
        log_info!("Configuración persistida en {:?}", path);
        Ok(())
    }

    pub fn is_favorite(&self, path: &Path) -> bool {
        self.favorites.iter().any(|favorite| crate::winutil::same_path(favorite, path))
    }

    pub fn is_excluded(&self, path: &Path) -> bool {
        self.excluded.iter().any(|excluded| crate::winutil::same_path(excluded, path))
    }

    /// Quita una imagen de la galería (sin tocar el archivo). Devuelve si no estaba ya quitada.
    pub fn exclude(&mut self, path: &Path) -> bool {
        if self.is_excluded(path) {
            return false;
        }
        self.excluded.push(path.to_path_buf());
        true
    }

    /// Marca o desmarca una favorita. Devuelve si queda marcada.
    pub fn toggle_favorite(&mut self, path: &Path) -> bool {
        let before = self.favorites.len();
        self.favorites.retain(|favorite| !crate::winutil::same_path(favorite, path));
        if self.favorites.len() == before {
            self.favorites.push(path.to_path_buf());
            true
        } else {
            false
        }
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

#[cfg(test)]
mod config_tests {
    use super::*;

    #[test]
    fn legacy_values_and_missing_fields_load() {
        let json = r#"{"wallpaper_dir":"C:\\Fotos","sort_by":"Date","fit_mode":"Fit","acrylic_enabled":true}"#;
        let config: AppConfig = serde_json::from_str(json).unwrap();
        assert_eq!(config.sort_by, SortOrder::Date);
        assert_eq!(config.fit_mode, FitMode::Fit);
        assert_eq!(config.thumb_size, 128);
        assert_eq!(config.hotkeys, HotkeyConfig::default());
        assert_eq!(config.rotation_minutes, 0);
        assert_eq!(config.wallpaper_dir, PathBuf::from(r"C:\Fotos"));
    }

    #[test]
    fn config_roundtrips() {
        let config = AppConfig {
            fit_mode: FitMode::Span,
            sort_by: SortOrder::Date,
            favorites: vec![PathBuf::from(r"C:\a.jpg")],
            ..AppConfig::default()
        };
        let json = serde_json::to_string_pretty(&config).unwrap();
        assert_eq!(serde_json::from_str::<AppConfig>(&json).unwrap(), config);
    }

    #[test]
    fn toggle_favorite_ignores_case() {
        let mut config = AppConfig::default();
        assert!(config.toggle_favorite(Path::new(r"C:\Fotos\A.jpg")));
        assert!(config.is_favorite(Path::new(r"c:\fotos\a.JPG")));
        assert!(!config.toggle_favorite(Path::new(r"c:\fotos\a.jpg")));
        assert!(config.favorites.is_empty());
    }

    #[test]
    fn exclude_is_idempotent_and_ignores_case() {
        let mut config = AppConfig::default();
        assert!(config.exclude(Path::new(r"C:\Fotos\A.jpg")));
        assert!(!config.exclude(Path::new(r"c:\fotos\a.jpg")));
        assert!(config.is_excluded(Path::new(r"C:\FOTOS\A.JPG")));
        assert_eq!(config.excluded.len(), 1);
    }

    #[test]
    fn random_transition_resolves_to_a_concrete_effect() {
        for pick in 0..10 {
            let effect = Transition::Random.resolve(|_| pick);
            assert_ne!(effect, Transition::Random);
        }
        assert_eq!(Transition::Wipe.resolve(|_| unreachable!()), Transition::Wipe);
    }

    #[test]
    fn write_atomically_replaces_file() {
        let path = std::env::temp_dir().join(format!("swepr-atomic-{}.json", std::process::id()));
        write_atomically(&path, b"uno").unwrap();
        write_atomically(&path, b"dos").unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "dos");
        let _ = std::fs::remove_file(&path);
    }
}
