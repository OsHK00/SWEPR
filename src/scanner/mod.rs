use crate::log_info;
use crate::log_warn;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::SystemTime;
use windows::Win32::Foundation::{HWND, LPARAM, WPARAM};
use windows::Win32::UI::WindowsAndMessaging::{PostMessageW, WM_APP};

pub const WM_APP_SCAN_COMPLETE: u32 = WM_APP + 10;

/// Extensiones de imagen que SWEPR muestra y que Windows acepta como fondo.
pub const SUPPORTED_EXTENSIONS: [&str; 9] = ["jpg", "jpeg", "jfif", "png", "bmp", "webp", "gif", "tif", "tiff"];

/// ¿El nombre (o la ruta) termina en una extensión de imagen soportada?
pub fn is_supported_image(name: &Path) -> bool {
    name.extension()
        .and_then(|e| e.to_str())
        .is_some_and(|ext| SUPPORTED_EXTENSIONS.iter().any(|supported| ext.eq_ignore_ascii_case(supported)))
}

#[derive(Debug, Clone)]
pub struct ScannedImage {
    pub path: PathBuf,
    pub file_name: String,
    pub is_new: bool,
    pub is_favorite: bool,
    /// Fecha de modificación: sirve para ordenar y para saber si hay que regenerar la miniatura.
    pub modified: Option<SystemTime>,
}

/// Lo que se recuerda de la galería entre ejecuciones, para que las imágenes nuevas
/// sigan marcadas como nuevas tras reiniciar SWEPR.
#[derive(Debug, Default, Serialize, Deserialize)]
struct GalleryState {
    folder: PathBuf,
    known: Vec<String>,
    new: Vec<String>,
}

fn gallery_state_path() -> Option<PathBuf> {
    std::env::var("LOCALAPPDATA").ok().map(|dir| PathBuf::from(dir).join("swepr").join("gallery.json"))
}

fn path_key(path: &Path) -> String {
    path.to_string_lossy().to_lowercase()
}

fn load_gallery_state() -> Option<GalleryState> {
    let contents = std::fs::read_to_string(gallery_state_path()?).ok()?;
    serde_json::from_str(&contents).ok()
}

/// Guarda qué imágenes conoce SWEPR y cuáles siguen siendo nuevas.
pub fn save_gallery_state(folder: &Path, images: &[ScannedImage]) {
    let Some(path) = gallery_state_path() else { return; };
    let state = GalleryState {
        folder: folder.to_path_buf(),
        known: images.iter().map(|image| path_key(&image.path)).collect(),
        new: images.iter().filter(|image| image.is_new).map(|image| path_key(&image.path)).collect(),
    };
    match serde_json::to_string(&state) {
        Ok(serialized) => {
            if let Err(e) = crate::config::write_atomically(&path, serialized.as_bytes()) {
                log_warn!("No se pudo guardar el estado de la galería: {e}");
            }
        }
        Err(e) => log_warn!("No se pudo serializar el estado de la galería: {e}"),
    }
}

pub struct Scanner {
    /// Cada escaneo recibe un número; solo el más reciente puede publicar su resultado.
    generation: Arc<AtomicU64>,
    /// Carpeta del último escaneo. Si cambia, el estado de "nueva" se toma del disco.
    last_folder: Mutex<Option<PathBuf>>,
}

/// Recorre la carpeta (y subcarpetas) devolviendo las imágenes encontradas.
/// Cada carpeta se visita una sola vez aunque haya enlaces o uniones que formen ciclos.
fn collect_images(folder: &Path, include_subdirs: bool) -> Vec<ScannedImage> {
    let mut found = Vec::new();
    let mut visited: HashSet<PathBuf> = HashSet::new();
    let mut stack: Vec<PathBuf> = vec![folder.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let canonical = std::fs::canonicalize(&dir).unwrap_or_else(|_| dir.clone());
        if !visited.insert(canonical) {
            log_warn!("Carpeta ya visitada (enlace circular), se omite: {:?}", dir);
            continue;
        }
        let entries = match std::fs::read_dir(&dir) {
            Ok(entries) => entries,
            Err(e) => {
                log_warn!("No se pudo leer directorio {:?}: {e}", dir);
                continue;
            }
        };
        for entry in entries.flatten() {
            // En Windows, el tipo y la fecha vienen del propio listado del directorio.
            let (file_type, modified) = match entry.file_type() {
                Ok(t) if t.is_symlink() => match std::fs::metadata(entry.path()) {
                    Ok(meta) => (meta.file_type(), meta.modified().ok()),
                    Err(_) => continue,
                },
                Ok(t) => (t, entry.metadata().ok().and_then(|m| m.modified().ok())),
                Err(_) => continue,
            };
            if file_type.is_dir() {
                if include_subdirs {
                    stack.push(entry.path());
                }
                continue;
            }
            let path = entry.path();
            if !file_type.is_file() || !is_supported_image(&path) {
                continue;
            }
            let file_name = entry.file_name().to_string_lossy().to_string();
            found.push(ScannedImage { path, file_name, is_new: false, is_favorite: false, modified });
        }
    }
    found
}

