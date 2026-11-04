mod carousel;

use crate::app;
use crate::config::{PanelMode, PanelSide};
use crate::log_info;
use crate::scanner::ScannedImage;
use crate::thumbs::ThumbnailCache;
use crate::winutil::{as_pcwstr, to_wide_chars};
use std::sync::{Arc, Mutex};
use windows::Win32::Foundation::{COLORREF, HINSTANCE, HWND, LPARAM, LRESULT, POINT, RECT, WPARAM};
use windows::Win32::Graphics::Gdi::{
    BeginPaint, BitBlt, CreateCompatibleBitmap, CreateCompatibleDC, CreateRoundRectRgn, CreateSolidBrush, DeleteDC, DeleteObject, EndPaint, FillRect, FrameRgn, GetMonitorInfoW, MonitorFromPoint, SelectClipRgn, SelectObject, SetStretchBltMode, StretchBlt, COLORONCOLOR, MONITOR_DEFAULTTONEAREST, MONITORINFO, SRCCOPY,
};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, GetCursorPos, GetWindowRect, KillTimer, LoadCursorW, RegisterClassW, SetCursor, SetTimer, SetWindowPos, ShowWindow,
    CS_HREDRAW, CS_VREDRAW, HWND_TOPMOST, IDC_ARROW, SWP_NOACTIVATE, SWP_NOZORDER, SW_SHOW, WM_CREATE, WM_DESTROY, WM_ERASEBKGND, WM_KEYDOWN, WM_KILLFOCUS, WM_LBUTTONDOWN, WM_LBUTTONUP, WM_MOUSEMOVE, WM_NCHITTEST, WM_PAINT, WM_SETCURSOR, WM_TIMER, WNDCLASSW,
    WS_EX_TOOLWINDOW, WS_EX_TOPMOST, WS_POPUP,
};

const PANEL_CLASS_NAME: &str = "SWEPR_PanelWindowClass";
const ANIM_TIMER_ID: usize = 8001;
const AUTO_CLOSE_TIMER_ID: usize = 8002;
const ANIM_DURATION_MS: i32 = 200;
const ANIM_STEP_MS: u32 = 16;
const SENSOR_CLOSE_CHECK_MS: u32 = 100;
const HOTKEY_CLICK_CHECK_MS: u32 = 10;
// Medidas a 96 DPI; se escalan con el DPI del monitor donde se abre el panel.
const BASE_THUMB_W: i32 = 160;
const BASE_THUMB_H: i32 = 90;
const BASE_THUMB_MARGIN: i32 = 8;
const BASE_SCROLLBAR: i32 = 3;
const BASE_CORNER_RADIUS: i32 = 8;
const BASE_CLOSE_GRACE: i32 = 12;

// Paleta del panel (COLORREF en formato 0x00BBGGRR).
const COLOR_BACKGROUND: COLORREF = COLORREF(0x00181818);
const COLOR_PLACEHOLDER: COLORREF = COLORREF(0x00303030);
const COLOR_SCROLL_TRACK: COLORREF = COLORREF(0x00404040);
const COLOR_SCROLL_THUMB: COLORREF = COLORREF(0x00FFFFFF);
const COLOR_TEXT: COLORREF = COLORREF(0x00D0D0D0);
/// Miniatura con el foco (ratón o teclado).
const COLOR_FOCUS: COLORREF = COLORREF(0x00FFFFFF);
/// Fondo de pantalla aplicado ahora (azul acento).
const COLOR_CURRENT: COLORREF = COLORREF(0x00FFC24C);
/// Punto de las imágenes recién descubiertas (verde).
const COLOR_NEW: COLORREF = COLORREF(0x005FCB6C);
const BASE_NEW_BADGE: i32 = 10;

static PANEL_DPI: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(96);

fn px(value: i32) -> i32 {
    value * PANEL_DPI.load(std::sync::atomic::Ordering::Relaxed) as i32 / 96
}
fn thumb_w() -> i32 { px(BASE_THUMB_W) }
fn thumb_h() -> i32 { px(BASE_THUMB_H) }
fn thumb_margin() -> i32 { px(BASE_THUMB_MARGIN) }
fn scrollbar_size() -> i32 { px(BASE_SCROLLBAR).max(1) }

const HOVER_ANIM_TIMER_ID: usize = 8003;
const HOVER_ANIM_STEP_MS: u32 = 16;
/// Tras cerrar el carrusel, sus tarjetas grandes se liberan pasado este tiempo.
const TILE_RELEASE_TIMER_ID: usize = 8004;
const TILE_RELEASE_DELAY_MS: u32 = 30_000;
/// Desplazamiento vertical de la animación de entrada/salida del carrusel.
const CAROUSEL_SLIDE_PX: i32 = 24;


#[derive(Clone)]
struct PanelState {
    target_x: i32,
    start_x: i32,
    current_x: i32,
    target_y: i32,
    start_y: i32,
    current_y: i32,
    anim_start_time: std::time::Instant,
    animating: bool,
}



static PANEL_STATE: Mutex<Option<PanelState>> = Mutex::new(None);
static PANEL_HWND: Mutex<Option<isize>> = Mutex::new(None);
static PANEL_VISIBLE: Mutex<bool> = Mutex::new(false);
static PANEL_EXITING: Mutex<bool> = Mutex::new(false);
static PANEL_CLOSE_ON_MOUSE_LEAVE: Mutex<bool> = Mutex::new(true);
static PANEL_SIDE: Mutex<PanelSide> = Mutex::new(PanelSide::Top);
static PANEL_MODE: Mutex<PanelMode> = Mutex::new(PanelMode::LowPower);
static PANEL_SHOW_SCROLLBAR: Mutex<bool> = Mutex::new(true);
static PANEL_MARGIN: Mutex<u32> = Mutex::new(0);
static PANEL_WIDTH: Mutex<i32> = Mutex::new(800);
static PANEL_HEIGHT: Mutex<i32> = Mutex::new(150);

static PANEL_THUMB_CACHE: Mutex<Option<ThumbnailCache>> = Mutex::new(None);
static PANEL_SCANNED: Mutex<Option<Arc<Mutex<Vec<ScannedImage>>>>> = Mutex::new(None);
static PANEL_SCROLL_OFFSET: Mutex<i32> = Mutex::new(0);
static PANEL_HOVER_INDEX: Mutex<Option<usize>> = Mutex::new(None);
static PANEL_SELECTED_INDEX: Mutex<Option<usize>> = Mutex::new(None);
static PANEL_DRAGGING: Mutex<bool> = Mutex::new(false);
static PANEL_LAST_MOUSE_X: Mutex<i32> = Mutex::new(0);
static PANEL_DRAG_START_X: Mutex<i32> = Mutex::new(0);
static PANEL_DRAG_START_Y: Mutex<i32> = Mutex::new(0);
static PANEL_THUMB_SCALES: Mutex<Vec<f32>> = Mutex::new(Vec::new());
static PANEL_THUMB_TARGETS: Mutex<Vec<f32>> = Mutex::new(Vec::new());
static PANEL_THUMB_ANIM: Mutex<bool> = Mutex::new(false);
static PANEL_CURRENT_WALLPAPER: Mutex<Option<std::path::PathBuf>> = Mutex::new(None);
/// Scroll hacia el que se anima el carrusel al cambiar la selección.
static PANEL_SCROLL_TARGET: Mutex<Option<i32>> = Mutex::new(None);

fn is_carousel() -> bool {
    PANEL_MODE.lock().map(|mode| *mode == PanelMode::Pro).unwrap_or(false)
}

