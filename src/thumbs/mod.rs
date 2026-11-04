use crate::log_debug;
use crate::log_info;
use crate::log_warn;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{channel, Sender};
use std::sync::{Arc, Mutex};
use windows::core::{Interface, PCWSTR};
use windows::Win32::Foundation::{HWND, LPARAM, WPARAM};
use windows::Win32::Graphics::Gdi::{
    CreateCompatibleDC, CreateDIBSection, DeleteDC, DeleteObject, GetObjectW, SelectObject, SetBrushOrgEx, SetStretchBltMode, StretchBlt, BITMAP,
    BITMAPINFO, BITMAPINFOHEADER, COLORONCOLOR, DIB_RGB_COLORS, HALFTONE, HBITMAP, HGDIOBJ, SRCCOPY,
};
use windows::Win32::UI::Shell::{IShellItem, IShellItemImageFactory, SHCreateItemFromParsingName, SIIGBF_RESIZETOFIT};
use windows::Win32::UI::WindowsAndMessaging::{PostMessageW, WM_APP};

pub const WM_APP_THUMB_READY: u32 = WM_APP + 20;

#[derive(Clone)]
pub struct Thumb {
    pub path_key: String,
    pub hbitmap: isize,
    pub width: i32,
    pub height: i32,
}

fn delete_bitmap(thumb: &Thumb) {
    if thumb.hbitmap != 0 {
        unsafe { let _ = DeleteObject(HGDIOBJ(thumb.hbitmap as *mut _)); }
    }
}

/// Caché de miniaturas pequeñas. El hilo de carga solo añade entradas; borrar o expulsar
/// se hace desde el hilo de interfaz, así un bitmap nunca desaparece mientras se pinta.
#[derive(Clone)]
pub struct ThumbnailCache {
    cache: Arc<Mutex<HashMap<String, (Thumb, u64)>>>,
    tick: Arc<AtomicU64>,
}

impl ThumbnailCache {
    pub fn new() -> Self {
        Self {
            cache: Arc::new(Mutex::new(HashMap::new())),
            tick: Arc::new(AtomicU64::new(0)),
        }
    }

    pub fn get(&self, key: &str) -> Option<Thumb> {
        self.cache.lock().ok().and_then(|guard| guard.get(key).map(|(thumb, _)| thumb.clone()))
    }

    pub fn contains(&self, key: &str) -> bool {
        self.cache.lock().map(|guard| guard.contains_key(key)).unwrap_or(false)
    }

    #[cfg(test)]
    pub fn len(&self) -> usize {
        self.cache.lock().map(|guard| guard.len()).unwrap_or(0)
    }

    pub fn insert(&self, thumb: Thumb) {
        let tick = self.tick.load(Ordering::Relaxed);
        if let Ok(mut guard) = self.cache.lock() {
            if let Some((old, _)) = guard.insert(thumb.path_key.clone(), (thumb, tick)) {
                // Liberar el bitmap GDI sustituido para evitar fugas de memoria GDI
                delete_bitmap(&old);
            }
        }
    }

    /// Marca miniaturas como usadas ahora (para la expulsión LRU).
    pub fn touch<'a>(&self, keys: impl Iterator<Item = &'a str>) {
        let tick = self.tick.fetch_add(1, Ordering::Relaxed) + 1;
        if let Ok(mut guard) = self.cache.lock() {
            for key in keys {
                if let Some(entry) = guard.get_mut(key) { entry.1 = tick; }
            }
        }
    }

    /// Libera las miniaturas usadas hace más tiempo hasta quedar en `capacity`.
    /// Solo desde el hilo de interfaz.
    pub fn evict_to(&self, capacity: usize) {
        if let Ok(mut guard) = self.cache.lock() {
            if guard.len() <= capacity { return; }
            let mut by_age: Vec<(u64, String)> = guard.iter().map(|(key, (_, used))| (*used, key.clone())).collect();
            by_age.sort_unstable();
            let excess = guard.len() - capacity;
            for (_, key) in by_age.into_iter().take(excess) {
                if let Some((thumb, _)) = guard.remove(&key) { delete_bitmap(&thumb); }
            }
        }
    }

    /// Libera las miniaturas cuya clave no cumple `keep` (por ejemplo, archivos borrados).
    /// Solo desde el hilo de interfaz.
    pub fn retain(&self, keep: impl Fn(&str) -> bool) -> usize {
        let Ok(mut guard) = self.cache.lock() else { return 0; };
        let before = guard.len();
        guard.retain(|key, (thumb, _)| {
            let kept = keep(key);
            if !kept { delete_bitmap(thumb); }
            kept
        });
        before - guard.len()
    }

    /// Limpia la caché y libera los bitmaps GDI.
    pub fn clear(&self) {
        if let Ok(mut guard) = self.cache.lock() {
            for (_, (thumb, _)) in guard.drain() {
                delete_bitmap(&thumb);
            }
        }
    }
}