fn sort_images(images: &mut [ScannedImage], order_by_date: bool) {
    if order_by_date {
        images.sort_by(|a, b| match (b.modified, a.modified) {
            (Some(bd), Some(ad)) => bd.cmp(&ad),
            (Some(_), None) => std::cmp::Ordering::Less,
            (None, Some(_)) => std::cmp::Ordering::Greater,
            (None, None) => a.file_name.to_lowercase().cmp(&b.file_name.to_lowercase()),
        });
    } else {
        images.sort_by_cached_key(|image| image.file_name.to_lowercase());
    }
}

impl Scanner {
    pub fn new() -> Self {
        Self { generation: Arc::new(AtomicU64::new(0)), last_folder: Mutex::new(None) }
    }

    /// Escanea la carpeta en un hilo aparte. Guarda el resultado en `results` y notifica a `hwnd` con WM_APP_SCAN_COMPLETE.
    /// Si mientras tanto empieza otro escaneo, el resultado de este se descarta.
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
        let generation = self.generation.fetch_add(1, Ordering::AcqRel) + 1;
        let latest = self.generation.clone();
        let same_folder = self.last_folder.lock()
            .map(|mut last| last.replace(folder.clone()).as_deref() == Some(folder.as_path()))
            .unwrap_or(false);
        // Con la misma carpeta se conserva el estado en memoria; si no, se lee del disco.
        let previous_new: Option<HashMap<PathBuf, bool>> = same_folder.then(|| results.lock().map(|images| {
            images.iter().map(|image| (image.path.clone(), image.is_new)).collect()
        }).unwrap_or_default());

        thread::spawn(move || {
            let start = std::time::Instant::now();
            let mut images = collect_images(&folder, include_subdirs);
            sort_images(&mut images, order_by_date);

            match &previous_new {
                Some(previous) => {
                    for image in &mut images {
                        image.is_new = previous.get(&image.path).copied().unwrap_or(true);
                    }
                }
                None => {
                    // Solo se marcan como nuevas si ya se conocía esta carpeta.
                    if let Some(state) = load_gallery_state().filter(|state| crate::winutil::same_path(&state.folder, &folder)) {
                        let known: HashSet<String> = state.known.into_iter().collect();
                        let new: HashSet<String> = state.new.into_iter().collect();
                        for image in &mut images {
                            let key = path_key(&image.path);
                            image.is_new = new.contains(&key) || !known.contains(&key);
                        }
                    }
                }
            }
            images.sort_by(|a, b| b.is_new.cmp(&a.is_new));

            let count = images.len();
            let elapsed = start.elapsed().as_millis();

            {
                let Ok(mut guard) = results.lock() else { return; };
                if latest.load(Ordering::Acquire) != generation {
                    log_info!("Escaneo de {:?} descartado: hay uno más reciente.", folder);
                    return;
                }
                save_gallery_state(&folder, &images);
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn supported_extensions_ignore_case() {
        assert!(is_supported_image(Path::new(r"C:\a\b.JPG")));
        assert!(is_supported_image(Path::new("x.webp")));
        assert!(is_supported_image(Path::new("x.Tiff")));
        assert!(!is_supported_image(Path::new("x.txt")));
        assert!(!is_supported_image(Path::new("carpeta")));
    }

    #[test]
    fn collect_images_survives_directory_cycles() {
        let root = std::env::temp_dir().join(format!("swepr-scan-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("sub")).unwrap();
        std::fs::write(root.join("a.png"), b"x").unwrap();
        std::fs::write(root.join("sub").join("b.jpg"), b"x").unwrap();
        std::fs::write(root.join("notes.txt"), b"x").unwrap();
        // Unión que apunta a la carpeta raíz: un ciclo. Si no se puede crear, la prueba sigue sin ella.
        let _ = std::process::Command::new("cmd")
            .args(["/C", "mklink", "/J"])
            .arg(root.join("sub").join("loop"))
            .arg(&root)
            .output();
        let mut names: Vec<String> = collect_images(&root, true).into_iter().map(|i| i.file_name).collect();
        names.sort();
        assert_eq!(names, vec!["a.png".to_string(), "b.jpg".to_string()]);
        let _ = std::fs::remove_dir(root.join("sub").join("loop"));
        let _ = std::fs::remove_dir_all(&root);
    }
}
