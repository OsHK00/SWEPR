//! Estilo carrusel: tarjetas inclinadas en una franja en el centro de la pantalla.
//!
//! Para no gastar memoria, las tarjetas grandes solo existen para las imágenes visibles
//! (más unas pocas vecinas). Se generan en un hilo propio, ya recortadas al tamaño exacto
//! de la tarjeta, y se guardan en una caché LRU acotada que se vacía al cerrar el panel.
//! Mientras una tarjeta carga se dibuja la miniatura pequeña de la galería.

use crate::log_info;
use crate::scanner::ScannedImage;
use crate::thumbs::{Thumb, ThumbnailCache};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{channel, Sender};
use std::sync::Mutex;
use windows::Win32::Foundation::{COLORREF, HWND, LPARAM, POINT, RECT, WPARAM};
use windows::Win32::Graphics::Gdi::{
    AlphaBlend, BitBlt, CreateCompatibleBitmap, CreateCompatibleDC, CreatePolygonRgn, CreateSolidBrush, DeleteDC, DeleteObject, FillRect, FrameRgn,
    SelectClipRgn, SelectObject, SetStretchBltMode, StretchBlt, ALTERNATE, BLENDFUNCTION, COLORONCOLOR, HBITMAP, HDC, HGDIOBJ, HRGN, SRCCOPY,
};
use windows::Win32::UI::WindowsAndMessaging::{PostMessageW, WM_APP};

/// Mensaje que el hilo de carga envía al panel cuando hay tarjetas listas.
pub const WM_APP_TILE_READY: u32 = WM_APP + 60;
/// Color que el panel vuelve transparente (LWA_COLORKEY) fuera de las tarjetas.
pub const COLOR_KEY: COLORREF = COLORREF(0x00FF01FE);
/// Escala de la tarjeta enfocada.
pub const FOCUS_SCALE: f32 = 1.12;

const PREFETCH: usize = 2;
const DIM_ALPHA: f32 = 110.0;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Layout {
    pub tile_w: i32,
    pub tile_h: i32,
    pub slant: i32,
    pub gap: i32,
    pub pad_y: i32,
    pub margin_x: i32,
}

impl Layout {
    pub fn for_monitor(monitor_height: i32) -> Self {
        let tile_h = ((monitor_height as f32 * 0.36) as i32).max(120);
        let gap = (tile_h / 40).max(6);
        Self {
            tile_w: tile_h * 56 / 100,
            tile_h,
            slant: tile_h * 22 / 100,
            gap,
            pad_y: (tile_h as f32 * (FOCUS_SCALE - 1.0) / 2.0).ceil() as i32 + 4,
            margin_x: gap,
        }
    }

    pub fn window_height(&self) -> i32 { self.tile_h + self.pad_y * 2 }
    pub fn pitch(&self) -> i32 { self.tile_w + self.gap }
    /// Ancho del rectángulo que contiene una tarjeta inclinada.
    pub fn bounds_w(&self) -> i32 { self.tile_w + self.slant }

    pub fn content_extent(&self, count: usize) -> i32 {
        if count == 0 { return 0; }
        count as i32 * self.pitch() - self.gap + self.slant + self.margin_x * 2
    }

    pub fn max_scroll(&self, count: usize, viewport: i32) -> i32 {
        (self.content_extent(count) - viewport).max(0)
    }

    /// X del borde izquierdo del rectángulo de la tarjeta (sin escalar).
    pub fn tile_x(&self, index: usize, count: usize, viewport: i32, scroll: i32) -> i32 {
        let content = self.content_extent(count);
        let origin = if content <= viewport { (viewport - content) / 2 } else { 0 };
        origin + self.margin_x + index as i32 * self.pitch() - scroll
    }