impl Default for ThumbnailCache {
    fn default() -> Self {
        Self::new()
    }
}

impl Drop for ThumbnailCache {
    fn drop(&mut self) {
        if Arc::strong_count(&self.cache) == 1 {
            self.clear();
        }
    }
}

/// Copia (y si hace falta recorta y escala) un bitmap a un DIB de 24 bits de `width`x`height`.
/// Las miniaturas no usan el canal alfa, así que se ahorra un 25 % sin cambio visible.
unsafe fn copy_to_dib24(source: HBITMAP, crop: (i32, i32, i32, i32), width: i32, height: i32) -> Option<HBITMAP> {
    let header = BITMAPINFOHEADER {
        biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
        biWidth: width,
        biHeight: -height,
        biPlanes: 1,
        biBitCount: 24,
        ..Default::default()
    };
    let bitmap_info = BITMAPINFO { bmiHeader: header, ..Default::default() };
    let mut bits = std::ptr::null_mut();
    let target = CreateDIBSection(None, &bitmap_info, DIB_RGB_COLORS, &mut bits, None, 0).ok()?;

    let src_dc = CreateCompatibleDC(None);
    let dst_dc = CreateCompatibleDC(None);
    let old_src = SelectObject(src_dc, HGDIOBJ(source.0));
    let old_dst = SelectObject(dst_dc, HGDIOBJ(target.0));
    let (crop_x, crop_y, crop_w, crop_h) = crop;
    if crop_w == width && crop_h == height {
        let _ = SetStretchBltMode(dst_dc, COLORONCOLOR);
    } else {
        let _ = SetStretchBltMode(dst_dc, HALFTONE);
        let _ = SetBrushOrgEx(dst_dc, 0, 0, None);
    }
    let _ = StretchBlt(dst_dc, 0, 0, width, height, src_dc, crop_x, crop_y, crop_w, crop_h, SRCCOPY);
    SelectObject(src_dc, old_src);
    SelectObject(dst_dc, old_dst);
    let _ = DeleteDC(src_dc);
    let _ = DeleteDC(dst_dc);
    Some(target)
}

/// Pide al Shell la imagen ajustada a la caja indicada. Requiere COM inicializado.
unsafe fn shell_image(path: &Path, box_w: i32, box_h: i32) -> Option<(HBITMAP, i32, i32)> {
    let path_wide: Vec<u16> = path.to_string_lossy().encode_utf16().chain(std::iter::once(0)).collect();
    let Ok(shell_item) = SHCreateItemFromParsingName::<PCWSTR, Option<&windows::Win32::System::Com::IBindCtx>, IShellItem>(
        PCWSTR(path_wide.as_ptr()),
        None,
    ) else {
        log_warn!("SHCreateItemFromParsingName fallo para {:?}", path);
        return None;
    };
    let Ok(factory) = shell_item.cast::<IShellItemImageFactory>() else {
        log_warn!("No se pudo obtener IShellItemImageFactory para {:?}", path);
        return None;
    };
    let bitmap = match factory.GetImage(windows::Win32::Foundation::SIZE { cx: box_w, cy: box_h }, SIIGBF_RESIZETOFIT) {
        Ok(bitmap) => bitmap,
        Err(e) => {
            log_warn!("GetImage fallo para {:?}: {e:?}", path);
            return None;
        }
    };
    let mut info = BITMAP::default();
    let read = GetObjectW(HGDIOBJ(bitmap.0), std::mem::size_of::<BITMAP>() as i32, Some(&mut info as *mut _ as *mut std::ffi::c_void));
    if read == 0 || info.bmWidth <= 0 || info.bmHeight == 0 {
        let _ = DeleteObject(HGDIOBJ(bitmap.0));
        return None;
    }
    Some((bitmap, info.bmWidth, info.bmHeight.abs()))
}

/// Genera la miniatura pequeña de la galería (cabe en `size_px` x 9/16 de alto).
unsafe fn load_thumbnail(path: &Path, size_px: i32) -> Option<Thumb> {
    let start = std::time::Instant::now();
    let (source, width, height) = shell_image(path, size_px, (size_px as f32 * 9.0 / 16.0).round() as i32)?;
    let copy = copy_to_dib24(source, (0, 0, width, height), width, height);
    let _ = DeleteObject(HGDIOBJ(source.0));
    let copy = copy?;
    log_debug!(
        "Miniatura generada para {:?} en {} ms ({}x{})",
        path.file_name().unwrap_or_default(),
        start.elapsed().as_millis(),
        width,
        height
    );
    Some(Thumb { path_key: path.to_string_lossy().to_string(), hbitmap: copy.0 as isize, width, height })
}

