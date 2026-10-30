pub mod autostart;
pub mod window;

use std::fs::{create_dir_all, File};
use std::io::{Read, Write};
use std::path::PathBuf;
use serde::{Deserialize, Serialize};
use crate::detector::zones::ZoneConfig;
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
                if let Ok(config) = serde_json::from_str::<AppConfig>(&contents) {
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