    /// Índice de la tarjeta bajo el punto, respetando los bordes inclinados.
    pub fn hit_test(&self, x: i32, y: i32, count: usize, viewport: i32, scroll: i32) -> Option<usize> {
        if count == 0 || y < self.pad_y || y >= self.pad_y + self.tile_h { return None; }
        let from_top = (y - self.pad_y) as f32 / self.tile_h as f32;
        let shift = (self.slant as f32 * (1.0 - from_top)) as i32;
        let offset = x - (self.tile_x(0, count, viewport, scroll) + shift);
        if offset < 0 { return None; }
        let index = (offset / self.pitch()) as usize;
        (index < count && offset % self.pitch() < self.tile_w).then_some(index)
    }

    /// Scroll que deja la tarjeta indicada en el centro de la franja.
    pub fn center_scroll(&self, index: usize, count: usize, viewport: i32) -> i32 {
        let center = self.margin_x + index as i32 * self.pitch() + self.bounds_w() / 2;
        (center - viewport / 2).clamp(0, self.max_scroll(count, viewport))
    }

    pub fn visible_range(&self, count: usize, viewport: i32, scroll: i32) -> std::ops::Range<usize> {
        if count == 0 { return 0..0; }
        let first_x = self.tile_x(0, count, viewport, scroll);
        let first = ((-first_x - self.bounds_w()).div_euclid(self.pitch()) + 1).max(0) as usize;
        let last = (viewport - first_x + self.pitch() - 1).div_euclid(self.pitch()).max(0) as usize;
        first.min(count)..last.min(count)
    }

    /// Vértices de la tarjeta escalada alrededor de su centro.
    fn polygon(&self, x: i32, scale: f32) -> ([POINT; 4], RECT) {
        let bw = self.bounds_w() as f32 * scale;
        let h = self.tile_h as f32 * scale;
        let cx = x as f32 + self.bounds_w() as f32 / 2.0;
        let cy = self.pad_y as f32 + self.tile_h as f32 / 2.0;
        let left = (cx - bw / 2.0).round() as i32;
        let top = (cy - h / 2.0).round() as i32;
        let right = left + bw.round() as i32;
        let bottom = top + h.round() as i32;
        let slant = (self.slant as f32 * scale).round() as i32;
        let points = [
            POINT { x: left + slant, y: top },
            POINT { x: right, y: top },
            POINT { x: right - slant, y: bottom },
            POINT { x: left, y: bottom },
        ];
        (points, RECT { left, top, right, bottom })
    }
}

static LAYOUT: Mutex<Option<Layout>> = Mutex::new(None);

pub fn set_layout(layout: Layout) {
    let changed = LAYOUT.lock().map(|mut current| current.replace(layout) != Some(layout)).unwrap_or(false);
    if changed { clear_tiles(); }
}

pub fn layout() -> Option<Layout> {
    LAYOUT.lock().ok().and_then(|g| *g)
}

// --- Caché de tarjetas ---

struct TileCache {
    tiles: HashMap<String, (Thumb, u64)>,
    tick: u64,
}

struct TileRequest {
    path: PathBuf,
    key: String,
    width: i32,
    height: i32,
    generation: u64,
    notify: isize,
}

/// Solo el hilo de interfaz toca esta caché, así un bitmap nunca se borra mientras se pinta.
static TILES: Mutex<Option<TileCache>> = Mutex::new(None);
/// Tarjetas pedidas al hilo de carga que aún no llegaron (o fallaron).
static PENDING: Mutex<Option<HashSet<String>>> = Mutex::new(None);
/// Tarjetas generadas que esperan a ser recogidas por el hilo de interfaz.
static READY: Mutex<Vec<(Thumb, u64)>> = Mutex::new(Vec::new());
static GENERATION: AtomicU64 = AtomicU64::new(0);
static WORKER: Mutex<Option<Sender<TileRequest>>> = Mutex::new(None);

fn delete_bitmap(thumb: &Thumb) {
    if thumb.hbitmap != 0 {
        unsafe { let _ = DeleteObject(HGDIOBJ(thumb.hbitmap as *mut _)); }
    }
}

