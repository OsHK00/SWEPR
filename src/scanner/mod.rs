use crate::log_info;
use crate::log_warn;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::thread;
use windows::Win32::Foundation::{HWND, LPARAM, WPARAM};
use windows::Win32::UI::WindowsAndMessaging::{PostMessageW, WM_APP};

pub const WM_APP_SCAN_COMPLETE: u32 = WM_APP + 10;

#[derive(Debug, Clone)]
pub struct ScannedImage {
    pub path: PathBuf,
    pub file_name: String,
}

pub struct Scanner;

impl Scanner {
    pub fn new() -> Self {
        Self
    }

    /// Escanea la carpeta en un hilo aparte. Guarda el resultado en `results` y notifica a `hwnd` con WM_APP_SCAN_COMPLETE.
    pub fn scan_folder<P: AsRef<Path>>(
        &self,
        hwnd: HWND,
        folder: P,
        include_subdirs: bool,
        order_by_date: bool,
        results: Arc<Mutex<Vec<ScannedImage>>>,
    ) {
        let folder = folder.as_ref().to_path_buf();
        let hwnd_val = hwnd.0 as isize;

        thread::spawn(move || {
            let start = std::time::Instant::now();
            let mut images: Vec<ScannedImage> = Vec::new();

            let mut stack: Vec<PathBuf> = vec![folder.clone()];
            while let Some(dir) = stack.pop() {
                if !include_subdirs && dir != folder {
                    continue;
                }
                match std::fs::read_dir(&dir) {
                    Ok(entries) => {
                        for entry in entries.flatten() {
                            let path = entry.path();
                            if path.is_dir() {
                                if include_subdirs {
                                    stack.push(path);
                                }
                                continue;
                            }
                            if !path.is_file() {
                                continue;
                            }
                            if let Some(ext) = path.extension().and_then(|e| e.to_str()) {
                                let ext_lower = ext.to_ascii_lowercase();
                                if matches!(ext_lower.as_str(), "jpg" | "jpeg" | "png" | "bmp") {
                                    let file_name = path
                                        .file_name()
                                        .and_then(|n| n.to_str())
                                        .unwrap_or("")
                                        .to_string();
                                    images.push(ScannedImage {
                                        path: path.clone(),
                                        file_name,
                                    });
                                }
                            }
                        }
                    }
                    Err(e) => {
                        log_warn!("No se pudo leer directorio {:?}: {e}", dir);
                    }
                }
            }

            if order_by_date {
                images.sort_by(|a, b| {
                    let ma = std::fs::metadata(&a.path).ok().and_then(|m| m.modified().ok());
                    let mb = std::fs::metadata(&b.path).ok().and_then(|m| m.modified().ok());
                    match (mb, ma) {
                        (Some(bd), Some(ad)) => bd.cmp(&ad),
                        (Some(_), None) => std::cmp::Ordering::Less,
                        (None, Some(_)) => std::cmp::Ordering::Greater,
                        (None, None) => a.file_name.to_lowercase().cmp(&b.file_name.to_lowercase()),
                    }
                });
            } else {
                images.sort_by(|a, b| a.file_name.to_lowercase().cmp(&b.file_name.to_lowercase()));
            }

            let count = images.len();
            let elapsed = start.elapsed().as_millis();

            if let Ok(mut guard) = results.lock() {
                *guard = images;
            }

            log_info!(
                "Escaneo completado: {} imagenes encontradas en {} ms (subcarpetas: {})",
                count,
                elapsed,
                include_subdirs
            );

            unsafe {
                let _ = PostMessageW(
                    HWND(hwnd_val as *mut _),
                    WM_APP_SCAN_COMPLETE,
                    WPARAM(count),
                    LPARAM(0),
                );
            }
        });
    }
}

impl Default for Scanner {
    fn default() -> Self {
        Self::new()
    }
}
