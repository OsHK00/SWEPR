use crate::log_info;
use crate::log_warn;
use std::path::{Path, PathBuf};
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use windows::Win32::Foundation::{HWND, LPARAM, WPARAM};
use windows::Win32::UI::WindowsAndMessaging::{PostMessageW, WM_APP};

pub const WM_APP_SCAN_COMPLETE: u32 = WM_APP + 10;

#[derive(Debug, Clone)]
pub struct ScannedImage {
    pub path: PathBuf,
    pub file_name: String,
    pub is_new: bool,
}

pub struct Scanner {
    has_scanned: AtomicBool,
}

impl Scanner {
    pub fn new() -> Self {
        Self { has_scanned: AtomicBool::new(false) }
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
        let is_initial_scan = !self.has_scanned.swap(true, Ordering::Relaxed);
        let previous_new: HashMap<PathBuf, bool> = results.lock().map(|images| {
            images.iter().map(|image| (image.path.clone(), image.is_new)).collect()
        }).unwrap_or_default();

        thread::spawn(move || {
            let start = std::time::Instant::now();
            // La fecha solo se lee si se ordena por fecha; `DirEntry` ya trae tipo y fecha
            // del listado del directorio, sin consultas extra al disco por archivo.
            let mut found: Vec<(ScannedImage, Option<std::time::SystemTime>)> = Vec::new();

            let mut stack: Vec<PathBuf> = vec![folder.clone()];
            while let Some(dir) = stack.pop() {
                match std::fs::read_dir(&dir) {
                    Ok(entries) => {
                        for entry in entries.flatten() {
                            // Los enlaces se resuelven como antes; el resto sale gratis del listado.
                            let file_type = match entry.file_type() {
                                Ok(t) if t.is_symlink() => match std::fs::metadata(entry.path()) {
                                    Ok(meta) => meta.file_type(),
                                    Err(_) => continue,
                                },
                                Ok(t) => t,
                                Err(_) => continue,
                            };
                            if file_type.is_dir() {
                                if include_subdirs {
                                    stack.push(entry.path());
                                }
                                continue;
                            }
                            if !file_type.is_file() {
                                continue;
                            }
                            let path = entry.path();
                            let is_image = path.extension().and_then(|e| e.to_str()).is_some_and(|ext| {
                                matches!(ext.to_ascii_lowercase().as_str(), "jpg" | "jpeg" | "png" | "bmp")
                            });
                            if !is_image {
                                continue;
                            }
                            let modified = if order_by_date {
                                entry.metadata().ok().and_then(|m| m.modified().ok())
                            } else {
                                None
                            };
                            let file_name = entry.file_name().to_string_lossy().to_string();
                            found.push((ScannedImage { path, file_name, is_new: false }, modified));
                        }
                    }
                    Err(e) => {
                        log_warn!("No se pudo leer directorio {:?}: {e}", dir);
                    }
                }
            }

            if order_by_date {
                found.sort_by(|(a, ma), (b, mb)| match (mb, ma) {
                    (Some(bd), Some(ad)) => bd.cmp(ad),
                    (Some(_), None) => std::cmp::Ordering::Less,
                    (None, Some(_)) => std::cmp::Ordering::Greater,
                    (None, None) => a.file_name.to_lowercase().cmp(&b.file_name.to_lowercase()),
                });
            } else {
                found.sort_by_cached_key(|(image, _)| image.file_name.to_lowercase());
            }
            let mut images: Vec<ScannedImage> = found.into_iter().map(|(image, _)| image).collect();

            for image in &mut images {
                image.is_new = if is_initial_scan {
                    false
                } else {
                    previous_new.get(&image.path).copied().unwrap_or(true)
                };
            }
            images.sort_by(|a, b| b.is_new.cmp(&a.is_new));

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