fn worker_sender() -> Option<Sender<TileRequest>> {
    let mut guard = WORKER.lock().ok()?;
    if guard.is_none() {
        let (sender, receiver) = channel::<TileRequest>();
        std::thread::spawn(move || unsafe {
            use windows::Win32::System::Com::{CoInitializeEx, COINIT_APARTMENTTHREADED};
            let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
            for request in receiver {
                // Saltar lo que ya no se necesita (se desplazó fuera o se cerró el panel).
                let still_wanted = request.generation == GENERATION.load(Ordering::Acquire)
                    && PENDING.lock().ok().map(|p| p.as_ref().is_some_and(|set| set.contains(&request.key))).unwrap_or(false);
                if !still_wanted { continue; }
                if let Some(thumb) = crate::thumbs::load_cover_bitmap(&request.path, request.width, request.height) {
                    if let Ok(mut ready) = READY.lock() { ready.push((thumb, request.generation)); }
                    let _ = PostMessageW(HWND(request.notify as *mut _), WM_APP_TILE_READY, WPARAM(0), LPARAM(0));
                }
            }
        });
        log_info!("Carrusel: hilo de carga de tarjetas iniciado");
        *guard = Some(sender);
    }
    guard.clone()
}

/// Pasa a la caché las tarjetas que terminó el hilo de carga. Llamar desde el hilo de interfaz.
pub fn collect_ready() {
    let ready: Vec<(Thumb, u64)> = READY.lock().map(|mut r| std::mem::take(&mut *r)).unwrap_or_default();
    if ready.is_empty() { return; }
    let generation = GENERATION.load(Ordering::Acquire);
    let size = layout().map(|l| (l.bounds_w(), l.tile_h));
    let mut tiles = TILES.lock().ok();
    let mut pending = PENDING.lock().ok();
    for (thumb, thumb_generation) in ready {
        if let Some(set) = pending.as_mut().and_then(|p| p.as_mut()) { set.remove(&thumb.path_key); }
        let fits = thumb_generation == generation && size == Some((thumb.width, thumb.height));
        match tiles.as_mut() {
            Some(cache) if fits => {
                let cache = cache.get_or_insert_with(|| TileCache { tiles: HashMap::new(), tick: 0 });
                let tick = cache.tick;
                if let Some((old, _)) = cache.tiles.insert(thumb.path_key.clone(), (thumb, tick)) {
                    delete_bitmap(&old);
                }
            }
            _ => delete_bitmap(&thumb),
        }
    }
}

/// Libera todas las tarjetas grandes y descarta las peticiones en curso.
pub fn clear_tiles() {
    GENERATION.fetch_add(1, Ordering::AcqRel);
    if let Ok(mut pending) = PENDING.lock() { *pending = None; }
    if let Ok(mut ready) = READY.lock() {
        for (thumb, _) in ready.drain(..) { delete_bitmap(&thumb); }
    }
    let released = TILES.lock().ok().and_then(|mut cache| cache.take()).map(|cache| {
        for (thumb, _) in cache.tiles.values() { delete_bitmap(thumb); }
        cache.tiles.len()
    }).unwrap_or(0);
    if released > 0 {
        log_info!("Carrusel: {} tarjetas liberadas", released);
    }
}

/// Pide las tarjetas que faltan y olvida las peticiones que ya no hacen falta.
fn request_missing(images: &[ScannedImage], wanted: std::ops::Range<usize>, layout: &Layout, notify: HWND) {
    let wanted_keys: Vec<(String, &Path)> = images[wanted]
        .iter()
        .map(|img| (img.path.to_string_lossy().to_string(), img.path.as_path()))
        .collect();
    let missing: Vec<(String, &Path)> = {
        let Ok(cache) = TILES.lock() else { return; };
        let Ok(mut pending) = PENDING.lock() else { return; };
        let set = pending.get_or_insert_with(HashSet::new);
        set.retain(|key| wanted_keys.iter().any(|(k, _)| k == key));
        wanted_keys
            .into_iter()
            .filter(|(key, _)| !cache.as_ref().is_some_and(|c| c.tiles.contains_key(key)) && set.insert(key.clone()))
            .collect()
    };
    if missing.is_empty() { return; }
    let Some(sender) = worker_sender() else { return; };
    let generation = GENERATION.load(Ordering::Acquire);
    for (key, path) in missing {
        let _ = sender.send(TileRequest {
            path: path.to_path_buf(),
            key,
            width: layout.bounds_w(),
            height: layout.tile_h,
            generation,
            notify: notify.0 as isize,
        });
    }
}

