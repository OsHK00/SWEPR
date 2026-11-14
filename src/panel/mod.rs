mod carousel;

use crate::app;
use crate::config::{PanelMode, PanelSide, ProPriority};
use crate::log_info;
use crate::scanner::ScannedImage;
use crate::thumbs::ThumbnailCache;
use crate::winutil::{as_pcwstr, same_path, to_wide_chars};
use std::cell::{Cell, RefCell};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use windows::Win32::Foundation::{COLORREF, HINSTANCE, HWND, LPARAM, LRESULT, POINT, RECT, WPARAM};
use windows::Win32::Graphics::Gdi::{
    BeginPaint, BitBlt, CreateCompatibleBitmap, CreateCompatibleDC, CreateRoundRectRgn, CreateSolidBrush, DeleteDC, DeleteObject, EndPaint, FillRect, FrameRgn, GetMonitorInfoW, MonitorFromPoint, SelectClipRgn, SelectObject, SetStretchBltMode, StretchBlt, COLORONCOLOR, HDC, MONITOR_DEFAULTTONEAREST, MONITORINFO, SRCCOPY,
};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, GetCursorPos, GetWindowRect, KillTimer, LoadCursorW, RegisterClassW, SetCursor, SetTimer, SetWindowPos, ShowWindow,
    CS_HREDRAW, CS_VREDRAW, HWND_TOPMOST, IDC_ARROW, SWP_NOACTIVATE, SWP_NOZORDER, SW_SHOW, WM_CREATE, WM_DESTROY, WM_ERASEBKGND, WM_KEYDOWN, WM_KILLFOCUS, WM_LBUTTONDOWN, WM_LBUTTONUP, WM_MOUSEMOVE, WM_NCHITTEST, WM_PAINT, WM_RBUTTONUP, WM_SETCURSOR, WM_TIMER, WNDCLASSW,
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
/// Estrella de las favoritas (dorado).
const COLOR_FAVORITE: COLORREF = COLORREF(0x0030C8FF);
const BASE_NEW_BADGE: i32 = 10;

/// Máximo de miniaturas pequeñas en memoria en modo bajo demanda.
const ON_DEMAND_CAPACITY: usize = 200;

const HOVER_ANIM_TIMER_ID: usize = 8003;
const HOVER_ANIM_STEP_MS: u32 = 16;
/// Tras cerrar el carrusel, sus tarjetas grandes se liberan pasado este tiempo.
const TILE_RELEASE_TIMER_ID: usize = 8004;
const TILE_RELEASE_DELAY_MS: u32 = 30_000;
/// Desplazamiento vertical de la animación de entrada/salida del carrusel.
const CAROUSEL_SLIDE_PX: i32 = 24;

// Menú contextual de una miniatura.
const IDM_APPLY: usize = 1;
const IDM_APPLY_MONITOR: usize = 2;
const IDM_LOCK_SCREEN: usize = 3;
const IDM_FAVORITE: usize = 4;
const IDM_REVEAL: usize = 5;
const IDM_EXCLUDE: usize = 6;

/// Botones que aparecen sobre la miniatura o tarjeta con el foco.
const BASE_BUTTON: i32 = 24;
const BASE_BUTTON_GAP: i32 = 6;
const COLOR_BUTTON_BG: COLORREF = COLORREF(0x00202020);
const COLOR_BUTTON_HOT: COLORREF = COLORREF(0x00484848);
const COLOR_BUTTON_BORDER: COLORREF = COLORREF(0x00707070);
const COLOR_BUTTON_GLYPH: COLORREF = COLORREF(0x00E8E8E8);

/// Botón sobre una imagen.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum ItemButton {
    Favorite,
    Exclude,
}

/// Botones dibujados en el último fotograma: imagen y rectángulos (favorita, quitar).
#[derive(Clone, Copy, Debug)]
pub(crate) struct ItemButtons {
    pub index: usize,
    pub favorite: RECT,
    pub exclude: RECT,
}

impl ItemButtons {
    fn at(&self, x: i32, y: i32) -> Option<ItemButton> {
        let inside = |r: &RECT| x >= r.left && x < r.right && y >= r.top && y < r.bottom;
        if inside(&self.favorite) {
            Some(ItemButton::Favorite)
        } else if inside(&self.exclude) {
            Some(ItemButton::Exclude)
        } else {
            None
        }
    }
}

/// Animación de entrada o salida del panel.
#[derive(Clone, Copy)]
struct Slide {
    target_x: i32,
    start_x: i32,
    target_y: i32,
    start_y: i32,
    anim_start_time: std::time::Instant,
}

/// Lienzo fuera de pantalla: se crea al pintar la primera vez y se reutiliza en cada fotograma
/// hasta que el panel se oculta.
#[derive(Clone, Copy)]
struct Backbuffer {
    dc: HDC,
    bitmap: windows::Win32::Graphics::Gdi::HBITMAP,
    old: windows::Win32::Graphics::Gdi::HGDIOBJ,
    width: i32,
    height: i32,
}

/// Todo el estado del panel. Solo lo usa el hilo de interfaz (el panel, la ventana principal y
/// la de configuración comparten hilo), así que vive en un `thread_local` con `Cell`/`RefCell`:
/// sin mutex, sin envenenamiento y sin riesgo de bloqueo al reentrar en el procedimiento de ventana.
struct PanelVars {
    slide: Cell<Option<Slide>>,
    hwnd: Cell<Option<HWND>>,
    visible: Cell<bool>,
    exiting: Cell<bool>,
    close_on_mouse_leave: Cell<bool>,
    /// Hay un menú contextual abierto: el panel no debe cerrarse solo.
    menu_open: Cell<bool>,
    side: Cell<PanelSide>,
    mode: Cell<PanelMode>,
    show_scrollbar: Cell<bool>,
    margin: Cell<u32>,
    width: Cell<i32>,
    height: Cell<i32>,
    dpi: Cell<u32>,
    thumbs_on_demand: Cell<bool>,
    thumb_size_px: Cell<i32>,
    thumb_cache: RefCell<Option<ThumbnailCache>>,
    scanned: RefCell<Option<Arc<Mutex<Vec<ScannedImage>>>>>,
    scroll: Cell<i32>,
    /// Scroll hacia el que se anima el carrusel al cambiar la selección.
    scroll_target: Cell<Option<i32>>,
    hover: Cell<Option<usize>>,
    selected: Cell<Option<usize>>,
    dragging: Cell<bool>,
    last_mouse: Cell<i32>,
    drag_start: Cell<(i32, i32)>,
    scales: RefCell<Vec<f32>>,
    targets: RefCell<Vec<f32>>,
    current_wallpaper: RefCell<Option<PathBuf>>,
    /// Posición virtual sin límite; el índice visible sigue siendo este valor módulo la cantidad.
    carousel_index: Cell<i64>,
    backbuffer: Cell<Option<Backbuffer>>,
    /// Hay animación de escala o desplazamiento en curso.
    animating: Cell<bool>,
    /// Momento del último paso de animación (los pasos dependen del tiempo, no de los ticks).
    anim_last: Cell<Option<std::time::Instant>>,
    /// Botones de la imagen con el foco, tal como se pintaron.
    buttons: Cell<Option<ItemButtons>>,
    /// Botón bajo el ratón.
    hot_button: Cell<Option<ItemButton>>,
}