fn is_vertical() -> bool {
    !is_carousel() && PANEL_SIDE.lock().map(|side| matches!(*side, PanelSide::Left | PanelSide::Right)).unwrap_or(false)
}

/// Escala de la miniatura con foco: el carrusel la agranda, las tiras la encogen.
fn focus_scale() -> f32 {
    if is_carousel() { carousel::FOCUS_SCALE } else { 0.9 }
}

fn image_count() -> usize {
    PANEL_SCANNED.lock().ok().and_then(|g| g.clone()).and_then(|arc| arc.lock().ok().map(|v| v.len())).unwrap_or(0)
}

fn viewport() -> i32 {
    if is_vertical() {
        PANEL_HEIGHT.lock().map(|g| *g).unwrap_or(600)
    } else {
        PANEL_WIDTH.lock().map(|g| *g).unwrap_or(800)
    }
}

fn max_scroll(count: usize) -> i32 {
    if let Some(layout) = carousel::layout().filter(|_| is_carousel()) {
        return layout.max_scroll(count, viewport());
    }
    (thumbnail_content_extent(count, is_vertical()) - viewport()).max(0)
}

/// Índice de la miniatura bajo un punto en coordenadas de cliente.
fn thumbnail_at(x: i32, y: i32) -> Option<usize> {
    let count = image_count();
    let scroll = PANEL_SCROLL_OFFSET.lock().map(|g| *g).unwrap_or(0);
    if let Some(layout) = carousel::layout().filter(|_| is_carousel()) {
        return layout.hit_test(x, y, count, viewport(), scroll);
    }
    let vertical = is_vertical();
    (0..count).find(|&idx| {
        let (tx, ty) = thumbnail_origin(idx, count, viewport(), scroll, vertical);
        x >= tx && x < tx + thumb_w() && y >= ty && y < ty + thumb_h()
    })
}

/// Selecciona una miniatura y desplaza la vista para que se vea (centrada en el carrusel).
fn select_index(hwnd: HWND, sel: usize) {
    if let Ok(mut g) = PANEL_SELECTED_INDEX.lock() { *g = Some(sel); }
    set_hover_target(hwnd, Some(sel));
    let count = image_count();
    if let Some(layout) = carousel::layout().filter(|_| is_carousel()) {
        if let Ok(mut target) = PANEL_SCROLL_TARGET.lock() { *target = Some(layout.center_scroll(sel, count, viewport())); }
        unsafe { let _ = SetTimer(hwnd, HOVER_ANIM_TIMER_ID, HOVER_ANIM_STEP_MS, None); }
        return;
    }
    let vertical = is_vertical();
    let viewport = viewport();
    let scroll = PANEL_SCROLL_OFFSET.lock().map(|g| *g).unwrap_or(0);
    let item_extent = if vertical { thumb_h() + thumb_margin() } else { thumb_w() + thumb_margin() };
    let item_size = if vertical { thumb_h() } else { thumb_w() };
    let item_x = sel as i32 * item_extent + thumb_margin();
    let mut new_scroll = scroll;
    if item_x < scroll { new_scroll = item_x.saturating_sub(thumb_margin()); }
    let right_edge = item_x + item_size;
    if right_edge > scroll + viewport { new_scroll = right_edge - viewport + thumb_margin(); }
    if let Ok(mut g) = PANEL_SCROLL_OFFSET.lock() { *g = new_scroll.max(0); }
    request_repaint();
}

unsafe fn set_panel_alpha(hwnd: HWND, alpha: u8) {
    use windows::Win32::UI::WindowsAndMessaging::{SetLayeredWindowAttributes, LWA_ALPHA, LWA_COLORKEY};
    if is_carousel() {
        let _ = SetLayeredWindowAttributes(hwnd, carousel::COLOR_KEY, alpha, LWA_COLORKEY | LWA_ALPHA);
    } else {
        let _ = SetLayeredWindowAttributes(hwnd, windows::Win32::Foundation::COLORREF(0), alpha, LWA_ALPHA);
    }
}

/// Punto verde en la esquina superior derecha de una miniatura nueva.
unsafe fn draw_new_badge(dc: windows::Win32::Graphics::Gdi::HDC, right: i32, top: i32) {
    use windows::Win32::Graphics::Gdi::{CreateEllipticRgn, FillRgn};
    let size = px(BASE_NEW_BADGE);
    let inset = px(6);
    let rgn = CreateEllipticRgn(right - inset - size, top + inset, right - inset, top + inset + size);
    let brush = CreateSolidBrush(COLOR_NEW);
    let _ = FillRgn(dc, rgn, brush);
    let _ = DeleteObject(brush);
    let _ = DeleteObject(rgn);
}

/// Aviso centrado cuando la carpeta no tiene imágenes.
unsafe fn draw_empty_state(dc: windows::Win32::Graphics::Gdi::HDC, width: i32, height: i32) {
    use windows::Win32::Graphics::Gdi::{CreateFontW, DrawTextW, SetBkMode, SetTextColor, DT_CENTER, DT_SINGLELINE, DT_VCENTER, DT_WORDBREAK, FW_NORMAL, TRANSPARENT};
    let box_w = px(420).min(width);
    let box_h = px(64).min(height);
    let mut rect = RECT { left: (width - box_w) / 2, top: (height - box_h) / 2, right: (width + box_w) / 2, bottom: (height + box_h) / 2 };
    let bg = CreateSolidBrush(COLOR_BACKGROUND);
    let _ = FillRect(dc, &rect, bg);
    let _ = DeleteObject(bg);
    let face = to_wide_chars("Segoe UI");
    let font = CreateFontW(-px(15), 0, 0, 0, FW_NORMAL.0 as i32, 0, 0, 0, Default::default(), Default::default(), Default::default(), Default::default(), 0, as_pcwstr(&face));
    let old_font = SelectObject(dc, font);
    let _ = SetBkMode(dc, TRANSPARENT);
    let _ = SetTextColor(dc, COLOR_TEXT);
    let mut text: Vec<u16> = crate::i18n::t("No wallpapers found in the folder").encode_utf16().collect();
    let flags = if vertical_text_wrap(width) { DT_CENTER | DT_WORDBREAK } else { DT_CENTER | DT_VCENTER | DT_SINGLELINE };
    let _ = DrawTextW(dc, &mut text, &mut rect, flags);
    SelectObject(dc, old_font);
    let _ = DeleteObject(font);
}

/// En la tira vertical el texto no cabe en una línea.
fn vertical_text_wrap(width: i32) -> bool {
    width < px(420)
}

fn effective_panel_margin(side: PanelSide, requested: u32, monitor_width: i32, monitor_height: i32) -> i32 {
    let requested = requested.min(i32::MAX as u32) as i32;
    let max_margin = match side {
        PanelSide::Top => monitor_height - (thumb_h() + thumb_margin() * 2 + scrollbar_size()),
        PanelSide::Left | PanelSide::Right => monitor_width - (thumb_w() + thumb_margin() * 2 + scrollbar_size()),
    };
    requested.min(max_margin.max(0))
}

fn thumbnail_content_extent(count: usize, vertical: bool) -> i32 {
    if count == 0 { return 0; }
    let item_size = if vertical { thumb_h() } else { thumb_w() };
    count as i32 * item_size + (count as i32 - 1) * thumb_margin() + thumb_margin() * 2
}

