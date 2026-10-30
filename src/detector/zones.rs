

use serde::{Deserialize, Serialize};
use crate::log_info;
use crate::winutil::{
    get_cursor_position, get_monitor_rect_from_point, is_any_mouse_button_down,
    is_notification_allowed,
};
use windows::Win32::Foundation::{HWND, POINT, RECT};
use windows::Win32::Graphics::Gdi::HMONITOR;
use windows::Win32::UI::WindowsAndMessaging::{KillTimer, SetTimer};

pub const ZONE_TIMER_ID: usize = 0x5001;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ZoneType {
    TopLeft,
    Top,
    TopRight,
    Left,
    Right,
    BottomLeft,
    Bottom,
    BottomRight,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ZoneConfig {
    pub enabled: bool,
    pub delay_ms: u32,
    pub edge_thickness: i32,
    pub corner_size: i32,
    pub active_zones: Vec<ZoneType>,
}

impl Default for ZoneConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            delay_ms: 300,
            edge_thickness: 4,
            corner_size: 24,
            active_zones: vec![
                ZoneType::TopLeft,
                ZoneType::Top,
                ZoneType::TopRight,
                // Zonas laterales e inferiores desactivadas: solo activacion desde arriba
            ],
        }
    }
}

/// Realiza el calculo geometrico para determinar si un punto se encuentra en alguna de las 8 zonas.
pub fn hit_test(config: &ZoneConfig, monitor_rect: &RECT, pt: &POINT) -> Option<ZoneType> {
    let x = pt.x;
    let y = pt.y;
    let left = monitor_rect.left;
    let top = monitor_rect.top;
    let right = monitor_rect.right;
    let bottom = monitor_rect.bottom;

    if x < left || x >= right || y < top || y >= bottom {
        return None;
    }

    let c = config.corner_size;
    let e = config.edge_thickness;

    // 1. Deteccion de esquinas
    let detected = if x < left + c && y < top + c {
        ZoneType::TopLeft
    } else if x >= right - c && y < top + c {
        ZoneType::TopRight
    } else if x < left + c && y >= bottom - c {
        ZoneType::BottomLeft
    } else if x >= right - c && y >= bottom - c {
        ZoneType::BottomRight
    // 2. Deteccion de bordes (entre esquinas)
    } else if y < top + e {
        ZoneType::Top
    } else if y >= bottom - e {
        ZoneType::Bottom
    } else if x < left + e {
        ZoneType::Left
    } else if x >= right - e {
        ZoneType::Right
    } else {
        return None;
    };

    if config.active_zones.contains(&detected) {
        Some(detected)
    } else {
        None
    }
}

/// Administrador del estado de las zonas y del temporizador transitorio de activacion.
use crate::thumbs::ThumbnailCache;
use crate::scanner::ScannedImage;
use std::sync::{Arc, Mutex};

pub struct ZoneDetector {
    pub hwnd: HWND,
    pub config: ZoneConfig,
    pub current_zone: Option<(ZoneType, HMONITOR)>,
    pub is_timer_running: bool,
    pub is_triggered: bool,
    pub thumb_cache: Option<ThumbnailCache>,
    pub scanned_images: Option<Arc<Mutex<Vec<ScannedImage>>>>,
}

impl ZoneDetector {
    pub fn new(hwnd: HWND, config: ZoneConfig) -> Self {
        Self {
            hwnd,
            config,
            current_zone: None,
            is_timer_running: false,
            is_triggered: false,
            thumb_cache: None,
            scanned_images: None,
        }
    }

    pub fn update_config(&mut self, config: ZoneConfig) {
        self.config = config;
        self.reset();
    }