/// Deja en caché solo las tarjetas usadas más recientemente.
fn evict(capacity: usize) {
    if let Ok(mut guard) = TILES.lock() {
        if let Some(cache) = guard.as_mut() {
            while cache.tiles.len() > capacity {
                let Some(oldest) = cache.tiles.iter().min_by_key(|(_, (_, tick))| *tick).map(|(key, _)| key.clone()) else { break; };
                if let Some((thumb, _)) = cache.tiles.remove(&oldest) { delete_bitmap(&thumb); }
            }
        }
    }
}

// --- Pintado ---

pub struct PaintInput<'a> {
    pub images: &'a [ScannedImage],
    pub small_thumbs: Option<&'a ThumbnailCache>,
    pub scroll: i32,
    pub focus: Option<usize>,
    pub current: Option<&'a Path>,
    pub scales: &'a [f32],
    pub notify: HWND,
}

pub unsafe fn paint(dc: HDC, width: i32, height: i32, input: &PaintInput) {
    let key_brush = CreateSolidBrush(COLOR_KEY);
    let _ = FillRect(dc, &RECT { left: 0, top: 0, right: width, bottom: height }, key_brush);
    let _ = DeleteObject(key_brush);

    let Some(layout) = layout() else { return; };
    let count = input.images.len();
    let visible = layout.visible_range(count, width, input.scroll);
    if visible.is_empty() { return; }

    let wanted = visible.start.saturating_sub(PREFETCH)..(visible.end + PREFETCH).min(count);
    request_missing(input.images, wanted.clone(), &layout, input.notify);
    evict(wanted.len() + 4);

    if let Ok(mut guard) = TILES.lock() {
        if let Some(cache) = guard.as_mut() {
            cache.tick += 1;
            let tick = cache.tick;
            for img in &input.images[visible.clone()] {
                if let Some(entry) = cache.tiles.get_mut(img.path.to_string_lossy().as_ref()) { entry.1 = tick; }
            }
        }
    }

    let scale_of = |index: usize| input.scales.get(index).copied().unwrap_or(1.0);
    let mut order: Vec<usize> = visible.collect();
    order.sort_by(|a, b| scale_of(*a).total_cmp(&scale_of(*b)));

    let src_dc = CreateCompatibleDC(dc);
    let shade_dc = CreateCompatibleDC(dc);
    let shade_bmp = CreateCompatibleBitmap(dc, 1, 1);
    let old_shade = SelectObject(shade_dc, shade_bmp);
    let black = CreateSolidBrush(COLORREF(0));
    let _ = FillRect(shade_dc, &RECT { left: 0, top: 0, right: 1, bottom: 1 }, black);
    let _ = DeleteObject(black);
    let tiles = TILES.lock().ok();

    for index in order {
        let img = &input.images[index];
        let scale = scale_of(index);
        let focus_amount = ((scale - 1.0) / (FOCUS_SCALE - 1.0)).clamp(0.0, 1.0);
        let x = layout.tile_x(index, count, width, input.scroll);
        let (points, rect) = layout.polygon(x, scale);
        let (dest_w, dest_h) = (rect.right - rect.left, rect.bottom - rect.top);
        let region = CreatePolygonRgn(&points, ALTERNATE);
        let _ = SelectClipRgn(dc, region);

        let key = img.path.to_string_lossy();
        let tile = tiles.as_ref().and_then(|t| t.as_ref()).and_then(|c| c.tiles.get(key.as_ref())).map(|(t, _)| t.clone());
        if let Some(tile) = tile {
            let old = SelectObject(src_dc, HGDIOBJ(tile.hbitmap as *mut _));
            if dest_w == tile.width && dest_h == tile.height {
                let _ = BitBlt(dc, rect.left, rect.top, dest_w, dest_h, src_dc, 0, 0, SRCCOPY);
            } else {
                let _ = SetStretchBltMode(dc, COLORONCOLOR);
                let _ = StretchBlt(dc, rect.left, rect.top, dest_w, dest_h, src_dc, 0, 0, tile.width, tile.height, SRCCOPY);
            }
            SelectObject(src_dc, old);
        } else if let Some(thumb) = input.small_thumbs.and_then(|c| c.get(key.as_ref())) {
            let (crop_x, crop_y, crop_w, crop_h) = if thumb.width * dest_h > thumb.height * dest_w {
                let crop_w = (thumb.height * dest_w / dest_h).max(1);
                ((thumb.width - crop_w) / 2, 0, crop_w, thumb.height)
            } else {
                let crop_h = (thumb.width * dest_h / dest_w).max(1);
                (0, (thumb.height - crop_h) / 2, thumb.width, crop_h)
            };
            let old = SelectObject(src_dc, HBITMAP(thumb.hbitmap as *mut _));
            let _ = SetStretchBltMode(dc, COLORONCOLOR);
            let _ = StretchBlt(dc, rect.left, rect.top, dest_w, dest_h, src_dc, crop_x, crop_y, crop_w, crop_h, SRCCOPY);
            SelectObject(src_dc, old);
        } else {
            let placeholder = CreateSolidBrush(super::COLOR_PLACEHOLDER);
            let _ = FillRect(dc, &rect, placeholder);
            let _ = DeleteObject(placeholder);
        }

        // Oscurecer las tarjetas que no tienen el foco.
        let dim = (DIM_ALPHA * (1.0 - focus_amount)) as u8;
        if dim > 0 {
            let blend = BLENDFUNCTION { BlendOp: 0, BlendFlags: 0, SourceConstantAlpha: dim, AlphaFormat: 0 };
            let _ = AlphaBlend(dc, rect.left, rect.top, dest_w, dest_h, shade_dc, 0, 0, 1, 1, blend);
        }
        let _ = SelectClipRgn(dc, HRGN(std::ptr::null_mut()));

        let is_current = input.current.is_some_and(|p| p == img.path);
        let frame = if input.focus == Some(index) && focus_amount > 0.5 {
            Some(super::COLOR_FOCUS)
        } else if is_current {
            Some(super::COLOR_CURRENT)
        } else {
            None
        };
        if let Some(color) = frame {
            let brush = CreateSolidBrush(color);
            let _ = FrameRgn(dc, region, brush, 2, 2);
            let _ = DeleteObject(brush);
        }
        let _ = DeleteObject(region);
        if img.is_new {
            super::draw_new_badge(dc, points[1].x, rect.top);
        }
    }

    drop(tiles);
    SelectObject(shade_dc, old_shade);
    let _ = DeleteObject(shade_bmp);
    let _ = DeleteDC(shade_dc);
    let _ = DeleteDC(src_dc);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hit_test_follows_slanted_edges() {
        let layout = Layout::for_monitor(1080);
        let (count, viewport) = (20, 1920);
        let x = layout.tile_x(3, count, viewport, 0);
        let bottom = layout.pad_y + layout.tile_h - 1;
        // Abajo la tarjeta empieza en x; arriba está desplazada `slant` a la derecha.
        assert_eq!(layout.hit_test(x + 1, bottom, count, viewport, 0), Some(3));
        assert_eq!(layout.hit_test(x + 1, layout.pad_y, count, viewport, 0), Some(2));
        assert_eq!(layout.hit_test(x + layout.slant + 1, layout.pad_y, count, viewport, 0), Some(3));
        // El hueco entre tarjetas no es de ninguna.
        assert_eq!(layout.hit_test(x - 1, bottom, count, viewport, 0), None);
        assert_eq!(layout.hit_test(x, layout.pad_y - 1, count, viewport, 0), None);
    }

    #[test]
    fn center_scroll_is_clamped_and_centers() {
        let layout = Layout::for_monitor(1080);
        let (count, viewport) = (30, 1920);
        assert_eq!(layout.center_scroll(0, count, viewport), 0);
        assert_eq!(layout.center_scroll(count - 1, count, viewport), layout.max_scroll(count, viewport));
        let scroll = layout.center_scroll(10, count, viewport);
        let center = layout.tile_x(10, count, viewport, scroll) + layout.bounds_w() / 2;
        assert!((center - viewport / 2).abs() <= 1);
    }

    #[test]
    fn visible_range_covers_partially_visible_tiles() {
        let layout = Layout::for_monitor(1080);
        let (count, viewport) = (30, 1920);
        let scroll = layout.center_scroll(12, count, viewport);
        let range = layout.visible_range(count, viewport, scroll);
        for index in 0..count {
            let x = layout.tile_x(index, count, viewport, scroll);
            let on_screen = x + layout.bounds_w() > 0 && x < viewport;
            assert_eq!(range.contains(&index), on_screen, "tarjeta {index}");
        }
        // Pocas imágenes: el contenido se centra y todas son visibles.
        assert_eq!(layout.visible_range(3, viewport, 0), 0..3);
    }

    /// Pinta el carrusel con imágenes reales en un BMP para revisarlo a ojo:
    /// `SWEPR_TEST_DIR=<carpeta> SWEPR_TEST_OUT=<archivo.bmp> cargo test render_preview -- --ignored`
    #[test]
    #[ignore]
    fn render_preview() {
        let dir = std::env::var("SWEPR_TEST_DIR").expect("SWEPR_TEST_DIR");
        let out = std::env::var("SWEPR_TEST_OUT").expect("SWEPR_TEST_OUT");
        let mut paths: Vec<PathBuf> = std::fs::read_dir(dir).unwrap().flatten().map(|e| e.path())
            .filter(|p| p.extension().is_some_and(|e| ["jpg", "jpeg", "png"].contains(&e.to_string_lossy().to_lowercase().as_str())))
            .collect();
        paths.sort();
        paths.truncate(14);
        let images: Vec<ScannedImage> = paths.iter().enumerate().map(|(i, p)| ScannedImage {
            path: p.clone(),
            file_name: String::new(),
            is_new: i == 1,
        }).collect();
        let layout = Layout::for_monitor(1080);
        set_layout(layout);
        let (width, height) = (1920, layout.window_height());
        unsafe {
            use windows::Win32::System::Com::{CoInitializeEx, COINIT_APARTMENTTHREADED};
            let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
            let started = std::time::Instant::now();
            let mut cache = TileCache { tiles: HashMap::new(), tick: 0 };
            for img in &images {
                let thumb = crate::thumbs::load_cover_bitmap(&img.path, layout.bounds_w(), layout.tile_h).expect("tarjeta");
                cache.tiles.insert(thumb.path_key.clone(), (thumb, 0));
            }
            println!("{} tarjetas en {} ms", images.len(), started.elapsed().as_millis());
            *TILES.lock().unwrap() = Some(cache);

            let focus = 5;
            let scales: Vec<f32> = (0..images.len()).map(|i| if i == focus { FOCUS_SCALE } else { 1.0 }).collect();
            let screen = windows::Win32::Graphics::Gdi::GetDC(None);
            let dc = CreateCompatibleDC(screen);
            let bmp = CreateCompatibleBitmap(screen, width, height);
            let old = SelectObject(dc, bmp);
            paint(dc, width, height, &PaintInput {
                images: &images,
                small_thumbs: None,
                scroll: layout.center_scroll(focus, images.len(), width),
                focus: Some(focus),
                current: images.get(2).map(|i| i.path.as_path()),
                scales: &scales,
                notify: HWND::default(),
            });
            SelectObject(dc, old);

            crate::panel::save_bmp(dc, bmp, width, height, &out);
            let _ = DeleteObject(bmp);
            let _ = DeleteDC(dc);
            clear_tiles();
        }
    }
}