fn thumbnail_origin(index: usize, count: usize, viewport: i32, scroll: i32, vertical: bool) -> (i32, i32) {
    if count == 0 { return (thumb_margin(), thumb_margin()); }
    let item_size = if vertical { thumb_h() } else { thumb_w() };
    let item_extent = item_size + thumb_margin();
    let content_size = count as i32 * item_size + (count as i32 - 1) * thumb_margin();
    let origin = if thumbnail_content_extent(count, vertical) <= viewport {
        (viewport - content_size) / 2
    } else {
        thumb_margin()
    } + index as i32 * item_extent - scroll;
    if vertical { (thumb_margin(), origin) } else { (origin, thumb_margin()) }
}

/// Actualiza el objetivo de animación hover/foco para las miniaturas e inicia el temporizador de interpolación.
fn set_hover_target(hwnd: HWND, new_hover: Option<usize>) {
    let prev_hover = if let Ok(g) = PANEL_HOVER_INDEX.lock() { *g } else { None };
    if prev_hover == new_hover {
        return;
    }
    if let Ok(mut g) = PANEL_HOVER_INDEX.lock() { *g = new_hover; }

    let n = if let Ok(g) = PANEL_SCANNED.lock() {
        if let Some(arc) = g.clone() {
            if let Ok(v) = arc.lock() { v.len() } else { 0 }
        } else { 0 }
    } else { 0 };

    if n > 0 {
        if let Ok(mut s) = PANEL_THUMB_SCALES.lock() {
            s.resize(n, 1.0);
        }
        if let Ok(mut t) = PANEL_THUMB_TARGETS.lock() {
            t.resize(n, 1.0);
            for v in t.iter_mut() { *v = 1.0; }
            if let Some(idx) = new_hover {
                if idx < t.len() { t[idx] = focus_scale(); }
            }
        }
        if let Ok(mut anim) = PANEL_THUMB_ANIM.lock() { *anim = true; }
        unsafe {
            let _ = SetTimer(hwnd, HOVER_ANIM_TIMER_ID, HOVER_ANIM_STEP_MS, None);
        }
    }
}

/// Pinta el contenido del panel (fondo, miniaturas o carrusel, barra y avisos) en `dc`.
unsafe fn paint_contents(dc: windows::Win32::Graphics::Gdi::HDC, hwnd: HWND, width: i32, height: i32) {
    let bg = CreateSolidBrush(COLOR_BACKGROUND);
    let _ = FillRect(dc, &RECT { left: 0, top: 0, right: width, bottom: height }, bg);
    let _ = DeleteObject(bg);

    let cache_opt = if let Ok(g) = PANEL_THUMB_CACHE.lock() { g.clone() } else { None };
    let images_opt = if let Ok(g) = PANEL_SCANNED.lock() { g.clone() } else { None };
    let scroll = if let Ok(g) = PANEL_SCROLL_OFFSET.lock() { *g } else { 0 };
    let vertical = is_vertical();
    let hover = if let Ok(g) = PANEL_HOVER_INDEX.lock() { *g } else { None };
    let current_wp = if let Ok(g) = PANEL_CURRENT_WALLPAPER.lock() { g.clone() } else { None };
    let selected = if let Ok(g) = PANEL_SELECTED_INDEX.lock() { *g } else { None };

    if is_carousel() {
        if let Some(images) = images_opt.as_ref().and_then(|arc| arc.lock().ok()) {
            let scales = PANEL_THUMB_SCALES.lock().map(|v| v.clone()).unwrap_or_default();
            carousel::paint(dc, width, height, &carousel::PaintInput {
                images: &images,
                small_thumbs: cache_opt.as_ref(),
                scroll,
                focus: hover,
                current: current_wp.as_deref(),
                scales: &scales,
                notify: hwnd,
            });
        }
    } else if let (Some(cache), Some(images_arc)) = (cache_opt, images_opt) {
        if let Ok(images) = images_arc.lock() {
            let focus = hover.or(selected);
            let viewport = if vertical { height } else { width };
            let radius = px(BASE_CORNER_RADIUS);
            let bmp_dc = CreateCompatibleDC(dc);
            let _ = SetStretchBltMode(dc, COLORONCOLOR);
            for (idx, img) in images.iter().enumerate() {
                let (base_x, base_y) = thumbnail_origin(idx, images.len(), viewport, scroll, vertical);
                // Solo se pintan las miniaturas que caen dentro de la ventana.
                let (start, size) = if vertical { (base_y, thumb_h()) } else { (base_x, thumb_w()) };
                if start + size <= 0 || start >= viewport { continue; }
                let key = img.path.to_string_lossy().to_string();
                let scale = PANEL_THUMB_SCALES.lock().ok().and_then(|v| v.get(idx).copied()).unwrap_or(1.0);
                let draw_w = ((thumb_w() as f32 * scale) as i32).max(1);
                let draw_h = ((thumb_h() as f32 * scale) as i32).max(1);
                let draw_x = base_x + (thumb_w() - draw_w) / 2;
                let draw_y = base_y + (thumb_h() - draw_h) / 2;
                let rgn = CreateRoundRectRgn(draw_x, draw_y, draw_x + draw_w, draw_y + draw_h, radius * 2, radius * 2);
                let _ = SelectClipRgn(dc, rgn);
                if let Some(thumb) = cache.get(&key) {
                    let old = SelectObject(bmp_dc, windows::Win32::Graphics::Gdi::HBITMAP(thumb.hbitmap as *mut _));
                    let (src_x, src_y, src_w, src_h) = if thumb.width * draw_h > thumb.height * draw_w {
                        let crop_w = (thumb.height * draw_w / draw_h).max(1);
                        ((thumb.width - crop_w) / 2, 0, crop_w, thumb.height)
                    } else {
                        let crop_h = (thumb.width * draw_h / draw_w).max(1);
                        (0, (thumb.height - crop_h) / 2, thumb.width, crop_h)
                    };
                    let _ = StretchBlt(dc, draw_x, draw_y, draw_w, draw_h, bmp_dc, src_x, src_y, src_w, src_h, SRCCOPY);
                    SelectObject(bmp_dc, old);
                } else {
                    let ph = CreateSolidBrush(COLOR_PLACEHOLDER);
                    let r = RECT { left: draw_x, top: draw_y, right: draw_x + draw_w, bottom: draw_y + draw_h };
                    let _ = FillRect(dc, &r, ph);
                    let _ = DeleteObject(ph);
                }
                let _ = SelectClipRgn(dc, windows::Win32::Graphics::Gdi::HRGN(std::ptr::null_mut()));
                let is_current = current_wp.as_ref().map(|p| p == &img.path).unwrap_or(false);
                let frame = if focus == Some(idx) {
                    Some(COLOR_FOCUS)
                } else if is_current {
                    Some(COLOR_CURRENT)
                } else {
                    None
                };
                if let Some(color) = frame {
                    let brush = CreateSolidBrush(color);
                    let _ = FrameRgn(dc, rgn, brush, 2, 2);
                    let _ = DeleteObject(brush);
                }
                let _ = DeleteObject(rgn);
                if img.is_new {
                    draw_new_badge(dc, draw_x + draw_w, draw_y);
                }
            }
            let _ = DeleteDC(bmp_dc);
            let total_main = thumbnail_content_extent(images.len(), vertical);
            let show_scrollbar = PANEL_SHOW_SCROLLBAR.lock().map(|show| *show).unwrap_or(true);
            if show_scrollbar && total_main > viewport {
                let track_brush = CreateSolidBrush(COLOR_SCROLL_TRACK);
                let thumb_brush = CreateSolidBrush(COLOR_SCROLL_THUMB);
                let max_scroll = (total_main - viewport).max(1);
                if vertical {
                    let track_x = thumb_margin() + thumb_w() + thumb_margin();
                    let track_rect = RECT { left: track_x, top: 0, right: track_x + scrollbar_size(), bottom: height };
                    let _ = FillRect(dc, &track_rect, track_brush);
                    let thumb_h = (((height as f32 / total_main as f32) * height as f32) as i32).max(20).min(height);
                    let thumb_y = ((scroll as f32 / max_scroll as f32) * (height - thumb_h) as f32) as i32;
                    let thumb_rect = RECT { left: track_x, top: thumb_y, right: track_x + scrollbar_size(), bottom: thumb_y + thumb_h };
                    let _ = FillRect(dc, &thumb_rect, thumb_brush);
                } else {
                    let track_y = thumb_margin() + thumb_h() + thumb_margin();
                    let track_rect = RECT { left: 0, top: track_y, right: width, bottom: track_y + scrollbar_size() };
                    let _ = FillRect(dc, &track_rect, track_brush);
                    let thumb_w = (((width as f32 / total_main as f32) * width as f32) as i32).max(20).min(width);
                    let thumb_x = ((scroll as f32 / max_scroll as f32) * (width - thumb_w) as f32) as i32;
                    let thumb_rect = RECT { left: thumb_x, top: track_y, right: thumb_x + thumb_w, bottom: track_y + scrollbar_size() };
                    let _ = FillRect(dc, &thumb_rect, thumb_brush);
                }
                let _ = DeleteObject(track_brush);
                let _ = DeleteObject(thumb_brush);
            }
        }
    }

    if image_count() == 0 {
        draw_empty_state(dc, width, height);
    }
}

