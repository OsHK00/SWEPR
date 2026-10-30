use crate::app;
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
    CreateWindowExW, DefWindowProcW, DestroyWindow, GetCursorPos, GetWindowRect, KillTimer, LoadCursorW, RegisterClassW, SetCursor, SetTimer, SetWindowPos, ShowWindow, UnregisterClassW,
    CS_HREDRAW, CS_VREDRAW, HWND_TOPMOST, IDC_ARROW, SWP_NOACTIVATE, SWP_NOZORDER, SW_SHOWNA, WM_CREATE, WM_DESTROY, WM_ERASEBKGND, WM_KEYDOWN, WM_KILLFOCUS, WM_LBUTTONDOWN, WM_LBUTTONUP, WM_MOUSEMOVE, WM_NCHITTEST, WM_PAINT, WM_SETCURSOR, WM_TIMER, WNDCLASSW,
    WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW, WS_EX_TOPMOST, WS_POPUP,
};

const PANEL_CLASS_NAME: &str = "SWEPR_PanelWindowClass";
const ANIM_TIMER_ID: usize = 8001;
const AUTO_CLOSE_TIMER_ID: usize = 8002;
const ANIM_DURATION_MS: i32 = 200;
const ANIM_STEP_MS: u32 = 16;
const AUTO_CLOSE_CHECK_MS: u32 = 100;
const THUMB_SIZE: i32 = 100;
const THUMB_MARGIN: i32 = 8;
const THUMB_LINE_THICK: i32 = 2;
const HOVER_SCALE: f32 = 1.1;
const HOVER_ANIM_TIMER_ID: usize = 8003;
const HOVER_ANIM_DURATION_MS: i32 = 250;
const HOVER_ANIM_STEP_MS: u32 = 16;

#[derive(Clone)]
struct PanelState {
    target_y: i32,
    start_y: i32,
    current_y: i32,
    anim_start_time: std::time::Instant,
    animating: bool,
}

struct HoverAnim {
    start_scale: f32,
    target_scale: f32,
    start_time: std::time::Instant,
    animating: bool,
}

static PANEL_STATE: Mutex<Option<PanelState>> = Mutex::new(None);
static PANEL_HWND: Mutex<Option<isize>> = Mutex::new(None);
static PANEL_HINSTANCE: Mutex<Option<isize>> = Mutex::new(None);
static PANEL_CLASS: Mutex<Option<Vec<u16>>> = Mutex::new(None);
static PANEL_X: Mutex<i32> = Mutex::new(0);
static PANEL_WIDTH: Mutex<i32> = Mutex::new(800);
static PANEL_HEIGHT: Mutex<i32> = Mutex::new(150);

