//! Modo Notch: una isla negra pegada al borde superior, como la de los teléfonos, que crece
//! hasta mostrar la galería y se encoge al cerrarse.
//!
//! A diferencia de la tira y el carrusel (GDI), se dibuja con Direct2D en un DIB de 32 bits
//! con alfa premultiplicado y se presenta con `UpdateLayeredWindow`: bordes suavizados,
//! sombra y transparencia real por píxel. Los píxeles transparentes no reciben clics.
//!
//! Todo lo de Direct2D (fábricas, destino, mapas de bits) solo existe mientras el notch está
//! abierto; al cerrarse se libera. Las imágenes grandes salen de la caché del carrusel.

use super::carousel;
use super::gallery::{families_present, family_of, Family};
use super::d2d::{
    accent, bg, color, draw_star, ease_out_back, fg, light, ease_out_cubic, fill_cover, inside, lerp, point, rect_f, rounded, smoothstep, white,
    Surface, COLOR_FAVORITE, COLOR_NEW,
};
use crate::app;
use crate::log_info;
use crate::winutil::{as_pcwstr, same_path, to_wide_chars};
use crate::thumbs::Thumb;
use std::cell::{Cell, RefCell};
use std::sync::Mutex;
use std::time::Instant;
use windows::Foundation::Numerics::Matrix3x2;
use windows::Win32::Foundation::{HINSTANCE, HWND, LPARAM, LRESULT, POINT, RECT, WPARAM};
use windows::Win32::Graphics::Direct2D::Common::{D2D1_FIGURE_BEGIN_FILLED, D2D1_FIGURE_END_CLOSED, D2D_POINT_2F, D2D_RECT_F, D2D_SIZE_F};
use windows::Win32::Graphics::Direct2D::{
    ID2D1Bitmap, ID2D1Factory, ID2D1Geometry, ID2D1PathGeometry, ID2D1RenderTarget, D2D1_ANTIALIAS_MODE_PER_PRIMITIVE, D2D1_ARC_SEGMENT,
    D2D1_ARC_SIZE_SMALL, D2D1_BITMAP_INTERPOLATION_MODE_LINEAR, D2D1_DRAW_TEXT_OPTIONS_CLIP, D2D1_ELLIPSE, D2D1_LAYER_OPTIONS_NONE,
    D2D1_LAYER_PARAMETERS, D2D1_QUADRATIC_BEZIER_SEGMENT, D2D1_SWEEP_DIRECTION_COUNTER_CLOCKWISE,
};
use windows::Win32::Graphics::DirectWrite::{
    IDWriteTextFormat, DWRITE_FONT_WEIGHT_NORMAL, DWRITE_FONT_WEIGHT_SEMI_BOLD, DWRITE_MEASURING_MODE_NATURAL, DWRITE_TEXT_ALIGNMENT_CENTER,
    DWRITE_TEXT_ALIGNMENT_LEADING, DWRITE_TEXT_ALIGNMENT_TRAILING,
};
use windows::Win32::Graphics::Gdi::{GetMonitorInfoW, MonitorFromPoint, MONITORINFO, MONITOR_DEFAULTTONEAREST};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::Input::KeyboardAndMouse::{GetAsyncKeyState, TrackMouseEvent, TME_LEAVE, TRACKMOUSEEVENT, VK_LBUTTON};
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, GetCursorPos, KillTimer, LoadCursorW, PostMessageW, RegisterClassW, SetCursor, SetTimer,
    ShowWindow, HWND_TOPMOST, IDC_ARROW, SWP_NOACTIVATE, SW_HIDE, SW_SHOWNA, WM_APP, WM_KEYDOWN,
    WM_LBUTTONUP, WM_MOUSEMOVE, WM_MOUSEWHEEL, WM_RBUTTONUP, WM_SETCURSOR, WM_TIMER, WNDCLASSW, WS_EX_LAYERED,
    WS_EX_TOOLWINDOW, WS_EX_TOPMOST, WS_POPUP,
};

const CLASS_NAME: &str = "SWEPR_NotchWindowClass";
const FRAME_TIMER_ID: usize = 8101;
const CLOSE_CHECK_TIMER_ID: usize = 8102;
const TILE_RELEASE_TIMER_ID: usize = 8103;
const PREVIEW_TIMER_ID: usize = 8104;
const NOTIFY_TIMER_ID: usize = 8105;
const FRAME_MS: u32 = 16;
const SENSOR_CLOSE_CHECK_MS: u32 = 100;
const HOTKEY_CLICK_CHECK_MS: u32 = 10;
const TILE_RELEASE_DELAY_MS: u32 = 30_000;
const WM_MOUSELEAVE: u32 = 0x02A3;
/// Pide un fotograma sin esperar al temporizador (se agrupan varias peticiones en una).
const WM_APP_NOTCH_RENDER: u32 = WM_APP + 70;
/// La miniatura del aviso ya está cargada.
const WM_APP_NOTCH_NOTIFY: u32 = WM_APP + 71;

/// Duración de la apertura (con un pequeño rebote) y del cierre.
const OPEN_MS: f32 = 520.0;
/// Parte de la apertura en la que solo se ve la pastilla.
const PILL_PHASE: f32 = 0.22;
const CLOSE_MS: f32 = 230.0;
/// Aviso de cambio de fondo: entrada, tiempo visible y salida.
const NOTIFY_IN_MS: f32 = 480.0;
const NOTIFY_HOLD_MS: f32 = 2200.0;
const NOTIFY_OUT_MS: f32 = 260.0;
const BASE_TOAST_W: f32 = 380.0;
const BASE_TOAST_H: f32 = 74.0;

// Medidas a 96 DPI (se escalan con el DPI del monitor).
const BASE_PILL_W: f32 = 132.0;
const BASE_PILL_H: f32 = 30.0;
const BASE_MAX_W: f32 = 1180.0;
const BASE_ITEM_H: f32 = 150.0;
const BASE_PAD_TOP: f32 = 12.0;
const BASE_HEADER_H: f32 = 26.0;
const BASE_HEADER_GAP: f32 = 10.0;
const BASE_PAD_BOTTOM: f32 = 18.0;
const BASE_PAD_X: f32 = 22.0;
const BASE_GAP: f32 = 12.0;
const BASE_RADIUS: f32 = 28.0;
const BASE_SHOULDER: f32 = 14.0;
const BASE_SHADOW: f32 = 22.0;
const BASE_ITEM_RADIUS: f32 = 14.0;
const BASE_BUTTON: f32 = 26.0;
const BASE_CLOSE_GRACE: f32 = 14.0;
/// Las vecinas de la imagen central se ven a este tamaño.
const SIDE_SCALE: f32 = 0.62;
/// Márgen para que el rebote de la apertura no se salga de la ventana.
const OVERSHOOT: f32 = 1.06;

const COLOR_BODY: (f32, f32, f32) = (0.043, 0.043, 0.051);
const COLOR_BODY_LIGHT: (f32, f32, f32) = (0.975, 0.975, 0.97);

/// Medidas del notch abierto en un monitor (en píxeles físicos).
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Metrics {
    scale: f32,
    /// Tamaño del cuerpo abierto (sin hombros ni sombra).
    pub body_w: f32,
    pub body_h: f32,
    pub item_w: f32,
    pub item_h: f32,
    pub window_w: i32,
    pub window_h: i32,
}

impl Metrics {
    pub fn new(monitor_w: i32, dpi: u32) -> Self {
        let scale = dpi.max(96) as f32 / 96.0;
        let item_h = (BASE_ITEM_H * scale).round();
        let item_w = (item_h * 16.0 / 9.0).round();
        let body_w = (BASE_MAX_W * scale).min(monitor_w as f32 * 0.72).max(item_w * 2.2).round();
        let body_h = ((BASE_PAD_TOP + BASE_HEADER_H + BASE_HEADER_GAP + BASE_PAD_BOTTOM) * scale + item_h).round();
        let side = (BASE_SHOULDER + BASE_SHADOW) * scale;
        let window_w = (body_w * OVERSHOOT + side * 2.0).ceil() as i32;
        let window_h = (body_h * OVERSHOOT + BASE_SHADOW * scale).ceil() as i32;
        Self { scale, body_w, body_h, item_w, item_h, window_w, window_h }
    }

    fn px(&self, value: f32) -> f32 { value * self.scale }
    fn side_w(&self) -> f32 { self.item_w * SIDE_SCALE }
    fn center_x(&self) -> f32 { self.window_w as f32 / 2.0 }
    /// Paso entre vecinas.
    fn pitch(&self) -> f32 { self.side_w() + self.px(BASE_GAP) }

    /// Centro horizontal (relativo al centro del notch) de una imagen a `d` posiciones de la central.
    fn item_offset(&self, d: f32) -> f32 {
        let near = d.abs().min(1.0);
        let far = (d.abs() - 1.0).max(0.0);
        let first = self.item_w / 2.0 + self.px(BASE_GAP) + self.side_w() / 2.0;
        d.signum() * (near * first + far * self.pitch())
    }

    fn item_scale(&self, d: f32) -> f32 { 1.0 - (1.0 - SIDE_SCALE) * d.abs().min(1.0) }

    /// Cuántas posiciones a cada lado pueden verse.
    fn reach(&self) -> i64 {
        ((self.body_w / 2.0 - self.item_w / 2.0) / self.pitch()).ceil() as i64 + 2
    }