unsafe extern "system" fn panel_wnd_proc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    match msg {
        WM_CREATE => {
            log_info!("Panel: ventana creada");
            LRESULT(0)
        }
        WM_TIMER => {
            if wparam.0 == ANIM_TIMER_ID {
                let mut should_hide = false;
                if let Ok(mut stg) = PANEL_STATE.lock() {
                    if let Some(ref mut st) = *stg {
                        if st.animating {
                            let elapsed = st.anim_start_time.elapsed().as_millis() as f32;
                            let progress = (elapsed / (ANIM_DURATION_MS as f32)).min(1.0);
                            let eased = 1.0 - (1.0 - progress).powi(3);
                            let x = st.start_x + ((st.target_x - st.start_x) as f32 * eased) as i32;
                            let y = st.start_y + ((st.target_y - st.start_y) as f32 * eased) as i32;
                            st.current_x = x;
                            st.current_y = y;
                            let pw = if let Ok(g) = PANEL_WIDTH.lock() { *g } else { 800 };
                            let ph = if let Ok(g) = PANEL_HEIGHT.lock() { *g } else { 150 };
                            let _ = SetWindowPos(hwnd, HWND_TOPMOST, x, y, pw, ph, SWP_NOACTIVATE | SWP_NOZORDER);
                            if is_carousel() {
                                let exiting = PANEL_EXITING.lock().map(|e| *e).unwrap_or(false);
                                let opacity = if exiting { 1.0 - eased } else { eased };
                                set_panel_alpha(hwnd, (opacity * 255.0) as u8);
                            }
                            if progress >= 1.0 {
                                st.animating = false;
                                let _ = KillTimer(hwnd, ANIM_TIMER_ID);
                                should_hide = if let Ok(e) = PANEL_EXITING.lock() { *e } else { false };
                            }
                        }
                    }
                }
                if should_hide {
                    let _ = windows::Win32::UI::WindowsAndMessaging::ShowWindow(hwnd, windows::Win32::UI::WindowsAndMessaging::SW_HIDE);
                    if let Ok(mut v) = PANEL_VISIBLE.lock() { *v = false; }
                    if let Ok(mut e) = PANEL_EXITING.lock() { *e = false; }
                    if let Ok(mut stg) = PANEL_STATE.lock() { *stg = None; }
                    if is_carousel() {
                        let _ = SetTimer(hwnd, TILE_RELEASE_TIMER_ID, TILE_RELEASE_DELAY_MS, None);
                    }
                }
                return LRESULT(0);
            }
            if wparam.0 == TILE_RELEASE_TIMER_ID {
                let _ = KillTimer(hwnd, TILE_RELEASE_TIMER_ID);
                carousel::clear_tiles();
                return LRESULT(0);
            }
            if wparam.0 == HOVER_ANIM_TIMER_ID {
                let mut done = true;
                if let Ok(mut target) = PANEL_SCROLL_TARGET.lock() {
                    if let Some(goal) = *target {
                        if let Ok(mut scroll) = PANEL_SCROLL_OFFSET.lock() {
                            let diff = goal - *scroll;
                            if diff.abs() <= 1 {
                                *scroll = goal;
                                *target = None;
                            } else {
                                let step = (diff as f32 * 0.25) as i32;
                                *scroll += if step == 0 { diff.signum() } else { step };
                                done = false;
                            }
                        }
                    }
                }
                if let (Ok(mut scales), Ok(targets)) = (PANEL_THUMB_SCALES.lock(), PANEL_THUMB_TARGETS.lock()) {
                    for i in 0..scales.len().min(targets.len()) {
                        let diff = targets[i] - scales[i];
                        if diff.abs() > 0.001 {
                            scales[i] += diff * 0.25;
                            done = false;
                        } else {
                            scales[i] = targets[i];
                        }
                    }
                }
                let _ = windows::Win32::Graphics::Gdi::InvalidateRect(hwnd, None, true);
                if done {
                    if let Ok(mut a) = PANEL_THUMB_ANIM.lock() { *a = false; }
                    let _ = KillTimer(hwnd, HOVER_ANIM_TIMER_ID);
                }
                return LRESULT(0);
            }
            if wparam.0 == AUTO_CLOSE_TIMER_ID {
                // El modo del sensor cierra al salir; el modo de atajo espera un clic fuera.
                let mut pt = POINT::default();
                let _ = GetCursorPos(&mut pt);
                let mut rect = RECT::default();
                let _ = GetWindowRect(hwnd, &mut rect);
                let close_on_leave = PANEL_CLOSE_ON_MOUSE_LEAVE.lock().map(|value| *value).unwrap_or(true);
                let mut inside = pt.x >= rect.left && pt.x < rect.right && pt.y >= rect.top && pt.y < rect.bottom;
                if inside && is_carousel() {
                    // Los huecos entre tarjetas son transparentes: un clic ahí cuenta como fuera.
                    inside = thumbnail_at(pt.x - rect.left, pt.y - rect.top).is_some();
                }
                if close_on_leave && !inside {
                    let side = PANEL_SIDE.lock().map(|value| *value).unwrap_or(PanelSide::Top);
                    let hmon = windows::Win32::Graphics::Gdi::MonitorFromWindow(hwnd, MONITOR_DEFAULTTONEAREST);
                    let mut monitor = MONITORINFO { cbSize: std::mem::size_of::<MONITORINFO>() as u32, ..Default::default() };
                    let _ = GetMonitorInfoW(hmon, &mut monitor);
                    let margin = PANEL_MARGIN.lock().map(|value| *value).unwrap_or(0);
                    let monitor_width = monitor.rcMonitor.right - monitor.rcMonitor.left;
                    let monitor_height = monitor.rcMonitor.bottom - monitor.rcMonitor.top;
                    let margin = effective_panel_margin(side, margin, monitor_width, monitor_height);
                    let grace = margin.saturating_add(px(BASE_CLOSE_GRACE));
                    inside = match side {
                        PanelSide::Top => {
                            pt.x >= rect.left && pt.x < rect.right
                                && pt.y >= monitor.rcMonitor.top
                                && pt.y < (monitor.rcMonitor.top + grace).min(monitor.rcMonitor.bottom)
                        }
                        PanelSide::Left => {
                            pt.y >= monitor.rcMonitor.top && pt.y < monitor.rcMonitor.bottom
                                && pt.x >= monitor.rcMonitor.left
                                && pt.x < (monitor.rcMonitor.left + grace).min(monitor.rcMonitor.right)
                        }
                        PanelSide::Right => {
                            pt.y >= monitor.rcMonitor.top && pt.y < monitor.rcMonitor.bottom
                                && pt.x >= (monitor.rcMonitor.right - grace).max(monitor.rcMonitor.left)
                                && pt.x < monitor.rcMonitor.right
                        }
                    };
                }
                let outside = !inside;
                let left_button_down = windows::Win32::UI::Input::KeyboardAndMouse::GetAsyncKeyState(
                    windows::Win32::UI::Input::KeyboardAndMouse::VK_LBUTTON.0 as i32,
                ) < 0;
                if outside && (close_on_leave || left_button_down) {
                    hide_panel();
                }
                return LRESULT(0);
            }
            DefWindowProcW(hwnd, msg, wparam, lparam)
        }
        WM_PAINT => {
            let mut ps = windows::Win32::Graphics::Gdi::PAINTSTRUCT::default();
            unsafe {
                let hdc = BeginPaint(hwnd, &mut ps);
                let mut rect = RECT::default();
                let _ = windows::Win32::UI::WindowsAndMessaging::GetClientRect(hwnd, &mut rect);
                let width = rect.right - rect.left;
                let height = rect.bottom - rect.top;

                let mem_dc = CreateCompatibleDC(hdc);
                let mem_bmp = CreateCompatibleBitmap(hdc, width, height);
                let old_bmp = SelectObject(mem_dc, mem_bmp);
                paint_contents(mem_dc, hwnd, width, height);

                let _ = BitBlt(hdc, 0, 0, width, height, mem_dc, 0, 0, SRCCOPY);
                SelectObject(mem_dc, old_bmp);
                let _ = DeleteObject(mem_bmp);
                let _ = DeleteDC(mem_dc);
                let _ = EndPaint(hwnd, &ps);
            }
            LRESULT(0)
        }
        WM_LBUTTONDOWN => {
            let x = (lparam.0 & 0xFFFF) as i16 as i32;
            let y = ((lparam.0 >> 16) & 0xFFFF) as i16 as i32;
            let vertical = is_vertical();
            if let Ok(mut g) = PANEL_DRAGGING.lock() { *g = true; }
            if let Ok(mut g) = PANEL_LAST_MOUSE_X.lock() { *g = if vertical { y } else { x }; }
            if let Ok(mut g) = PANEL_DRAG_START_X.lock() { *g = x; }
            if let Ok(mut g) = PANEL_DRAG_START_Y.lock() { *g = y; }
            LRESULT(0)
        }
        WM_LBUTTONUP => {
            let was_dragging = if let Ok(mut g) = PANEL_DRAGGING.lock() {
                let was = *g;
                *g = false;
                was
            } else { false };

            let start_x = if let Ok(g) = PANEL_DRAG_START_X.lock() { *g } else { 0 };
            let start_y = if let Ok(g) = PANEL_DRAG_START_Y.lock() { *g } else { 0 };
            let x = (lparam.0 & 0xFFFF) as i16 as i32;
            let y = ((lparam.0 >> 16) & 0xFFFF) as i16 as i32;

            // Si el cursor se movió 5 px o menos, es un clic sobre la miniatura
            if was_dragging && (x - start_x).abs() <= 5 && (y - start_y).abs() <= 5 {
                if let Some(idx) = thumbnail_at(x, y) {
                    let path = PANEL_SCANNED.lock().ok().and_then(|g| g.clone())
                        .and_then(|arc| arc.lock().ok().and_then(|v| v.get(idx).map(|img| img.path.clone())));
                    if let Some(path) = path {
                        log_info!("Panel: clic en miniatura {} -> {:?}", idx, path);
                        app::request_apply_wallpaper(&path);
                        hide_panel();
                    }
                }
            }
            LRESULT(0)
        }
        WM_MOUSEMOVE => {
            let x = (lparam.0 & 0xFFFF) as i16 as i32;
            let y = ((lparam.0 >> 16) & 0xFFFF) as i16 as i32;
            let vertical = is_vertical();
            set_hover_target(hwnd, thumbnail_at(x, y));

            // Drag scroll (solo si está arrastrando)
            let dragging = if let Ok(g) = PANEL_DRAGGING.lock() { *g } else { false };
            if dragging {
                let last = if let Ok(g) = PANEL_LAST_MOUSE_X.lock() { *g } else { 0 };
                let axis = if vertical { y } else { x };
                let delta = last - axis;
                let max_scroll = max_scroll(image_count());
                if let Ok(mut target) = PANEL_SCROLL_TARGET.lock() { *target = None; }
                if let Ok(mut g) = PANEL_SCROLL_OFFSET.lock() {
                    // Clamp: no se puede scrollear más allá del final de las imágenes
                    *g = (*g + delta).max(0).min(max_scroll);
                }
                if let Ok(mut g) = PANEL_LAST_MOUSE_X.lock() { *g = axis; }
            }
            request_repaint();
            LRESULT(0)
        }
        0x020A => { // WM_MOUSEWHEEL
            let delta = (wparam.0 >> 16) as i16 as i32;
            if is_carousel() {
                // En el carrusel cada paso de la rueda avanza una tarjeta.
                let count = image_count();
                if count > 0 {
                    let sel = PANEL_SELECTED_INDEX.lock().ok().and_then(|g| *g).unwrap_or(0);
                    let sel = if delta > 0 { sel.saturating_sub(1) } else { (sel + 1).min(count - 1) };
                    select_index(hwnd, sel);
                }
                return LRESULT(0);
            }
            let scroll_delta = if delta > 0 { -30 } else { 30 };
            let max_scroll = max_scroll(image_count());
            if let Ok(mut g) = PANEL_SCROLL_OFFSET.lock() {
                // Clamp: no se puede scrollear más allá del final de las imágenes
                *g = (*g + scroll_delta).max(0).min(max_scroll);
            }
            request_repaint();
            LRESULT(0)
        }
        WM_KEYDOWN => {
            if wparam.0 == 0x1B { // VK_ESCAPE
                hide_panel();
                return LRESULT(0);
            }
            let key = wparam.0 as i32;
            let vertical = is_vertical();
            let previous_key = if vertical { 0x26 } else { 0x25 };
            let next_key = if vertical { 0x28 } else { 0x27 };
            if key == previous_key || key == next_key || key == 0x0D {
                let count = image_count();
                if count == 0 { return LRESULT(0); }
                let sel = PANEL_SELECTED_INDEX.lock().ok().and_then(|g| *g).unwrap_or(0);
                if key == previous_key {
                    select_index(hwnd, sel.saturating_sub(1));
                } else if key == next_key {
                    select_index(hwnd, (sel + 1).min(count - 1));
                } else { // Enter -> aplicar y cerrar panel
                    let path = PANEL_SCANNED.lock().ok().and_then(|g| g.clone())
                        .and_then(|arc| arc.lock().ok().and_then(|v| v.get(sel).map(|img| img.path.clone())));
                    if let Some(p) = path {
                        log_info!("Panel: Enter en miniatura {} -> {:?}", sel, p);
                        crate::app::request_apply_wallpaper(&p);
                        crate::panel::set_current_wallpaper(&p);
                        hide_panel();
                    }
                }
                return LRESULT(0);
            }
            DefWindowProcW(hwnd, msg, wparam, lparam)
        }
        carousel::WM_APP_TILE_READY => {
            carousel::collect_ready();
            request_repaint();
            LRESULT(0)
        }
        WM_KILLFOCUS => {
            LRESULT(0)
        }
        WM_SETCURSOR => {
            unsafe {
                if let Ok(hcur) = LoadCursorW(None, IDC_ARROW) {
                    let _ = SetCursor(hcur);
                }
            }
            LRESULT(1)
        }
        WM_ERASEBKGND => LRESULT(1),
        WM_NCHITTEST => LRESULT(1),
        WM_DESTROY => {
            log_info!("Panel: ventana destruida");
            LRESULT(0)
        }
        _ => DefWindowProcW(hwnd, msg, wparam, lparam),
    }
}

