use crate::app;
use crate::config::PanelSide;
use crate::log_info;
use crate::scanner::ScannedImage;
use crate::thumbs::ThumbnailCache;
use crate::winutil::{as_pcwstr, to_wide_chars};
use std::sync::{Arc, Mutex};
use windows::Win32::Foundation::{HINSTANCE, HWND, LPARAM, LRESULT, POINT, RECT, WPARAM};
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
const THUMB_W: i32 = 160;
const THUMB_H: i32 = 90;
const THUMB_MARGIN: i32 = 8;

const HOVER_ANIM_TIMER_ID: usize = 8003;
const HOVER_ANIM_STEP_MS: u32 = 16;



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
static PANEL_SHOW_SCROLLBAR: Mutex<bool> = Mutex::new(true);
static PANEL_HINSTANCE: Mutex<Option<isize>> = Mutex::new(None);
static PANEL_CLASS: Mutex<Option<Vec<u16>>> = Mutex::new(None);
static PANEL_X: Mutex<i32> = Mutex::new(0);
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
static PANEL_HOVER_SCALE: Mutex<f32> = Mutex::new(1.0);
static PANEL_THUMB_SCALES: Mutex<Vec<f32>> = Mutex::new(Vec::new());
static PANEL_THUMB_TARGETS: Mutex<Vec<f32>> = Mutex::new(Vec::new());
static PANEL_THUMB_ANIM: Mutex<bool> = Mutex::new(false);
static PANEL_CURRENT_WALLPAPER: Mutex<Option<std::path::PathBuf>> = Mutex::new(None);

fn thumbnail_content_extent(count: usize, vertical: bool) -> i32 {
    if count == 0 { return 0; }
    let item_size = if vertical { THUMB_H } else { THUMB_W };
    count as i32 * item_size + (count as i32 - 1) * THUMB_MARGIN + THUMB_MARGIN * 2
}