    /// Rectángulo de una imagen en coordenadas de la ventana, con el notch abierto del todo.
    fn item_rect(&self, d: f32) -> D2D_RECT_F {
        let s = self.item_scale(d);
        let (w, h) = (self.item_w * s, self.item_h * s);
        let cx = self.center_x() + self.item_offset(d);
        let bottom = self.body_h - self.px(BASE_PAD_BOTTOM);
        rect_f(cx - w / 2.0, bottom - h, cx + w / 2.0, bottom)
    }

    /// Cuerpo del notch para una apertura dada: (ancho, alto, radio inferior, hombro).
    /// Primero baja una pastilla pequeña (hasta `PILL_PHASE`) y luego se expande con rebote.
    fn shape(&self, open: f32) -> (f32, f32, f32, f32) { self.shape_to(open, self.body_w, self.body_h) }

    /// Tamaño del aviso de cambio de fondo.
    fn toast_size(&self) -> (f32, f32) { (self.px(BASE_TOAST_W).min(self.body_w), self.px(BASE_TOAST_H)) }

    fn toast_shape(&self, open: f32) -> (f32, f32, f32, f32) {
        let (w, h) = self.toast_size();
        self.shape_to(open, w, h)
    }

    fn shape_to(&self, open: f32, body_w: f32, body_h: f32) -> (f32, f32, f32, f32) {
        let (pill_w, pill_h) = (self.px(BASE_PILL_W), self.px(BASE_PILL_H));
        let (w, h, radius) = if open < PILL_PHASE {
            let a = ease_out_cubic(open / PILL_PHASE);
            (pill_w * lerp(0.55, 1.0, a), pill_h * a, pill_h / 2.0)
        } else {
            let b = (open - PILL_PHASE) / (1.0 - PILL_PHASE);
            (
                lerp(pill_w, body_w, ease_out_back(b)),
                lerp(pill_h, body_h, ease_out_cubic(b)),
                lerp(pill_h / 2.0, self.px(BASE_RADIUS), ease_out_cubic(b)),
            )
        };
        let shoulder = self.px(BASE_SHOULDER).min(h * 0.5);
        let radius = radius.min(h - shoulder).min(w / 2.0).max(0.0);
        (w, h, radius, shoulder)
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Phase {
    Hidden,
    Opening,
    Open,
    Closing,
    /// Aviso breve de cambio de fondo (no toma el foco).
    Notify,
}

/// Lo que se pintó en el último fotograma, para saber qué hay bajo el ratón.
#[derive(Clone, Copy, Debug)]
struct HitItem {
    virtual_index: i64,
    rect: D2D_RECT_F,
}

#[derive(Clone, Copy, Debug)]
struct HitButtons {
    virtual_index: i64,
    favorite: D2D_RECT_F,
    exclude: D2D_RECT_F,
}

struct Renderer {
    surface: Surface,
    title_format: Option<IDWriteTextFormat>,
    meta_format: Option<IDWriteTextFormat>,
}

impl std::ops::Deref for Renderer {
    type Target = Surface;
    fn deref(&self) -> &Surface { &self.surface }
}

impl std::ops::DerefMut for Renderer {
    fn deref_mut(&mut self) -> &mut Surface { &mut self.surface }
}

impl Renderer {
    unsafe fn new(width: i32, height: i32, scale: f32) -> windows::core::Result<Self> {
        let surface = Surface::new(width, height)?;
        let title_format = surface.format(13.0 * scale, DWRITE_FONT_WEIGHT_SEMI_BOLD, DWRITE_TEXT_ALIGNMENT_CENTER);
        let meta_format = surface.format(12.0 * scale, DWRITE_FONT_WEIGHT_NORMAL, DWRITE_TEXT_ALIGNMENT_TRAILING);
        Ok(Self { surface, title_format, meta_format })
    }
}

struct NotchVars {
    hwnd: Cell<Option<HWND>>,
    phase: Cell<Phase>,
    /// Apertura de 0 (cerrado) a 1 (abierto), lineal en el tiempo.
    open: Cell<f32>,
    phase_start: Cell<Instant>,
    phase_from: Cell<f32>,
    close_on_leave: Cell<bool>,
    metrics: Cell<Option<Metrics>>,
    monitor: Cell<RECT>,
    origin: Cell<POINT>,
    /// Posición animada del carrusel y posición objetivo (índices virtuales, sin límite).
    pos: Cell<f32>,
    target: Cell<i64>,
    last_step: Cell<Option<Instant>>,
    hover: Cell<Option<i64>>,
    hot_button: Cell<Option<super::ItemButton>>,
    tracking_leave: Cell<bool>,
    render_pending: Cell<bool>,
    items: RefCell<Vec<HitItem>>,
    buttons: Cell<Option<HitButtons>>,
    renderer: RefCell<Option<Renderer>>,
    notify_start: Cell<Instant>,
    /// Filtro por color (fichas de la cabecera) y las imágenes que pasan, como índices de la galería.
    family: Cell<Option<Family>>,
    view: RefCell<Vec<usize>>,
    chips: RefCell<Vec<(Family, D2D_RECT_F)>>,
    hot_chip: Cell<Option<Family>>,
    notify_path: RefCell<Option<std::path::PathBuf>>,
    notify_thumb: RefCell<Option<Thumb>>,
}

impl Default for NotchVars {
    fn default() -> Self {
        Self {
            hwnd: Cell::new(None),
            phase: Cell::new(Phase::Hidden),
            open: Cell::new(0.0),
            phase_start: Cell::new(Instant::now()),
            phase_from: Cell::new(0.0),
            close_on_leave: Cell::new(true),
            metrics: Cell::new(None),
            monitor: Cell::new(RECT::default()),
            origin: Cell::new(POINT::default()),
            pos: Cell::new(0.0),
            target: Cell::new(0),
            last_step: Cell::new(None),
            hover: Cell::new(None),
            hot_button: Cell::new(None),
            tracking_leave: Cell::new(false),
            render_pending: Cell::new(false),
            items: RefCell::new(Vec::new()),
            buttons: Cell::new(None),
            renderer: RefCell::new(None),
            notify_start: Cell::new(Instant::now()),
            family: Cell::new(None),
            view: RefCell::new(Vec::new()),
            chips: RefCell::new(Vec::new()),
            hot_chip: Cell::new(None),
            notify_path: RefCell::new(None),
            notify_thumb: RefCell::new(None),
        }
    }
}

thread_local! {
    static NOTCH: NotchVars = NotchVars::default();
}

fn nv<R>(f: impl FnOnce(&NotchVars) -> R) -> R { NOTCH.with(f) }

pub fn is_visible() -> bool { nv(|v| v.phase.get()) != Phase::Hidden }
pub fn is_closing() -> bool { nv(|v| v.phase.get()) == Phase::Closing }
pub fn closes_on_leave() -> bool { nv(|v| v.close_on_leave.get()) }
pub fn is_notifying() -> bool { nv(|v| v.phase.get()) == Phase::Notify }

fn real_index(virtual_index: i64, count: usize) -> usize { virtual_index.rem_euclid(count.max(1) as i64) as usize }

/// Imágenes que se ven con el filtro de color actual (índices de la galería).
fn filtered(images: &[crate::scanner::ScannedImage], family: Option<Family>) -> Vec<usize> {
    match family {
        None => (0..images.len()).collect(),
        Some(family) => {
            let small = super::vars(|v| v.thumb_cache.borrow().clone());
            (0..images.len()).filter(|i| family_of(&images[*i].path.to_string_lossy(), small.as_ref()) == Some(family)).collect()
        }
    }
}

fn view_count() -> usize { nv(|v| v.view.borrow().len()) }

/// Índice en la galería de la imagen en esta posición del carrusel.
fn image_at(virtual_index: i64) -> Option<usize> {
    nv(|v| {
        let view = v.view.borrow();
        if view.is_empty() { None } else { view.get(real_index(virtual_index, view.len())).copied() }
    })
}

/// Pulsar una ficha de color: filtra por ese color (o lo quita) sin perder la imagen elegida.
fn toggle_family(hwnd: HWND, family: Family) {
    let Some(images_arc) = super::scanned() else { return; };
    let Ok(images) = images_arc.lock() else { return; };
    let next = if nv(|v| v.family.get()) == Some(family) { None } else { Some(family) };
    let focused = image_at(nv(|v| v.target.get()));
    let view = filtered(&images, next);
    let target = focused.and_then(|f| view.iter().position(|i| *i == f)).unwrap_or(0) as i64;
    nv(|v| {
        v.family.set(next);
        *v.view.borrow_mut() = view;
        v.target.set(target);
        v.pos.set(target as f32);
    });
    drop(images);
    unsafe { let _ = SetTimer(hwnd, PREVIEW_TIMER_ID, super::gallery::PREVIEW_DELAY_MS, None); }
    request_render();
}

fn chip_at(x: f32, y: f32) -> Option<Family> {
    nv(|v| v.chips.borrow().iter().find(|(_, rect)| inside(rect, x, y)).map(|(f, _)| *f))
}

unsafe fn ensure_window() -> Option<HWND> {
    if let Some(hwnd) = nv(|v| v.hwnd.get()) { return Some(hwnd); }
    let hinstance = HINSTANCE(GetModuleHandleW(None).ok()?.0);
    let class_name = to_wide_chars(CLASS_NAME);
    let wc = WNDCLASSW {
        lpfnWndProc: Some(wnd_proc),
        hInstance: hinstance,
        hIcon: crate::resources::app_icon(),
        lpszClassName: as_pcwstr(&class_name),
        ..Default::default()
    };
    let _ = RegisterClassW(&wc);
    let hwnd = CreateWindowExW(
        WS_EX_LAYERED | WS_EX_TOPMOST | WS_EX_TOOLWINDOW,
        as_pcwstr(&class_name),
        as_pcwstr(&to_wide_chars("SWEPR Notch")),
        WS_POPUP,
        0, 0, 1, 1,
        None, None, hinstance, None,
    ).ok()?;
    nv(|v| v.hwnd.set(Some(hwnd)));
    Some(hwnd)
}

/// Ventana, medidas, monitor y origen del notch en el monitor del cursor (con el destino de dibujo listo).
unsafe fn place() -> Option<(HWND, Metrics, RECT, POINT, u32)> {
    let hwnd = ensure_window()?;
    let mut pt = POINT::default();
    let _ = GetCursorPos(&mut pt);
    let hmon = MonitorFromPoint(pt, MONITOR_DEFAULTTONEAREST);
    let mut mi = MONITORINFO { cbSize: std::mem::size_of::<MONITORINFO>() as u32, ..Default::default() };
    let _ = GetMonitorInfoW(hmon, &mut mi);
    let (mut dpi_x, mut dpi_y) = (96u32, 96u32);
    let _ = windows::Win32::UI::HiDpi::GetDpiForMonitor(hmon, windows::Win32::UI::HiDpi::MDT_EFFECTIVE_DPI, &mut dpi_x, &mut dpi_y);
    let monitor = mi.rcMonitor;
    let metrics = Metrics::new(monitor.right - monitor.left, dpi_x);
    let origin = POINT { x: monitor.left + (monitor.right - monitor.left - metrics.window_w) / 2, y: monitor.top };

    // Destino de dibujo nuevo si cambió el tamaño (otro monitor u otro DPI).
    let reuse = nv(|v| v.renderer.borrow().as_ref().is_some_and(|r| r.width == metrics.window_w && r.height == metrics.window_h));
    if !reuse {
        match Renderer::new(metrics.window_w, metrics.window_h, metrics.scale) {
            Ok(renderer) => nv(|v| *v.renderer.borrow_mut() = Some(renderer)),
            Err(e) => {
                crate::log_warn!("Notch: Direct2D no disponible ({e})");
                return None;
            }
        }
    }
    Some((hwnd, metrics, monitor, origin, dpi_x))
}

/// Abre el notch en el monitor del cursor.
pub fn show(close_on_leave: bool) {
    unsafe {
        match nv(|v| v.phase.get()) {
            Phase::Opening | Phase::Open => return,
            Phase::Notify => {
                // El aviso deja paso al panel.
                if let Some(hwnd) = nv(|v| v.hwnd.get()) { finish_hide(hwnd); }
            }
            _ => {}
        }
        let Some((hwnd, metrics, monitor, origin, dpi_x)) = place() else { return; };

        // Las tarjetas grandes del carrusel se generan al tamaño de la imagen central.
        carousel::set_layout(carousel::Layout {
            tile_w: metrics.item_w as i32,
            tile_h: metrics.item_h as i32,
            slant: 0,
            gap: 0,
            pad_y: 0,
            margin_x: 0,
        });

        let current = super::vars(|v| v.current_wallpaper.borrow().clone());
        let selected = super::scanned()
            .and_then(|arc| arc.lock().ok().and_then(|images| current.as_ref().and_then(|cur| images.iter().position(|img| same_path(&img.path, cur)))))
            .unwrap_or(0);
        let from = if nv(|v| v.phase.get()) == Phase::Closing { nv(|v| v.open.get()) } else { 0.0 };
        if from == 0.0 {
            nv(|v| {
                v.family.set(None);
                v.hot_chip.set(None);
                v.pos.set(selected as f32);
                v.target.set(selected as i64);
                v.hover.set(None);
                v.hot_button.set(None);
            });
        }
        nv(|v| {
            v.metrics.set(Some(metrics));
            v.monitor.set(monitor);
            v.origin.set(origin);
            v.close_on_leave.set(close_on_leave);
            v.phase.set(Phase::Opening);
            v.phase_from.set(from);
            v.open.set(from);
            v.phase_start.set(Instant::now());
            v.last_step.set(Some(Instant::now()));
        });
        super::vars(|v| v.dpi.set(dpi_x.max(96)));

        let _ = KillTimer(hwnd, TILE_RELEASE_TIMER_ID);
        render(hwnd);
        let _ = ShowWindow(hwnd, SW_SHOWNA);
        let _ = windows::Win32::UI::WindowsAndMessaging::SetWindowPos(hwnd, HWND_TOPMOST, 0, 0, 0, 0,
            SWP_NOACTIVATE | windows::Win32::UI::WindowsAndMessaging::SWP_NOMOVE | windows::Win32::UI::WindowsAndMessaging::SWP_NOSIZE);
        super::force_focus(hwnd);
        let _ = SetTimer(hwnd, FRAME_TIMER_ID, FRAME_MS, None);
        let _ = SetTimer(hwnd, CLOSE_CHECK_TIMER_ID, if close_on_leave { SENSOR_CLOSE_CHECK_MS } else { HOTKEY_CLICK_CHECK_MS }, None);
        crate::log_debug!("Notch: abriendo");
    }
}

/// Cierra el notch con su animación.
pub fn hide() {
    if !matches!(nv(|v| v.phase.get()), Phase::Opening | Phase::Open) { return; }
    let Some(hwnd) = nv(|v| v.hwnd.get()) else { return; };
    nv(|v| {
        v.phase_from.set(v.open.get());
        v.phase.set(Phase::Closing);
        v.phase_start.set(Instant::now());
        v.hover.set(None);
        v.hot_button.set(None);
        v.buttons.set(None);
    });
    super::preview_end();
    unsafe {
        let _ = KillTimer(hwnd, CLOSE_CHECK_TIMER_ID);
        let _ = KillTimer(hwnd, PREVIEW_TIMER_ID);
        let _ = SetTimer(hwnd, FRAME_TIMER_ID, FRAME_MS, None);
    }
}

/// Cierra el notch sin animación y libera Direct2D.
pub fn hide_now() {
    let Some(hwnd) = nv(|v| v.hwnd.get()) else { return; };
    if nv(|v| v.phase.get()) == Phase::Hidden { return; }
    super::preview_end();
    finish_hide(hwnd);
}

fn finish_hide(hwnd: HWND) {
    unsafe {
        let _ = KillTimer(hwnd, FRAME_TIMER_ID);
        let _ = KillTimer(hwnd, CLOSE_CHECK_TIMER_ID);
        let _ = KillTimer(hwnd, PREVIEW_TIMER_ID);
        let _ = KillTimer(hwnd, NOTIFY_TIMER_ID);
        let _ = ShowWindow(hwnd, SW_HIDE);
        let _ = SetTimer(hwnd, TILE_RELEASE_TIMER_ID, TILE_RELEASE_DELAY_MS, None);
    }
    nv(|v| {
        v.phase.set(Phase::Hidden);
        v.open.set(0.0);
        v.items.borrow_mut().clear();
        v.buttons.set(None);
        v.notify_path.borrow_mut().take();
        if let Some(thumb) = v.notify_thumb.borrow_mut().take() { delete_thumb(&thumb); }
        // Fábricas, destino y mapas de bits solo viven mientras el notch está abierto.
        *v.renderer.borrow_mut() = None;
    });
    crate::log::log_resource_usage();
    super::flush_notify();
}

/// Pide un fotograma nuevo (por ejemplo, porque llegó una miniatura o cambió una favorita).
pub fn request_render() {
    if !is_visible() || nv(|v| v.render_pending.replace(true)) { return; }
    if let Some(hwnd) = nv(|v| v.hwnd.get()) {
        unsafe { let _ = PostMessageW(hwnd, WM_APP_NOTCH_RENDER, WPARAM(0), LPARAM(0)); }
    }
}

/// Avanza las animaciones. Devuelve si alguna sigue en curso.
fn step(hwnd: HWND) -> bool {
    let now = Instant::now();
    let mut busy = false;
    let elapsed_ms = nv(|v| now.duration_since(v.phase_start.get()).as_secs_f32() * 1000.0);
    match nv(|v| v.phase.get()) {
        Phase::Opening => {
            let from = nv(|v| v.phase_from.get());
            let open = (from + elapsed_ms / OPEN_MS).min(1.0);
            nv(|v| v.open.set(open));
            if open >= 1.0 { nv(|v| v.phase.set(Phase::Open)); } else { busy = true; }
        }
        Phase::Closing => {
            let from = nv(|v| v.phase_from.get());
            let open = (from - elapsed_ms / CLOSE_MS).max(0.0);
            nv(|v| v.open.set(open));
            if open <= 0.0 {
                finish_hide(hwnd);
                return false;
            }
            busy = true;
        }
        Phase::Notify => return step_notify(hwnd),
        _ => {}
    }
    // Desplazamiento suave hacia la imagen elegida (independiente de la frecuencia de ticks).
    let elapsed = nv(|v| v.last_step.replace(Some(now))).map(|last| now.duration_since(last).as_secs_f32()).unwrap_or(1.0 / 60.0).min(0.1);
    let factor = 1.0 - 0.78f32.powf(elapsed * 60.0);
    let (pos, target) = nv(|v| (v.pos.get(), v.target.get() as f32));
    if (target - pos).abs() > 0.002 {
        nv(|v| v.pos.set(pos + (target - pos) * factor));
        busy = true;
    } else {
        nv(|v| v.pos.set(target));
    }
    busy
}

fn move_selection(hwnd: HWND, delta: i64) {
    if view_count() == 0 { return; }
    nv(|v| {
        v.target.set(v.target.get() + delta);
        v.last_step.set(Some(Instant::now()));
    });
    unsafe {
        let _ = SetTimer(hwnd, FRAME_TIMER_ID, FRAME_MS, None);
        let _ = SetTimer(hwnd, PREVIEW_TIMER_ID, super::gallery::PREVIEW_DELAY_MS, None);
    }
}

// --- Aviso de cambio de fondo ---

/// Miniatura del aviso que dejó el hilo de carga: (ruta, miniatura).
static NOTIFY_READY: Mutex<Option<(std::path::PathBuf, Thumb)>> = Mutex::new(None);

fn delete_thumb(thumb: &Thumb) {
    if thumb.hbitmap != 0 {
        unsafe { let _ = windows::Win32::Graphics::Gdi::DeleteObject(windows::Win32::Graphics::Gdi::HGDIOBJ(thumb.hbitmap as *mut _)); }
    }
}

/// Baja la pastilla con la miniatura y el nombre del fondo nuevo durante unos segundos.
/// La miniatura se carga aparte; el aviso aparece cuando está lista.
pub fn notify(path: &std::path::Path) {
    if matches!(nv(|v| v.phase.get()), Phase::Opening | Phase::Open | Phase::Closing) { return; }
    if !crate::winutil::is_notification_allowed() { return; }
    let Some(hwnd) = (unsafe { ensure_window() }) else { return; };
    let dpi = unsafe {
        let mut pt = POINT::default();
        let _ = GetCursorPos(&mut pt);
        let hmon = MonitorFromPoint(pt, MONITOR_DEFAULTTONEAREST);
        let (mut dpi_x, mut dpi_y) = (96u32, 96u32);
        let _ = windows::Win32::UI::HiDpi::GetDpiForMonitor(hmon, windows::Win32::UI::HiDpi::MDT_EFFECTIVE_DPI, &mut dpi_x, &mut dpi_y);
        dpi_x.max(96)
    };
    let (thumb_w, thumb_h) = toast_thumb_size(dpi as f32 / 96.0);
    let path = path.to_path_buf();
    let notify = hwnd.0 as isize;
    std::thread::spawn(move || unsafe {
        use windows::Win32::System::Com::{CoInitializeEx, COINIT_APARTMENTTHREADED};
        let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
        let Some(thumb) = crate::thumbs::load_cover_bitmap(&path, thumb_w as i32, thumb_h as i32) else { return; };
        if let Ok(mut ready) = NOTIFY_READY.lock() {
            if let Some((_, old)) = ready.replace((path, thumb)) { delete_thumb(&old); }
        }
        let _ = PostMessageW(HWND(notify as *mut _), WM_APP_NOTCH_NOTIFY, WPARAM(0), LPARAM(0));
    });
}

fn toast_thumb_size(scale: f32) -> (f32, f32) {
    let h = ((BASE_TOAST_H - 24.0) * scale).round();
    ((h * 16.0 / 9.0).round(), h)
}

/// Llegó la miniatura: mostrar (o renovar) el aviso.
unsafe fn start_notify(hwnd: HWND) {
    let Some((path, thumb)) = NOTIFY_READY.lock().ok().and_then(|mut r| r.take()) else { return; };
    // Mientras tanto se abrió el panel: el aviso ya no hace falta.
    if matches!(nv(|v| v.phase.get()), Phase::Opening | Phase::Open | Phase::Closing) || super::is_panel_visible() {
        delete_thumb(&thumb);
        return;
    }
    if nv(|v| v.phase.get()) == Phase::Notify {
        // Otro cambio con el aviso a la vista: sigue abierto con el fondo nuevo.
        let shown = std::time::Duration::from_secs_f32(nv(|v| v.open.get()).min(1.0) * NOTIFY_IN_MS / 1000.0);
        nv(|v| v.notify_start.set(Instant::now() - shown));
    } else {
        let Some((_, metrics, monitor, origin, _)) = place() else {
            delete_thumb(&thumb);
            return;
        };
        nv(|v| {
            v.metrics.set(Some(metrics));
            v.monitor.set(monitor);
            v.origin.set(origin);
            v.open.set(0.0);
            v.phase.set(Phase::Notify);
            v.notify_start.set(Instant::now());
            v.hover.set(None);
            v.items.borrow_mut().clear();
            v.buttons.set(None);
        });
    }
    nv(|v| {
        *v.notify_path.borrow_mut() = Some(path);
        if let Some(old) = v.notify_thumb.borrow_mut().replace(thumb) { delete_thumb(&old); }
    });
    let _ = KillTimer(hwnd, TILE_RELEASE_TIMER_ID);
    let _ = KillTimer(hwnd, NOTIFY_TIMER_ID);
    render(hwnd);
    let _ = ShowWindow(hwnd, SW_SHOWNA);
    let _ = windows::Win32::UI::WindowsAndMessaging::SetWindowPos(hwnd, HWND_TOPMOST, 0, 0, 0, 0,
        SWP_NOACTIVATE | windows::Win32::UI::WindowsAndMessaging::SWP_NOMOVE | windows::Win32::UI::WindowsAndMessaging::SWP_NOSIZE);
    let _ = SetTimer(hwnd, FRAME_TIMER_ID, FRAME_MS, None);
}

/// Animación del aviso. Mientras está quieto no se pintan fotogramas: espera con un temporizador.
fn step_notify(hwnd: HWND) -> bool {
    let t = nv(|v| v.notify_start.get().elapsed().as_secs_f32() * 1000.0);
    if t < NOTIFY_IN_MS {
        nv(|v| v.open.set(t / NOTIFY_IN_MS));
        return true;
    }
    if t < NOTIFY_IN_MS + NOTIFY_HOLD_MS {
        nv(|v| v.open.set(1.0));
        unsafe { let _ = SetTimer(hwnd, NOTIFY_TIMER_ID, (NOTIFY_IN_MS + NOTIFY_HOLD_MS - t).max(1.0) as u32, None); }
        return false;
    }
    let out = (t - NOTIFY_IN_MS - NOTIFY_HOLD_MS) / NOTIFY_OUT_MS;
    if out >= 1.0 {
        finish_hide(hwnd);
        return false;
    }
    nv(|v| v.open.set(1.0 - out));
    true
}

/// Termina el tiempo visible del aviso; si el ratón está encima, espera un poco más.
unsafe fn notify_hold_elapsed(hwnd: HWND) {
    let _ = KillTimer(hwnd, NOTIFY_TIMER_ID);
    if !is_notifying() { return; }
    let mut pt = POINT::default();
    let _ = GetCursorPos(&mut pt);
    if cursor_over_notch(pt) {
        nv(|v| v.notify_start.set(v.notify_start.get() + std::time::Duration::from_millis(600)));
        let _ = SetTimer(hwnd, NOTIFY_TIMER_ID, 600, None);
        return;
    }
    let _ = SetTimer(hwnd, FRAME_TIMER_ID, FRAME_MS, None);
}

/// Contenido del aviso: miniatura a la izquierda, "Fondo cambiado" y el nombre.
unsafe fn draw_toast(renderer: &mut Renderer, target: &ID2D1RenderTarget, metrics: &Metrics) {
    let (w, h) = metrics.toast_size();
    let cx = metrics.center_x();
    let left = cx - w / 2.0;
    let (thumb_w, thumb_h) = toast_thumb_size(metrics.scale);
    let top = h - metrics.px(12.0) - thumb_h;
    let thumb_left = left + metrics.px(18.0);
    let thumb_rect = rect_f(thumb_left, top, thumb_left + thumb_w, top + thumb_h);
    let path = nv(|v| v.notify_path.borrow().clone());
    let hbitmap = nv(|v| v.notify_thumb.borrow().as_ref().map(|t| t.hbitmap));
    let shape = rounded(thumb_rect, metrics.px(8.0));
    if let (Some(path), Some(hbitmap)) = (path.as_ref(), hbitmap) {
        let key = format!("notify|{}", path.to_string_lossy());
        if let Some(bitmap) = renderer.bitmap(&key, false, hbitmap) { fill_cover(target, &bitmap, &shape, 1.0); }
    }
    let Ok(brush) = target.CreateSolidColorBrush(&fg(0.14), None) else { return; };
    target.DrawRoundedRectangle(&shape, &brush, 1.0, None);
    let text_left = thumb_rect.right + metrics.px(14.0);
    let text_right = cx + w / 2.0 - metrics.px(22.0);
    let mid = (thumb_rect.top + thumb_rect.bottom) / 2.0;
    if let Some(format) = renderer.meta_format.as_ref() {
        let _ = format.SetTextAlignment(DWRITE_TEXT_ALIGNMENT_LEADING);
        brush.SetColor(&fg(0.5));
        let text: Vec<u16> = crate::i18n::t("Wallpaper changed").encode_utf16().collect();
        // Sin nombre, el aviso queda centrado en vertical.
        let line = if super::show_names() { rect_f(text_left, mid - metrics.px(22.0), text_right, mid) } else { rect_f(text_left, mid - metrics.px(11.0), text_right, mid + metrics.px(11.0)) };
        target.DrawText(&text, format, &line, &brush, D2D1_DRAW_TEXT_OPTIONS_CLIP, DWRITE_MEASURING_MODE_NATURAL);
        let _ = format.SetTextAlignment(DWRITE_TEXT_ALIGNMENT_TRAILING);
    }
    if let Some(format) = renderer.title_format.as_ref().filter(|_| super::show_names()) {
        let _ = format.SetTextAlignment(DWRITE_TEXT_ALIGNMENT_LEADING);
        brush.SetColor(&fg(0.95));
        let name = path.as_ref().and_then(|p| p.file_stem()).map(|s| s.to_string_lossy().to_string()).unwrap_or_default();
        let text: Vec<u16> = name.encode_utf16().collect();
        target.DrawText(&text, format, &rect_f(text_left, mid, text_right, mid + metrics.px(22.0)), &brush, D2D1_DRAW_TEXT_OPTIONS_CLIP, DWRITE_MEASURING_MODE_NATURAL);
        let _ = format.SetTextAlignment(DWRITE_TEXT_ALIGNMENT_CENTER);
    }
}

fn select_virtual(hwnd: HWND, virtual_index: i64) {
    let delta = virtual_index - nv(|v| v.target.get());
    move_selection(hwnd, delta);
}

fn selected_path() -> Option<std::path::PathBuf> {
    image_at(nv(|v| v.target.get())).and_then(super::image_path)
}

fn monitor_rect() -> RECT { nv(|v| v.monitor.get()) }

fn apply(path: std::path::PathBuf) {
    log_info!("Notch: aplicar {:?}", path);
    app::request(app::Action::ApplyFromPanel(path.clone(), monitor_rect()));
    super::set_current_wallpaper(&path);
    super::hide_panel();
}

// --- Pintado ---

/// Pinta un fotograma y lo presenta en la ventana.
unsafe fn render(hwnd: HWND) {
    nv(|v| v.render_pending.set(false));
    let Some(metrics) = nv(|v| v.metrics.get()) else { return; };
    let origin = nv(|v| v.origin.get());
    let ok = nv(|v| {
        let mut guard = v.renderer.borrow_mut();
        let Some(renderer) = guard.as_mut() else { return false; };
        draw_frame(renderer, &metrics, hwnd, None);
        renderer.present(hwnd, origin, 255)
    });
    if !ok { crate::log_debug!("Notch: no se pudo presentar el fotograma"); }
}

/// Silueta del notch: hombros cóncavos arriba y esquinas redondeadas abajo.
unsafe fn notch_geometry(factory: &ID2D1Factory, cx: f32, w: f32, h: f32, radius: f32, shoulder: f32) -> Option<ID2D1PathGeometry> {
    let geometry = factory.CreatePathGeometry().ok()?;
    let sink = geometry.Open().ok()?;
    let (left, right) = (cx - w / 2.0, cx + w / 2.0);
    let arc = |end: D2D_POINT_2F| D2D1_ARC_SEGMENT {
        point: end,
        size: D2D_SIZE_F { width: radius, height: radius },
        rotationAngle: 0.0,
        sweepDirection: D2D1_SWEEP_DIRECTION_COUNTER_CLOCKWISE,
        arcSize: D2D1_ARC_SIZE_SMALL,
    };
    sink.BeginFigure(point(left - shoulder, 0.0), D2D1_FIGURE_BEGIN_FILLED);
    sink.AddQuadraticBezier(&D2D1_QUADRATIC_BEZIER_SEGMENT { point1: point(left, 0.0), point2: point(left, shoulder) });
    sink.AddLine(point(left, h - radius));
    sink.AddArc(&arc(point(left + radius, h)));
    sink.AddLine(point(right - radius, h));
    sink.AddArc(&arc(point(right, h - radius)));
    sink.AddLine(point(right, shoulder));
    sink.AddQuadraticBezier(&D2D1_QUADRATIC_BEZIER_SEGMENT { point1: point(right, 0.0), point2: point(right + shoulder, 0.0) });
    sink.EndFigure(D2D1_FIGURE_END_CLOSED);
    sink.Close().ok()?;
    Some(geometry)
}

/// Dibuja el fotograma completo. `backdrop` solo lo usan las pruebas para ver el notch sobre un fondo.
unsafe fn draw_frame(renderer: &mut Renderer, metrics: &Metrics, notify: HWND, backdrop: Option<&ID2D1Bitmap>) {
    renderer.tick += 1;
    let target = renderer.rt();
    target.BeginDraw();
    target.Clear(Some(&color((0.0, 0.0, 0.0), 0.0)));
    if let Some(backdrop) = backdrop {
        target.DrawBitmap(backdrop, Some(&rect_f(0.0, 0.0, renderer.width as f32, renderer.height as f32)), 1.0, D2D1_BITMAP_INTERPOLATION_MODE_LINEAR, None);
    }

    let open = nv(|v| v.open.get());
    let notifying = nv(|v| v.phase.get()) == Phase::Notify;
    let (w, h, radius, shoulder) = if notifying { metrics.toast_shape(open) } else { metrics.shape(open) };
    let cx = metrics.center_x();
    let body_alpha = smoothstep(open / (PILL_PHASE * 0.5));
    let mut items = Vec::new();
    let mut buttons = None;

    if let Some(geometry) = notch_geometry(&renderer.factory, cx, w, h, radius, shoulder) {
        let geometry: ID2D1Geometry = geometry.into();
        // Sombra suave: trazos cada vez más estrechos que se acumulan hacia el borde.
        if let Ok(shadow) = target.CreateSolidColorBrush(&color((0.0, 0.0, 0.0), 0.05 * body_alpha), None) {
            let steps = 7;
            for i in (1..=steps).rev() {
                target.DrawGeometry(&geometry, &shadow, metrics.px(BASE_SHADOW) * 2.0 * i as f32 / steps as f32, None);
            }
        }
        if let Ok(body) = target.CreateSolidColorBrush(&color(if light() { COLOR_BODY_LIGHT } else { COLOR_BODY }, body_alpha), None) {
            target.FillGeometry(&geometry, &body, None);
        }
        // Filo muy sutil que separa el notch de fondos oscuros (sin la línea del borde superior).
        if let Ok(edge) = target.CreateSolidColorBrush(&fg(0.07 * body_alpha), None) {
            target.PushAxisAlignedClip(&rect_f(0.0, shoulder + 1.0, renderer.width as f32, renderer.height as f32), D2D1_ANTIALIAS_MODE_PER_PRIMITIVE);
            target.DrawGeometry(&geometry, &edge, 1.0, None);
            target.PopAxisAlignedClip();
        }

        let content_alpha = smoothstep((open - 0.55) / 0.35);
        if content_alpha > 0.0 {
            // El contenido se recorta a la silueta mientras crece.
            let parameters = D2D1_LAYER_PARAMETERS {
                contentBounds: rect_f(0.0, 0.0, renderer.width as f32, renderer.height as f32),
                geometricMask: std::mem::ManuallyDrop::new(Some(geometry.clone())),
                maskAntialiasMode: D2D1_ANTIALIAS_MODE_PER_PRIMITIVE,
                maskTransform: Matrix3x2::identity(),
                opacity: content_alpha,
                opacityBrush: std::mem::ManuallyDrop::new(None),
                layerOptions: D2D1_LAYER_OPTIONS_NONE,
            };
            let pushed = if let Some(layer) = renderer.layer.clone() { target.PushLayer(&parameters, &layer); true } else { false };
            // El contenido entra deslizándose un poco desde arriba.
            let slide = (1.0 - content_alpha) * -metrics.px(10.0);
            target.SetTransform(&Matrix3x2::translation(0.0, slide));
            if notifying {
                draw_toast(renderer, &target, metrics);
            } else {
                (items, buttons) = draw_content(renderer, &target, metrics, notify);
            }
            target.SetTransform(&Matrix3x2::identity());
            if pushed { target.PopLayer(); }
            drop(std::mem::ManuallyDrop::into_inner(parameters.geometricMask));
        }
    }

    let _ = target.EndDraw(None, None);
    nv(|v| {
        *v.items.borrow_mut() = items;
        v.buttons.set(buttons);
    });
    renderer.trim(48);
}

/// Cabecera (nombre e índice) y fila de imágenes. Devuelve lo que se pintó para el ratón.
unsafe fn draw_content(renderer: &mut Renderer, target: &ID2D1RenderTarget, metrics: &Metrics, notify: HWND) -> (Vec<HitItem>, Option<HitButtons>) {
    let mut hits = Vec::new();
    let mut buttons = None;
    let Some(images_arc) = super::scanned() else { return (hits, buttons); };
    let Ok(images) = images_arc.lock() else { return (hits, buttons); };
    let count = images.len();
    let cx = metrics.center_x();
    let left = cx - metrics.body_w / 2.0;
    let right = cx + metrics.body_w / 2.0;

    if count == 0 {
        if let (Some(format), Ok(brush)) = (renderer.title_format.as_ref(), target.CreateSolidColorBrush(&fg(0.7), None)) {
            let text: Vec<u16> = crate::i18n::t("No wallpapers found in the folder").encode_utf16().collect();
            target.DrawText(&text, format, &rect_f(left, 0.0, right, metrics.body_h), &brush, D2D1_DRAW_TEXT_OPTIONS_CLIP, DWRITE_MEASURING_MODE_NATURAL);
        }
        return (hits, buttons);
    }

    // Vista filtrada por color (sin filtro, toda la galería).
    let view = filtered(&images, nv(|v| v.family.get()));
    nv(|v| *v.view.borrow_mut() = view.clone());
    let small_for_chips = super::vars(|v| v.thumb_cache.borrow().clone());
    let families = families_present(&images, small_for_chips.as_ref());
    let header_top = metrics.px(BASE_PAD_TOP);
    let header = rect_f(left + metrics.px(BASE_PAD_X), header_top, right - metrics.px(BASE_PAD_X), header_top + metrics.px(BASE_HEADER_H));
    draw_chips(target, metrics, &header, &families);
    let len = view.len();
    if len == 0 {
        if let (Some(format), Ok(brush)) = (renderer.title_format.as_ref(), target.CreateSolidColorBrush(&fg(0.7), None)) {
            let text: Vec<u16> = crate::i18n::t("No images match").encode_utf16().collect();
            target.DrawText(&text, format, &rect_f(left, header.bottom, right, metrics.body_h), &brush, D2D1_DRAW_TEXT_OPTIONS_CLIP, DWRITE_MEASURING_MODE_NATURAL);
        }
        return (hits, buttons);
    }

    let pos = nv(|v| v.pos.get());
    let target_index = nv(|v| v.target.get());
    let hover = nv(|v| v.hover.get());
    let hot_button = nv(|v| v.hot_button.get());
    let current = super::vars(|v| v.current_wallpaper.borrow().clone());
    let small_cache = super::vars(|v| v.thumb_cache.borrow().clone());

    // Cabecera: nombre de la imagen elegida y posición en la galería.
    let selected = &images[view[real_index(target_index, len)]];
    if let Ok(brush) = target.CreateSolidColorBrush(&fg(0.92), None) {
        if let Some(format) = renderer.title_format.as_ref().filter(|_| super::show_names()) {
            let name = selected.path.file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or_default();
            let text: Vec<u16> = name.encode_utf16().collect();
            let inset = metrics.body_w * 0.2;
            target.DrawText(&text, format, &rect_f(header.left + inset, header.top, header.right - inset, header.bottom), &brush, D2D1_DRAW_TEXT_OPTIONS_CLIP, DWRITE_MEASURING_MODE_NATURAL);
        }
        if let Some(format) = renderer.meta_format.as_ref() {
            brush.SetColor(&fg(0.45));
            let text: Vec<u16> = format!("{} / {}", real_index(target_index, len) + 1, len).encode_utf16().collect();
            target.DrawText(&text, format, &header, &brush, D2D1_DRAW_TEXT_OPTIONS_CLIP, DWRITE_MEASURING_MODE_NATURAL);
            // Sin colores conocidos todavía (miniaturas sin cargar), el nombre de la app.
            if families.is_empty() {
                let _ = format.SetTextAlignment(DWRITE_TEXT_ALIGNMENT_LEADING);
                let text: Vec<u16> = "SWEPR".encode_utf16().collect();
                target.DrawText(&text, format, &header, &brush, D2D1_DRAW_TEXT_OPTIONS_CLIP, DWRITE_MEASURING_MODE_NATURAL);
                let _ = format.SetTextAlignment(DWRITE_TEXT_ALIGNMENT_TRAILING);
            }
        }
        // Punto azul si la elegida es el fondo actual.
        if current.as_ref().is_some_and(|cur| same_path(cur, &selected.path)) {
            brush.SetColor(&color(accent(), 1.0));
            let r = metrics.px(3.0);
            target.FillEllipse(&D2D1_ELLIPSE { point: point(cx, header.bottom + metrics.px(2.0)), radiusX: r, radiusY: r }, &brush);
        }
    }

    // Imágenes: de las más lejanas a la central, para que la central quede encima.
    let reach = metrics.reach();
    let first = pos.floor() as i64 - reach;
    let last = pos.ceil() as i64 + reach;
    let mut order: Vec<i64> = (first..=last).collect();
    order.sort_by(|a, b| (*b as f32 - pos).abs().total_cmp(&(*a as f32 - pos).abs()));

    // Pedir las tarjetas grandes visibles, primero las más cercanas al centro.
    let mut wanted: Vec<usize> = Vec::new();
    for virtual_index in order.iter().rev() {
        let index = view[real_index(*virtual_index, len)];
        if !wanted.contains(&index) { wanted.push(index); }
    }
    let keys: Vec<String> = wanted.iter().map(|i| images[*i].path.to_string_lossy().to_string()).collect();
    carousel::keep_tiles(keys.iter().map(|k| k.as_str()), wanted.len() + 6);
    let prefetch: Vec<usize> = wanted.iter().copied()
        .chain([first - 1, first - 2, last + 1, last + 2].into_iter().map(|i| view[real_index(i, len)]))
        .collect();
    carousel::request_tiles(&images, prefetch, notify);

    let fade_start = metrics.body_w / 2.0 - metrics.px(BASE_PAD_X);
    let fade_len = metrics.side_w() * 0.9;
    let Ok(brush) = target.CreateSolidColorBrush(&fg(1.0), None) else { return (hits, buttons); };
    for virtual_index in order {
        let d = virtual_index as f32 - pos;
        let rect = metrics.item_rect(d);
        // Desvanecer las que llegan al borde del notch.
        let outer = (rect.right - cx).abs().max((rect.left - cx).abs());
        let alpha = 1.0 - smoothstep((outer - fade_start) / fade_len);
        if alpha <= 0.01 || rect.right < left || rect.left > right { continue; }
        let img = &images[view[real_index(virtual_index, len)]];
        let key = img.path.to_string_lossy().to_string();
        let item_radius = metrics.px(BASE_ITEM_RADIUS) * metrics.item_scale(d);
        let shape = rounded(rect, item_radius);

        let large = carousel::tile(&key).and_then(|tile| renderer.bitmap(&key, true, tile.hbitmap));
        let bitmap = large.or_else(|| small_cache.as_ref().and_then(|c| c.get(&key)).and_then(|thumb| renderer.bitmap(&key, false, thumb.hbitmap)));
        match bitmap {
            Some(bitmap) => fill_cover(target, &bitmap, &shape, alpha),
            None => {
                brush.SetColor(&fg(0.07 * alpha));
                target.FillRoundedRectangle(&shape, &brush);
            }
        }

        let centered = 1.0 - d.abs().min(1.0);
        let hovered = hover == Some(virtual_index);
        // Las vecinas se oscurecen; la que tiene el ratón encima, menos.
        let dim = (1.0 - centered) * if hovered { 0.12 } else { 0.38 };
        if dim > 0.0 {
            brush.SetColor(&bg(dim * alpha));
            target.FillRoundedRectangle(&shape, &brush);
        }
        let is_current = current.as_ref().is_some_and(|cur| same_path(cur, &img.path));
        if is_current {
            brush.SetColor(&color(accent(), alpha));
            target.DrawRoundedRectangle(&shape, &brush, metrics.px(2.0), None);
        } else {
            brush.SetColor(&fg((0.10 + 0.55 * centered) * alpha));
            target.DrawRoundedRectangle(&shape, &brush, metrics.px(1.0).max(1.0), None);
        }

        let inset = metrics.px(8.0) * metrics.item_scale(d);
        if centered > 0.5 && hovered {
            buttons = Some(draw_buttons(&renderer.factory, target, &brush, metrics, virtual_index, rect, img.is_favorite, hot_button));
        } else {
            if img.is_new {
                brush.SetColor(&color(COLOR_NEW, alpha));
                let r = metrics.px(4.5) * metrics.item_scale(d);
                target.FillEllipse(&D2D1_ELLIPSE { point: point(rect.right - inset - r, rect.top + inset + r), radiusX: r, radiusY: r }, &brush);
            }
            if img.is_favorite {
                brush.SetColor(&color(COLOR_FAVORITE, alpha));
                let r = metrics.px(7.0) * metrics.item_scale(d);
                draw_star(&renderer.factory, target, &brush, point(rect.left + inset + r, rect.top + inset + r), r);
            }
        }
        hits.push(HitItem { virtual_index, rect });
    }
    // Para el ratón, la de encima (la última pintada) va primero.
    hits.reverse();
    (hits, buttons)
}


/// Fichas de color a la izquierda de la cabecera: filtran el carrusel por color dominante.
unsafe fn draw_chips(target: &ID2D1RenderTarget, metrics: &Metrics, header: &D2D_RECT_F, families: &[Family]) {
    let mut chips = Vec::new();
    let selected = nv(|v| v.family.get());
    let hot = nv(|v| v.hot_chip.get());
    if let Ok(brush) = target.CreateSolidColorBrush(&fg(1.0), None) {
        let d = metrics.px(12.0);
        let gap = metrics.px(8.0);
        let cy = (header.top + header.bottom) / 2.0;
        let mut x = header.left;
        for family in families {
            let center = point(x + d / 2.0, cy);
            let grow = if hot == Some(*family) && selected != Some(*family) { 1.15 } else { 1.0 };
            let faded = selected.is_some() && selected != Some(*family);
            brush.SetColor(&color(family.swatch(), if faded { 0.45 } else { 1.0 }));
            target.FillEllipse(&D2D1_ELLIPSE { point: center, radiusX: d / 2.0 * grow, radiusY: d / 2.0 * grow }, &brush);
            if selected == Some(*family) {
                brush.SetColor(&fg(0.95));
                let ring = d / 2.0 + metrics.px(3.0);
                target.DrawEllipse(&D2D1_ELLIPSE { point: center, radiusX: ring, radiusY: ring }, &brush, metrics.px(1.5), None);
            }
            chips.push((*family, rect_f(x - gap / 2.0, header.top, x + d + gap / 2.0, header.bottom)));
            x += d + gap;
        }
    }
    nv(|v| *v.chips.borrow_mut() = chips);
}

/// Botones "favorita" y "quitar" en la esquina superior derecha de la imagen central.
#[allow(clippy::too_many_arguments)]
unsafe fn draw_buttons(
    factory: &ID2D1Factory,
    target: &ID2D1RenderTarget,
    brush: &windows::Win32::Graphics::Direct2D::ID2D1SolidColorBrush,
    metrics: &Metrics,
    virtual_index: i64,
    rect: D2D_RECT_F,
    favorite: bool,
    hot: Option<super::ItemButton>,
) -> HitButtons {
    let size = metrics.px(BASE_BUTTON);
    let gap = metrics.px(6.0);
    let inset = metrics.px(8.0);
    let exclude = rect_f(rect.right - inset - size, rect.top + inset, rect.right - inset, rect.top + inset + size);
    let favorite_rect = rect_f(exclude.left - gap - size, exclude.top, exclude.left - gap, exclude.bottom);
    for (area, button) in [(favorite_rect, super::ItemButton::Favorite), (exclude, super::ItemButton::Exclude)] {
        let center = point((area.left + area.right) / 2.0, (area.top + area.bottom) / 2.0);
        let ellipse = D2D1_ELLIPSE { point: center, radiusX: size / 2.0, radiusY: size / 2.0 };
        brush.SetColor(&color((0.0, 0.0, 0.0), if hot == Some(button) { 0.78 } else { 0.55 }));
        target.FillEllipse(&ellipse, brush);
        brush.SetColor(&white(0.22));
        target.DrawEllipse(&ellipse, brush, 1.0, None);
    }
    // Estrella (dorada si ya es favorita) y aspa.
    let star_center = point((favorite_rect.left + favorite_rect.right) / 2.0, (favorite_rect.top + favorite_rect.bottom) / 2.0);
    brush.SetColor(&if favorite { color(COLOR_FAVORITE, 1.0) } else { white(0.92) });
    draw_star(factory, target, brush, star_center, size * 0.3);
    brush.SetColor(&white(0.92));
    let (x, y) = ((exclude.left + exclude.right) / 2.0, (exclude.top + exclude.bottom) / 2.0);
    let arm = size * 0.2;
    let stroke = metrics.px(1.8);
    target.DrawLine(point(x - arm, y - arm), point(x + arm, y + arm), brush, stroke, None);
    target.DrawLine(point(x + arm, y - arm), point(x - arm, y + arm), brush, stroke, None);
    HitButtons { virtual_index, favorite: favorite_rect, exclude }
}

// --- Ventana ---


fn button_at(x: f32, y: f32) -> Option<(i64, super::ItemButton)> {
    let buttons = nv(|v| v.buttons.get())?;
    if inside(&buttons.favorite, x, y) { return Some((buttons.virtual_index, super::ItemButton::Favorite)); }
    if inside(&buttons.exclude, x, y) { return Some((buttons.virtual_index, super::ItemButton::Exclude)); }
    None
}

fn item_at(x: f32, y: f32) -> Option<i64> {
    nv(|v| v.items.borrow().iter().find(|item| inside(&item.rect, x, y)).map(|item| item.virtual_index))
}

/// ¿Está el cursor (pantalla) sobre el notch o en la franja de arriba que lo abre?
fn cursor_over_notch(pt: POINT) -> bool {
    let Some(metrics) = nv(|v| v.metrics.get()) else { return false; };
    let origin = nv(|v| v.origin.get());
    let open = nv(|v| v.open.get());
    let (w, h, _, shoulder) = if is_notifying() { metrics.toast_shape(open) } else { metrics.shape(open) };
    let grace = metrics.px(BASE_CLOSE_GRACE);
    let x = (pt.x - origin.x) as f32;
    let y = (pt.y - origin.y) as f32;
    let cx = metrics.center_x();
    x >= cx - w / 2.0 - shoulder - grace && x <= cx + w / 2.0 + shoulder + grace && y >= -1.0 && y <= h + grace
}

fn lparam_point(lparam: LPARAM) -> (f32, f32) {
    ((lparam.0 & 0xFFFF) as i16 as f32, ((lparam.0 >> 16) & 0xFFFF) as i16 as f32)
}

unsafe extern "system" fn wnd_proc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    match msg {
        WM_TIMER => {
            match wparam.0 {
                FRAME_TIMER_ID => {
                    let busy = step(hwnd);
                    if is_visible() {
                        render(hwnd);
                        if !busy { let _ = KillTimer(hwnd, FRAME_TIMER_ID); }
                    }
                }
                CLOSE_CHECK_TIMER_ID => {
                    if super::vars(|v| v.menu_open.get()) || nv(|v| v.phase.get()) == Phase::Closing { return LRESULT(0); }
                    let mut pt = POINT::default();
                    let _ = GetCursorPos(&mut pt);
                    let over = cursor_over_notch(pt);
                    let clicked = GetAsyncKeyState(VK_LBUTTON.0 as i32) < 0;
                    if !over && (nv(|v| v.close_on_leave.get()) || clicked) {
                        super::hide_panel();
                    }
                }
                PREVIEW_TIMER_ID => {
                    let _ = KillTimer(hwnd, PREVIEW_TIMER_ID);
                    if matches!(nv(|v| v.phase.get()), Phase::Opening | Phase::Open) {
                        super::preview_focus(selected_path(), monitor_rect());
                    }
                }
                NOTIFY_TIMER_ID => notify_hold_elapsed(hwnd),
                TILE_RELEASE_TIMER_ID => {
                    let _ = KillTimer(hwnd, TILE_RELEASE_TIMER_ID);
                    if !is_visible() { carousel::clear_tiles(); }
                }
                _ => {}
            }
            LRESULT(0)
        }
        WM_APP_NOTCH_RENDER => {
            if is_visible() { render(hwnd); }
            LRESULT(0)
        }
        carousel::WM_APP_TILE_READY => {
            carousel::collect_ready();
            request_render();
            LRESULT(0)
        }
        WM_MOUSEMOVE => {
            if !nv(|v| v.tracking_leave.replace(true)) {
                let mut track = TRACKMOUSEEVENT { cbSize: std::mem::size_of::<TRACKMOUSEEVENT>() as u32, dwFlags: TME_LEAVE, hwndTrack: hwnd, dwHoverTime: 0 };
                let _ = TrackMouseEvent(&mut track);
            }
            let (x, y) = lparam_point(lparam);
            let chip = chip_at(x, y);
            if nv(|v| v.hot_chip.replace(chip)) != chip { request_render(); }
            let over_button = button_at(x, y);
            let hover = over_button.map(|(index, _)| index).or_else(|| item_at(x, y));
            let hot = over_button.map(|(_, button)| button);
            let changed = nv(|v| v.hover.replace(hover) != hover) | nv(|v| v.hot_button.replace(hot) != hot);
            if changed { request_render(); }
            LRESULT(0)
        }
        WM_MOUSELEAVE => {
            nv(|v| {
                v.tracking_leave.set(false);
                v.hot_chip.set(None);
                v.hover.set(None);
                v.hot_button.set(None);
            });
            request_render();
            LRESULT(0)
        }
        WM_APP_NOTCH_NOTIFY => {
            start_notify(hwnd);
            LRESULT(0)
        }
        WM_LBUTTONUP if is_notifying() => {
            // Clic en el aviso: se recoge ya.
            let _ = KillTimer(hwnd, NOTIFY_TIMER_ID);
            let skip = std::time::Duration::from_secs_f32((NOTIFY_IN_MS + NOTIFY_HOLD_MS) / 1000.0);
            nv(|v| v.notify_start.set(Instant::now() - skip));
            let _ = SetTimer(hwnd, FRAME_TIMER_ID, FRAME_MS, None);
            LRESULT(0)
        }
        WM_LBUTTONUP => {
            let (x, y) = lparam_point(lparam);
            if let Some(family) = chip_at(x, y) {
                toggle_family(hwnd, family);
                return LRESULT(0);
            }
            if view_count() == 0 { return LRESULT(0); }
            if let Some((virtual_index, button)) = button_at(x, y) {
                if let Some(path) = image_at(virtual_index).and_then(super::image_path) {
                    match button {
                        super::ItemButton::Favorite => app::request(app::Action::ToggleFavorite(path)),
                        super::ItemButton::Exclude => app::request(app::Action::Exclude(path)),
                    }
                }
            } else if let Some(virtual_index) = item_at(x, y) {
                // Clic en la central: aplicarla. En una vecina: traerla al centro.
                if virtual_index == nv(|v| v.target.get()) {
                    if let Some(path) = image_at(virtual_index).and_then(super::image_path) { apply(path); }
                } else {
                    select_virtual(hwnd, virtual_index);
                }
            }
            LRESULT(0)
        }
        WM_RBUTTONUP => {
            let (x, y) = lparam_point(lparam);
            if let Some(virtual_index) = item_at(x, y) {
                if let Some(index) = image_at(virtual_index) { super::show_thumbnail_menu(hwnd, index); }
            }
            LRESULT(0)
        }
        WM_MOUSEWHEEL => {
            let delta = (wparam.0 >> 16) as i16 as i32;
            move_selection(hwnd, if delta < 0 { 1 } else { -1 });
            LRESULT(0)
        }
        WM_KEYDOWN => {
            match wparam.0 as u16 {
                0x1B => super::hide_panel(),          // Esc
                0x25 => move_selection(hwnd, -1),     // Izquierda
                0x27 => move_selection(hwnd, 1),      // Derecha
                0x0D => if let Some(path) = selected_path() { apply(path); },
                0x5D => {                              // Tecla de menú
                    if let Some(index) = image_at(nv(|v| v.target.get())) { super::show_thumbnail_menu(hwnd, index); }
                }
                _ => return DefWindowProcW(hwnd, msg, wparam, lparam),
            }
            LRESULT(0)
        }
        WM_SETCURSOR => {
            if let Ok(cursor) = LoadCursorW(None, IDC_ARROW) { let _ = SetCursor(cursor); }
            LRESULT(1)
        }
        _ => DefWindowProcW(hwnd, msg, wparam, lparam),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use super::super::d2d::gdi_to_d2d;
    use crate::scanner::ScannedImage;
    use windows::Win32::Graphics::Gdi::HBITMAP;

    #[test]
    fn metrics_fit_the_monitor() {
        for (width, dpi) in [(1920, 96), (2560, 144), (1366, 96), (3840, 192)] {
            let m = Metrics::new(width, dpi);
            assert!(m.window_w <= width, "{width}@{dpi}: {} > {width}", m.window_w);
            assert!(m.body_w > m.item_w * 2.0);
            // Abierto del todo (con el rebote) el cuerpo cabe en la ventana.
            let (w, h, _, shoulder) = m.shape(0.8);
            assert!(w + shoulder * 2.0 <= m.window_w as f32, "{width}@{dpi}");
            assert!(h <= m.window_h as f32);
        }
    }

    #[test]
    fn items_are_ordered_and_do_not_overlap() {
        let m = Metrics::new(1920, 96);
        for pos in [0.0f32, 0.3, 0.5, 0.9] {
            let rects: Vec<D2D_RECT_F> = (-4..=4).map(|i| m.item_rect(i as f32 - pos)).collect();
            for pair in rects.windows(2) {
                assert!(pair[0].right <= pair[1].left + 0.5, "pos={pos}: {:?} {:?}", pair[0], pair[1]);
            }
        }
        // La central está en el centro y es la más grande.
        let center = m.item_rect(0.0);
        assert!(((center.left + center.right) / 2.0 - m.center_x()).abs() < 0.5);
        assert!((center.right - center.left - m.item_w).abs() < 0.5);
    }

    #[test]
    fn shape_grows_from_pill() {
        let m = Metrics::new(1920, 96);
        let (_, h0, ..) = m.shape(0.0);
        let (wp, hp, ..) = m.shape(PILL_PHASE);
        let (w1, h1, ..) = m.shape(1.0);
        assert!(h0 < 0.5);
        assert!((wp - BASE_PILL_W).abs() < 0.5 && (hp - BASE_PILL_H).abs() < 0.5);
        assert!((w1 - m.body_w).abs() < 0.5 && (h1 - m.body_h).abs() < 0.5);
    }

    /// Pinta el notch abierto sobre la primera imagen en un BMP para revisarlo a ojo:
    /// `SWEPR_TEST_DIR=<carpeta> SWEPR_TEST_OUT=<prefijo> cargo test render_notch_preview -- --ignored`
    #[test]
    #[ignore]
    fn render_notch_preview() {
        if std::env::var("SWEPR_TEST_LIGHT").is_ok() { crate::winutil::set_theme(2); }
        use std::path::PathBuf;
        use std::sync::{Arc, Mutex};
        let dir = std::env::var("SWEPR_TEST_DIR").expect("SWEPR_TEST_DIR");
        let out = std::env::var("SWEPR_TEST_OUT").expect("SWEPR_TEST_OUT");
        let mut paths: Vec<PathBuf> = std::fs::read_dir(dir).unwrap().flatten().map(|e| e.path())
            .filter(|p| crate::scanner::is_supported_image(p))
            .collect();
        paths.sort();
        paths.truncate(14);
        let images: Vec<ScannedImage> = paths.iter().enumerate().map(|(i, p)| ScannedImage {
            path: p.clone(),
            file_name: String::new(),
            is_new: i == 6,
            is_favorite: i == 3,
            modified: None,
        }).collect();
        let metrics = Metrics::new(1920, 96);
        unsafe {
            use windows::Win32::System::Com::{CoInitializeEx, COINIT_APARTMENTTHREADED};
            let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
            carousel::set_layout(carousel::Layout { tile_w: metrics.item_w as i32, tile_h: metrics.item_h as i32, slant: 0, gap: 0, pad_y: 0, margin_x: 0 });
            let mut renderer = Renderer::new(metrics.window_w, metrics.window_h, metrics.scale).expect("Direct2D");
            // Fondo de prueba: la primera imagen recortada al tamaño de la ventana.
            let backdrop = crate::thumbs::load_cover_bitmap(&images[0].path, metrics.window_w, metrics.window_h)
                .and_then(|thumb| gdi_to_d2d(&renderer.target.clone().into(), thumb.hbitmap));
            // Generar las tarjetas aquí (sin hilo de carga) y meterlas en la caché como si llegaran.
            let small = crate::thumbs::ThumbnailCache::new();
            for img in &images {
                if let Some(thumb) = crate::thumbs::load_cover_bitmap(&img.path, metrics.item_w as i32, metrics.item_h as i32) {
                    small.insert(thumb);
                }
            }
            let focus = 5usize;
            super::super::vars(|v| {
                *v.current_wallpaper.borrow_mut() = Some(images[2].path.clone());
                *v.thumb_cache.borrow_mut() = Some(small);
                *v.scanned.borrow_mut() = Some(Arc::new(Mutex::new(images.clone())));
            });
            for (suffix, open, pos, hover) in [("open", 1.0f32, focus as f32, Some(focus as i64)), ("moving", 1.0, focus as f32 + 0.45, None), ("pill", PILL_PHASE, focus as f32, None), ("growing", 0.5, focus as f32, None)] {
                nv(|v| {
                    v.open.set(open);
                    v.pos.set(pos);
                    v.target.set(focus as i64);
                    v.hover.set(hover);
                });
                draw_frame(&mut renderer, &metrics, HWND::default(), backdrop.as_ref());
                let bmp = HBITMAP(renderer.dib.0);
                super::super::save_bmp(renderer.dc, bmp, renderer.width, renderer.height, &format!("{out}-{suffix}.bmp"));
            }
            // Filtrado por el color de la primera ficha.
            let family = families_present(&images, super::super::vars(|v| v.thumb_cache.borrow().clone()).as_ref()).into_iter().next();
            nv(|v| {
                v.family.set(family);
                v.open.set(1.0);
                v.pos.set(0.0);
                v.target.set(0);
                v.hover.set(None);
            });
            draw_frame(&mut renderer, &metrics, HWND::default(), backdrop.as_ref());
            super::super::save_bmp(renderer.dc, HBITMAP(renderer.dib.0), renderer.width, renderer.height, &format!("{out}-colors.bmp"));
            nv(|v| v.family.set(None));
            // Aviso de cambio de fondo, a medio entrar y entero.
            let (tw, th) = toast_thumb_size(metrics.scale);
            let thumb = crate::thumbs::load_cover_bitmap(&images[4].path, tw as i32, th as i32);
            nv(|v| {
                v.phase.set(Phase::Notify);
                *v.notify_path.borrow_mut() = Some(images[4].path.clone());
                *v.notify_thumb.borrow_mut() = thumb;
            });
            for (suffix, open) in [("toast", 1.0f32), ("toast-in", 0.5)] {
                nv(|v| v.open.set(open));
                draw_frame(&mut renderer, &metrics, HWND::default(), backdrop.as_ref());
                let bmp = HBITMAP(renderer.dib.0);
                super::super::save_bmp(renderer.dc, bmp, renderer.width, renderer.height, &format!("{out}-{suffix}.bmp"));
            }
            nv(|v| v.phase.set(Phase::Hidden));
        }
    }
}