/// Genera un bitmap de 24 bits de exactamente `width`x`height` que cubre el área recortando
/// el centro de la imagen. Solo se guarda el recorte, sin el bitmap intermedio del Shell.
/// Requiere COM inicializado en el hilo que llama.
pub unsafe fn load_cover_bitmap(path: &Path, width: i32, height: i32) -> Option<Thumb> {
    if width <= 0 || height <= 0 { return None; }
    // Caja ancha: las imágenes horizontales se ajustan por alto y luego se recortan a lo ancho.
    let (source, src_w, src_h) = shell_image(path, width.max(height * 3), height)?;
    let crop = if src_w * height > src_h * width {
        let crop_w = (src_h * width / height).max(1);
        ((src_w - crop_w) / 2, 0, crop_w, src_h)
    } else {
        let crop_h = (src_w * height / width).max(1);
        (0, (src_h - crop_h) / 2, src_w, crop_h)
    };
    let copy = copy_to_dib24(source, crop, width, height);
    let _ = DeleteObject(HGDIOBJ(source.0));
    Some(Thumb { path_key: path.to_string_lossy().to_string(), hbitmap: copy?.0 as isize, width, height })
}

// --- Hilo único de carga ---

struct Request {
    path: PathBuf,
    key: String,
    size_px: i32,
    generation: u64,
    cache: ThumbnailCache,
    notify: isize,
}

static WORKER: Mutex<Option<Sender<Request>>> = Mutex::new(None);
/// Claves pedidas que aún no se han generado. Evita pedir dos veces la misma miniatura.
static PENDING: Mutex<Option<HashSet<String>>> = Mutex::new(None);
static GENERATION: AtomicU64 = AtomicU64::new(0);

fn worker_sender() -> Option<Sender<Request>> {
    let mut guard = WORKER.lock().ok()?;
    if guard.is_none() {
        let (sender, receiver) = channel::<Request>();
        std::thread::spawn(move || unsafe {
            use windows::Win32::System::Com::{CoInitializeEx, COINIT_APARTMENTTHREADED};
            let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
            for request in receiver {
                let wanted = request.generation == GENERATION.load(Ordering::Acquire)
                    && PENDING.lock().map(|p| p.as_ref().is_some_and(|set| set.contains(&request.key))).unwrap_or(false);
                if !wanted || request.cache.contains(&request.key) {
                    if let Ok(mut pending) = PENDING.lock() {
                        if let Some(set) = pending.as_mut() { set.remove(&request.key); }
                    }
                    continue;
                }
                let thumb = load_thumbnail(&request.path, request.size_px);
                // Insertar con PENDING bloqueado: así `cancel_pending` + `clear` nunca dejan
                // entrar una miniatura de una configuración anterior.
                if let Ok(mut pending) = PENDING.lock() {
                    if let Some(set) = pending.as_mut() { set.remove(&request.key); }
                    match thumb {
                        Some(thumb) if request.generation == GENERATION.load(Ordering::Acquire) => request.cache.insert(thumb),
                        Some(thumb) => delete_bitmap(&thumb),
                        None => {}
                    }
                }
                let _ = PostMessageW(HWND(request.notify as *mut _), WM_APP_THUMB_READY, WPARAM(0), LPARAM(0));
            }
        });
        log_info!("Miniaturas: hilo de carga iniciado");
        *guard = Some(sender);
    }
    guard.clone()
}

/// Encola las miniaturas que faltan, en orden. Las que ya están en caché o pedidas se saltan.
pub fn request_thumbnails<'a>(paths: impl Iterator<Item = &'a Path>, size_px: i32, cache: &ThumbnailCache, notify: HWND) {
    let Some(sender) = worker_sender() else { return; };
    let generation = GENERATION.load(Ordering::Acquire);
    let Ok(mut pending) = PENDING.lock() else { return; };
    let set = pending.get_or_insert_with(HashSet::new);
    for path in paths {
        let key = path.to_string_lossy().to_string();
        if cache.contains(&key) || !set.insert(key.clone()) { continue; }
        let _ = sender.send(Request {
            path: path.to_path_buf(),
            key,
            size_px,
            generation,
            cache: cache.clone(),
            notify: notify.0 as isize,
        });
    }
}

/// Olvida las peticiones que ya no hacen falta (por ejemplo, al desplazarse lejos).
pub fn retain_pending(keep: impl Fn(&str) -> bool) {
    if let Ok(mut pending) = PENDING.lock() {
        if let Some(set) = pending.as_mut() { set.retain(|key| keep(key)); }
    }
}

/// Descarta todas las peticiones en cola (cambio de configuración o nuevo lote completo).
pub fn cancel_pending() {
    if let Ok(mut pending) = PENDING.lock() {
        GENERATION.fetch_add(1, Ordering::AcqRel);
        *pending = None;
    }
}