/// Fuerza el foco del teclado y la activación en primer plano hacia la ventana del panel.
/// Utiliza AttachThreadInput para salvar las restricciones de cambio de foco entre hilos de Windows.
unsafe fn force_focus(hwnd: HWND) {
    use windows::Win32::Foundation::BOOL;
    use windows::Win32::System::Threading::{AttachThreadInput, GetCurrentThreadId};
    use windows::Win32::UI::Input::KeyboardAndMouse::{SetActiveWindow, SetFocus};
    use windows::Win32::UI::WindowsAndMessaging::{
        BringWindowToTop, GetForegroundWindow, GetWindowThreadProcessId, SetForegroundWindow,
    };

    let fg_hwnd = GetForegroundWindow();
    let fg_thread = GetWindowThreadProcessId(fg_hwnd, None);
    let cur_thread = GetCurrentThreadId();

    if fg_thread != 0 && fg_thread != cur_thread {
        let _ = AttachThreadInput(cur_thread, fg_thread, BOOL(1));
        let _ = BringWindowToTop(hwnd);
        let _ = SetForegroundWindow(hwnd);
        let _ = SetActiveWindow(hwnd);
        let _ = SetFocus(hwnd);
        let _ = AttachThreadInput(cur_thread, fg_thread, BOOL(0));
    } else {
        let _ = BringWindowToTop(hwnd);
        let _ = SetForegroundWindow(hwnd);
        let _ = SetActiveWindow(hwnd);
        let _ = SetFocus(hwnd);
    }
}