    /// Maneja el movimiento del mouse proveniente de WM_INPUT mientras el escritorio tiene el foco.
    pub fn on_mouse_move(&mut self, pt: POINT) {
        if !self.config.enabled {
            self.reset();
            return;
        }

        // Descarta si hay botones del mouse presionados (arrastre de iconos o recuadro de seleccion)
        if is_any_mouse_button_down() {
            if self.is_timer_running {
                log_info!("Timer cancelado: boton del mouse presionado durante movimiento.");
                self.stop_timer();
            }
            self.current_zone = None;
            self.is_triggered = false;
            return;
        }

        let monitor_info = match get_monitor_rect_from_point(pt) {
            Some(info) => info,
            None => {
                self.reset();
                return;
            }
        };

        let (hmon, rect) = monitor_info;
        let detected = hit_test(&self.config, &rect, &pt);

        match detected {
            Some(zone) => {
                if let Some((current_zone, current_hmon)) = self.current_zone {
                    if current_zone == zone && current_hmon == hmon {
                        // El cursor sigue dentro de la misma zona
                        return;
                    }
                }

                // Ingreso a una nueva zona o monitor distinto
                self.stop_timer();
                self.start_timer();
                self.current_zone = Some((zone, hmon));
                self.is_triggered = false;

                log_info!(
                    "Cursor entro en zona {:?} (Monitor: {:?}). Demora configurada: {} ms. Timer temporal iniciado.",
                    zone,
                    hmon,
                    self.config.delay_ms
                );
            }
            None => {
                if self.is_timer_running {
                    log_info!("Cursor salio de la zona antes del timeout. Destruyendo timer.");
                    self.stop_timer();
                }
                self.current_zone = None;
                self.is_triggered = false;
            }
        }
    }

    /// Maneja el evento WM_TIMER correspondiente a la demora de activacion.
    pub fn on_timer(&mut self, timer_id: usize) {
        if timer_id != ZONE_TIMER_ID {
            return;
        }

        self.stop_timer();

        if is_any_mouse_button_down() {
            log_info!("Activacion descartada: boton del mouse presionado al vencer el timer.");
            self.current_zone = None;
            return;
        }

        if !is_notification_allowed() {
            log_info!("Activacion descartada: SHQueryUserNotificationState indica pantalla completa, juego o presentacion.");
            self.current_zone = None;
            return;
        }

        if let Some(pt) = get_cursor_position() {
            if let Some((hmon, rect)) = get_monitor_rect_from_point(pt) {
                if let Some(zone) = hit_test(&self.config, &rect, &pt) {
                    if let Some((expected_zone, expected_hmon)) = self.current_zone {
                        if zone == expected_zone && hmon == expected_hmon && !self.is_triggered {
                            self.is_triggered = true;
                            log_info!(
                                ">>> ZONA ACTIVADA! Monitor: {:?} | Zona: {:?} | Coordenadas: ({}, {}) <<<",
                                hmon,
                                zone,
                                pt.x,
                                pt.y
                            );
                            let cache = self.thumb_cache.clone().unwrap_or_default();
                            let images = self.scanned_images.clone().unwrap_or_default();
                            crate::panel::set_draw_context(cache, images);
                            crate::panel::show_panel();
                            return;
                        }
                    }
                }
            }
        }

        log_info!("Activacion descartada: el cursor ya no se encontraba en la zona al cumplirse el plazo.");
        self.current_zone = None;
        self.is_triggered = false;
    }

    fn start_timer(&mut self) {
        unsafe {
            let _ = SetTimer(self.hwnd, ZONE_TIMER_ID, self.config.delay_ms, None);
        }
        self.is_timer_running = true;
    }

    fn stop_timer(&mut self) {
        if self.is_timer_running {
            unsafe {
                let _ = KillTimer(self.hwnd, ZONE_TIMER_ID);
            }
            self.is_timer_running = false;
        }
    }

    /// Cancela timers activos y reinicia el estado de deteccion (por ejemplo ante perdida de foco o suspension).
    pub fn reset(&mut self) {
        self.stop_timer();
        self.current_zone = None;
        self.is_triggered = false;
    }
}

impl Drop for ZoneDetector {
    fn drop(&mut self) {
        self.reset();
    }
}