fn thumbnail_origin(index: usize, count: usize, viewport: i32, scroll: i32, vertical: bool) -> (i32, i32) {
    if count == 0 { return (THUMB_MARGIN, THUMB_MARGIN); }
    let item_size = if vertical { THUMB_H } else { THUMB_W };
    let item_extent = item_size + THUMB_MARGIN;
    let content_size = count as i32 * item_size + (count as i32 - 1) * THUMB_MARGIN;
    let origin = if thumbnail_content_extent(count, vertical) <= viewport {
        (viewport - content_size) / 2
    } else {
        THUMB_MARGIN
    } + index as i32 * item_extent - scroll;
    if vertical { (THUMB_MARGIN, origin) } else { (origin, THUMB_MARGIN) }
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
                if idx < t.len() { t[idx] = 0.9; }
            }
        }
        if let Ok(mut anim) = PANEL_THUMB_ANIM.lock() { *anim = true; }
        unsafe {
            let _ = SetTimer(hwnd, HOVER_ANIM_TIMER_ID, HOVER_ANIM_STEP_MS, None);
        }
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
                }
                return LRESULT(0);
            }
            if wparam.0 == HOVER_ANIM_TIMER_ID {
                let mut done = true;
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
                let outside = pt.x < rect.left || pt.x >= rect.right || pt.y < rect.top || pt.y >= rect.bottom;
                let close_on_leave = PANEL_CLOSE_ON_MOUSE_LEAVE.lock().map(|value| *value).unwrap_or(true);
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
                let bg = CreateSolidBrush(windows::Win32::Foundation::COLORREF(0x00181818));
                let _ = FillRect(mem_dc, &rect, bg);
                let _ = DeleteObject(bg);

                let cache_opt = if let Ok(g) = PANEL_THUMB_CACHE.lock() { g.clone() } else { None };
                let images_opt = if let Ok(g) = PANEL_SCANNED.lock() { g.clone() } else { None };
                let scroll = if let Ok(g) = PANEL_SCROLL_OFFSET.lock() { *g } else { 0 };
                let vertical = PANEL_SIDE.lock().map(|side| *side != PanelSide::Top).unwrap_or(false);
                let hover = if let Ok(g) = PANEL_HOVER_INDEX.lock() { *g } else { None };
                let _hover_scale = if let Ok(g) = PANEL_HOVER_SCALE.lock() { *g } else { 1.0 };
                let current_wp = if let Ok(g) = PANEL_CURRENT_WALLPAPER.lock() { g.clone() } else { None };
                let selected = if let Ok(g) = PANEL_SELECTED_INDEX.lock() { *g } else { None };


                if let (Some(cache), Some(images_arc)) = (cache_opt, images_opt) {
                    if let Ok(images) = images_arc.lock() {
                        let half_g = 0;
                        for (idx, img) in images.iter().enumerate() {
                            let key = img.path.to_string_lossy().to_string();
                            let scale = PANEL_THUMB_SCALES.lock().ok().and_then(|v| v.get(idx).copied()).unwrap_or(1.0);
                            let tw = (THUMB_W as f32 * scale) as i32;
                            let th = (THUMB_H as f32 * scale) as i32;
                            let draw_w = tw.max(1);
                            let draw_h = th.max(1);
                            let (base_x, base_y) = thumbnail_origin(idx, images.len(), if vertical { height } else { width }, scroll, vertical);
                            let mut x = base_x;
                            if let Some(hi) = hover {
                                if idx < hi { x -= half_g; }
                                else if idx > hi { x += half_g; }
                                else { x -= half_g; }
                            }
                            let draw_x = x + (THUMB_W - draw_w) / 2;
                            let draw_y = base_y + (THUMB_H - draw_h) / 2;
                            let radius = 8i32;
                            let clip_rgn = CreateRoundRectRgn(draw_x, draw_y, draw_x + draw_w, draw_y + draw_h, radius * 2, radius * 2);
                            let _ = SelectClipRgn(mem_dc, clip_rgn);
                            if let Some(thumb) = cache.get(&key) {
                                let bmp_dc = CreateCompatibleDC(hdc);
                                let old = SelectObject(bmp_dc, windows::Win32::Graphics::Gdi::HBITMAP(thumb.hbitmap as *mut _));
                                let _ = SetStretchBltMode(mem_dc, COLORONCOLOR);
                                let (src_x, src_y, src_w, src_h) = if thumb.width * draw_h > thumb.height * draw_w {
                                    let crop_w = (thumb.height * draw_w / draw_h).max(1);
                                    ((thumb.width - crop_w) / 2, 0, crop_w, thumb.height)
                                } else {
                                    let crop_h = (thumb.width * draw_h / draw_w).max(1);
                                    (0, (thumb.height - crop_h) / 2, thumb.width, crop_h)
                                };
                                let _ = StretchBlt(mem_dc, draw_x, draw_y, draw_w, draw_h, bmp_dc, src_x, src_y, src_w, src_h, SRCCOPY);
                                SelectObject(bmp_dc, old);
                                let _ = DeleteDC(bmp_dc);
                            } else {
                                let ph = CreateSolidBrush(windows::Win32::Foundation::COLORREF(0x00303030));
                                let r = RECT { left: draw_x, top: draw_y, right: draw_x + draw_w, bottom: draw_y + draw_h };
                                let _ = FillRect(mem_dc, &r, ph);
                                let _ = DeleteObject(ph);
                            }
                            let _ = SelectClipRgn(mem_dc, windows::Win32::Graphics::Gdi::HRGN(std::ptr::null_mut()));
                            let _ = DeleteObject(clip_rgn);
                            let is_current = current_wp.as_ref().map(|p| p == &img.path).unwrap_or(false);
                            if is_current || img.is_new || hover == Some(idx) {
                                let color = if hover == Some(idx) {
                                    windows::Win32::Foundation::COLORREF(0x00FFFFFF)
                                } else {
                                    windows::Win32::Foundation::COLORREF(0x0000FF00)
                                };
                                let rgn = CreateRoundRectRgn(draw_x, draw_y, draw_x + draw_w, draw_y + draw_h, radius * 2, radius * 2);
                                let brush = CreateSolidBrush(color);
                                let _ = FrameRgn(mem_dc, rgn, brush, 2, 2);
                                let _ = DeleteObject(brush);
                                let _ = DeleteObject(rgn);
                            }
                            if selected == Some(idx) {
                                let rgn = CreateRoundRectRgn(draw_x, draw_y, draw_x + draw_w, draw_y + draw_h, radius * 2, radius * 2);
                                let brush = CreateSolidBrush(windows::Win32::Foundation::COLORREF(0x0000FFFF));
                                let _ = FrameRgn(mem_dc, rgn, brush, 2, 2);
                                let _ = DeleteObject(brush);
                                let _ = DeleteObject(rgn);
                            }
                        }
                        let total_main = thumbnail_content_extent(images.len(), vertical);
                        let viewport = if vertical { height } else { width };
                        let show_scrollbar = PANEL_SHOW_SCROLLBAR.lock().map(|show| *show).unwrap_or(true);
                        if show_scrollbar && total_main > viewport {
                            let track_brush = CreateSolidBrush(windows::Win32::Foundation::COLORREF(0x00404040));
                            let thumb_brush = CreateSolidBrush(windows::Win32::Foundation::COLORREF(0x00FFFFFF));
                            let max_scroll = (total_main - viewport).max(1);
                            if vertical {
                                let track_x = THUMB_MARGIN + THUMB_W + THUMB_MARGIN;
                                let track_rect = RECT { left: track_x, top: 0, right: track_x + 3, bottom: height };
                                let _ = FillRect(mem_dc, &track_rect, track_brush);
                                let thumb_h = (((height as f32 / total_main as f32) * height as f32) as i32).max(20).min(height);
                                let thumb_y = ((scroll as f32 / max_scroll as f32) * (height - thumb_h) as f32) as i32;
                                let thumb_rect = RECT { left: track_x, top: thumb_y, right: track_x + 3, bottom: thumb_y + thumb_h };
                                let _ = FillRect(mem_dc, &thumb_rect, thumb_brush);
                            } else {
                                let track_y = THUMB_MARGIN + THUMB_H + THUMB_MARGIN;
                                let track_rect = RECT { left: 0, top: track_y, right: width, bottom: track_y + 3 };
                                let _ = FillRect(mem_dc, &track_rect, track_brush);
                                let thumb_w = (((width as f32 / total_main as f32) * width as f32) as i32).max(20).min(width);
                                let thumb_x = ((scroll as f32 / max_scroll as f32) * (width - thumb_w) as f32) as i32;
                                let thumb_rect = RECT { left: thumb_x, top: track_y, right: thumb_x + thumb_w, bottom: track_y + 3 };
                                let _ = FillRect(mem_dc, &thumb_rect, thumb_brush);
                            }
                            let _ = DeleteObject(track_brush);
                            let _ = DeleteObject(thumb_brush);
                        }
                    }
                }

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
            let vertical = PANEL_SIDE.lock().map(|side| *side != PanelSide::Top).unwrap_or(false);
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
                let scroll = if let Ok(g) = PANEL_SCROLL_OFFSET.lock() { *g } else { 0 };
                let vertical = PANEL_SIDE.lock().map(|side| *side != PanelSide::Top).unwrap_or(false);
                let hover_idx = if let Ok(g) = PANEL_HOVER_INDEX.lock() { *g } else { None };
                let images_opt = if let Ok(g) = PANEL_SCANNED.lock() { g.clone() } else { None };
                if let Some(images_arc) = images_opt {
                    if let Ok(images) = images_arc.lock() {
                        let half_g = 0;
                        for (idx, img) in images.iter().enumerate() {
                            let (mut tx, ty) = thumbnail_origin(idx, images.len(), if vertical { PANEL_HEIGHT.lock().map(|g| *g).unwrap_or(600) } else { PANEL_WIDTH.lock().map(|g| *g).unwrap_or(800) }, scroll, vertical);
                            if let Some(hi) = hover_idx {
                                if idx < hi { tx -= half_g; }
                                else if idx > hi { tx += half_g; }
                                else { tx -= half_g; }
                            }
                            if x >= tx && x < tx + THUMB_W && y >= ty && y < ty + THUMB_H {
                                log_info!("Panel: clic en miniatura {} -> {:?}", idx, img.path);
                                app::request_apply_wallpaper(&img.path);
                                hide_panel();
                                break;
                            }
                        }
                    }
                }
            }
            LRESULT(0)
        }
        WM_MOUSEMOVE => {
            let x = (lparam.0 & 0xFFFF) as i16 as i32;
            let y = ((lparam.0 >> 16) & 0xFFFF) as i16 as i32;
            let vertical = PANEL_SIDE.lock().map(|side| *side != PanelSide::Top).unwrap_or(false);
            // Hover
            let scroll = if let Ok(g) = PANEL_SCROLL_OFFSET.lock() { *g } else { 0 };
            let hover_idx = if let Ok(g) = PANEL_HOVER_INDEX.lock() { *g } else { None };
            let hover_scale = if let Ok(g) = PANEL_HOVER_SCALE.lock() { *g } else { 1.0 };
            let images_opt = if let Ok(g) = PANEL_SCANNED.lock() { g.clone() } else { None };
            let mut new_hover = None;
            if let Some(images_arc) = images_opt {
                if let Ok(images) = images_arc.lock() {
                    let half_g = 0;
                    for (idx, _img) in images.iter().enumerate() {
                        let _tw = (THUMB_W as f32 * if Some(idx) == hover_idx { hover_scale } else { 1.0 }) as i32;
                        let (mut tx, ty) = thumbnail_origin(idx, images.len(), if vertical { PANEL_HEIGHT.lock().map(|g| *g).unwrap_or(600) } else { PANEL_WIDTH.lock().map(|g| *g).unwrap_or(800) }, scroll, vertical);
                        if let Some(hi) = hover_idx {
                            if idx < hi { tx -= half_g; }
                            else if idx > hi { tx += half_g; }
                            else { tx -= half_g; }
                        }
                        if x >= tx && x < tx + THUMB_W && y >= ty && y < ty + THUMB_H {
                            new_hover = Some(idx);
                            break;
                        }
                    }
                }
            }
            set_hover_target(hwnd, new_hover);

            // Drag scroll (solo si está arrastrando)
            let dragging = if let Ok(g) = PANEL_DRAGGING.lock() { *g } else { false };
            if dragging {
                let last = if let Ok(g) = PANEL_LAST_MOUSE_X.lock() { *g } else { 0 };
                let axis = if vertical { y } else { x };
                let delta = last - axis;
                if let Ok(mut g) = PANEL_SCROLL_OFFSET.lock() {
                    // Clamp: no se puede scrollear más allá del final de las imágenes
                    let mut image_count: usize = 0;
                    {
                        let images_opt = if let Ok(g) = PANEL_SCANNED.lock() { g.clone() } else { None };
                        if let Some(images_arc) = images_opt {
                            if let Ok(images) = images_arc.lock() {
                                image_count = images.len();
                            }
                        }
                    }
                    let viewport = if vertical {
                        PANEL_HEIGHT.lock().map(|g| *g).unwrap_or(600)
                    } else {
                        PANEL_WIDTH.lock().map(|g| *g).unwrap_or(800)
                    };
                    let max_scroll = (thumbnail_content_extent(image_count, vertical) - viewport).max(0);
                    *g = (*g + delta).max(0).min(max_scroll);
                }
                if let Ok(mut g) = PANEL_LAST_MOUSE_X.lock() { *g = axis; }
            }
            // Repintar
            unsafe {
                if let Some(hwnd_val) = PANEL_HWND.lock().ok().and_then(|g| *g) {
                    let _ = windows::Win32::Graphics::Gdi::InvalidateRect(HWND(hwnd_val as *mut _), None, true);
                }
            }
            LRESULT(0)
        }
        0x020A => { // WM_MOUSEWHEEL
            let delta = (wparam.0 >> 16) as i16 as i32;
            let scroll_delta = if delta > 0 { -30 } else { 30 };
            let vertical = PANEL_SIDE.lock().map(|side| *side != PanelSide::Top).unwrap_or(false);
            if let Ok(mut g) = PANEL_SCROLL_OFFSET.lock() {
                // Clamp: no se puede scrollear más allá del final de las imágenes
                let mut image_count: usize = 0;
                {
                    let images_opt = if let Ok(g) = PANEL_SCANNED.lock() { g.clone() } else { None };
                    if let Some(images_arc) = images_opt {
                        if let Ok(images) = images_arc.lock() {
                            image_count = images.len();
                        }
                    }
                }
                let viewport = if vertical {
                    PANEL_HEIGHT.lock().map(|g| *g).unwrap_or(600)
                } else {
                    PANEL_WIDTH.lock().map(|g| *g).unwrap_or(800)
                };
                let max_scroll = (thumbnail_content_extent(image_count, vertical) - viewport).max(0);
                *g = (*g + scroll_delta).max(0).min(max_scroll);
            }
            unsafe {
                if let Some(hwnd_val) = PANEL_HWND.lock().ok().and_then(|g| *g) {
                    let _ = windows::Win32::Graphics::Gdi::InvalidateRect(HWND(hwnd_val as *mut _), None, true);
                }
            }
            LRESULT(0)
        }
        WM_KEYDOWN => {
            if wparam.0 == 0x1B { // VK_ESCAPE
                hide_panel();
                return LRESULT(0);
            }
            let key = wparam.0 as i32;
            let vertical = PANEL_SIDE.lock().map(|side| *side != PanelSide::Top).unwrap_or(false);
            let previous_key = if vertical { 0x26 } else { 0x25 };
            let next_key = if vertical { 0x28 } else { 0x27 };
            if key == previous_key || key == next_key || key == 0x0D {
                // Obtener cantidad de imagenes
                let count = if let Ok(g) = PANEL_SCANNED.lock() {
                    if let Some(arc) = g.clone() {
                        if let Ok(v) = arc.lock() { v.len() } else { 0 }
                    } else { 0 }
                } else { 0 };
                if count == 0 { return LRESULT(0); }
                if key == previous_key {
                    let mut sel = PANEL_SELECTED_INDEX.lock().ok().and_then(|g| *g).unwrap_or(0);
                    if sel > 0 { sel -= 1; }
                    if let Ok(mut g) = PANEL_SELECTED_INDEX.lock() { *g = Some(sel); }
                    set_hover_target(hwnd, Some(sel));
                } else if key == next_key {
                    let mut sel = PANEL_SELECTED_INDEX.lock().ok().and_then(|g| *g).unwrap_or(0);
                    if sel + 1 < count { sel += 1; }
                    if let Ok(mut g) = PANEL_SELECTED_INDEX.lock() { *g = Some(sel); }
                    set_hover_target(hwnd, Some(sel));
                } else if key == 0x0D { // Enter -> aplicar y cerrar panel
                    let sel = PANEL_SELECTED_INDEX.lock().ok().and_then(|g| *g).unwrap_or(0);
                    let path = if let Ok(g) = PANEL_SCANNED.lock() {
                        if let Some(arc) = g.clone() {
                            if let Ok(v) = arc.lock() { v.get(sel).map(|img| img.path.clone()) } else { None }
                        } else { None }
                    } else { None };
                    if let Some(p) = path {
                        log_info!("Panel: Enter en miniatura {} -> {:?}", sel, p);
                        crate::app::request_apply_wallpaper(&p);
                        crate::panel::set_current_wallpaper(&p);
                        hide_panel();
                    }
                    return LRESULT(0);
                }
                // Ajustar scroll para mantener seleccion visible
                let sel = PANEL_SELECTED_INDEX.lock().ok().and_then(|g| *g).unwrap_or(0);
                let viewport = if vertical {
                    PANEL_HEIGHT.lock().map(|g| *g).unwrap_or(600)
                } else {
                    PANEL_WIDTH.lock().map(|g| *g).unwrap_or(800)
                };
                let scroll = PANEL_SCROLL_OFFSET.lock().map(|g| *g).unwrap_or(0);
                let item_extent = if vertical { THUMB_H + THUMB_MARGIN } else { THUMB_W + THUMB_MARGIN };
                let item_size = if vertical { THUMB_H } else { THUMB_W };
                let item_x = sel as i32 * item_extent + THUMB_MARGIN;
                let mut new_scroll = scroll;
                if item_x < scroll { new_scroll = item_x.saturating_sub(THUMB_MARGIN); }
                let right_edge = item_x + item_size;
                if right_edge > scroll + viewport { new_scroll = right_edge - viewport + THUMB_MARGIN; }
                new_scroll = new_scroll.max(0);
                if let Ok(mut g) = PANEL_SCROLL_OFFSET.lock() { *g = new_scroll; }
                if let Some(hwnd_val) = PANEL_HWND.lock().ok().and_then(|g| *g) {
                    unsafe { let _ = windows::Win32::Graphics::Gdi::InvalidateRect(HWND(hwnd_val as *mut _), None, true); }
                }
                return LRESULT(0);
            }
            DefWindowProcW(hwnd, msg, wparam, lparam)
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

pub fn set_scrollbar_visible(visible: bool) {
    if let Ok(mut show) = PANEL_SHOW_SCROLLBAR.lock() { *show = visible; }
    request_repaint();
}

fn show_panel_with_close_on_mouse_leave(close_on_mouse_leave: bool) {
    unsafe {
        if let Ok(v) = PANEL_VISIBLE.lock() {
            if *v { return; }
        }
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

        let side = PANEL_SIDE.lock().map(|value| *value).unwrap_or(PanelSide::Top);
        let work_width = mi.rcWork.right - mi.rcWork.left;
        let work_height = mi.rcWork.bottom - mi.rcWork.top;
        let vertical = side != PanelSide::Top;
        let width = if vertical { THUMB_W + THUMB_MARGIN * 2 + 3 } else { work_width.max(THUMB_W + THUMB_MARGIN * 2) };
        let height = if vertical { work_height } else { THUMB_H + THUMB_MARGIN * 2 + 3 };
        let target_x = match side {
            PanelSide::Top => mi.rcWork.left,
            PanelSide::Left => mi.rcMonitor.left,
            PanelSide::Right => mi.rcMonitor.right - width,
        };
        let target_y = if vertical { mi.rcWork.top } else { mi.rcMonitor.top };
        let start_x = match side {
            PanelSide::Left => mi.rcMonitor.left - width,
            _ => target_x,
        };
        let start_y = if side == PanelSide::Top { mi.rcMonitor.top - height } else { target_y };
        let viewport = if vertical { height } else { width };
        let item_size = if vertical { THUMB_H } else { THUMB_W };
        let item_extent = item_size + THUMB_MARGIN;

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
                    let item_x = idx as i32 * item_extent + THUMB_MARGIN;
                    let center_scroll = (item_x + item_size / 2) - (viewport / 2);
                    let max_scroll = (thumbnail_content_extent(count, vertical) - viewport).max(0);
                    let scroll = center_scroll.clamp(0, max_scroll);
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
                WS_EX_TOPMOST | WS_EX_TOOLWINDOW,
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
            if let Ok(mut g) = PANEL_HINSTANCE.lock() { *g = Some(hinstance.0 as isize); }
            if let Ok(mut g) = PANEL_CLASS.lock() { *g = Some(class_name_wide.clone()); }
            hwnd
        };

        if let Ok(mut g) = PANEL_X.lock() { *g = target_x; }
        if let Ok(mut g) = PANEL_WIDTH.lock() { *g = width; }
        if let Ok(mut g) = PANEL_HEIGHT.lock() { *g = height; }
        if let Ok(mut g) = PANEL_SCROLL_OFFSET.lock() { *g = initial_scroll; }
        if let Ok(mut g) = PANEL_SELECTED_INDEX.lock() { *g = Some(selected_idx); }
        if let Ok(mut g) = PANEL_HOVER_INDEX.lock() { *g = Some(selected_idx); }
        if let Ok(mut g) = PANEL_DRAGGING.lock() { *g = false; }
        if let Ok(mut gs) = PANEL_HOVER_SCALE.lock() { *gs = 1.0; }

        if image_count > 0 {
            if let Ok(mut s) = PANEL_THUMB_SCALES.lock() {
                s.resize(image_count, 1.0);
                for (i, v) in s.iter_mut().enumerate() {
                    *v = if i == selected_idx { 0.9 } else { 1.0 };
                }
            }
            if let Ok(mut t) = PANEL_THUMB_TARGETS.lock() {
                t.resize(image_count, 1.0);
                for (i, v) in t.iter_mut().enumerate() {
                    *v = if i == selected_idx { 0.9 } else { 1.0 };
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
            let height = if let Ok(g) = PANEL_HEIGHT.lock() { *g } else { 8 + THUMB_H + 8 + 3 };
            let width = if let Ok(g) = PANEL_WIDTH.lock() { *g } else { THUMB_W + THUMB_MARGIN * 2 + 3 };
            let hmon = windows::Win32::Graphics::Gdi::MonitorFromWindow(hwnd, windows::Win32::Graphics::Gdi::MONITOR_DEFAULTTONEAREST);
            let mut mi = MONITORINFO { cbSize: std::mem::size_of::<MONITORINFO>() as u32, ..Default::default() };
            let _ = windows::Win32::Graphics::Gdi::GetMonitorInfoW(hmon, &mut mi);
            let (target_x, target_y) = match side {
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