pub fn show_panel() {
    show_panel_with_close_on_mouse_leave(true);
}

pub fn show_panel_from_hotkey() {
    show_panel_with_close_on_mouse_leave(false);
}

pub fn set_panel_side(side: PanelSide) {
    let old_side = PANEL_SIDE.lock().map(|current| *current).unwrap_or(PanelSide::Top);
    if old_side == side { return; }
    if let Ok(mut current) = PANEL_SIDE.lock() { *current = side; }
    reopen_if_visible();
}

pub fn set_panel_mode(mode: PanelMode) {
    let old_mode = PANEL_MODE.lock().map(|current| *current).unwrap_or(PanelMode::LowPower);
    if old_mode == mode { return; }
    if let Ok(mut current) = PANEL_MODE.lock() { *current = mode; }
    if old_mode == PanelMode::Pro { carousel::clear_tiles(); }
    reopen_if_visible();
}

/// Vuelve a mostrar el panel con la nueva disposición si estaba abierto.
fn reopen_if_visible() {
    if is_panel_visible() {
        let close_on_leave = PANEL_CLOSE_ON_MOUSE_LEAVE.lock().map(|value| *value).unwrap_or(true);
        if let Some(hwnd_value) = PANEL_HWND.lock().ok().and_then(|value| *value) {
            unsafe {
                let hwnd = HWND(hwnd_value as *mut _);
                let _ = KillTimer(hwnd, ANIM_TIMER_ID);
                let _ = KillTimer(hwnd, AUTO_CLOSE_TIMER_ID);
                let _ = KillTimer(hwnd, HOVER_ANIM_TIMER_ID);
                let _ = ShowWindow(hwnd, windows::Win32::UI::WindowsAndMessaging::SW_HIDE);
            }
        }
        if let Ok(mut visible) = PANEL_VISIBLE.lock() { *visible = false; }
        if let Ok(mut exiting) = PANEL_EXITING.lock() { *exiting = false; }
        if let Ok(mut state) = PANEL_STATE.lock() { *state = None; }
        show_panel_with_close_on_mouse_leave(close_on_leave);
    }
}

/// Libera las tarjetas grandes del carrusel (por ejemplo, al cambiar la configuración).
pub fn clear_carousel_tiles() {
    carousel::clear_tiles();
}

pub fn set_scrollbar_visible(visible: bool) {
    if let Ok(mut show) = PANEL_SHOW_SCROLLBAR.lock() { *show = visible; }
    request_repaint();
}

pub fn set_panel_margin(margin: u32) {
    if let Ok(mut current) = PANEL_MARGIN.lock() { *current = margin; }
}