impl Default for PanelVars {
    fn default() -> Self {
        Self {
            slide: Cell::new(None),
            hwnd: Cell::new(None),
            visible: Cell::new(false),
            exiting: Cell::new(false),
            close_on_mouse_leave: Cell::new(true),
            menu_open: Cell::new(false),
            side: Cell::new(PanelSide::Top),
            mode: Cell::new(PanelMode::LowPower),
            show_scrollbar: Cell::new(true),
            margin: Cell::new(0),
            width: Cell::new(800),
            height: Cell::new(150),
            dpi: Cell::new(96),
            thumbs_on_demand: Cell::new(false),
            thumb_size_px: Cell::new(128),
            thumb_cache: RefCell::new(None),
            scanned: RefCell::new(None),
            scroll: Cell::new(0),
            scroll_target: Cell::new(None),
            hover: Cell::new(None),
            selected: Cell::new(None),
            dragging: Cell::new(false),
            last_mouse: Cell::new(0),
            drag_start: Cell::new((0, 0)),
            scales: RefCell::new(Vec::new()),
            targets: RefCell::new(Vec::new()),
            current_wallpaper: RefCell::new(None),
            carousel_index: Cell::new(0),
            backbuffer: Cell::new(None),
            animating: Cell::new(false),
            anim_last: Cell::new(None),
            buttons: Cell::new(None),
            hot_button: Cell::new(None),
        }
    }
}

thread_local! {
    static VARS: PanelVars = PanelVars::default();
}

/// Acceso al estado del panel.
fn vars<R>(f: impl FnOnce(&PanelVars) -> R) -> R {
    VARS.with(f)
}

fn px(value: i32) -> i32 {
    value * vars(|v| v.dpi.get()) as i32 / 96
}
fn thumb_w() -> i32 { px(BASE_THUMB_W) }
fn thumb_h() -> i32 { px(BASE_THUMB_H) }
fn thumb_margin() -> i32 { px(BASE_THUMB_MARGIN) }
fn scrollbar_size() -> i32 { px(BASE_SCROLLBAR).max(1) }

fn is_carousel() -> bool {
    vars(|v| v.mode.get() == PanelMode::Pro)
}

fn is_vertical() -> bool {
    !is_carousel() && matches!(vars(|v| v.side.get()), PanelSide::Left | PanelSide::Right)
}

/// Escala de la miniatura con foco: el carrusel la agranda, las tiras la encogen.
fn focus_scale() -> f32 {
    if is_carousel() { carousel::FOCUS_SCALE } else { 0.9 }
}

fn scanned() -> Option<Arc<Mutex<Vec<ScannedImage>>>> {
    vars(|v| v.scanned.borrow().clone())
}

fn image_count() -> usize {
    scanned().and_then(|arc| arc.lock().ok().map(|images| images.len())).unwrap_or(0)
}

/// Ruta de la imagen en `index`.
fn image_path(index: usize) -> Option<PathBuf> {
    scanned().and_then(|arc| arc.lock().ok().and_then(|images| images.get(index).map(|img| img.path.clone())))
}

fn viewport() -> i32 {
    if is_vertical() { vars(|v| v.height.get()) } else { vars(|v| v.width.get()) }
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
    let scroll = vars(|v| v.scroll.get());
    if let Some(layout) = carousel::layout().filter(|_| is_carousel()) {
        return layout.loop_hit_test(x, y, count, scroll)
            .map(|virtual_index| virtual_index.rem_euclid(count.max(1) as i64) as usize);
    }
    let vertical = is_vertical();
    (0..count).find(|&idx| {
        let (tx, ty) = thumbnail_origin(idx, count, viewport(), scroll, vertical);
        x >= tx && x < tx + thumb_w() && y >= ty && y < ty + thumb_h()
    })
}

/// Índice siguiente o anterior con vuelta: tras la última viene la primera y al revés.
fn wrapped_step(sel: usize, count: usize, forward: bool) -> usize {
    if forward { (sel + 1) % count } else { (sel + count - 1) % count }
}

/// Mueve la selección una posición con vuelta al principio/final.
fn step_selection(hwnd: HWND, forward: bool) {
    let count = image_count();
    if count == 0 { return; }
    let sel = vars(|v| v.selected.get()).unwrap_or(0).min(count - 1);
    if let Some(layout) = carousel::layout().filter(|_| is_carousel()) {
        let virtual_index = vars(|v| {
            let index = v.carousel_index.get() + if forward { 1 } else { -1 };
            v.carousel_index.set(index);
            index
        });
        let next = virtual_index.rem_euclid(count as i64) as usize;
        vars(|v| v.selected.set(Some(next)));
        set_hover_target(hwnd, Some(next));
        vars(|v| v.scroll_target.set(Some(layout.loop_center_scroll(virtual_index, viewport()))));
        start_animation(hwnd);
        return;
    }
    select_index(hwnd, wrapped_step(sel, count, forward));
}