static PANEL_THUMB_CACHE: Mutex<Option<ThumbnailCache>> = Mutex::new(None);
static PANEL_SCANNED: Mutex<Option<Arc<Mutex<Vec<ScannedImage>>>>> = Mutex::new(None);
static PANEL_SCROLL_OFFSET: Mutex<i32> = Mutex::new(0);
static PANEL_HOVER_INDEX: Mutex<Option<usize>> = Mutex::new(None);
static PANEL_DRAGGING: Mutex<bool> = Mutex::new(false);
static PANEL_LAST_MOUSE_X: Mutex<i32> = Mutex::new(0);
static PANEL_PENDING_APPLY: Mutex<Option<usize>> = Mutex::new(None);
static PANEL_HOVER_SCALE: Mutex<f32> = Mutex::new(1.0);
static PANEL_HOVER_ANIM: Mutex<Option<HoverAnim>> = Mutex::new(None);
static PANEL_CURRENT_WALLPAPER: Mutex<Option<std::path::PathBuf>> = Mutex::new(None);

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
                if let Ok(mut stg) = PANEL_STATE.lock() {
                    if let Some(ref mut st) = *stg {
                        if st.animating {
                            let elapsed = st.anim_start_time.elapsed().as_millis() as f32;
                            let progress = (elapsed / (ANIM_DURATION_MS as f32)).min(1.0);
                            let eased = 1.0 - (1.0 - progress).powi(3);
                            let y = st.start_y + ((st.target_y - st.start_y) as f32 * eased) as i32;
                            st.current_y = y;
                            let px = if let Ok(g) = PANEL_X.lock() { *g } else { 0 };
                            let pw = if let Ok(g) = PANEL_WIDTH.lock() { *g } else { 800 };
                            let ph = if let Ok(g) = PANEL_HEIGHT.lock() { *g } else { 150 };
                            let _ = SetWindowPos(hwnd, HWND_TOPMOST, px, y, pw, ph, SWP_NOACTIVATE | SWP_NOZORDER);
                            if progress >= 1.0 {
                                st.animating = false;
                                let _ = KillTimer(hwnd, ANIM_TIMER_ID);
                            }
                        }
                    }
                }
                return LRESULT(0);
            }
            if wparam.0 == HOVER_ANIM_TIMER_ID {
                if let Ok(mut ha) = PANEL_HOVER_ANIM.lock() {
                    if let Some(ref mut a) = *ha {
                        if a.animating {
                            let elapsed = a.start_time.elapsed().as_millis() as f32;
                            let progress = (elapsed / (HOVER_ANIM_DURATION_MS as f32)).min(1.0);
                            let eased = 1.0 - (1.0 - progress).powi(2);
                            let s = a.start_scale + (a.target_scale - a.start_scale) * eased;
                            if let Ok(mut gs) = PANEL_HOVER_SCALE.lock() { *gs = s; }
                            if let Ok(mut stg) = PANEL_STATE.lock() {
                                if let Some(ref mut st) = *stg {
                                    let px = if let Ok(g) = PANEL_X.lock() { *g } else { 0 };
                                    let pw = if let Ok(g) = PANEL_WIDTH.lock() { *g } else { 800 };
                                    let panel_h = (8.0 + THUMB_SIZE as f32 * s + THUMB_LINE_THICK as f32) as i32;
                                    let _ = SetWindowPos(hwnd, HWND_TOPMOST, px, st.current_y, pw, panel_h, SWP_NOACTIVATE | SWP_NOZORDER);
                                    if progress >= 1.0 {
                                        a.animating = false;
                                        let _ = KillTimer(hwnd, HOVER_ANIM_TIMER_ID);
                                        if a.target_scale == 1.0 {
                                            let _ = SetWindowPos(hwnd, HWND_TOPMOST, px, st.current_y, pw, 8 + THUMB_SIZE, SWP_NOACTIVATE | SWP_NOZORDER);
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
                return LRESULT(0);
            }
            if wparam.0 == AUTO_CLOSE_TIMER_ID {
                // Verificar si el cursor está fuera del panel
                let mut pt = POINT::default();
                let _ = GetCursorPos(&mut pt);
                let mut rect = RECT::default();
                let _ = GetWindowRect(hwnd, &mut rect);
                if pt.x < rect.left || pt.x > rect.right || pt.y < rect.top || pt.y > rect.bottom {
                    // Cursor fuera del panel -> cerrar
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
                let hover = if let Ok(g) = PANEL_HOVER_INDEX.lock() { *g } else { None };
                let hover_scale = if let Ok(g) = PANEL_HOVER_SCALE.lock() { *g } else { 1.0 };
                let current_wp = if let Ok(g) = PANEL_CURRENT_WALLPAPER.lock() { g.clone() } else { None };


                if let (Some(cache), Some(images_arc)) = (cache_opt, images_opt) {
                    if let Ok(images) = images_arc.lock() {
                        let mut x = THUMB_MARGIN - scroll;
                        for (idx, img) in images.iter().enumerate() {
                            let key = img.path.to_string_lossy().to_string();
                            let scale = if hover == Some(idx) { hover_scale } else { 1.0 };
                            let tw = (THUMB_SIZE as f32 * scale) as i32;
                            let draw_w = if hover == Some(idx) { tw } else { THUMB_SIZE };
                            let radius = 8i32;
                            let clip_rgn = CreateRoundRectRgn(x, 8, x + draw_w, 8 + draw_w, radius * 2, radius * 2);
                            let _ = SelectClipRgn(mem_dc, clip_rgn);
                            if let Some(thumb) = cache.get(&key) {
                                let bmp_dc = CreateCompatibleDC(hdc);
                                let old = SelectObject(bmp_dc, windows::Win32::Graphics::Gdi::HBITMAP(thumb.hbitmap as *mut _));
                                let _ = SetStretchBltMode(mem_dc, COLORONCOLOR);
                                let _ = StretchBlt(mem_dc, x, 8, draw_w, draw_w, bmp_dc, 0, 0, thumb.width, thumb.height, SRCCOPY);
                                SelectObject(bmp_dc, old);
                                let _ = DeleteDC(bmp_dc);
                            } else {
                                let ph = CreateSolidBrush(windows::Win32::Foundation::COLORREF(0x00303030));
                                let r = RECT { left: x, top: 8, right: x + draw_w, bottom: 8 + draw_w };
                                let _ = FillRect(mem_dc, &r, ph);
                                let _ = DeleteObject(ph);
                            }
                            let _ = SelectClipRgn(mem_dc, windows::Win32::Graphics::Gdi::HRGN(std::ptr::null_mut()));
                            let _ = DeleteObject(clip_rgn);
                            let is_current = current_wp.as_ref().map(|p| p == &img.path).unwrap_or(false);
                            if is_current || hover == Some(idx) {
                                let color = if hover == Some(idx) {
                                    windows::Win32::Foundation::COLORREF(0x00FFFFFF)
                                } else {
                                    windows::Win32::Foundation::COLORREF(0x0000FF00)
                                };
                                let rgn = CreateRoundRectRgn(x, 8, x + draw_w, 8 + draw_w, radius * 2, radius * 2);
                                let brush = CreateSolidBrush(color);
                                let _ = FrameRgn(mem_dc, rgn, brush, 2, 2);
                                let _ = DeleteObject(brush);
                                let _ = DeleteObject(rgn);
                            }
                            x += THUMB_SIZE + THUMB_MARGIN;
                        }
                        let total_w = images.len() as i32 * (THUMB_SIZE + THUMB_MARGIN) + THUMB_MARGIN;
                        if total_w > width {
                            let track_y = height - 4;
                            let track_h = 3;
                            let track_brush = CreateSolidBrush(windows::Win32::Foundation::COLORREF(0x00404040));
                            let track_rect = RECT { left: 0, top: track_y, right: width, bottom: track_y + track_h };
                            let _ = FillRect(mem_dc, &track_rect, track_brush);
                            let _ = DeleteObject(track_brush);

                            let max_scroll = (total_w - width).max(1);
                            let mut thumb_w = ((width as f32 / total_w as f32) * width as f32) as i32;
                            thumb_w = thumb_w.max(20).min(width);
                            let thumb_x = ((scroll as f32 / max_scroll as f32) * (width - thumb_w) as f32) as i32;
                            let thumb_brush = CreateSolidBrush(windows::Win32::Foundation::COLORREF(0x00FFFFFF));
                            let thumb_rect = RECT { left: thumb_x, top: track_y, right: thumb_x + thumb_w, bottom: track_y + track_h };
                            let _ = FillRect(mem_dc, &thumb_rect, thumb_brush);
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
            // Hit testing
            let mut pending_apply: Option<usize> = None;
            let scroll = if let Ok(g) = PANEL_SCROLL_OFFSET.lock() { *g } else { 0 };
            let hover_idx = if let Ok(g) = PANEL_HOVER_INDEX.lock() { *g } else { None };
            let hover_scale = if let Ok(g) = PANEL_HOVER_SCALE.lock() { *g } else { 1.0 };
            let images_opt = if let Ok(g) = PANEL_SCANNED.lock() { g.clone() } else { None };
            if let Some(images_arc) = images_opt {
                if let Ok(images) = images_arc.lock() {
                    let mut tx = THUMB_MARGIN - scroll;
                    for (idx, img) in images.iter().enumerate() {
                        let tw = (THUMB_SIZE as f32 * if Some(idx) == hover_idx { hover_scale } else { 1.0 }) as i32;
                        if x >= tx && x < tx + tw && y >= 8 && y < 8 + tw {
                            log_info!("Panel: clic en miniatura {} -> {:?}", idx, img.path);
                            pending_apply = Some(idx);
                        }
                        tx += THUMB_SIZE + THUMB_MARGIN;
                    }
                }
            }
            // Preparar: pending_apply indica miniatura a aplicar en click; el drag-scroll se decide en MOUSEMOVE/LBUTTONUP
            if let Ok(mut g) = PANEL_DRAGGING.lock() { *g = true; }
            if let Ok(mut g) = PANEL_LAST_MOUSE_X.lock() { *g = x; }
            if let Ok(mut g) = PANEL_PENDING_APPLY.lock() { *g = pending_apply; }
            LRESULT(0)
        }
        WM_LBUTTONUP => {
            if let Ok(mut g) = PANEL_DRAGGING.lock() { *g = false; }
            // Si fue un click (sin arrastre) sobre una miniatura, aplicar el wallpaper
            let pending = if let Ok(mut g) = PANEL_PENDING_APPLY.lock() { g.take() } else { None };
            let last_x = if let Ok(g) = PANEL_LAST_MOUSE_X.lock() { *g } else { 0 };
            let x = (lparam.0 & 0xFFFF) as i16 as i32;
            if let Some(idx) = pending {
                if (x - last_x).abs() <= 3 {
                    // Click completo detectado -> aplicar wallpaper
                    let path = if let Ok(images_arc) = PANEL_SCANNED.lock() {
                        if let Some(images) = images_arc.clone() {
                            if let Ok(images) = images.lock() {
                                images.get(idx).map(|img| img.path.clone())
                            } else { None }
                        } else { None }
                    } else { None };
                    if let Some(p) = path {
                        log_info!("Panel: clic en miniatura {} -> {:?}", idx, p);
                        app::request_apply_wallpaper(&p);
                        hide_panel();
                    }
                }
            }
            LRESULT(0)
        }
        WM_MOUSEMOVE => {
            let x = (lparam.0 & 0xFFFF) as i16 as i32;
            // Hover
            let scroll = if let Ok(g) = PANEL_SCROLL_OFFSET.lock() { *g } else { 0 };
            let hover_idx = if let Ok(g) = PANEL_HOVER_INDEX.lock() { *g } else { None };
            let hover_scale = if let Ok(g) = PANEL_HOVER_SCALE.lock() { *g } else { 1.0 };
            let images_opt = if let Ok(g) = PANEL_SCANNED.lock() { g.clone() } else { None };
            let mut new_hover = None;
            if let Some(images_arc) = images_opt {
                if let Ok(images) = images_arc.lock() {
                    let mut tx = THUMB_MARGIN - scroll;
                    for (idx, _img) in images.iter().enumerate() {
                        let tw = (THUMB_SIZE as f32 * if Some(idx) == hover_idx { hover_scale } else { 1.0 }) as i32;
                        if x >= tx && x < tx + tw {
                            new_hover = Some(idx);
                            break;
                        }
                        tx += THUMB_SIZE + THUMB_MARGIN;
                    }
                }
            }
            let prev_hover = if let Ok(g) = PANEL_HOVER_INDEX.lock() { *g } else { None };
            if let Ok(mut g) = PANEL_HOVER_INDEX.lock() { *g = new_hover; }

            // Animación del hover: la miniatura con línea baja un poco hacia abajo
            if prev_hover != new_hover {
                let target = if new_hover.is_some() { HOVER_SCALE } else { 1.0 };
                let cur = if let Ok(g) = PANEL_HOVER_SCALE.lock() { *g } else { 1.0 };
                if let Ok(mut ha) = PANEL_HOVER_ANIM.lock() {
                    *ha = Some(HoverAnim { start_scale: cur, target_scale: target, start_time: std::time::Instant::now(), animating: true });
                }
                let _ = SetTimer(hwnd, HOVER_ANIM_TIMER_ID, HOVER_ANIM_STEP_MS, None);
            }

            // Drag scroll (solo si está arrastrando)
            let dragging = if let Ok(g) = PANEL_DRAGGING.lock() { *g } else { false };
            if dragging {
                // Si hubo movimiento real, ya no es un click sobre la miniatura
                if let Ok(mut g) = PANEL_PENDING_APPLY.lock() { *g = None; }
                let last = if let Ok(g) = PANEL_LAST_MOUSE_X.lock() { *g } else { 0 };
                let delta = last - x;
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
                    let panel_w = PANEL_WIDTH.lock().map(|g| *g).unwrap_or(800);
                    let max_scroll = (image_count as i32 * (THUMB_SIZE + THUMB_MARGIN) - panel_w).max(0);
                    *g = (*g + delta).max(0).min(max_scroll);
                }
                if let Ok(mut g) = PANEL_LAST_MOUSE_X.lock() { *g = x; }
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
                let panel_w = PANEL_WIDTH.lock().map(|g| *g).unwrap_or(800);
                let max_scroll = (image_count as i32 * (THUMB_SIZE + THUMB_MARGIN) - panel_w).max(0);
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
            DefWindowProcW(hwnd, msg, wparam, lparam)
        }
        WM_KILLFOCUS => {
            // Cerrar al perder foco
            hide_panel();
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

pub fn show_panel() {
    unsafe {
        if let Ok(guard) = PANEL_HWND.lock() {
            if guard.is_some() {
                return;
            }
        }
        let hinstance = match GetModuleHandleW(None) {
            Ok(h) => HINSTANCE(h.0),
            Err(_) => return,
        };
        let class_name_wide = to_wide_chars(PANEL_CLASS_NAME);
        let wc = WNDCLASSW {
            style: CS_HREDRAW | CS_VREDRAW,
            lpfnWndProc: Some(panel_wnd_proc),
            hInstance: hinstance,
            lpszClassName: as_pcwstr(&class_name_wide),
            ..Default::default()
        };
        let _ = RegisterClassW(&wc);

        let mut pt = POINT::default();
        let _ = GetCursorPos(&mut pt);
        let hmon = MonitorFromPoint(pt, MONITOR_DEFAULTTONEAREST);
        let mut mi = MONITORINFO {
            cbSize: std::mem::size_of::<MONITORINFO>() as u32,
            ..Default::default()
        };
        let _ = GetMonitorInfoW(hmon, &mut mi);

        let width = 800i32;
        // Altura ajustada al contenido (margen superior + miniatura), sin espacio muerto
        let height = 8 + THUMB_SIZE;
        let x = mi.rcMonitor.left + (mi.rcWork.right - mi.rcWork.left - width) / 2;
        let start_y = mi.rcMonitor.top - height;
        let target_y = mi.rcMonitor.top;

        let hwnd = match CreateWindowExW(
            WS_EX_TOPMOST | WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE,
            as_pcwstr(&class_name_wide),
            as_pcwstr(&to_wide_chars("SWEPR Panel")),
            WS_POPUP,
            x,
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

        if let Ok(mut stg) = PANEL_STATE.lock() {
            *stg = Some(PanelState {
                target_y,
                start_y,
                current_y: start_y,
                anim_start_time: std::time::Instant::now(),
                animating: true,
            });
        }
        if let Ok(mut g) = PANEL_HWND.lock() { *g = Some(hwnd.0 as isize); }
        if let Ok(mut g) = PANEL_HINSTANCE.lock() { *g = Some(hinstance.0 as isize); }
        if let Ok(mut g) = PANEL_CLASS.lock() { *g = Some(class_name_wide.clone()); }
        if let Ok(mut g) = PANEL_X.lock() { *g = x; }
        if let Ok(mut g) = PANEL_WIDTH.lock() { *g = width; }
        if let Ok(mut g) = PANEL_HEIGHT.lock() { *g = height; }
        if let Ok(mut g) = PANEL_SCROLL_OFFSET.lock() { *g = 0; }
        if let Ok(mut g) = PANEL_HOVER_INDEX.lock() { *g = None; }
        if let Ok(mut g) = PANEL_DRAGGING.lock() { *g = false; }
        if let Ok(mut gs) = PANEL_HOVER_SCALE.lock() { *gs = 1.0; }
        if let Ok(mut ha) = PANEL_HOVER_ANIM.lock() { *ha = None; }

        let _ = SetWindowPos(hwnd, HWND_TOPMOST, x, start_y, width, height, SWP_NOACTIVATE | SWP_NOZORDER);
        let _ = ShowWindow(hwnd, SW_SHOWNA);
        let _ = SetTimer(hwnd, ANIM_TIMER_ID, ANIM_STEP_MS, None);
        let _ = SetTimer(hwnd, AUTO_CLOSE_TIMER_ID, AUTO_CLOSE_CHECK_MS, None);
        log_info!("Panel: mostrado en monitor con animación desde arriba");
    }
}

pub fn hide_panel() {
    unsafe {
        let hwnd_val = if let Ok(mut g) = PANEL_HWND.lock() { g.take() } else { None };
        let hinst_val = if let Ok(mut g) = PANEL_HINSTANCE.lock() { g.take() } else { None };
        let class_val = if let Ok(mut g) = PANEL_CLASS.lock() { g.take() } else { None };
        if let Ok(mut stg) = PANEL_STATE.lock() { *stg = None; }
        if let Ok(mut gs) = PANEL_HOVER_SCALE.lock() { *gs = 1.0; }
        if let Ok(mut ha) = PANEL_HOVER_ANIM.lock() { *ha = None; }
        if let Some(hv) = hwnd_val {
            let hwnd = HWND(hv as *mut _);
            let _ = KillTimer(hwnd, ANIM_TIMER_ID);
            let _ = KillTimer(hwnd, AUTO_CLOSE_TIMER_ID);
            let _ = KillTimer(hwnd, HOVER_ANIM_TIMER_ID);
            let _ = DestroyWindow(hwnd);
            if let (Some(iv), Some(cv)) = (hinst_val, class_val) {
                let hinst = HINSTANCE(iv as *mut _);
                let _ = UnregisterClassW(as_pcwstr(&cv), hinst);
            }
            log_info!("Panel: ocultado y destruido");
        }
    }
}

pub fn set_draw_context(cache: ThumbnailCache, images: Arc<Mutex<Vec<ScannedImage>>>) {
    if let Ok(mut g) = PANEL_THUMB_CACHE.lock() { *g = Some(cache); }
    if let Ok(mut g) = PANEL_SCANNED.lock() { *g = Some(images); }
    // Forzar repintado
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