fn show_panel_with_close_on_mouse_leave(close_on_mouse_leave: bool) {
    unsafe {
        if let Ok(v) = PANEL_VISIBLE.lock() {
            if *v { return; }
        }
        // El carrusel queda lejos del borde: solo se cierra con clic fuera, Esc o al elegir.
        let close_on_mouse_leave = close_on_mouse_leave && !is_carousel();
        if let Ok(mut mode) = PANEL_CLOSE_ON_MOUSE_LEAVE.lock() {
            *mode = close_on_mouse_leave;
        }

        // Posicion y dimensiones segun lado configurado y monitor actual
        let mut pt = POINT::default();
        let _ = GetCursorPos(&mut pt);
        let hmon = MonitorFromPoint(pt, MONITOR_DEFAULTTONEAREST);
        let mut mi = MONITORINFO {
            cbSize: std::mem::size_of::<MONITORINFO>() as u32,
            ..Default::default()
        };
        let _ = GetMonitorInfoW(hmon, &mut mi);
        let (mut dpi_x, mut dpi_y) = (96u32, 96u32);
        if windows::Win32::UI::HiDpi::GetDpiForMonitor(hmon, windows::Win32::UI::HiDpi::MDT_EFFECTIVE_DPI, &mut dpi_x, &mut dpi_y).is_ok() {
            PANEL_DPI.store(dpi_x.max(96), std::sync::atomic::Ordering::Relaxed);
        }

        let side = PANEL_SIDE.lock().map(|value| *value).unwrap_or(PanelSide::Top);
        let vertical = is_vertical();
        let monitor_width = mi.rcMonitor.right - mi.rcMonitor.left;
        let monitor_height = mi.rcMonitor.bottom - mi.rcMonitor.top;
        let requested_margin = PANEL_MARGIN.lock().map(|margin| *margin).unwrap_or(0);
        let margin = effective_panel_margin(side, requested_margin, monitor_width, monitor_height);
        let layout = is_carousel().then(|| carousel::Layout::for_monitor(monitor_height));
        if let Some(layout) = layout { carousel::set_layout(layout); }
        let width = if vertical { thumb_w() + thumb_margin() * 2 + scrollbar_size() } else { monitor_width };
        let height = match layout {
            Some(layout) => layout.window_height(),
            None if vertical => monitor_height,
            None => thumb_h() + thumb_margin() * 2 + scrollbar_size(),
        };
        let target_x = match side {
            _ if layout.is_some() => mi.rcMonitor.left,
            PanelSide::Top => mi.rcMonitor.left,
            PanelSide::Left => mi.rcMonitor.left + margin,
            PanelSide::Right => mi.rcMonitor.right - width - margin,
        };
        let target_y = match side {
            _ if layout.is_some() => mi.rcMonitor.top + (monitor_height - height) / 2,
            PanelSide::Top => mi.rcMonitor.top + margin,
            PanelSide::Left | PanelSide::Right => mi.rcMonitor.top,
        };
        let start_x = match side {
            _ if layout.is_some() => target_x,
            PanelSide::Left => mi.rcMonitor.left - width,
            PanelSide::Right => mi.rcMonitor.right,
            PanelSide::Top => target_x,
        };
        let start_y = match side {
            _ if layout.is_some() => target_y + CAROUSEL_SLIDE_PX,
            PanelSide::Top => mi.rcMonitor.top - height,
            PanelSide::Left | PanelSide::Right => target_y,
        };
        let viewport = if vertical { height } else { width };
        let item_size = if vertical { thumb_h() } else { thumb_w() };
        let item_extent = item_size + thumb_margin();

        // Calcular fondo seleccionado actual y posición de scroll centrada
        let (selected_idx, initial_scroll, image_count) = {
            let current_wp = PANEL_CURRENT_WALLPAPER.lock().ok().and_then(|g| g.clone());
            let images_opt = PANEL_SCANNED.lock().ok().and_then(|g| g.clone());
            if let Some(images_arc) = images_opt {
                if let Ok(images) = images_arc.lock() {
                    let count = images.len();
                    let idx = if let Some(ref cur_path) = current_wp {
                        images.iter().position(|img| &img.path == cur_path).unwrap_or(0)
                    } else {
                        0
                    };
                    let scroll = if let Some(layout) = layout {
                        layout.center_scroll(idx, count, viewport)
                    } else {
                        let item_x = idx as i32 * item_extent + thumb_margin();
                        let center_scroll = (item_x + item_size / 2) - (viewport / 2);
                        let max_scroll = (thumbnail_content_extent(count, vertical) - viewport).max(0);
                        center_scroll.clamp(0, max_scroll)
                    };
                    (idx, scroll, count)
                } else {
                    (0, 0, 0)
                }
            } else {
                (0, 0, 0)
            }
        };

        // Reutilizar ventana cacheada si existe
        let cached = PANEL_HWND.lock().ok().and_then(|g| *g);
        let hwnd = if let Some(hwnd_val) = cached {
            HWND(hwnd_val as *mut _)
        } else {
            let hinstance = match GetModuleHandleW(None) {
                Ok(h) => HINSTANCE(h.0),
                Err(_) => return,
            };
            let class_name_wide = to_wide_chars(PANEL_CLASS_NAME);
            let wc = WNDCLASSW {
                style: CS_HREDRAW | CS_VREDRAW,
                lpfnWndProc: Some(panel_wnd_proc),
                hInstance: hinstance,
                hIcon: crate::resources::app_icon(),
                lpszClassName: as_pcwstr(&class_name_wide),
                ..Default::default()
            };
            let _ = RegisterClassW(&wc);

            let hwnd = match CreateWindowExW(
                WS_EX_TOPMOST | WS_EX_TOOLWINDOW | windows::Win32::UI::WindowsAndMessaging::WS_EX_LAYERED,
                as_pcwstr(&class_name_wide),
                as_pcwstr(&to_wide_chars("SWEPR Panel")),
                WS_POPUP,
                start_x,
                start_y,
                width,
                height,
                None,
                None,
                hinstance,
                None,
            ) {
                Ok(h) => h,
                Err(_) => return,
            };

            if let Ok(mut g) = PANEL_HWND.lock() { *g = Some(hwnd.0 as isize); }
            hwnd
        };

        if let Ok(mut g) = PANEL_WIDTH.lock() { *g = width; }
        if let Ok(mut g) = PANEL_HEIGHT.lock() { *g = height; }
        if let Ok(mut g) = PANEL_SCROLL_OFFSET.lock() { *g = initial_scroll; }
        if let Ok(mut g) = PANEL_SELECTED_INDEX.lock() { *g = Some(selected_idx); }
        if let Ok(mut g) = PANEL_HOVER_INDEX.lock() { *g = Some(selected_idx); }
        if let Ok(mut g) = PANEL_DRAGGING.lock() { *g = false; }
        if let Ok(mut g) = PANEL_SCROLL_TARGET.lock() { *g = None; }

        if image_count > 0 {
            if let Ok(mut s) = PANEL_THUMB_SCALES.lock() {
                s.resize(image_count, 1.0);
                for (i, v) in s.iter_mut().enumerate() {
                    *v = if i == selected_idx { focus_scale() } else { 1.0 };
                }
            }
            if let Ok(mut t) = PANEL_THUMB_TARGETS.lock() {
                t.resize(image_count, 1.0);
                for (i, v) in t.iter_mut().enumerate() {
                    *v = if i == selected_idx { focus_scale() } else { 1.0 };
                }
            }
        }

        if let Ok(mut stg) = PANEL_STATE.lock() {
            *stg = Some(PanelState {
                target_x,
                start_x,
                current_x: start_x,
                target_y,
                start_y,
                current_y: start_y,
                anim_start_time: std::time::Instant::now(),
                animating: true,
            });
        }
        let _ = SetWindowPos(hwnd, HWND_TOPMOST, start_x, start_y, width, height, SWP_NOACTIVATE | SWP_NOZORDER);
        let _ = KillTimer(hwnd, TILE_RELEASE_TIMER_ID);
        // La ventana es layered: sin atributos no se vería. El carrusel entra con fundido.
        set_panel_alpha(hwnd, if layout.is_some() { 0 } else { 255 });

        let _ = ShowWindow(hwnd, SW_SHOW);
        force_focus(hwnd);
        let _ = SetTimer(hwnd, ANIM_TIMER_ID, ANIM_STEP_MS, None);
        let close_check_ms = if close_on_mouse_leave {
            SENSOR_CLOSE_CHECK_MS
        } else {
            HOTKEY_CLICK_CHECK_MS
        };
        let _ = SetTimer(hwnd, AUTO_CLOSE_TIMER_ID, close_check_ms, None);
        if let Ok(mut v) = PANEL_VISIBLE.lock() { *v = true; }
        log_info!("Panel: mostrado en monitor con animacion desde arriba");
    }
}
pub fn hide_panel() {
    unsafe {
        if let Ok(v) = PANEL_VISIBLE.lock() { if !*v { return; } }
        if let Ok(e) = PANEL_EXITING.lock() { if *e { return; } }

        if let Some(hwnd_val) = PANEL_HWND.lock().ok().and_then(|g| *g) {
            let hwnd = HWND(hwnd_val as *mut _);
            let _ = KillTimer(hwnd, AUTO_CLOSE_TIMER_ID);
            let _ = KillTimer(hwnd, HOVER_ANIM_TIMER_ID);

            let mut wr = RECT::default();
            let _ = windows::Win32::UI::WindowsAndMessaging::GetWindowRect(hwnd, &mut wr);
            let side = PANEL_SIDE.lock().map(|value| *value).unwrap_or(PanelSide::Top);
            let start_x = wr.left;
            let start_y = wr.top;
            let height = if let Ok(g) = PANEL_HEIGHT.lock() { *g } else { thumb_h() + thumb_margin() * 2 + scrollbar_size() };
            let width = if let Ok(g) = PANEL_WIDTH.lock() { *g } else { thumb_w() + thumb_margin() * 2 + scrollbar_size() };
            let hmon = windows::Win32::Graphics::Gdi::MonitorFromWindow(hwnd, windows::Win32::Graphics::Gdi::MONITOR_DEFAULTTONEAREST);
            let mut mi = MONITORINFO { cbSize: std::mem::size_of::<MONITORINFO>() as u32, ..Default::default() };
            let _ = windows::Win32::Graphics::Gdi::GetMonitorInfoW(hmon, &mut mi);
            let (target_x, target_y) = match side {
                _ if is_carousel() => (start_x, start_y + CAROUSEL_SLIDE_PX),
                PanelSide::Top => (start_x, mi.rcMonitor.top - height),
                PanelSide::Left => (mi.rcMonitor.left - width, start_y),
                PanelSide::Right => (mi.rcMonitor.right, start_y),
            };

            if let Ok(mut stg) = PANEL_STATE.lock() {
                *stg = Some(PanelState {
                    target_x,
                    start_x,
                    current_x: start_x,
                    target_y,
                    start_y,
                    current_y: start_y,
                    anim_start_time: std::time::Instant::now(),
                    animating: true,
                });
            }
            if let Ok(mut e) = PANEL_EXITING.lock() { *e = true; }
            let _ = SetTimer(hwnd, ANIM_TIMER_ID, ANIM_STEP_MS, None);
            log_info!("Panel: animando salida");
        }
    }
}