/// Selecciona una miniatura y desplaza la vista para que se vea (centrada en el carrusel).
fn select_index(hwnd: HWND, sel: usize) {
    vars(|v| v.selected.set(Some(sel)));
    set_hover_target(hwnd, Some(sel));
    let count = image_count();
    if let Some(layout) = carousel::layout().filter(|_| is_carousel()) {
        let current = vars(|v| v.carousel_index.get());
        let scroll = vars(|v| v.scroll.get());
        let base = current.div_euclid(count.max(1) as i64) * count as i64 + sel as i64;
        let virtual_index = [base - count as i64, base, base + count as i64]
            .into_iter().min_by_key(|i| (layout.loop_center_scroll(*i, viewport()) - scroll).abs())
            .unwrap_or(base);
        vars(|v| {
            v.carousel_index.set(virtual_index);
            v.scroll_target.set(Some(layout.loop_center_scroll(virtual_index, viewport())));
        });
        start_animation(hwnd);
        return;
    }
    let vertical = is_vertical();
    let viewport = viewport();
    let scroll = vars(|v| v.scroll.get());
    let item_extent = if vertical { thumb_h() + thumb_margin() } else { thumb_w() + thumb_margin() };
    let item_size = if vertical { thumb_h() } else { thumb_w() };
    let item_x = sel as i32 * item_extent + thumb_margin();
    let mut new_scroll = scroll;
    if item_x < scroll { new_scroll = item_x.saturating_sub(thumb_margin()); }
    let right_edge = item_x + item_size;
    if right_edge > scroll + viewport { new_scroll = right_edge - viewport + thumb_margin(); }
    vars(|v| v.scroll.set(new_scroll.max(0)));
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
unsafe fn draw_new_badge(dc: HDC, right: i32, top: i32) {
    use windows::Win32::Graphics::Gdi::{CreateEllipticRgn, FillRgn};
    let size = px(BASE_NEW_BADGE);
    let inset = px(6);
    let rgn = CreateEllipticRgn(right - inset - size, top + inset, right - inset, top + inset + size);
    let brush = CreateSolidBrush(COLOR_NEW);
    let _ = FillRgn(dc, rgn, brush);
    let _ = DeleteObject(brush);
    let _ = DeleteObject(rgn);
}

/// Estrella dorada en la esquina superior izquierda de una favorita.
unsafe fn draw_favorite_badge(dc: HDC, left: i32, top: i32) {
    use windows::Win32::Graphics::Gdi::{CreatePolygonRgn, FillRgn, WINDING};
    let radius = px(BASE_NEW_BADGE) as f32 * 0.75;
    let inset = px(6) as f32;
    let (cx, cy) = (left as f32 + inset + radius, top as f32 + inset + radius);
    let points: Vec<POINT> = (0..10).map(|i| {
        let r = if i % 2 == 0 { radius } else { radius * 0.45 };
        let angle = -std::f32::consts::FRAC_PI_2 + i as f32 * std::f32::consts::PI / 5.0;
        POINT { x: (cx + r * angle.cos()).round() as i32, y: (cy + r * angle.sin()).round() as i32 }
    }).collect();
    let rgn = CreatePolygonRgn(&points, WINDING);
    let brush = CreateSolidBrush(COLOR_FAVORITE);
    let _ = FillRgn(dc, rgn, brush);
    let _ = DeleteObject(brush);
    let _ = DeleteObject(rgn);
}

/// Aviso centrado cuando la carpeta no tiene imágenes.
unsafe fn draw_empty_state(dc: HDC, width: i32, height: i32) {
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

/// Pone en marcha la animación de escala/desplazamiento (si no lo estaba ya).
fn start_animation(hwnd: HWND) {
    if vars(|v| v.animating.replace(true)) { return; }
    vars(|v| v.anim_last.set(Some(std::time::Instant::now())));
    unsafe { let _ = SetTimer(hwnd, HOVER_ANIM_TIMER_ID, HOVER_ANIM_STEP_MS, None); }
}

/// Avanza la animación según el tiempo transcurrido desde el paso anterior. Así va a la misma
/// velocidad aunque los ticks del temporizador se retrasen (por ejemplo, con el ratón moviéndose
/// rápido, Windows prioriza los mensajes del ratón y los WM_TIMER llegan tarde o no llegan).
fn advance_animation(hwnd: HWND) {
    if !vars(|v| v.animating.get()) { return; }
    let now = std::time::Instant::now();
    let elapsed = vars(|v| v.anim_last.replace(Some(now)))
        .map(|last| now.duration_since(last).as_secs_f32())
        .unwrap_or(1.0 / 60.0)
        .min(0.1);
    // Cada 1/60 s se recorre un 25 % de lo que falta (el mismo ritmo que antes a 60 fps).
    let factor = 1.0 - 0.75f32.powf(elapsed * 60.0);
    let mut done = true;
    if let Some(goal) = vars(|v| v.scroll_target.get()) {
        let scroll = vars(|v| v.scroll.get());
        let diff = goal - scroll;
        if diff.abs() <= 1 {
            vars(|v| {
                v.scroll.set(goal);
                v.scroll_target.set(None);
            });
        } else {
            let step = (diff as f32 * factor) as i32;
            vars(|v| v.scroll.set(scroll + if step == 0 { diff.signum() } else { step }));
            done = false;
        }
    }
    vars(|v| {
        let mut scales = v.scales.borrow_mut();
        let targets = v.targets.borrow();
        for (scale, target) in scales.iter_mut().zip(targets.iter()) {
            let diff = target - *scale;
            if diff.abs() > 0.002 {
                *scale += diff * factor;
                done = false;
            } else {
                *scale = *target;
            }
        }
    });
    request_repaint();
    if done {
        vars(|v| v.animating.set(false));
        unsafe { let _ = KillTimer(hwnd, HOVER_ANIM_TIMER_ID); }
    }
}

/// Dibuja los botones "favorita" y "quitar de la galería" en la esquina superior derecha.
unsafe fn draw_item_buttons(dc: HDC, index: usize, right: i32, top: i32, favorite: bool, hot: Option<ItemButton>) -> ItemButtons {
    use windows::Win32::Graphics::Gdi::{CreateEllipticRgn, CreatePen, CreatePolygonRgn, FillRgn, LineTo, MoveToEx, FrameRgn as Frame, PS_SOLID, WINDING};
    let size = px(BASE_BUTTON);
    let gap = px(BASE_BUTTON_GAP);
    let inset = px(6);
    let exclude = RECT { left: right - inset - size, top: top + inset, right: right - inset, bottom: top + inset + size };
    let favorite_rect = RECT { left: exclude.left - gap - size, top: exclude.top, right: exclude.left - gap, bottom: exclude.bottom };
    for (rect, button) in [(favorite_rect, ItemButton::Favorite), (exclude, ItemButton::Exclude)] {
        let rgn = CreateEllipticRgn(rect.left, rect.top, rect.right + 1, rect.bottom + 1);
        let fill = CreateSolidBrush(if hot == Some(button) { COLOR_BUTTON_HOT } else { COLOR_BUTTON_BG });
        let border = CreateSolidBrush(COLOR_BUTTON_BORDER);
        let _ = FillRgn(dc, rgn, fill);
        let _ = Frame(dc, rgn, border, 1, 1);
        let _ = DeleteObject(fill);
        let _ = DeleteObject(border);
        let _ = DeleteObject(rgn);
    }
    // Estrella: dorada si ya es favorita.
    let (cx, cy) = ((favorite_rect.left + favorite_rect.right) as f32 / 2.0, (favorite_rect.top + favorite_rect.bottom) as f32 / 2.0);
    let radius = size as f32 * 0.32;
    let points: Vec<POINT> = (0..10).map(|i| {
        let r = if i % 2 == 0 { radius } else { radius * 0.45 };
        let angle = -std::f32::consts::FRAC_PI_2 + i as f32 * std::f32::consts::PI / 5.0;
        POINT { x: (cx + r * angle.cos()).round() as i32, y: (cy + r * angle.sin()).round() as i32 }
    }).collect();
    let star = CreatePolygonRgn(&points, WINDING);
    let star_brush = CreateSolidBrush(if favorite { COLOR_FAVORITE } else { COLOR_BUTTON_GLYPH });
    let _ = FillRgn(dc, star, star_brush);
    let _ = DeleteObject(star_brush);
    let _ = DeleteObject(star);
    // Aspa de "quitar".
    let pen = CreatePen(PS_SOLID, px(2).max(1), COLOR_BUTTON_GLYPH);
    let old_pen = SelectObject(dc, pen);
    let arm = size * 22 / 100;
    let (x, y) = ((exclude.left + exclude.right) / 2, (exclude.top + exclude.bottom) / 2);
    let _ = MoveToEx(dc, x - arm, y - arm, None);
    let _ = LineTo(dc, x + arm, y + arm);
    let _ = MoveToEx(dc, x + arm, y - arm, None);
    let _ = LineTo(dc, x - arm, y + arm);
    SelectObject(dc, old_pen);
    let _ = DeleteObject(pen);
    ItemButtons { index, favorite: favorite_rect, exclude }
}

/// Botón bajo el punto (coordenadas de cliente), según el último fotograma pintado.
fn button_at(x: i32, y: i32) -> Option<(usize, ItemButton)> {
    let buttons = vars(|v| v.buttons.get())?;
    buttons.at(x, y).map(|button| (buttons.index, button))
}

/// Actualiza el objetivo de animación hover/foco para las miniaturas e inicia el temporizador de interpolación.
fn set_hover_target(hwnd: HWND, new_hover: Option<usize>) {
    if vars(|v| v.hover.replace(new_hover)) == new_hover {
        return;
    }
    let n = image_count();
    if n > 0 {
        vars(|v| {
            v.scales.borrow_mut().resize(n, 1.0);
            let mut targets = v.targets.borrow_mut();
            targets.clear();
            targets.resize(n, 1.0);
            if let Some(idx) = new_hover.filter(|idx| *idx < n) {
                targets[idx] = focus_scale();
            }
        });
        start_animation(hwnd);
    }
}

unsafe fn backbuffer_dc(hdc: HDC, width: i32, height: i32) -> Option<HDC> {
    if let Some(buffer) = vars(|v| v.backbuffer.get()) {
        if buffer.width == width && buffer.height == height { return Some(buffer.dc); }
        release_backbuffer();
    }
    let dc = CreateCompatibleDC(hdc);
    let bitmap = CreateCompatibleBitmap(hdc, width, height);
    if dc.is_invalid() || bitmap.is_invalid() {
        let _ = DeleteObject(bitmap);
        let _ = DeleteDC(dc);
        return None;
    }
    let old = SelectObject(dc, bitmap);
    vars(|v| v.backbuffer.set(Some(Backbuffer { dc, bitmap, old, width, height })));
    Some(dc)
}

fn release_backbuffer() {
    if let Some(buffer) = vars(|v| v.backbuffer.take()) {
        unsafe {
            SelectObject(buffer.dc, buffer.old);
            let _ = DeleteObject(buffer.bitmap);
            let _ = DeleteDC(buffer.dc);
        }
    }
}

/// Rango de índices de la tira que se ve en pantalla.
fn strip_visible_range(count: usize, viewport: i32, scroll: i32, vertical: bool) -> std::ops::Range<usize> {
    if count == 0 { return 0..0; }
    let item = if vertical { thumb_h() } else { thumb_w() };
    let extent = item + thumb_margin();
    let (first_x, first_y) = thumbnail_origin(0, count, viewport, scroll, vertical);
    let first = if vertical { first_y } else { first_x };
    let start = ((-first - item).div_euclid(extent) + 1).max(0) as usize;
    let end = (viewport - first + extent - 1).div_euclid(extent).max(0) as usize;
    start.min(count)..end.min(count)
}

/// En modo bajo demanda: pide las miniaturas visibles y las de alrededor, y expulsa
/// las usadas hace más tiempo cuando se supera la capacidad.
fn load_thumbnails_on_demand(images: &[ScannedImage], cache: &ThumbnailCache, visible: std::ops::Range<usize>) {
    if !vars(|v| v.thumbs_on_demand.get()) || visible.is_empty() { return; }
    // Margen de dos "pantallas" a cada lado para que el desplazamiento normal no muestre huecos.
    let margin = (visible.len() * 2).max(4);
    let wanted = visible.start.saturating_sub(margin)..(visible.end + margin).min(images.len());
    let keys: std::collections::HashSet<String> = images[wanted.clone()].iter().map(|img| img.path.to_string_lossy().to_string()).collect();
    crate::thumbs::retain_pending(|key| keys.contains(key));
    // Primero las visibles, luego las del margen.
    let order = images[visible.clone()].iter().chain(images[wanted.start..visible.start].iter().rev()).chain(images[visible.end..wanted.end].iter());
    let notify = crate::app::MAIN_HWND.lock().ok().and_then(|g| *g).map(|h| HWND(h as *mut _)).unwrap_or_default();
    crate::thumbs::request_thumbnails(order.map(|img| img.path.as_path()), vars(|v| v.thumb_size_px.get()), cache, notify);
    cache.touch(keys.iter().map(|key| key.as_str()));
    cache.evict_to(ON_DEMAND_CAPACITY.max(keys.len()));
}

pub fn set_thumbnail_loading(on_demand: bool, size_px: i32) {
    vars(|v| {
        v.thumbs_on_demand.set(on_demand);
        v.thumb_size_px.set(size_px);
    });
}

unsafe fn paint_contents(dc: HDC, hwnd: HWND, width: i32, height: i32) {
    let bg = CreateSolidBrush(COLOR_BACKGROUND);
    let _ = FillRect(dc, &RECT { left: 0, top: 0, right: width, bottom: height }, bg);
    let _ = DeleteObject(bg);

    let cache_opt = vars(|v| v.thumb_cache.borrow().clone());
    let images_opt = scanned();
    let scroll = vars(|v| v.scroll.get());
    let vertical = is_vertical();
    let hover = vars(|v| v.hover.get());
    let current_wp = vars(|v| v.current_wallpaper.borrow().clone());
    let selected = vars(|v| v.selected.get());
    let scales = vars(|v| v.scales.borrow().clone());
    let hot_button = vars(|v| v.hot_button.get());
    let mut buttons = None;

    if let (Some(cache), Some(images)) = (cache_opt.as_ref(), images_opt.as_ref().and_then(|arc| arc.lock().ok())) {
        let visible = match carousel::layout().filter(|_| is_carousel()) {
            Some(layout) => layout.visible_range(images.len(), width, scroll),
            None => strip_visible_range(images.len(), if vertical { height } else { width }, scroll, vertical),
        };
        load_thumbnails_on_demand(&images, cache, visible);
    }

    if is_carousel() {
        if let Some(images) = images_opt.as_ref().and_then(|arc| arc.lock().ok()) {
            buttons = carousel::paint(dc, width, height, &carousel::PaintInput {
                images: &images,
                small_thumbs: cache_opt.as_ref(),
                scroll,
                focus: hover,
                current: current_wp.as_deref(),
                scales: &scales,
                notify: hwnd,
                hot_button,
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
                let scale = scales.get(idx).copied().unwrap_or(1.0);
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
                let is_current = current_wp.as_ref().is_some_and(|p| same_path(p, &img.path));
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
                if hover == Some(idx) {
                    // La imagen con el ratón encima muestra sus botones (que ya indican si es favorita).
                    buttons = Some(draw_item_buttons(dc, idx, draw_x + draw_w, draw_y, img.is_favorite, hot_button));
                } else {
                    if img.is_new {
                        draw_new_badge(dc, draw_x + draw_w, draw_y);
                    }
                    if img.is_favorite {
                        draw_favorite_badge(dc, draw_x, draw_y);
                    }
                }
            }
            let _ = DeleteDC(bmp_dc);
            let total_main = thumbnail_content_extent(images.len(), vertical);
            if vars(|v| v.show_scrollbar.get()) && total_main > viewport {
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

    vars(|v| v.buttons.set(buttons));

    if image_count() == 0 {
        draw_empty_state(dc, width, height);
    }
}

/// Rectángulo del monitor donde está el panel.
unsafe fn panel_monitor_rect(hwnd: HWND) -> RECT {
    let hmon = windows::Win32::Graphics::Gdi::MonitorFromWindow(hwnd, MONITOR_DEFAULTTONEAREST);
    let mut monitor = MONITORINFO { cbSize: std::mem::size_of::<MONITORINFO>() as u32, ..Default::default() };
    let _ = GetMonitorInfoW(hmon, &mut monitor);
    monitor.rcMonitor
}

/// Menú contextual de una miniatura (aplicar, monitor, bloqueo, favorita, Explorador, papelera).
unsafe fn show_thumbnail_menu(hwnd: HWND, index: usize) {
    use windows::Win32::UI::WindowsAndMessaging::{
        AppendMenuW, CreatePopupMenu, DestroyMenu, GetSystemMetrics, TrackPopupMenu, MF_SEPARATOR, MF_STRING, SM_CMONITORS,
        TPM_RETURNCMD, TPM_RIGHTBUTTON,
    };
    let Some((path, favorite)) = scanned().and_then(|arc| arc.lock().ok()
        .and_then(|images| images.get(index).map(|img| (img.path.clone(), img.is_favorite)))) else { return; };
    let Ok(menu) = CreatePopupMenu() else { return; };
    let item = |id: usize, key: &str| {
        let text = to_wide_chars(&crate::i18n::t(key));
        let _ = AppendMenuW(menu, MF_STRING, id, as_pcwstr(&text));
    };
    item(IDM_APPLY, "Set as wallpaper");
    if GetSystemMetrics(SM_CMONITORS) > 1 {
        item(IDM_APPLY_MONITOR, "Set on this monitor only");
    }
    item(IDM_LOCK_SCREEN, "Set as lock screen");
    item(IDM_FAVORITE, if favorite { "Remove from favorites" } else { "Add to favorites" });
    let _ = AppendMenuW(menu, MF_SEPARATOR, 0, windows::core::PCWSTR::null());
    item(IDM_REVEAL, "Show in Explorer");
    item(IDM_EXCLUDE, "Remove from gallery");

    let mut pt = POINT::default();
    let _ = GetCursorPos(&mut pt);
    vars(|v| v.menu_open.set(true));
    let selected = TrackPopupMenu(menu, TPM_RIGHTBUTTON | TPM_RETURNCMD, pt.x, pt.y, 0, hwnd, None);
    vars(|v| v.menu_open.set(false));
    let _ = DestroyMenu(menu);

    match selected.0 as usize {
        IDM_APPLY => {
            app::request(app::Action::ApplyFromPanel(path, panel_monitor_rect(hwnd)));
            hide_panel();
        }
        IDM_APPLY_MONITOR => {
            app::request(app::Action::ApplyToMonitor(path, panel_monitor_rect(hwnd)));
            hide_panel();
        }
        IDM_LOCK_SCREEN => {
            app::request(app::Action::LockScreen(path));
            hide_panel();
        }
        IDM_FAVORITE => app::request(app::Action::ToggleFavorite(path)),
        IDM_REVEAL => {
            app::request(app::Action::Reveal(path));
            hide_panel();
        }
        IDM_EXCLUDE => app::request(app::Action::Exclude(path)),
        _ => {}
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
                if let Some(slide) = vars(|v| v.slide.get()) {
                    let elapsed = slide.anim_start_time.elapsed().as_millis() as f32;
                    let progress = (elapsed / (ANIM_DURATION_MS as f32)).min(1.0);
                    let eased = 1.0 - (1.0 - progress).powi(3);
                    let x = slide.start_x + ((slide.target_x - slide.start_x) as f32 * eased) as i32;
                    let y = slide.start_y + ((slide.target_y - slide.start_y) as f32 * eased) as i32;
                    let (pw, ph) = vars(|v| (v.width.get(), v.height.get()));
                    let _ = SetWindowPos(hwnd, HWND_TOPMOST, x, y, pw, ph, SWP_NOACTIVATE | SWP_NOZORDER);
                    if is_carousel() {
                        let opacity = if vars(|v| v.exiting.get()) { 1.0 - eased } else { eased };
                        set_panel_alpha(hwnd, (opacity * 255.0) as u8);
                    }
                    if progress >= 1.0 {
                        vars(|v| v.slide.set(None));
                        let _ = KillTimer(hwnd, ANIM_TIMER_ID);
                        should_hide = vars(|v| v.exiting.get());
                    }
                }
                if should_hide {
                    let _ = ShowWindow(hwnd, windows::Win32::UI::WindowsAndMessaging::SW_HIDE);
                    vars(|v| {
                        v.visible.set(false);
                        v.exiting.set(false);
                    });
                    release_backbuffer();
                    if is_carousel() {
                        let _ = SetTimer(hwnd, TILE_RELEASE_TIMER_ID, TILE_RELEASE_DELAY_MS, None);
                    }
                    crate::log::log_resource_usage();
                }
                return LRESULT(0);
            }
            if wparam.0 == TILE_RELEASE_TIMER_ID {
                let _ = KillTimer(hwnd, TILE_RELEASE_TIMER_ID);
                carousel::clear_tiles();
                return LRESULT(0);
            }
            if wparam.0 == HOVER_ANIM_TIMER_ID {
                advance_animation(hwnd);
                return LRESULT(0);
            }
            if wparam.0 == AUTO_CLOSE_TIMER_ID {
                // Con un menú contextual abierto el cursor sale del panel: no cerrarlo.
                if vars(|v| v.menu_open.get()) {
                    return LRESULT(0);
                }
                // El modo del sensor cierra al salir; el modo de atajo espera un clic fuera.
                let mut pt = POINT::default();
                let _ = GetCursorPos(&mut pt);
                let mut rect = RECT::default();
                let _ = GetWindowRect(hwnd, &mut rect);
                let close_on_leave = vars(|v| v.close_on_mouse_leave.get());
                let mut inside = pt.x >= rect.left && pt.x < rect.right && pt.y >= rect.top && pt.y < rect.bottom;
                if inside && is_carousel() {
                    // Los huecos entre tarjetas son transparentes: un clic ahí cuenta como fuera.
                    inside = thumbnail_at(pt.x - rect.left, pt.y - rect.top).is_some();
                }
                if close_on_leave && !inside {
                    let side = vars(|v| v.side.get());
                    let monitor = panel_monitor_rect(hwnd);
                    let monitor_width = monitor.right - monitor.left;
                    let monitor_height = monitor.bottom - monitor.top;
                    let margin = effective_panel_margin(side, vars(|v| v.margin.get()), monitor_width, monitor_height);
                    let grace = margin.saturating_add(px(BASE_CLOSE_GRACE));
                    inside = match side {
                        PanelSide::Top => {
                            pt.x >= rect.left && pt.x < rect.right
                                && pt.y >= monitor.top
                                && pt.y < (monitor.top + grace).min(monitor.bottom)
                        }
                        PanelSide::Left => {
                            pt.y >= monitor.top && pt.y < monitor.bottom
                                && pt.x >= monitor.left
                                && pt.x < (monitor.left + grace).min(monitor.right)
                        }
                        PanelSide::Right => {
                            pt.y >= monitor.top && pt.y < monitor.bottom
                                && pt.x >= (monitor.right - grace).max(monitor.left)
                                && pt.x < monitor.right
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
            let hdc = BeginPaint(hwnd, &mut ps);
            let mut rect = RECT::default();
            let _ = windows::Win32::UI::WindowsAndMessaging::GetClientRect(hwnd, &mut rect);
            let width = rect.right - rect.left;
            let height = rect.bottom - rect.top;
            if let Some(mem_dc) = backbuffer_dc(hdc, width, height) {
                paint_contents(mem_dc, hwnd, width, height);
                let _ = BitBlt(hdc, 0, 0, width, height, mem_dc, 0, 0, SRCCOPY);
            }
            let _ = EndPaint(hwnd, &ps);
            LRESULT(0)
        }
        WM_LBUTTONDOWN => {
            let x = (lparam.0 & 0xFFFF) as i16 as i32;
            let y = ((lparam.0 >> 16) & 0xFFFF) as i16 as i32;
            let vertical = is_vertical();
            vars(|v| {
                v.dragging.set(true);
                v.last_mouse.set(if vertical { y } else { x });
                v.drag_start.set((x, y));
            });
            LRESULT(0)
        }
        WM_LBUTTONUP => {
            let was_dragging = vars(|v| v.dragging.replace(false));
            let (start_x, start_y) = vars(|v| v.drag_start.get());
            let x = (lparam.0 & 0xFFFF) as i16 as i32;
            let y = ((lparam.0 >> 16) & 0xFFFF) as i16 as i32;

            // Si el cursor se movió 5 px o menos, es un clic sobre la miniatura
            if was_dragging && (x - start_x).abs() <= 5 && (y - start_y).abs() <= 5 {
                if let Some((idx, button)) = button_at(x, y) {
                    // Los botones actúan sobre la imagen sin aplicarla ni cerrar el panel.
                    if let Some(path) = image_path(idx) {
                        match button {
                            ItemButton::Favorite => app::request(app::Action::ToggleFavorite(path)),
                            ItemButton::Exclude => app::request(app::Action::Exclude(path)),
                        }
                    }
                } else if let Some(idx) = thumbnail_at(x, y) {
                    if let Some(path) = image_path(idx) {
                        log_info!("Panel: clic en miniatura {} -> {:?}", idx, path);
                        app::request(app::Action::ApplyFromPanel(path, panel_monitor_rect(hwnd)));
                        hide_panel();
                    }
                }
            }
            LRESULT(0)
        }
        WM_RBUTTONUP => {
            let x = (lparam.0 & 0xFFFF) as i16 as i32;
            let y = ((lparam.0 >> 16) & 0xFFFF) as i16 as i32;
            if let Some(idx) = thumbnail_at(x, y) {
                show_thumbnail_menu(hwnd, idx);
            }
            LRESULT(0)
        }
        WM_MOUSEMOVE => {
            let x = (lparam.0 & 0xFFFF) as i16 as i32;
            let y = ((lparam.0 >> 16) & 0xFFFF) as i16 as i32;
            let vertical = is_vertical();
            let dragging = vars(|v| v.dragging.get());
            // Sobre un botón el foco se queda en su imagen (el botón puede sobresalir un poco).
            let over_button = if dragging { None } else { button_at(x, y) };
            let hover = over_button.map(|(index, _)| index).or_else(|| thumbnail_at(x, y));
            set_hover_target(hwnd, hover);
            let hot = over_button.map(|(_, button)| button);
            if vars(|v| v.hot_button.replace(hot)) != hot {
                request_repaint();
            }

            if dragging {
                let axis = if vertical { y } else { x };
                let delta = vars(|v| v.last_mouse.replace(axis)) - axis;
                if delta != 0 {
                    let max_scroll = max_scroll(image_count());
                    vars(|v| {
                        v.scroll_target.set(None);
                        // Clamp: no se puede scrollear más allá del final de las imágenes
                        v.scroll.set((v.scroll.get() + delta).max(0).min(max_scroll));
                    });
                    request_repaint();
                }
            }
            // Con el ratón en movimiento los WM_TIMER pueden no llegar: avanzar desde aquí.
            if vars(|v| v.anim_last.get()).is_some_and(|last| last.elapsed().as_millis() >= HOVER_ANIM_STEP_MS as u128) {
                advance_animation(hwnd);
            }
            LRESULT(0)
        }
        0x020A => { // WM_MOUSEWHEEL
            let delta = (wparam.0 >> 16) as i16 as i32;
            if is_carousel() {
                // En el carrusel cada paso de la rueda avanza una tarjeta.
                step_selection(hwnd, delta < 0);
                return LRESULT(0);
            }
            let scroll_delta = if delta > 0 { -30 } else { 30 };
            let max_scroll = max_scroll(image_count());
            vars(|v| {
                let current = v.scroll.get();
                // Al seguir girando en un extremo se salta al otro.
                v.scroll.set(if max_scroll > 0 && current >= max_scroll && scroll_delta > 0 {
                    0
                } else if max_scroll > 0 && current <= 0 && scroll_delta < 0 {
                    max_scroll
                } else {
                    (current + scroll_delta).max(0).min(max_scroll)
                });
            });
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
                if image_count() == 0 { return LRESULT(0); }
                let sel = vars(|v| v.selected.get()).unwrap_or(0);
                if key == previous_key || key == next_key {
                    step_selection(hwnd, key == next_key);
                } else if let Some(p) = image_path(sel) { // Enter -> aplicar y cerrar panel
                    log_info!("Panel: Enter en miniatura {} -> {:?}", sel, p);
                    app::request(app::Action::ApplyFromPanel(p.clone(), panel_monitor_rect(hwnd)));
                    set_current_wallpaper(&p);
                    hide_panel();
                }
                return LRESULT(0);
            }
            if key == 0x5D { // VK_APPS (tecla de menú): menú de la miniatura seleccionada
                if let Some(sel) = vars(|v| v.selected.get()).filter(|sel| *sel < image_count()) {
                    show_thumbnail_menu(hwnd, sel);
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
            if let Ok(hcur) = LoadCursorW(None, IDC_ARROW) {
                let _ = SetCursor(hcur);
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
    if vars(|v| v.side.replace(side)) == side { return; }
    reopen_if_visible();
}

pub fn set_panel_mode(mode: PanelMode) {
    let old_mode = vars(|v| v.mode.replace(mode));
    if old_mode == mode { return; }
    if old_mode == PanelMode::Pro { carousel::clear_tiles(); }
    reopen_if_visible();
}

pub fn set_pro_priority(priority: ProPriority) {
    carousel::set_smooth_priority(priority == ProPriority::Smooth);
}

/// Vuelve a mostrar el panel con la nueva disposición si estaba abierto.
fn reopen_if_visible() {
    if is_panel_visible() {
        let close_on_leave = vars(|v| v.close_on_mouse_leave.get());
        if let Some(hwnd) = vars(|v| v.hwnd.get()) {
            unsafe {
                let _ = KillTimer(hwnd, ANIM_TIMER_ID);
                let _ = KillTimer(hwnd, AUTO_CLOSE_TIMER_ID);
                let _ = KillTimer(hwnd, HOVER_ANIM_TIMER_ID);
                let _ = ShowWindow(hwnd, windows::Win32::UI::WindowsAndMessaging::SW_HIDE);
                vars(|v| v.animating.set(false));
            }
        }
        vars(|v| {
            v.visible.set(false);
            v.exiting.set(false);
            v.slide.set(None);
        });
        show_panel_with_close_on_mouse_leave(close_on_leave);
    }
}

/// Libera las tarjetas grandes del carrusel (por ejemplo, al cambiar la configuración).
pub fn clear_carousel_tiles() {
    carousel::clear_tiles();
}

/// Olvida las tarjetas del carrusel de estos archivos (se editaron y hay que regenerarlas).
pub fn forget_tiles(keys: &std::collections::HashSet<String>) {
    carousel::forget(keys);
    request_repaint();
}

pub fn set_scrollbar_visible(visible: bool) {
    vars(|v| v.show_scrollbar.set(visible));
    request_repaint();
}

pub fn set_panel_margin(margin: u32) {
    vars(|v| v.margin.set(margin));
}

fn show_panel_with_close_on_mouse_leave(close_on_mouse_leave: bool) {
    unsafe {
        if vars(|v| v.visible.get()) { return; }
        // El carrusel queda lejos del borde: solo se cierra con clic fuera, Esc o al elegir.
        let close_on_mouse_leave = close_on_mouse_leave && !is_carousel();
        vars(|v| v.close_on_mouse_leave.set(close_on_mouse_leave));

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
            vars(|v| v.dpi.set(dpi_x.max(96)));
        }

        let side = vars(|v| v.side.get());
        let vertical = is_vertical();
        let monitor_width = mi.rcMonitor.right - mi.rcMonitor.left;
        let monitor_height = mi.rcMonitor.bottom - mi.rcMonitor.top;
        let margin = effective_panel_margin(side, vars(|v| v.margin.get()), monitor_width, monitor_height);
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
        let current_wp = vars(|v| v.current_wallpaper.borrow().clone());
        let (selected_idx, initial_scroll, image_count) = scanned()
            .and_then(|arc| arc.lock().ok().map(|images| {
                let count = images.len();
                let idx = current_wp.as_ref()
                    .and_then(|cur| images.iter().position(|img| same_path(&img.path, cur)))
                    .unwrap_or(0);
                let scroll = if let Some(layout) = layout {
                    layout.loop_center_scroll(idx as i64, viewport)
                } else {
                    let item_x = idx as i32 * item_extent + thumb_margin();
                    let center_scroll = (item_x + item_size / 2) - (viewport / 2);
                    let max_scroll = (thumbnail_content_extent(count, vertical) - viewport).max(0);
                    center_scroll.clamp(0, max_scroll)
                };
                (idx, scroll, count)
            }))
            .unwrap_or((0, 0, 0));

        // Reutilizar ventana cacheada si existe
        let hwnd = if let Some(hwnd) = vars(|v| v.hwnd.get()) {
            hwnd
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
            vars(|v| v.hwnd.set(Some(hwnd)));
            hwnd
        };

        vars(|v| {
            v.width.set(width);
            v.height.set(height);
            v.scroll.set(initial_scroll);
            v.carousel_index.set(selected_idx as i64);
            v.selected.set(Some(selected_idx));
            v.hover.set(Some(selected_idx));
            v.dragging.set(false);
            v.scroll_target.set(None);
            if image_count > 0 {
                let initial = |i: usize| if i == selected_idx { focus_scale() } else { 1.0 };
                *v.scales.borrow_mut() = (0..image_count).map(initial).collect();
                *v.targets.borrow_mut() = (0..image_count).map(initial).collect();
            }
            v.slide.set(Some(Slide {
                target_x,
                start_x,
                target_y,
                start_y,
                anim_start_time: std::time::Instant::now(),
            }));
        });
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
        vars(|v| v.visible.set(true));
        crate::log_debug!("Panel: mostrado");
    }
}

pub fn hide_panel() {
    unsafe {
        if !vars(|v| v.visible.get()) || vars(|v| v.exiting.get()) { return; }
        let Some(hwnd) = vars(|v| v.hwnd.get()) else { return; };
        let _ = KillTimer(hwnd, AUTO_CLOSE_TIMER_ID);
        let _ = KillTimer(hwnd, HOVER_ANIM_TIMER_ID);
        vars(|v| {
            v.animating.set(false);
            v.buttons.set(None);
            v.hot_button.set(None);
        });

        let mut wr = RECT::default();
        let _ = GetWindowRect(hwnd, &mut wr);
        let side = vars(|v| v.side.get());
        let (start_x, start_y) = (wr.left, wr.top);
        let (width, height) = vars(|v| (v.width.get(), v.height.get()));
        let monitor = panel_monitor_rect(hwnd);
        let (target_x, target_y) = match side {
            _ if is_carousel() => (start_x, start_y + CAROUSEL_SLIDE_PX),
            PanelSide::Top => (start_x, monitor.top - height),
            PanelSide::Left => (monitor.left - width, start_y),
            PanelSide::Right => (monitor.right, start_y),
        };

        vars(|v| {
            v.slide.set(Some(Slide {
                target_x,
                start_x,
                target_y,
                start_y,
                anim_start_time: std::time::Instant::now(),
            }));
            v.exiting.set(true);
        });
        let _ = SetTimer(hwnd, ANIM_TIMER_ID, ANIM_STEP_MS, None);
        crate::log_debug!("Panel: animando salida");
    }
}

pub fn set_draw_context(cache: ThumbnailCache, images: Arc<Mutex<Vec<ScannedImage>>>) {
    // Inicializar escalas por miniatura
    let n = images.lock().map(|imgs| imgs.len()).unwrap_or(0);
    vars(|v| {
        v.scales.borrow_mut().resize(n, 1.0);
        let mut targets = v.targets.borrow_mut();
        targets.clear();
        targets.resize(n, 1.0);
        *v.thumb_cache.borrow_mut() = Some(cache);
        *v.scanned.borrow_mut() = Some(images);
    });
    request_repaint();
}

pub fn set_current_wallpaper(path: &Path) {
    vars(|v| *v.current_wallpaper.borrow_mut() = Some(path.to_path_buf()));
    request_repaint();
}

pub fn request_repaint() {
    if let Some(hwnd) = vars(|v| v.hwnd.get()) {
        unsafe {
            let _ = windows::Win32::Graphics::Gdi::InvalidateRect(hwnd, None, true);
        }
    }
}

pub fn is_panel_visible() -> bool {
    vars(|v| v.visible.get())
}

#[cfg(test)]
pub(crate) unsafe fn save_bmp(dc: HDC, bmp: windows::Win32::Graphics::Gdi::HBITMAP, width: i32, height: i32, path: &str) {
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

    #[test]
    fn wrapped_step_loops_around() {
        assert_eq!(wrapped_step(4, 5, true), 0);
        assert_eq!(wrapped_step(0, 5, false), 4);
        assert_eq!(wrapped_step(2, 5, true), 3);
        assert_eq!(wrapped_step(2, 5, false), 1);
        assert_eq!(wrapped_step(0, 1, true), 0);
    }

    #[test]
    fn strip_visible_range_matches_drawn_thumbnails() {
        for vertical in [false, true] {
            let (count, viewport) = (40, 1000);
            for scroll in [0, 7, 333, 1500] {
                let range = strip_visible_range(count, viewport, scroll, vertical);
                for index in 0..count {
                    let (x, y) = thumbnail_origin(index, count, viewport, scroll, vertical);
                    let (start, size) = if vertical { (y, thumb_h()) } else { (x, thumb_w()) };
                    let on_screen = start + size > 0 && start < viewport;
                    assert_eq!(range.contains(&index), on_screen, "vertical={vertical} scroll={scroll} index={index}");
                }
            }
        }
        // Pocas miniaturas: se centran y todas se ven.
        assert_eq!(strip_visible_range(3, 1000, 0, false), 0..3);
    }

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
        let mut paths: Vec<PathBuf> = std::fs::read_dir(dir).unwrap().flatten().map(|e| e.path())
            .filter(|p| crate::scanner::is_supported_image(p))
            .collect();
        paths.sort();
        paths.truncate(10);
        let images: Vec<ScannedImage> = paths.iter().enumerate().map(|(i, p)| ScannedImage {
            path: p.clone(),
            file_name: String::new(),
            is_new: i == 4 || i == 6,
            is_favorite: i == 3,
            modified: None,
        }).collect();
        let cache = ThumbnailCache::new();
        unsafe {
            use windows::Win32::System::Com::{CoInitializeEx, COINIT_APARTMENTTHREADED};
            let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
        }
        crate::thumbs::request_thumbnails(images.iter().map(|img| img.path.as_path()), 128, &cache, HWND::default());
        // El hilo de carga trabaja aparte: esperar a que estén todas.
        let started = std::time::Instant::now();
        while cache.len() < images.len() && started.elapsed().as_secs() < 10 {
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        let width = 1600;
        vars(|v| {
            v.dpi.set(144);
            v.side.set(PanelSide::Top);
            v.mode.set(PanelMode::LowPower);
            v.show_scrollbar.set(true);
            *v.current_wallpaper.borrow_mut() = Some(images[2].path.clone());
            v.selected.set(Some(2));
            v.hover.set(Some(5));
            *v.scales.borrow_mut() = (0..images.len()).map(|i| if i == 5 { 0.9 } else { 1.0 }).collect();
        });
        let height = thumb_h() + thumb_margin() * 2 + scrollbar_size();
        vars(|v| {
            v.width.set(width);
            v.height.set(height);
            v.scroll.set(0);
            *v.thumb_cache.borrow_mut() = Some(cache);
            *v.scanned.borrow_mut() = Some(Arc::new(Mutex::new(images)));
        });
        unsafe {
            render(width, height, &format!("{out}-strip.bmp"));
            vars(|v| *v.scanned.borrow_mut() = Some(Arc::new(Mutex::new(Vec::new()))));
            render(width, height, &format!("{out}-empty.bmp"));
        }
    }
}