pub fn set_draw_context(cache: ThumbnailCache, images: Arc<Mutex<Vec<ScannedImage>>>) {
    // Inicializar escalas por miniatura
    if let Ok(imgs) = images.lock() {
        let n = imgs.len();
        if let Ok(mut s) = PANEL_THUMB_SCALES.lock() {
            s.resize(n, 1.0);
            for v in s.iter_mut() { if *v == 0.0 { *v = 1.0; } }
        }
        if let Ok(mut t) = PANEL_THUMB_TARGETS.lock() {
            t.resize(n, 1.0);
            for v in t.iter_mut() { *v = 1.0; }
        }
    }
    if let Ok(mut g) = PANEL_THUMB_CACHE.lock() { *g = Some(cache); }
    if let Ok(mut g) = PANEL_SCANNED.lock() { *g = Some(images); }
    request_repaint();
}

pub fn set_current_wallpaper(path: &std::path::Path) {
    if let Ok(mut g) = PANEL_CURRENT_WALLPAPER.lock() {
        *g = Some(path.to_path_buf());
    }
    request_repaint();
}

pub fn request_repaint() {
    if let Some(hwnd_val) = PANEL_HWND.lock().ok().and_then(|g| *g) {
        unsafe {
            let _ = windows::Win32::Graphics::Gdi::InvalidateRect(HWND(hwnd_val as *mut _), None, true);
        }
    }
}

pub fn is_panel_visible() -> bool {
    PANEL_VISIBLE.lock().map(|v| *v).unwrap_or(false)
}

#[cfg(test)]
pub(crate) unsafe fn save_bmp(dc: windows::Win32::Graphics::Gdi::HDC, bmp: windows::Win32::Graphics::Gdi::HBITMAP, width: i32, height: i32, path: &str) {
    use windows::Win32::Graphics::Gdi::{GetDIBits, BITMAPINFO, BITMAPINFOHEADER, DIB_RGB_COLORS};
    let mut info = BITMAPINFO {
        bmiHeader: BITMAPINFOHEADER {
            biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
            biWidth: width,
            biHeight: height,
            biPlanes: 1,
            biBitCount: 24,
            ..Default::default()
        },
        ..Default::default()
    };
    let stride = ((width * 3 + 3) & !3) as usize;
    let mut pixels = vec![0u8; stride * height as usize];
    GetDIBits(dc, bmp, 0, height as u32, Some(pixels.as_mut_ptr() as *mut _), &mut info, DIB_RGB_COLORS);
    let mut file = Vec::new();
    file.extend_from_slice(b"BM");
    file.extend_from_slice(&(54 + pixels.len() as u32).to_le_bytes());
    file.extend_from_slice(&0u32.to_le_bytes());
    file.extend_from_slice(&54u32.to_le_bytes());
    file.extend_from_slice(std::slice::from_raw_parts(&info.bmiHeader as *const _ as *const u8, 40));
    file.extend_from_slice(&pixels);
    std::fs::write(path, file).unwrap();
}

#[cfg(test)]
mod tests {
    use super::*;

    unsafe fn render(width: i32, height: i32, path: &str) {
        let screen = windows::Win32::Graphics::Gdi::GetDC(None);
        let dc = CreateCompatibleDC(screen);
        let bmp = CreateCompatibleBitmap(screen, width, height);
        let old = SelectObject(dc, bmp);
        paint_contents(dc, HWND::default(), width, height);
        SelectObject(dc, old);
        save_bmp(dc, bmp, width, height, path);
        let _ = DeleteObject(bmp);
        let _ = DeleteDC(dc);
    }

    /// Pinta la tira de bajo consumo (al 150 % de escala) y el aviso de carpeta vacía:
    /// `SWEPR_TEST_DIR=<carpeta> SWEPR_TEST_OUT=<prefijo> cargo test render_strip_preview -- --ignored`
    #[test]
    #[ignore]
    fn render_strip_preview() {
        let dir = std::env::var("SWEPR_TEST_DIR").expect("SWEPR_TEST_DIR");
        let out = std::env::var("SWEPR_TEST_OUT").expect("SWEPR_TEST_OUT");
        let mut paths: Vec<std::path::PathBuf> = std::fs::read_dir(dir).unwrap().flatten().map(|e| e.path())
            .filter(|p| p.extension().is_some_and(|e| ["jpg", "jpeg", "png"].contains(&e.to_string_lossy().to_lowercase().as_str())))
            .collect();
        paths.sort();
        paths.truncate(10);
        let images: Vec<ScannedImage> = paths.iter().enumerate().map(|(i, p)| ScannedImage {
            path: p.clone(),
            file_name: String::new(),
            is_new: i == 4 || i == 6,
        }).collect();
        let cache = ThumbnailCache::new();
        for img in &images {
            crate::thumbs::request_thumbnail(HWND::default(), &img.path, 128, cache.clone());
        }
        PANEL_DPI.store(144, std::sync::atomic::Ordering::Relaxed);
        *PANEL_SIDE.lock().unwrap() = PanelSide::Top;
        *PANEL_MODE.lock().unwrap() = PanelMode::LowPower;
        *PANEL_SHOW_SCROLLBAR.lock().unwrap() = true;
        *PANEL_CURRENT_WALLPAPER.lock().unwrap() = Some(images[2].path.clone());
        *PANEL_SELECTED_INDEX.lock().unwrap() = Some(2);
        *PANEL_HOVER_INDEX.lock().unwrap() = Some(5);
        *PANEL_THUMB_SCALES.lock().unwrap() = (0..images.len()).map(|i| if i == 5 { 0.9 } else { 1.0 }).collect();
        let width = 1600;
        let height = thumb_h() + thumb_margin() * 2 + scrollbar_size();
        *PANEL_WIDTH.lock().unwrap() = width;
        *PANEL_HEIGHT.lock().unwrap() = height;
        *PANEL_SCROLL_OFFSET.lock().unwrap() = 0;
        *PANEL_THUMB_CACHE.lock().unwrap() = Some(cache);
        *PANEL_SCANNED.lock().unwrap() = Some(Arc::new(Mutex::new(images)));
        unsafe {
            render(width, height, &format!("{out}-strip.bmp"));
            *PANEL_SCANNED.lock().unwrap() = Some(Arc::new(Mutex::new(Vec::new())));
            render(width, height, &format!("{out}-empty.bmp"));
        }
    }
}
