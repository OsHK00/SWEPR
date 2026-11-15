//! Galería Pro dibujada con Direct2D, a pantalla completa sobre el escritorio atenuado.
//! Tres estilos comparten ventana, filtros y búsqueda:
//!
//! - **Showcase**: tarjetas inclinadas; la central se ensancha y muestra nombre y acciones.
//! - **Abanico**: tarjetas abiertas en arco que giran alrededor de un punto bajo la pantalla.
//! - **Mosaico**: cuadrícula con búsqueda por nombre.
//!
//! Arriba hay una barra con Todas / Favoritas / Nuevas y fichas de color (el color dominante
//! de cada miniatura). Escribir filtra por nombre en cualquier estilo.
//! Como el notch, todo lo de Direct2D solo vive mientras la galería está abierta.

use super::carousel;
use super::d2d::{
    accent, bg, black, color, draw_star, fg, light, on_accent, draw_text, ease_out_back, ease_out_cubic, fill_cover, fill_cover_geometry, hsv, inflate, inside, lerp,
    point, rect_f, rounded, slanted_geometry, smoothstep, soft_shadow, text_width, white, Rgb, Surface, COLOR_FAVORITE, COLOR_NEW,
};
use crate::app;
use crate::config::ProStyle;
use crate::log_info;
use crate::scanner::ScannedImage;
use crate::thumbs::ThumbnailCache;
use crate::winutil::{as_pcwstr, same_path, to_wide_chars};
use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::time::Instant;
use windows::Foundation::Numerics::Matrix3x2;
use windows::Win32::Foundation::{HINSTANCE, HWND, LPARAM, LRESULT, POINT, RECT, WPARAM};
use windows::Win32::Graphics::Direct2D::Common::D2D_RECT_F;
use windows::Win32::Graphics::Direct2D::{ID2D1Bitmap, ID2D1RenderTarget, ID2D1SolidColorBrush, D2D1_ANTIALIAS_MODE_PER_PRIMITIVE, D2D1_ELLIPSE};
use windows::Win32::Graphics::DirectWrite::{
    IDWriteTextFormat, DWRITE_FONT_WEIGHT_NORMAL, DWRITE_FONT_WEIGHT_SEMI_BOLD, DWRITE_TEXT_ALIGNMENT_CENTER, DWRITE_TEXT_ALIGNMENT_LEADING,
};
use windows::Win32::Graphics::Gdi::{GetMonitorInfoW, MonitorFromPoint, MONITORINFO, MONITOR_DEFAULTTONEAREST};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::Input::KeyboardAndMouse::{GetAsyncKeyState, VK_LBUTTON, VK_RBUTTON};
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, GetCursorPos, KillTimer, LoadCursorW, PostMessageW, RegisterClassW, SetCursor, SetTimer, SetWindowPos,
    ShowWindow, HWND_TOPMOST, IDC_ARROW, IDC_HAND, SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOSIZE, SW_HIDE, SW_SHOWNA, WM_ACTIVATE, WM_APP, WM_CHAR,
    WM_KEYDOWN, WM_LBUTTONUP, WM_MOUSEMOVE, WM_MOUSEWHEEL, WM_RBUTTONUP, WM_SETCURSOR, WM_TIMER, WNDCLASSW, WS_EX_LAYERED,
    WS_EX_TOOLWINDOW, WS_EX_TOPMOST, WS_POPUP,
};

const CLASS_NAME: &str = "SWEPR_GalleryWindowClass";
const FRAME_TIMER_ID: usize = 8201;
const CLOSE_CHECK_TIMER_ID: usize = 8202;
const TILE_RELEASE_TIMER_ID: usize = 8203;
const PREVIEW_TIMER_ID: usize = 8204;
const FRAME_MS: u32 = 16;
const CLOSE_CHECK_MS: u32 = 40;
const TILE_RELEASE_DELAY_MS: u32 = 30_000;
/// Espera tras el último movimiento antes de mostrar la vista previa en el escritorio.
pub(super) const PREVIEW_DELAY_MS: u32 = 320;
const WM_APP_GALLERY_RENDER: u32 = WM_APP + 80;

const OPEN_MS: f32 = 380.0;
const CLOSE_MS: f32 = 200.0;

/// Familias de color de las fichas del filtro, con el color con el que se dibujan.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Hash)]
pub(crate) enum Family {
    Red,
    Orange,
    Yellow,
    Green,
    Teal,
    Blue,
    Purple,
    Pink,
    Neutral,
}

impl Family {
    pub(super) const ALL: [Family; 9] = [
        Family::Red, Family::Orange, Family::Yellow, Family::Green, Family::Teal, Family::Blue, Family::Purple, Family::Pink, Family::Neutral,
    ];

    pub(super) fn swatch(self) -> Rgb {
        match self {
            Family::Red => (0.90, 0.27, 0.25),
            Family::Orange => (0.96, 0.56, 0.20),
            Family::Yellow => (0.96, 0.82, 0.28),
            Family::Green => (0.38, 0.76, 0.38),
            Family::Teal => (0.22, 0.75, 0.74),
            Family::Blue => (0.28, 0.52, 0.96),
            Family::Purple => (0.62, 0.40, 0.92),
            Family::Pink => (0.94, 0.42, 0.72),
            Family::Neutral => (0.56, 0.56, 0.60),
        }
    }

    pub(crate) fn of(rgb: Rgb) -> Family {
        let (h, s, v) = hsv(rgb.0, rgb.1, rgb.2);
        if s < 0.25 || v < 0.18 { return Family::Neutral; }
        match h {
            h if !(15.0..345.0).contains(&h) => Family::Red,
            h if h < 40.0 => Family::Orange,
            h if h < 66.0 => Family::Yellow,
            h if h < 160.0 => Family::Green,
            h if h < 195.0 => Family::Teal,
            h if h < 255.0 => Family::Blue,
            h if h < 290.0 => Family::Purple,
            _ => Family::Pink,
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Kind {
    All,
    Favorites,
    New,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Button {
    Apply,
    Favorite,
    Reveal,
    Exclude,
}

#[derive(Clone, Copy, PartialEq, Debug)]
enum HitId {
    /// Imagen: índice virtual en la vista filtrada (Showcase y Abanico) o índice en la vista (Mosaico).
    Card(i64),
    Button(Button, i64),
    Filter(Kind),
    Chip(Family),
    Search,
    ClearSearch,
    /// Flechas de la columna del marco: avanzar o retroceder.
    Step(i64),
    /// La imagen grande del marco: no hace nada, pero no cierra la galería.
    Backdrop,
}

/// Zona pulsable. `rect` está en coordenadas locales alrededor de `center` si hay giro.
#[derive(Clone, Copy, Debug)]
struct Hit {
    id: HitId,
    rect: D2D_RECT_F,
    center: (f32, f32),
    angle: f32,
}

impl Hit {
    fn flat(id: HitId, rect: D2D_RECT_F) -> Self { Self { id, rect, center: (0.0, 0.0), angle: 0.0 } }

    fn contains(&self, x: f32, y: f32) -> bool {
        let (dx, dy) = (x - self.center.0, y - self.center.1);
        let (sin, cos) = self.angle.sin_cos();
        // Girar el punto al revés para comprobarlo en los ejes de la tarjeta.
        let lx = dx * cos + dy * sin;
        let ly = -dx * sin + dy * cos;
        inside(&self.rect, lx, ly)
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Phase {
    Hidden,
    Opening,
    Open,
    Closing,
}

struct Renderer {
    surface: Surface,
    title: Option<IDWriteTextFormat>,
    meta: Option<IDWriteTextFormat>,
    label: Option<IDWriteTextFormat>,
    small: Option<IDWriteTextFormat>,
    search: Option<IDWriteTextFormat>,
    name: Option<IDWriteTextFormat>,
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
        Ok(Self {
            title: surface.format(22.0 * scale, DWRITE_FONT_WEIGHT_SEMI_BOLD, DWRITE_TEXT_ALIGNMENT_CENTER),
            meta: surface.format(13.0 * scale, DWRITE_FONT_WEIGHT_NORMAL, DWRITE_TEXT_ALIGNMENT_CENTER),
            label: surface.format(13.0 * scale, DWRITE_FONT_WEIGHT_SEMI_BOLD, DWRITE_TEXT_ALIGNMENT_CENTER),
            small: surface.format(12.0 * scale, DWRITE_FONT_WEIGHT_NORMAL, DWRITE_TEXT_ALIGNMENT_LEADING),
            search: surface.format(14.0 * scale, DWRITE_FONT_WEIGHT_NORMAL, DWRITE_TEXT_ALIGNMENT_LEADING),
            name: surface.format(16.0 * scale, DWRITE_FONT_WEIGHT_SEMI_BOLD, DWRITE_TEXT_ALIGNMENT_LEADING),
            surface,
        })
    }
}

struct GalleryVars {
    hwnd: Cell<Option<HWND>>,
    phase: Cell<Phase>,
    open: Cell<f32>,
    phase_start: Cell<Instant>,
    phase_from: Cell<f32>,
    close_on_leave: Cell<bool>,
    style: Cell<ProStyle>,
    monitor: Cell<RECT>,
    scale: Cell<f32>,
    kind: Cell<Kind>,
    family: Cell<Option<Family>>,
    query: RefCell<String>,
    /// Índices (en la galería completa) de las imágenes que pasan el filtro.
    view: RefCell<Vec<usize>>,
    /// Imagen enfocada, para no perderla cuando la vista cambia.
    focus_key: RefCell<Option<String>>,
    pos: Cell<f32>,
    target: Cell<i64>,
    /// Desplazamiento vertical del mosaico (actual y objetivo).
    scroll: Cell<f32>,
    scroll_target: Cell<f32>,
    last_step: Cell<Option<Instant>>,
    hover: Cell<Option<HitId>>,
    hits: RefCell<Vec<Hit>>,
    /// Columnas del mosaico en el último fotograma (para moverse con arriba y abajo).
    columns: Cell<usize>,
    render_pending: Cell<bool>,
    families: RefCell<HashMap<String, Option<Family>>>,
    renderer: RefCell<Option<Renderer>>,
}

impl Default for GalleryVars {
    fn default() -> Self {
        Self {
            hwnd: Cell::new(None),
            phase: Cell::new(Phase::Hidden),
            open: Cell::new(0.0),
            phase_start: Cell::new(Instant::now()),
            phase_from: Cell::new(0.0),
            close_on_leave: Cell::new(false),
            style: Cell::new(ProStyle::Showcase),
            monitor: Cell::new(RECT::default()),
            scale: Cell::new(1.0),
            kind: Cell::new(Kind::All),
            family: Cell::new(None),
            query: RefCell::new(String::new()),
            view: RefCell::new(Vec::new()),
            focus_key: RefCell::new(None),
            pos: Cell::new(0.0),
            target: Cell::new(0),
            scroll: Cell::new(0.0),
            scroll_target: Cell::new(0.0),
            last_step: Cell::new(None),
            hover: Cell::new(None),
            hits: RefCell::new(Vec::new()),
            columns: Cell::new(1),
            render_pending: Cell::new(false),
            families: RefCell::new(HashMap::new()),
            renderer: RefCell::new(None),
        }
    }
}

thread_local! {
    static GALLERY: GalleryVars = GalleryVars::default();
}

fn gv<R>(f: impl FnOnce(&GalleryVars) -> R) -> R { GALLERY.with(f) }

pub fn is_visible() -> bool { gv(|v| v.phase.get()) != Phase::Hidden }
pub fn is_closing() -> bool { gv(|v| v.phase.get()) == Phase::Closing }
pub fn closes_on_leave() -> bool { gv(|v| v.close_on_leave.get()) }

fn is_loop() -> bool { gv(|v| v.style.get()) != ProStyle::Mosaic }
fn view_len() -> usize { gv(|v| v.view.borrow().len()) }
fn real_index(virtual_index: i64, count: usize) -> usize { virtual_index.rem_euclid(count.max(1) as i64) as usize }

/// Índice en la galería de la imagen de la vista en esta posición.
fn image_of(virtual_index: i64) -> Option<usize> {
    gv(|v| {
        let view = v.view.borrow();
        if view.is_empty() { return None; }
        let i = if is_loop() { real_index(virtual_index, view.len()) } else { virtual_index.clamp(0, view.len() as i64 - 1) as usize };
        view.get(i).copied()
    })
}

fn focused_image() -> Option<usize> { image_of(gv(|v| v.target.get())) }

unsafe fn ensure_window() -> Option<HWND> {
    if let Some(hwnd) = gv(|v| v.hwnd.get()) { return Some(hwnd); }
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
        as_pcwstr(&to_wide_chars("SWEPR Gallery")),
        WS_POPUP,
        0, 0, 1, 1,
        None, None, hinstance, None,
    ).ok()?;
    gv(|v| v.hwnd.set(Some(hwnd)));
    Some(hwnd)
}

/// Medidas de cada estilo para un monitor (píxeles físicos).
#[derive(Clone, Copy, Debug)]
struct Geo {
    w: f32,
    h: f32,
    s: f32,
}

impl Geo {
    fn current() -> Self {
        let (monitor, s) = gv(|v| (v.monitor.get(), v.scale.get()));
        Self { w: (monitor.right - monitor.left) as f32, h: (monitor.bottom - monitor.top) as f32, s }
    }
    fn px(&self, value: f32) -> f32 { value * self.s }
    fn cx(&self) -> f32 { self.w / 2.0 }
    fn bar_top(&self) -> f32 { self.px(26.0) }
    fn bar_h(&self) -> f32 { self.px(36.0) }

    // Showcase
    fn show_card_h(&self) -> f32 {
        let h = (self.h * 0.44).min(self.px(540.0));
        let w = (h * 16.0 / 9.0).min(self.w * 0.52);
        w * 9.0 / 16.0
    }
    fn show_focus_w(&self) -> f32 { self.show_card_h() * 16.0 / 9.0 }
    fn show_side_w(&self) -> f32 { self.show_card_h() * 0.34 }
    fn show_slant(&self) -> f32 { self.show_card_h() * 0.16 }
    fn show_gap(&self) -> f32 { self.px(14.0) }
    fn show_cy(&self) -> f32 { self.h * 0.45 }
    fn show_offset(&self, d: f32) -> f32 {
        let near = d.abs().min(1.0);
        let far = (d.abs() - 1.0).max(0.0);
        let first = self.show_focus_w() / 2.0 + self.show_gap() + self.show_side_w() / 2.0;
        d.signum() * (near * first + far * (self.show_side_w() + self.show_gap()))
    }
    fn show_rect(&self, d: f32) -> D2D_RECT_F {
        let c = 1.0 - d.abs().min(1.0);
        // Lineal, igual que el desplazamiento: así las tarjetas nunca se pisan.
        let w = lerp(self.show_side_w(), self.show_focus_w(), c);
        let h = self.show_card_h() * lerp(0.84, 1.0, c);
        let x = self.cx() + self.show_offset(d);
        rect_f(x - w / 2.0, self.show_cy() - h / 2.0, x + w / 2.0, self.show_cy() + h / 2.0)
    }
    fn show_reach(&self) -> i64 { (self.w / 2.0 / (self.show_side_w() + self.show_gap())).ceil() as i64 + 2 }

    // Abanico
    fn fan_card_h(&self) -> f32 { (self.h * 0.42).min(self.px(470.0)) }
    fn fan_card_w(&self) -> f32 { self.fan_card_h() * 0.68 }
    fn fan_radius(&self) -> f32 { self.h * 0.95 }
    fn fan_pivot_y(&self) -> f32 { self.h * 0.6 + self.fan_radius() }
    fn fan_step(&self) -> f32 { 9.0f32.to_radians() }

    // Mosaico
    fn grid_top(&self) -> f32 { self.bar_top() + self.bar_h() + self.px(26.0) }
    fn grid_bottom(&self) -> f32 { self.h - self.px(24.0) }
    fn grid_margin(&self) -> f32 { (self.w * 0.06).max(self.px(32.0)) }
    fn grid_gap(&self) -> f32 { self.px(14.0) }
    fn grid_columns(&self) -> usize {
        let avail = self.w - self.grid_margin() * 2.0;
        (((avail + self.grid_gap()) / (self.px(300.0) + self.grid_gap())).floor() as usize).max(2)
    }
    fn grid_cell(&self) -> (f32, f32) {
        let cols = self.grid_columns() as f32;
        let avail = self.w - self.grid_margin() * 2.0;
        let w = (avail - self.grid_gap() * (cols - 1.0)) / cols;
        (w, w * 9.0 / 16.0)
    }
    fn grid_rect(&self, index: usize, scroll: f32) -> D2D_RECT_F {
        let cols = self.grid_columns();
        let (cw, ch) = self.grid_cell();
        let (row, col) = ((index / cols) as f32, (index % cols) as f32);
        let x = self.grid_margin() + col * (cw + self.grid_gap());
        let y = self.grid_top() + row * (ch + self.grid_gap()) - scroll;
        rect_f(x, y, x + cw, y + ch)
    }
    fn grid_max_scroll(&self, count: usize) -> f32 {
        let rows = count.div_ceil(self.grid_columns()) as f32;
        let (_, ch) = self.grid_cell();
        let content = rows * (ch + self.grid_gap()) - self.grid_gap();
        (content - (self.grid_bottom() - self.grid_top())).max(0.0)
    }

    // Marco
    /// Zona de la imagen grande (antes de recortar las esquinas).
    fn frame_rect(&self) -> D2D_RECT_F { rect_f(self.px(14.0), self.px(14.0), self.w - self.px(132.0), self.h - self.px(14.0)) }
    fn frame_radius(&self) -> f32 { self.px(28.0) }
    /// Hueco de arriba a la izquierda (cuadro de búsqueda y acciones).
    fn frame_top_notch(&self) -> (f32, f32) { (self.px(360.0), self.px(262.0)) }
    /// Hueco de abajo a la derecha (miniaturas en cascada).
    fn frame_bottom_notch(&self) -> (f32, f32) { (self.px(450.0), self.px(300.0)) }
    fn frame_card(&self) -> (f32, f32) { (self.px(300.0), self.px(169.0)) }

    /// Tamaño al que se generan las tarjetas grandes para este estilo.
    fn tile_size(&self, style: ProStyle) -> (i32, i32) {
        match style {
            ProStyle::Frame => {
                let r = self.frame_rect();
                ((r.right - r.left) as i32, (r.bottom - r.top) as i32)
            }
            ProStyle::Fan => ((self.fan_card_w() * 1.14) as i32, (self.fan_card_h() * 1.14) as i32),
            ProStyle::Mosaic => {
                let (w, h) = self.grid_cell();
                ((w * 1.05) as i32, (h * 1.05) as i32)
            }
            _ => (self.show_focus_w() as i32, self.show_card_h() as i32),
        }
    }
}

/// Abre la galería en el monitor del cursor con el estilo pedido.
pub fn show(style: ProStyle, close_on_leave: bool) {
    unsafe {
        if matches!(gv(|v| v.phase.get()), Phase::Opening | Phase::Open) { return; }
        let Some(hwnd) = ensure_window() else { return; };
        let mut pt = POINT::default();
        let _ = GetCursorPos(&mut pt);
        let hmon = MonitorFromPoint(pt, MONITOR_DEFAULTTONEAREST);
        let mut mi = MONITORINFO { cbSize: std::mem::size_of::<MONITORINFO>() as u32, ..Default::default() };
        let _ = GetMonitorInfoW(hmon, &mut mi);
        let (mut dpi_x, mut dpi_y) = (96u32, 96u32);
        let _ = windows::Win32::UI::HiDpi::GetDpiForMonitor(hmon, windows::Win32::UI::HiDpi::MDT_EFFECTIVE_DPI, &mut dpi_x, &mut dpi_y);
        let monitor = mi.rcMonitor;
        let scale = dpi_x.max(96) as f32 / 96.0;
        let (width, height) = (monitor.right - monitor.left, monitor.bottom - monitor.top);

        let reuse = gv(|v| v.renderer.borrow().as_ref().is_some_and(|r| r.width == width && r.height == height));
        if !reuse {
            match Renderer::new(width, height, scale) {
                Ok(renderer) => gv(|v| *v.renderer.borrow_mut() = Some(renderer)),
                Err(e) => {
                    crate::log_warn!("Galería: Direct2D no disponible ({e}); no se puede abrir");
                    return;
                }
            }
        }
        let reopening = gv(|v| v.phase.get()) == Phase::Closing && gv(|v| v.style.get()) == style;
        gv(|v| {
            v.monitor.set(monitor);
            v.scale.set(scale);
            v.style.set(style);
            v.close_on_leave.set(close_on_leave);
        });
        let geo = Geo::current();
        let (tile_w, tile_h) = geo.tile_size(style);
        carousel::set_layout(carousel::Layout { tile_w, tile_h, slant: 0, gap: 0, pad_y: 0, margin_x: 0 });

        let from = if reopening { gv(|v| v.open.get()) } else { 0.0 };
        if !reopening {
            // Se empieza siempre por el fondo actual, sin filtros.
            gv(|v| {
                v.kind.set(Kind::All);
                v.family.set(None);
                v.query.borrow_mut().clear();
                v.hover.set(None);
            });
            rebuild_view();
            let current = super::vars(|v| v.current_wallpaper.borrow().clone());
            let selected = super::scanned()
                .and_then(|arc| arc.lock().ok().and_then(|images| current.as_ref().and_then(|cur| images.iter().position(|img| same_path(&img.path, cur)))))
                .unwrap_or(0);
            let index = gv(|v| v.view.borrow().iter().position(|i| *i == selected).unwrap_or(0)) as i64;
            gv(|v| {
                v.pos.set(index as f32);
                v.target.set(index);
            });
            remember_focus();
            let scroll = mosaic_scroll_for(index as usize, 0.0, &geo);
            gv(|v| {
                v.scroll.set(scroll);
                v.scroll_target.set(scroll);
            });
        }
        gv(|v| {
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
        let _ = SetWindowPos(hwnd, HWND_TOPMOST, 0, 0, 0, 0, SWP_NOACTIVATE | SWP_NOMOVE | SWP_NOSIZE);
        super::force_focus(hwnd);
        let _ = SetTimer(hwnd, FRAME_TIMER_ID, FRAME_MS, None);
        let _ = SetTimer(hwnd, CLOSE_CHECK_TIMER_ID, CLOSE_CHECK_MS, None);
        log_info!("Galería Pro: abriendo ({:?})", style);
    }
}

/// Cierra la galería con su animación y quita la vista previa del escritorio.
pub fn hide() {
    if !matches!(gv(|v| v.phase.get()), Phase::Opening | Phase::Open) { return; }
    let Some(hwnd) = gv(|v| v.hwnd.get()) else { return; };
    gv(|v| {
        v.phase_from.set(v.open.get());
        v.phase.set(Phase::Closing);
        v.phase_start.set(Instant::now());
        v.hover.set(None);
    });
    super::preview_end();
    unsafe {
        let _ = KillTimer(hwnd, CLOSE_CHECK_TIMER_ID);
        let _ = KillTimer(hwnd, PREVIEW_TIMER_ID);
        let _ = SetTimer(hwnd, FRAME_TIMER_ID, FRAME_MS, None);
    }
}

pub fn hide_now() {
    let Some(hwnd) = gv(|v| v.hwnd.get()) else { return; };
    if gv(|v| v.phase.get()) == Phase::Hidden { return; }
    super::preview_end();
    finish_hide(hwnd);
}

fn finish_hide(hwnd: HWND) {
    unsafe {
        let _ = KillTimer(hwnd, FRAME_TIMER_ID);
        let _ = KillTimer(hwnd, CLOSE_CHECK_TIMER_ID);
        let _ = KillTimer(hwnd, PREVIEW_TIMER_ID);
        let _ = ShowWindow(hwnd, SW_HIDE);
        let _ = SetTimer(hwnd, TILE_RELEASE_TIMER_ID, TILE_RELEASE_DELAY_MS, None);
    }
    gv(|v| {
        v.phase.set(Phase::Hidden);
        v.open.set(0.0);
        v.hits.borrow_mut().clear();
        *v.renderer.borrow_mut() = None;
    });
    crate::log::log_resource_usage();
    super::flush_notify();
}

pub fn request_render() {
    if !is_visible() || gv(|v| v.render_pending.replace(true)) { return; }
    if let Some(hwnd) = gv(|v| v.hwnd.get()) {
        unsafe { let _ = PostMessageW(hwnd, WM_APP_GALLERY_RENDER, WPARAM(0), LPARAM(0)); }
    }
}

// --- Filtro ---

/// Familia de color de una imagen, calculada una vez a partir de su miniatura pequeña.
pub(super) fn family_of(key: &str, small: Option<&ThumbnailCache>) -> Option<Family> {
    if let Some(known) = gv(|v| v.families.borrow().get(key).copied()) { return known; }
    let thumb = small.and_then(|cache| cache.get(key))?;
    let family = unsafe { super::d2d::dominant_color(thumb.hbitmap) }.map(Family::of);
    gv(|v| v.families.borrow_mut().insert(key.to_string(), family));
    family
}

/// Colores que hay en la galería, en el orden de las fichas (una pasada; el de cada imagen se guarda).
pub(super) fn families_present(images: &[ScannedImage], small: Option<&ThumbnailCache>) -> Vec<Family> {
    let mut present = [false; Family::ALL.len()];
    for img in images {
        if let Some(f) = family_of(&img.path.to_string_lossy(), small) {
            if let Some(i) = Family::ALL.iter().position(|x| *x == f) { present[i] = true; }
        }
    }
    Family::ALL.into_iter().zip(present).filter(|(_, p)| *p).map(|(f, _)| f).collect()
}

fn matches(img: &ScannedImage, kind: Kind, family: Option<Family>, query: &str, small: Option<&ThumbnailCache>) -> bool {
    let kind_ok = match kind {
        Kind::All => true,
        Kind::Favorites => img.is_favorite,
        Kind::New => img.is_new,
    };
    if !kind_ok { return false; }
    if !query.is_empty() {
        let name = img.path.file_stem().map(|s| s.to_string_lossy().to_lowercase()).unwrap_or_default();
        if !query.split_whitespace().all(|word| name.contains(word)) { return false; }
    }
    match family {
        Some(family) => family_of(&img.path.to_string_lossy(), small) == Some(family),
        None => true,
    }
}

/// Recalcula qué imágenes se ven y mantiene el foco en la misma imagen si sigue en la vista.
fn rebuild_view() {
    let Some(images_arc) = super::scanned() else { gv(|v| v.view.borrow_mut().clear()); return; };
    let Ok(images) = images_arc.lock() else { return; };
    let small = super::vars(|v| v.thumb_cache.borrow().clone());
    let (kind, family) = gv(|v| (v.kind.get(), v.family.get()));
    let query = gv(|v| v.query.borrow().to_lowercase());
    let view: Vec<usize> = images.iter().enumerate()
        .filter(|(_, img)| matches(img, kind, family, &query, small.as_ref()))
        .map(|(i, _)| i)
        .collect();
    let focus = gv(|v| v.focus_key.borrow().clone());
    let found = focus.and_then(|key| view.iter().position(|i| images[*i].path.to_string_lossy() == key));
    gv(|v| {
        let len = view.len();
        let old_target = v.target.get();
        let current = if len == 0 { 0 } else if is_loop() { real_index(old_target, len) as i64 } else { old_target.clamp(0, len as i64 - 1) };
        match found {
            Some(p) => {
                let shift = p as i64 - current;
                v.target.set(old_target + shift);
                v.pos.set(v.pos.get() + shift as f32);
            }
            None if !is_loop() => {
                v.target.set(current);
                v.pos.set(current as f32);
            }
            None => {}
        }
        *v.view.borrow_mut() = view;
    });
}

fn remember_focus() {
    let key = focused_image().and_then(super::image_path).map(|p| p.to_string_lossy().to_string());
    gv(|v| *v.focus_key.borrow_mut() = key);
}

fn set_filter(kind: Option<Kind>, family: Option<Option<Family>>) {
    gv(|v| {
        if let Some(kind) = kind { v.kind.set(kind); }
        if let Some(family) = family { v.family.set(family); }
    });
    rebuild_view();
    if !is_loop() {
        let geo = Geo::current();
        let target = gv(|v| v.target.get()) as usize;
        let scroll = mosaic_scroll_for(target, gv(|v| v.scroll_target.get()), &geo);
        gv(|v| v.scroll_target.set(scroll.min(geo.grid_max_scroll(view_len()))));
    }
    focus_changed();
    request_render();
}

// --- Movimiento ---

fn start_frames(hwnd: HWND) { unsafe { let _ = SetTimer(hwnd, FRAME_TIMER_ID, FRAME_MS, None); } }

/// La imagen enfocada cambió: recordar cuál es y programar la vista previa.
fn focus_changed() {
    remember_focus();
    if let Some(hwnd) = gv(|v| v.hwnd.get()) {
        unsafe { let _ = SetTimer(hwnd, PREVIEW_TIMER_ID, PREVIEW_DELAY_MS, None); }
    }
}

fn move_to(hwnd: HWND, target: i64) {
    let len = view_len();
    if len == 0 { return; }
    let target = if is_loop() { target } else { target.clamp(0, len as i64 - 1) };
    if gv(|v| v.target.replace(target)) == target { return; }
    gv(|v| v.last_step.set(Some(Instant::now())));
    if !is_loop() {
        let geo = Geo::current();
        let scroll = mosaic_scroll_for(target as usize, gv(|v| v.scroll_target.get()), &geo);
        gv(|v| v.scroll_target.set(scroll));
    }
    focus_changed();
    start_frames(hwnd);
}

fn move_by(hwnd: HWND, delta: i64) { move_to(hwnd, gv(|v| v.target.get()) + delta); }

/// Desplazamiento del mosaico que deja visible la celda `index`.
fn mosaic_scroll_for(index: usize, scroll: f32, geo: &Geo) -> f32 {
    let rect = geo.grid_rect(index, 0.0);
    let (top, bottom) = (geo.grid_top(), geo.grid_bottom());
    let view_h = bottom - top;
    let mut scroll = scroll;
    if rect.top - scroll < top { scroll = rect.top - top; }
    if rect.bottom - scroll > bottom { scroll = rect.bottom - top - view_h; }
    scroll.clamp(0.0, geo.grid_max_scroll(view_len()))
}

/// Avanza las animaciones. Devuelve si alguna sigue en curso.
fn step(hwnd: HWND) -> bool {
    let now = Instant::now();
    let mut busy = false;
    let elapsed_ms = gv(|v| now.duration_since(v.phase_start.get()).as_secs_f32() * 1000.0);
    match gv(|v| v.phase.get()) {
        Phase::Opening => {
            let open = (gv(|v| v.phase_from.get()) + elapsed_ms / OPEN_MS).min(1.0);
            gv(|v| v.open.set(open));
            if open >= 1.0 { gv(|v| v.phase.set(Phase::Open)); } else { busy = true; }
        }
        Phase::Closing => {
            let open = (gv(|v| v.phase_from.get()) - elapsed_ms / CLOSE_MS).max(0.0);
            gv(|v| v.open.set(open));
            if open <= 0.0 {
                finish_hide(hwnd);
                return false;
            }
            busy = true;
        }
        _ => {}
    }
    let elapsed = gv(|v| v.last_step.replace(Some(now))).map(|last| now.duration_since(last).as_secs_f32()).unwrap_or(1.0 / 60.0).min(0.1);
    let factor = 1.0 - 0.78f32.powf(elapsed * 60.0);
    let (pos, target) = gv(|v| (v.pos.get(), v.target.get() as f32));
    if (target - pos).abs() > 0.002 {
        gv(|v| v.pos.set(pos + (target - pos) * factor));
        busy = true;
    } else {
        gv(|v| v.pos.set(target));
    }
    let (scroll, scroll_target) = gv(|v| (v.scroll.get(), v.scroll_target.get()));
    if (scroll_target - scroll).abs() > 0.5 {
        gv(|v| v.scroll.set(scroll + (scroll_target - scroll) * factor));
        busy = true;
    } else {
        gv(|v| v.scroll.set(scroll_target));
    }
    busy
}

fn monitor_rect() -> RECT { gv(|v| v.monitor.get()) }

fn apply(index: usize) {
    let Some(path) = super::image_path(index) else { return; };
    log_info!("Galería: aplicar {:?}", path);
    app::request(app::Action::ApplyFromPanel(path.clone(), monitor_rect()));
    super::set_current_wallpaper(&path);
    super::hide_panel();
}

fn run_button(button: Button, index: usize) {
    let Some(path) = super::image_path(index) else { return; };
    match button {
        Button::Apply => apply(index),
        Button::Favorite => app::request(app::Action::ToggleFavorite(path)),
        Button::Reveal => {
            app::request(app::Action::Reveal(path));
            super::hide_panel();
        }
        Button::Exclude => app::request(app::Action::Exclude(path)),
    }
}

// --- Pintado ---

unsafe fn render(hwnd: HWND) {
    gv(|v| v.render_pending.set(false));
    let monitor = monitor_rect();
    let origin = POINT { x: monitor.left, y: monitor.top };
    gv(|v| {
        let mut guard = v.renderer.borrow_mut();
        let Some(renderer) = guard.as_mut() else { return; };
        draw_frame(renderer, hwnd, None);
        // El marco es opaco: entra y sale con un fundido de toda la ventana.
        let alpha = if gv(|v| v.style.get()) == ProStyle::Frame {
            let open = gv(|v| v.open.get());
            (ease_out_cubic(open) * 255.0).round() as u8
        } else {
            255
        };
        if !renderer.present(hwnd, origin, alpha) { crate::log_debug!("Galería: no se pudo presentar el fotograma"); }
    });
}

/// Dibuja un fotograma completo. `backdrop` solo lo usan las pruebas.
unsafe fn draw_frame(renderer: &mut Renderer, notify: HWND, backdrop: Option<&ID2D1Bitmap>) {
    renderer.tick += 1;
    rebuild_view();
    let target = renderer.rt();
    let geo = Geo::current();
    let style = gv(|v| v.style.get());
    let open = gv(|v| v.open.get());
    let closing = gv(|v| v.phase.get()) == Phase::Closing;
    let e = if closing { smoothstep(open) } else { ease_out_cubic(open) };
    let mut hits = Vec::new();

    target.BeginDraw();
    target.Clear(Some(&black(0.0)));
    if let Some(backdrop) = backdrop {
        target.DrawBitmap(backdrop, Some(&rect_f(0.0, 0.0, geo.w, geo.h)), 1.0, windows::Win32::Graphics::Direct2D::D2D1_BITMAP_INTERPOLATION_MODE_LINEAR, None);
    }
    if let Ok(brush) = target.CreateSolidColorBrush(&black(0.0), None) {
        let scrim = if style == ProStyle::Mosaic { 0.82 } else { 0.64 };
        brush.SetColor(&bg(scrim * e));
        target.FillRectangle(&rect_f(0.0, 0.0, geo.w, geo.h), &brush);

        if let Some(images_arc) = super::scanned() {
            if let Ok(images) = images_arc.lock() {
                let small = super::vars(|v| v.thumb_cache.borrow().clone());
                let current = super::vars(|v| v.current_wallpaper.borrow().clone());
                let ctx = Ctx { images: &images, small: small.as_ref(), current: current.as_deref(), geo, e, notify, closing };
                if style == ProStyle::Frame {
                    draw_framed(renderer, &target, &brush, &ctx, &mut hits);
                } else {
                let lift = (1.0 - e) * geo.px(36.0);
                target.SetTransform(&Matrix3x2::translation(0.0, -lift * 0.5));
                draw_bar(renderer, &target, &brush, &ctx, &mut hits);
                target.SetTransform(&Matrix3x2::translation(0.0, lift));
                match style {
                    ProStyle::Fan => draw_fan(renderer, &target, &brush, &ctx, &mut hits),
                    ProStyle::Mosaic => draw_mosaic(renderer, &target, &brush, &ctx, &mut hits),
                    _ => draw_showcase(renderer, &target, &brush, &ctx, &mut hits),
                }
                target.SetTransform(&Matrix3x2::identity());
                }
                // Los desplazamientos de la animación de entrada no afectan al ratón cuando ya está abierta.
            }
        }
    }
    let _ = target.EndDraw(None, None);
    // La última pintada queda encima: primero para el ratón.
    hits.reverse();
    gv(|v| *v.hits.borrow_mut() = hits);
    renderer.trim(96);
}

struct Ctx<'a> {
    images: &'a [ScannedImage],
    small: Option<&'a ThumbnailCache>,
    current: Option<&'a std::path::Path>,
    geo: Geo,
    e: f32,
    notify: HWND,
    closing: bool,
}

impl Ctx<'_> {
    fn is_current(&self, img: &ScannedImage) -> bool { self.current.is_some_and(|cur| same_path(cur, &img.path)) }
}

/// Mapa de bits de una imagen: la tarjeta grande si ya está, si no la miniatura pequeña.
unsafe fn image_bitmap(renderer: &mut Renderer, img: &ScannedImage, small: Option<&ThumbnailCache>) -> Option<ID2D1Bitmap> {
    let key = img.path.to_string_lossy().to_string();
    carousel::tile(&key).and_then(|tile| renderer.bitmap(&key, true, tile.hbitmap))
        .or_else(|| small.and_then(|c| c.get(&key)).and_then(|thumb| renderer.bitmap(&key, false, thumb.hbitmap)))
}

/// Pide las tarjetas grandes de estas imágenes (las primeras, antes) y suelta las demás.
fn request_tiles(ctx: &Ctx, wanted: Vec<usize>, extra: usize) {
    let keys: Vec<String> = wanted.iter().filter_map(|i| ctx.images.get(*i)).map(|img| img.path.to_string_lossy().to_string()).collect();
    carousel::keep_tiles(keys.iter().map(|k| k.as_str()), wanted.len() + extra);
    carousel::request_tiles(ctx.images, wanted, ctx.notify);
}

fn image_name(img: &ScannedImage) -> String {
    img.path.file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or_default()
}

fn hovered(id: HitId) -> bool { gv(|v| v.hover.get()) == Some(id) }

/// Barra superior: búsqueda, Todas / Favoritas / Nuevas y fichas de color.
unsafe fn draw_bar(renderer: &mut Renderer, target: &ID2D1RenderTarget, brush: &ID2D1SolidColorBrush, ctx: &Ctx, hits: &mut Vec<Hit>) {
    let geo = ctx.geo;
    let (Some(label), Some(search_format), Some(dwrite)) = (renderer.label.clone(), renderer.search.clone(), renderer.dwrite.clone()) else { return; };
    let style = gv(|v| v.style.get());
    let query = gv(|v| v.query.borrow().clone());
    let (kind, family) = gv(|v| (v.kind.get(), v.family.get()));
    let show_search = style == ProStyle::Mosaic || !query.is_empty();
    let families = families_present(ctx.images, ctx.small);
    let kinds = [(Kind::All, crate::i18n::t("All")), (Kind::Favorites, crate::i18n::t("Favorites")), (Kind::New, crate::i18n::t("New"))];

    let h = geo.bar_h();
    let pad = geo.px(16.0);
    let gap = geo.px(8.0);
    let chip = geo.px(22.0);
    let search_w = geo.px(300.0);
    let pill_w: Vec<f32> = kinds.iter().map(|(_, text)| text_width(&dwrite, &label, text) + pad * 2.0).collect();
    let mut total = pill_w.iter().sum::<f32>() + gap * (kinds.len() as f32 - 1.0);
    if show_search { total += search_w + geo.px(18.0); }
    if !families.is_empty() { total += geo.px(18.0) + families.len() as f32 * (chip + gap) - gap; }
    let mut x = geo.cx() - total / 2.0;
    let top = geo.bar_top();
    let a = ctx.e;

    if show_search {
        let rect = rect_f(x, top, x + search_w, top + h);
        let shape = rounded(rect, h / 2.0);
        brush.SetColor(&fg(0.08 * a));
        target.FillRoundedRectangle(&shape, brush);
        brush.SetColor(&if query.is_empty() { fg(0.16 * a) } else { color(accent(), 0.9 * a) });
        target.DrawRoundedRectangle(&shape, brush, geo.px(1.0).max(1.0), None);
        // Lupa
        let (lx, ly, r) = (rect.left + geo.px(18.0), rect.top + h / 2.0 - geo.px(1.0), geo.px(5.5));
        brush.SetColor(&fg(0.7 * a));
        target.DrawEllipse(&D2D1_ELLIPSE { point: point(lx, ly), radiusX: r, radiusY: r }, brush, geo.px(1.6), None);
        target.DrawLine(point(lx + r * 0.7, ly + r * 0.7), point(lx + r * 1.6, ly + r * 1.6), brush, geo.px(1.6), None);
        let text_rect = rect_f(rect.left + geo.px(34.0), rect.top, rect.right - geo.px(34.0), rect.bottom);
        if query.is_empty() {
            brush.SetColor(&fg(0.42 * a));
            draw_text(target, &crate::i18n::t("Type to search"), &search_format, text_rect, brush);
        } else {
            brush.SetColor(&fg(0.95 * a));
            draw_text(target, &format!("{query}\u{258F}"), &search_format, text_rect, brush);
            let clear = rect_f(rect.right - geo.px(32.0), rect.top, rect.right - geo.px(6.0), rect.bottom);
            let (cx, cy, arm) = ((clear.left + clear.right) / 2.0, (clear.top + clear.bottom) / 2.0, geo.px(4.5));
            brush.SetColor(&fg(if hovered(HitId::ClearSearch) { 0.95 } else { 0.6 } * a));
            target.DrawLine(point(cx - arm, cy - arm), point(cx + arm, cy + arm), brush, geo.px(1.6), None);
            target.DrawLine(point(cx + arm, cy - arm), point(cx - arm, cy + arm), brush, geo.px(1.6), None);
            hits.push(Hit::flat(HitId::Search, rect));
            hits.push(Hit::flat(HitId::ClearSearch, clear));
        }
        if query.is_empty() { hits.push(Hit::flat(HitId::Search, rect)); }
        x += search_w + geo.px(18.0);
    }

    for ((k, text), w) in kinds.iter().zip(pill_w) {
        let rect = rect_f(x, top, x + w, top + h);
        let shape = rounded(rect, h / 2.0);
        let selected = kind == *k;
        let hot = hovered(HitId::Filter(*k));
        brush.SetColor(&if selected { fg(0.92 * a) } else { fg(if hot { 0.16 } else { 0.08 } * a) });
        target.FillRoundedRectangle(&shape, brush);
        brush.SetColor(&if selected { bg(a) } else { fg(0.85 * a) });
        draw_text(target, text, &label, rect, brush);
        hits.push(Hit::flat(HitId::Filter(*k), rect));
        x += w + gap;
    }

    if !families.is_empty() {
        x += geo.px(18.0) - gap;
        // Separador
        brush.SetColor(&fg(0.18 * a));
        target.FillRectangle(&rect_f(x - geo.px(10.0), top + h * 0.25, x - geo.px(9.0), top + h * 0.75), brush);
        for f in families {
            let center = point(x + chip / 2.0, top + h / 2.0);
            let selected = family == Some(f);
            let hot = hovered(HitId::Chip(f));
            brush.SetColor(&color(f.swatch(), a));
            let r = chip / 2.0 * if hot && !selected { 1.1 } else { 1.0 };
            target.FillEllipse(&D2D1_ELLIPSE { point: center, radiusX: r, radiusY: r }, brush);
            if selected || hot {
                brush.SetColor(&fg(if selected { 0.95 } else { 0.5 } * a));
                let ring = chip / 2.0 + geo.px(4.0);
                target.DrawEllipse(&D2D1_ELLIPSE { point: center, radiusX: ring, radiusY: ring }, brush, geo.px(2.0), None);
            }
            hits.push(Hit::flat(HitId::Chip(f), rect_f(x - gap / 2.0, top, x + chip + gap / 2.0, top + h)));
            x += chip + gap;
        }
    }
}

/// Insignias de favorita y nueva en la esquina de una tarjeta.
unsafe fn draw_badges(renderer: &Renderer, target: &ID2D1RenderTarget, brush: &ID2D1SolidColorBrush, img: &ScannedImage, rect: D2D_RECT_F, scale: f32, alpha: f32) {
    let inset = 10.0 * scale;
    if img.is_favorite {
        brush.SetColor(&color(COLOR_FAVORITE, alpha));
        let r = 8.0 * scale;
        draw_star(&renderer.factory, target, brush, point(rect.left + inset + r, rect.top + inset + r), r);
    }
    if img.is_new {
        brush.SetColor(&color(COLOR_NEW, alpha));
        let r = 5.0 * scale;
        target.FillEllipse(&D2D1_ELLIPSE { point: point(rect.right - inset - r, rect.top + inset + r), radiusX: r, radiusY: r }, brush);
    }
}

/// Nombre, datos y botones de la imagen enfocada, centrados en `cx` a partir de `top`.
#[allow(clippy::too_many_arguments)]
unsafe fn draw_info(renderer: &Renderer, target: &ID2D1RenderTarget, brush: &ID2D1SolidColorBrush, ctx: &Ctx, img: &ScannedImage, virtual_index: i64, top: f32, alpha: f32, hits: &mut Vec<Hit>) {
    if alpha <= 0.01 { return; }
    let geo = ctx.geo;
    let (Some(title), Some(meta), Some(label), Some(dwrite)) = (renderer.title.as_ref(), renderer.meta.as_ref(), renderer.label.as_ref(), renderer.dwrite.as_ref()) else { return; };
    let cx = geo.cx();
    let half = geo.w * 0.35;
    brush.SetColor(&fg(0.96 * alpha));
    if super::show_names() { draw_text(target, &image_name(img), title, rect_f(cx - half, top, cx + half, top + geo.px(32.0)), brush); }

    let len = view_len();
    let mut details = vec![format!("{} / {}", real_index(virtual_index, len) + 1, len)];
    if ctx.is_current(img) { details.push(crate::i18n::t("Current wallpaper")); }
    if img.is_new { details.push(crate::i18n::t("New")); }
    brush.SetColor(&fg(0.55 * alpha));
    draw_text(target, &details.join("  ·  "), meta, rect_f(cx - half, top + geo.px(32.0), cx + half, top + geo.px(54.0)), brush);

    let buttons = [
        (Button::Apply, crate::i18n::t("Apply")),
        (Button::Favorite, crate::i18n::t(if img.is_favorite { "Unfavorite" } else { "Favorite" })),
        (Button::Reveal, crate::i18n::t("Explorer")),
        (Button::Exclude, crate::i18n::t("Remove")),
    ];
    let (h, pad, gap) = (geo.px(34.0), geo.px(18.0), geo.px(10.0));
    let widths: Vec<f32> = buttons.iter().map(|(_, text)| text_width(dwrite, label, text) + pad * 2.0).collect();
    let total = widths.iter().sum::<f32>() + gap * (widths.len() as f32 - 1.0);
    let mut x = cx - total / 2.0;
    let y = top + geo.px(66.0);
    for ((button, text), w) in buttons.iter().zip(widths) {
        let rect = rect_f(x, y, x + w, y + h);
        let shape = rounded(rect, h / 2.0);
        let hot = hovered(HitId::Button(*button, virtual_index));
        if *button == Button::Apply {
            let (r, g, b) = accent();
            let boost = if hot { 0.12 } else { 0.0 };
            brush.SetColor(&color(((r + boost).min(1.0), (g + boost).min(1.0), (b + boost).min(1.0)), alpha));
            target.FillRoundedRectangle(&shape, brush);
            brush.SetColor(&on_accent(alpha));
        } else {
            brush.SetColor(&fg(if hot { 0.2 } else { 0.1 } * alpha));
            target.FillRoundedRectangle(&shape, brush);
            brush.SetColor(&fg(0.12 * alpha));
            target.DrawRoundedRectangle(&shape, brush, 1.0, None);
            brush.SetColor(&if *button == Button::Favorite && img.is_favorite { color(COLOR_FAVORITE, alpha) } else { fg(0.92 * alpha) });
        }
        draw_text(target, text, label, rect, brush);
        hits.push(Hit::flat(HitId::Button(*button, virtual_index), rect));
        x += w + gap;
    }
}

fn draw_empty(renderer: &Renderer, target: &ID2D1RenderTarget, brush: &ID2D1SolidColorBrush, ctx: &Ctx) {
    let Some(title) = renderer.meta.as_ref() else { return; };
    let geo = ctx.geo;
    let text = if ctx.images.is_empty() { "No wallpapers found in the folder" } else { "No images match" };
    unsafe {
        brush.SetColor(&fg(0.7 * ctx.e));
        draw_text(target, &crate::i18n::t(text), title, rect_f(0.0, geo.h * 0.4, geo.w, geo.h * 0.5), brush);
    }
}

/// Showcase: tarjetas inclinadas en fila; la enfocada se ensancha.
unsafe fn draw_showcase(renderer: &mut Renderer, target: &ID2D1RenderTarget, brush: &ID2D1SolidColorBrush, ctx: &Ctx, hits: &mut Vec<Hit>) {
    let geo = ctx.geo;
    let view = gv(|v| v.view.borrow().clone());
    if view.is_empty() { return draw_empty(renderer, target, brush, ctx); }
    let len = view.len();
    let pos = gv(|v| v.pos.get());
    let reach = geo.show_reach();
    let first = pos.floor() as i64 - reach;
    let last = pos.ceil() as i64 + reach;
    let mut order: Vec<i64> = (first..=last).collect();
    order.sort_by(|a, b| (*b as f32 - pos).abs().total_cmp(&(*a as f32 - pos).abs()));

    let mut wanted: Vec<usize> = Vec::new();
    for vi in order.iter().rev().chain([last + 1, last + 2, first - 1, first - 2].iter()) {
        let index = view[real_index(*vi, len)];
        if !wanted.contains(&index) { wanted.push(index); }
    }
    request_tiles(ctx, wanted, 8);

    let slant = geo.show_slant();
    let fade_len = geo.show_side_w() * 2.4;
    let fade_start = geo.w / 2.0 - fade_len - geo.px(20.0);
    let radius = geo.px(10.0);
    for vi in order {
        let d = vi as f32 - pos;
        let rect = geo.show_rect(d);
        let offset = ((rect.left + rect.right) / 2.0 - geo.cx()).abs() + (rect.right - rect.left) / 2.0;
        let alpha = (1.0 - smoothstep((offset - fade_start) / fade_len)) * ctx.e;
        if alpha <= 0.01 { continue; }
        let img = &ctx.images[view[real_index(vi, len)]];
        let c = 1.0 - d.abs().min(1.0);
        // Sombra suave bajo la tarjeta.
        soft_shadow(target, brush, inflate(rect, -slant * 0.2), radius, geo.px(26.0) * lerp(0.6, 1.0, c), geo.px(14.0), 0.55 * alpha);
        let Some(shape) = slanted_geometry(&renderer.factory, rect, slant) else { continue; };
        let bounds = rect_f(rect.left - slant / 2.0, rect.top, rect.right + slant / 2.0, rect.bottom);
        match image_bitmap(renderer, img, ctx.small) {
            Some(bitmap) => fill_cover_geometry(target, &bitmap, &shape, bounds, alpha),
            None => {
                brush.SetColor(&fg(0.08 * alpha));
                target.FillGeometry(&shape, brush, None);
            }
        }
        let hot = hovered(HitId::Card(vi));
        let dim = (1.0 - c) * if hot { 0.15 } else { 0.42 };
        if dim > 0.0 {
            brush.SetColor(&bg(dim * alpha));
            target.FillGeometry(&shape, brush, None);
        }
        if ctx.is_current(img) {
            brush.SetColor(&color(accent(), alpha));
            target.DrawGeometry(&shape, brush, geo.px(2.5), None);
        } else {
            brush.SetColor(&fg((0.12 + 0.4 * c + if hot { 0.25 } else { 0.0 }) * alpha));
            target.DrawGeometry(&shape, brush, geo.px(1.0).max(1.0), None);
        }
        draw_badges(renderer, target, brush, img, rect_f(rect.left + slant * 0.4, rect.top, rect.right, rect.bottom), geo.s, alpha);
        hits.push(Hit::flat(HitId::Card(vi), rect));
    }

    let nearest = pos.round() as i64;
    let info_alpha = (1.0 - (pos - nearest as f32).abs() * 2.5).max(0.0) * ctx.e;
    let img = &ctx.images[view[real_index(nearest, len)]];
    let top = geo.show_cy() + geo.show_card_h() / 2.0 + geo.px(26.0);
    if !ctx.closing { draw_info(renderer, target, brush, ctx, img, nearest, top, info_alpha, hits); }
}

/// Matriz de giro de `angle` radianes alrededor de (x, y).
fn rotation(angle: f32, x: f32, y: f32) -> Matrix3x2 {
    let (sin, cos) = angle.sin_cos();
    Matrix3x2 { M11: cos, M12: sin, M21: -sin, M22: cos, M31: x - x * cos + y * sin, M32: y - x * sin - y * cos }
}

/// Abanico: tarjetas en arco alrededor de un punto bajo la pantalla.
unsafe fn draw_fan(renderer: &mut Renderer, target: &ID2D1RenderTarget, brush: &ID2D1SolidColorBrush, ctx: &Ctx, hits: &mut Vec<Hit>) {
    let geo = ctx.geo;
    let view = gv(|v| v.view.borrow().clone());
    if view.is_empty() { return draw_empty(renderer, target, brush, ctx); }
    let len = view.len();
    let pos = gv(|v| v.pos.get());
    let reach = 7i64;
    let first = pos.floor() as i64 - reach;
    let last = pos.ceil() as i64 + reach;
    let mut order: Vec<i64> = (first..=last).collect();
    order.sort_by(|a, b| (*b as f32 - pos).abs().total_cmp(&(*a as f32 - pos).abs()));
    let mut wanted: Vec<usize> = Vec::new();
    for vi in order.iter().rev() {
        let index = view[real_index(*vi, len)];
        if !wanted.contains(&index) { wanted.push(index); }
    }
    request_tiles(ctx, wanted, 6);

    // Al abrirse, el abanico se despliega desde el centro.
    let spread = if ctx.closing { ctx.e } else { ease_out_back(ctx.e) };
    let (card_w, card_h) = (geo.fan_card_w(), geo.fan_card_h());
    let radius = geo.fan_radius();
    let pivot = (geo.cx(), geo.fan_pivot_y());
    let corner = geo.px(14.0);
    for vi in order {
        let d = vi as f32 - pos;
        let alpha = (1.0 - smoothstep((d.abs() - 4.5) / 2.0)) * ctx.e;
        if alpha <= 0.01 { continue; }
        let img = &ctx.images[view[real_index(vi, len)]];
        let c = 1.0 - d.abs().min(1.0);
        let angle = d * geo.fan_step() * spread;
        let lift = geo.px(28.0) * c;
        let center = (pivot.0 + (radius + lift) * angle.sin(), pivot.1 - (radius + lift) * angle.cos());
        let scale = lerp(0.9, 1.12, c);
        let (hw, hh) = (card_w * scale / 2.0, card_h * scale / 2.0);
        let local = rect_f(-hw, -hh, hw, hh);
        let rect = rect_f(center.0 - hw, center.1 - hh, center.0 + hw, center.1 + hh);
        let base = rotation(angle, center.0, center.1);
        let lift_y = (1.0 - ctx.e) * geo.px(36.0);
        target.SetTransform(&(base * Matrix3x2::translation(0.0, lift_y)));
        soft_shadow(target, brush, rect, corner, geo.px(22.0), geo.px(10.0), 0.5 * alpha);
        let shape = rounded(rect, corner);
        match image_bitmap(renderer, img, ctx.small) {
            Some(bitmap) => fill_cover(target, &bitmap, &shape, alpha),
            None => {
                brush.SetColor(&fg(0.1 * alpha));
                target.FillRoundedRectangle(&shape, brush);
            }
        }
        let hot = hovered(HitId::Card(vi));
        let dim = (1.0 - c) * if hot { 0.12 } else { 0.38 };
        if dim > 0.0 {
            brush.SetColor(&bg(dim * alpha));
            target.FillRoundedRectangle(&shape, brush);
        }
        if ctx.is_current(img) {
            brush.SetColor(&color(accent(), alpha));
            target.DrawRoundedRectangle(&shape, brush, geo.px(2.5), None);
        } else {
            brush.SetColor(&fg((0.14 + 0.4 * c + if hot { 0.25 } else { 0.0 }) * alpha));
            target.DrawRoundedRectangle(&shape, brush, geo.px(1.0).max(1.0), None);
        }
        draw_badges(renderer, target, brush, img, rect, geo.s, alpha);
        hits.push(Hit { id: HitId::Card(vi), rect: local, center, angle });
    }
    target.SetTransform(&Matrix3x2::translation(0.0, (1.0 - ctx.e) * geo.px(36.0)));

    let nearest = pos.round() as i64;
    let info_alpha = (1.0 - (pos - nearest as f32).abs() * 2.5).max(0.0) * ctx.e;
    let img = &ctx.images[view[real_index(nearest, len)]];
    let card_top = geo.h * 0.6 - geo.px(28.0) - card_h * 1.12 / 2.0;
    let top = (card_top - geo.px(118.0)).max(geo.bar_top() + geo.bar_h() + geo.px(18.0));
    if !ctx.closing { draw_info(renderer, target, brush, ctx, img, nearest, top, info_alpha, hits); }
}

/// Mosaico: cuadrícula con desplazamiento vertical.
unsafe fn draw_mosaic(renderer: &mut Renderer, target: &ID2D1RenderTarget, brush: &ID2D1SolidColorBrush, ctx: &Ctx, hits: &mut Vec<Hit>) {
    let geo = ctx.geo;
    let view = gv(|v| v.view.borrow().clone());
    gv(|v| v.columns.set(geo.grid_columns()));
    if view.is_empty() { return draw_empty(renderer, target, brush, ctx); }
    let scroll = gv(|v| v.scroll.get());
    let selected = gv(|v| v.target.get());
    let (top, bottom) = (geo.grid_top(), geo.grid_bottom());
    let cols = geo.grid_columns();
    let (_, ch) = geo.grid_cell();
    let row_h = ch + geo.grid_gap();
    let first_row = ((scroll / row_h).floor() as usize).saturating_sub(0);
    let rows_visible = ((bottom - top) / row_h).ceil() as usize + 1;
    let start = (first_row * cols).min(view.len());
    let end = ((first_row + rows_visible) * cols).min(view.len());
    // Tarjetas visibles más una fila por arriba y por abajo.
    let wanted: Vec<usize> = (start..end).chain(end..(end + cols).min(view.len())).chain(start.saturating_sub(cols)..start).map(|i| view[i]).collect();
    request_tiles(ctx, wanted, cols * 2);

    target.PushAxisAlignedClip(&rect_f(0.0, top - geo.px(12.0), geo.w, bottom + geo.px(12.0)), D2D1_ANTIALIAS_MODE_PER_PRIMITIVE);
    let corner = geo.px(10.0);
    let small = renderer.small.clone();
    for (i, image_index) in view.iter().enumerate().take(end).skip(start) {
        let img = &ctx.images[*image_index];
        let mut rect = geo.grid_rect(i, scroll);
        // Entrada escalonada por filas.
        let row = (i / cols) as f32 - first_row as f32;
        let stagger = smoothstep((ctx.e * 1.4 - row * 0.08).clamp(0.0, 1.0));
        let alpha = stagger;
        if alpha <= 0.01 { continue; }
        let hot = hovered(HitId::Card(i as i64));
        let focused = selected == i as i64;
        if hot || focused { rect = inflate(rect, geo.px(4.0)); }
        let shape = rounded(rect, corner);
        soft_shadow(target, brush, rect, corner, geo.px(if hot { 20.0 } else { 12.0 }), geo.px(6.0), 0.45 * alpha);
        match image_bitmap(renderer, img, ctx.small) {
            Some(bitmap) => fill_cover(target, &bitmap, &shape, alpha),
            None => {
                brush.SetColor(&fg(0.1 * alpha));
                target.FillRoundedRectangle(&shape, brush);
            }
        }
        if ctx.is_current(img) {
            brush.SetColor(&color(accent(), alpha));
            target.DrawRoundedRectangle(&shape, brush, geo.px(2.5), None);
        } else if focused || hot {
            brush.SetColor(&fg(0.85 * alpha));
            target.DrawRoundedRectangle(&shape, brush, geo.px(2.0), None);
        } else {
            brush.SetColor(&fg(0.1 * alpha));
            target.DrawRoundedRectangle(&shape, brush, 1.0, None);
        }
        draw_badges(renderer, target, brush, img, rect, geo.s, alpha);
        if (hot || focused) && super::show_names() {
            // Nombre sobre una franja oscura en la parte de abajo.
            if let Some(format) = small.as_ref() {
                let strip = rect_f(rect.left, rect.bottom - geo.px(30.0), rect.right, rect.bottom);
                target.PushAxisAlignedClip(&strip, D2D1_ANTIALIAS_MODE_PER_PRIMITIVE);
                brush.SetColor(&black(0.6 * alpha));
                target.FillRoundedRectangle(&shape, brush);
                target.PopAxisAlignedClip();
                brush.SetColor(&white(0.95 * alpha));
                draw_text(target, &image_name(img), format, rect_f(strip.left + geo.px(12.0), strip.top, strip.right - geo.px(12.0), strip.bottom), brush);
            }
        }
        hits.push(Hit::flat(HitId::Card(i as i64), rect));
    }
    target.PopAxisAlignedClip();

    // Barra de desplazamiento fina a la derecha.
    let max_scroll = geo.grid_max_scroll(view.len());
    if max_scroll > 0.0 {
        let track = bottom - top;
        let thumb = (track * track / (track + max_scroll)).max(geo.px(40.0));
        let y = top + (track - thumb) * (scroll / max_scroll).clamp(0.0, 1.0);
        let x = geo.w - geo.grid_margin() / 2.0;
        brush.SetColor(&fg(0.25 * ctx.e));
        target.FillRoundedRectangle(&rounded(rect_f(x - geo.px(2.0), y, x + geo.px(2.0), y + thumb), geo.px(2.0)), brush);
    }
}

// --- Marco ---

/// Colores del marco: papel claro y tinta oscura, o al revés en el tema oscuro.
fn paper() -> Rgb { if light() { (0.945, 0.945, 0.937) } else { (0.075, 0.078, 0.082) } }
fn ink() -> Rgb { if light() { (0.11, 0.12, 0.13) } else { (0.93, 0.93, 0.92) } }
fn field() -> Rgb { if light() { (0.875, 0.878, 0.87) } else { (0.17, 0.175, 0.18) } }
/// Borde de las miniaturas de la cascada.
fn card_border() -> Rgb { if light() { (1.0, 1.0, 1.0) } else { (0.24, 0.245, 0.25) } }
fn mix(a: Rgb, b: Rgb, t: f32) -> Rgb { (lerp(a.0, b.0, t), lerp(a.1, b.1, t), lerp(a.2, b.2, t)) }

/// Silueta de la imagen grande: rectángulo redondeado con dos huecos en esquinas opuestas,
/// unidos con curvas suaves (arriba a la izquierda y abajo a la derecha).
unsafe fn framed_geometry(
    factory: &windows::Win32::Graphics::Direct2D::ID2D1Factory,
    o: D2D_RECT_F,
    r: f32,
    (nw, nh): (f32, f32),
    (bw, bh): (f32, f32),
) -> Option<windows::Win32::Graphics::Direct2D::ID2D1Geometry> {
    use windows::Win32::Graphics::Direct2D::Common::{D2D1_FIGURE_BEGIN_FILLED, D2D1_FIGURE_END_CLOSED, D2D_SIZE_F};
    use windows::Win32::Graphics::Direct2D::{
        D2D1_ARC_SEGMENT, D2D1_ARC_SIZE_SMALL, D2D1_SWEEP_DIRECTION_CLOCKWISE, D2D1_SWEEP_DIRECTION_COUNTER_CLOCKWISE,
    };
    let geometry = factory.CreatePathGeometry().ok()?;
    let sink = geometry.Open().ok()?;
    let arc = |x: f32, y: f32, clockwise: bool| D2D1_ARC_SEGMENT {
        point: point(x, y),
        size: D2D_SIZE_F { width: r, height: r },
        rotationAngle: 0.0,
        sweepDirection: if clockwise { D2D1_SWEEP_DIRECTION_CLOCKWISE } else { D2D1_SWEEP_DIRECTION_COUNTER_CLOCKWISE },
        arcSize: D2D1_ARC_SIZE_SMALL,
    };
    let (l, t, rr, b) = (o.left, o.top, o.right, o.bottom);
    sink.BeginFigure(point(l + nw + r, t), D2D1_FIGURE_BEGIN_FILLED);
    sink.AddLine(point(rr - r, t));
    sink.AddArc(&arc(rr, t + r, true));
    sink.AddLine(point(rr, b - bh - r));
    sink.AddArc(&arc(rr - r, b - bh, true));
    sink.AddLine(point(rr - bw + r, b - bh));
    sink.AddArc(&arc(rr - bw, b - bh + r, false));
    sink.AddLine(point(rr - bw, b - r));
    sink.AddArc(&arc(rr - bw - r, b, true));
    sink.AddLine(point(l + r, b));
    sink.AddArc(&arc(l, b - r, true));
    sink.AddLine(point(l, t + nh + r));
    sink.AddArc(&arc(l + r, t + nh, true));
    sink.AddLine(point(l + nw - r, t + nh));
    sink.AddArc(&arc(l + nw, t + nh - r, false));
    sink.AddLine(point(l + nw, t + r));
    sink.AddArc(&arc(l + nw + r, t, true));
    sink.EndFigure(D2D1_FIGURE_END_CLOSED);
    sink.Close().ok()?;
    Some(geometry.into())
}

/// Botón de texto del marco. `dark`: relleno oscuro con texto claro.
#[allow(clippy::too_many_arguments)]
unsafe fn frame_button(target: &ID2D1RenderTarget, brush: &ID2D1SolidColorBrush, format: &IDWriteTextFormat, rect: D2D_RECT_F, text: &str, dark: bool, hot: bool, radius: f32) {
    let shape = rounded(rect, radius);
    if dark {
        brush.SetColor(&color(if hot { mix(ink(), paper(), 0.14) } else { ink() }, 1.0));
    } else {
        brush.SetColor(&color(if hot { mix(field(), ink(), 0.08) } else { field() }, 1.0));
    }
    target.FillRoundedRectangle(&shape, brush);
    brush.SetColor(&if dark { color(paper(), 0.96) } else { color(ink(), 0.9) });
    draw_text(target, text, format, rect, brush);
}

/// Marco: la imagen enfocada en grande, el cuadro de búsqueda, colores y acciones arriba a la
/// izquierda, los filtros en una columna a la derecha y las miniaturas en cascada abajo a la derecha.
unsafe fn draw_framed(renderer: &mut Renderer, target: &ID2D1RenderTarget, brush: &ID2D1SolidColorBrush, ctx: &Ctx, hits: &mut Vec<Hit>) {
    let geo = ctx.geo;
    let px = |v: f32| geo.px(v);
    brush.SetColor(&color(paper(), 1.0));
    target.FillRectangle(&rect_f(0.0, 0.0, geo.w, geo.h), brush);

    let view = gv(|v| v.view.borrow().clone());
    let len = view.len();
    let pos = gv(|v| v.pos.get());
    let target_index = gv(|v| v.target.get());
    let o = geo.frame_rect();
    let top_notch = geo.frame_top_notch();
    let bottom_notch = geo.frame_bottom_notch();
    hits.push(Hit::flat(HitId::Backdrop, o));

    // Pedir la imagen grande y las vecinas.
    if len > 0 {
        let mut wanted = Vec::new();
        for d in [0i64, 1, -1, 2, -2, 3, -3] {
            let index = view[real_index(pos.round() as i64 + d, len)];
            if !wanted.contains(&index) { wanted.push(index); }
        }
        request_tiles(ctx, wanted, 3);
    }

    // Imagen grande con fundido cruzado entre la anterior y la siguiente mientras se mueve.
    if let Some(shape) = framed_geometry(&renderer.factory, o, geo.frame_radius(), top_notch, bottom_notch) {
        brush.SetColor(&color(field(), 1.0));
        target.FillGeometry(&shape, brush, None);
        if len > 0 {
            let base = pos.floor() as i64;
            let frac = pos - base as f32;
            for (vi, alpha) in [(base, 1.0f32), (base + 1, frac)] {
                if alpha <= 0.01 { continue; }
                let img = &ctx.images[view[real_index(vi, len)]];
                if let Some(bitmap) = image_bitmap(renderer, img, ctx.small) {
                    fill_cover_geometry(target, &bitmap, &shape, o, alpha);
                }
            }
        } else if let Some(format) = renderer.meta.as_ref() {
            brush.SetColor(&color(ink(), 0.7));
            let text = if ctx.images.is_empty() { "No wallpapers found in the folder" } else { "No images match" };
            draw_text(target, &crate::i18n::t(text), format, rect_f(o.left, o.top, o.right, o.bottom), brush);
        }
        brush.SetColor(&color(ink(), 0.10));
        target.DrawGeometry(&shape, brush, 1.0, None);
    }

    let focused = if len > 0 { Some(&ctx.images[view[real_index(target_index, len)]]) } else { None };
    draw_frame_panel(renderer, target, brush, ctx, focused, target_index, len, hits);
    draw_frame_column(renderer, target, brush, ctx, hits);
    if len == 0 { return; }

    // Cascada: la anterior arriba a la derecha, la elegida en medio y la siguiente abajo a la izquierda.
    let (card_w, card_h) = geo.frame_card();
    let anchor = (o.right - bottom_notch.0 * 0.5 + px(40.0), o.bottom - bottom_notch.1 * 0.5 + px(14.0));
    let first = pos.round() as i64 - 3;
    let mut order: Vec<i64> = (first..=first + 6).collect();
    order.sort_by(|a, b| (*b as f32 - pos).abs().total_cmp(&(*a as f32 - pos).abs()));
    let corner = px(10.0);
    for vi in order {
        let d = vi as f32 - pos;
        let ad = d.abs();
        let alpha = 1.0 - smoothstep((ad - 1.1) / 0.6);
        if alpha <= 0.01 { continue; }
        let scale = if ad <= 1.0 { lerp(1.0, 0.6, ad) } else { 0.6 * 0.85f32.powf(ad - 1.0) };
        let (cx, cy) = (anchor.0 - d * px(190.0), anchor.1 + d * px(72.0));
        let (hw, hh) = (card_w * scale / 2.0, card_h * scale / 2.0);
        let rect = rect_f(cx - hw, cy - hh, cx + hw, cy + hh);
        let img = &ctx.images[view[real_index(vi, len)]];
        let hot = hovered(HitId::Card(vi));
        soft_shadow(target, brush, rect, corner, px(18.0), px(8.0), 0.35 * alpha);
        // Borde blanco alrededor de cada miniatura.
        let border = inflate(rect, px(if hot { 5.0 } else { 4.0 }));
        brush.SetColor(&color(card_border(), alpha));
        target.FillRoundedRectangle(&rounded(border, corner + px(4.0)), brush);
        let shape = rounded(rect, corner);
        match image_bitmap(renderer, img, ctx.small) {
            Some(bitmap) => fill_cover(target, &bitmap, &shape, alpha),
            None => {
                brush.SetColor(&color(field(), alpha));
                target.FillRoundedRectangle(&shape, brush);
            }
        }
        if ctx.is_current(img) {
            brush.SetColor(&color(accent(), alpha));
            target.DrawRoundedRectangle(&rounded(border, corner + px(4.0)), brush, px(2.5), None);
        }
        draw_badges(renderer, target, brush, img, rect, geo.s * scale.max(0.7), alpha);
        if ad < 0.5 {
            if let (Some(format), Some(dwrite)) = (renderer.small.as_ref(), renderer.dwrite.as_ref()) {
                let text = crate::i18n::t("Selected").to_uppercase();
                let w = text_width(dwrite, format, &text) + px(14.0);
                let badge = rect_f(rect.right - w - px(8.0), rect.bottom - px(26.0), rect.right - px(8.0), rect.bottom - px(8.0));
                brush.SetColor(&color(ink(), 0.85 * alpha));
                target.FillRoundedRectangle(&rounded(badge, px(9.0)), brush);
                brush.SetColor(&color(paper(), 0.95 * alpha));
                draw_text(target, &text, format, rect_f(badge.left + px(7.0), badge.top, badge.right, badge.bottom), brush);
            }
        }
        hits.push(Hit::flat(HitId::Card(vi), border));
    }
}

/// Cuadro de arriba a la izquierda: búsqueda, nombre, colores y acciones.
#[allow(clippy::too_many_arguments)]
unsafe fn draw_frame_panel(
    renderer: &Renderer,
    target: &ID2D1RenderTarget,
    brush: &ID2D1SolidColorBrush,
    ctx: &Ctx,
    focused: Option<&ScannedImage>,
    target_index: i64,
    len: usize,
    hits: &mut Vec<Hit>,
) {
    let geo = ctx.geo;
    let px = |v: f32| geo.px(v);
    let (Some(name_format), Some(small), Some(meta), Some(label), Some(search_format)) =
        (renderer.name.as_ref(), renderer.small.as_ref(), renderer.meta.as_ref(), renderer.label.as_ref(), renderer.search.as_ref())
    else { return; };
    let o = geo.frame_rect();
    let left = o.left + px(8.0);
    let right = o.left + geo.frame_top_notch().0 - px(26.0);
    let mut y = o.top + px(6.0);

    // Cabecera: marca y posición.
    brush.SetColor(&color(accent(), 1.0));
    target.FillRoundedRectangle(&rounded(rect_f(left, y + px(6.0), left + px(14.0), y + px(20.0)), px(3.0)), brush);
    brush.SetColor(&color(ink(), 1.0));
    draw_text(target, "SWEPR", name_format, rect_f(left + px(22.0), y, right - px(70.0), y + px(26.0)), brush);
    if len > 0 {
        brush.SetColor(&color(ink(), 0.5));
        draw_text(target, &format!("{} / {}", real_index(target_index, len) + 1, len), meta, rect_f(right - px(80.0), y, right, y + px(26.0)), brush);
    }
    y += px(32.0);

    // Búsqueda
    let query = gv(|v| v.query.borrow().clone());
    let search_box = rect_f(left, y, right, y + px(34.0));
    brush.SetColor(&color(field(), 1.0));
    target.FillRoundedRectangle(&rounded(search_box, px(9.0)), brush);
    if !query.is_empty() {
        brush.SetColor(&color(ink(), 0.8));
        target.DrawRoundedRectangle(&rounded(search_box, px(9.0)), brush, px(1.2), None);
    }
    let (lx, ly, r) = (search_box.left + px(16.0), (search_box.top + search_box.bottom) / 2.0 - px(1.0), px(5.0));
    brush.SetColor(&color(ink(), 0.55));
    target.DrawEllipse(&D2D1_ELLIPSE { point: point(lx, ly), radiusX: r, radiusY: r }, brush, px(1.5), None);
    target.DrawLine(point(lx + r * 0.7, ly + r * 0.7), point(lx + r * 1.6, ly + r * 1.6), brush, px(1.5), None);
    let text_rect = rect_f(search_box.left + px(30.0), search_box.top, search_box.right - px(30.0), search_box.bottom);
    if query.is_empty() {
        brush.SetColor(&color(ink(), 0.45));
        draw_text(target, &crate::i18n::t("Type to search"), search_format, text_rect, brush);
    } else {
        brush.SetColor(&color(ink(), 0.95));
        draw_text(target, &format!("{query}\u{258F}"), search_format, text_rect, brush);
        let clear = rect_f(search_box.right - px(30.0), search_box.top, search_box.right - px(4.0), search_box.bottom);
        let (cx, cy, arm) = ((clear.left + clear.right) / 2.0, (clear.top + clear.bottom) / 2.0, px(4.0));
        brush.SetColor(&color(ink(), if hovered(HitId::ClearSearch) { 0.95 } else { 0.55 }));
        target.DrawLine(point(cx - arm, cy - arm), point(cx + arm, cy + arm), brush, px(1.5), None);
        target.DrawLine(point(cx + arm, cy - arm), point(cx - arm, cy + arm), brush, px(1.5), None);
        hits.push(Hit::flat(HitId::Search, search_box));
        hits.push(Hit::flat(HitId::ClearSearch, clear));
    }
    if query.is_empty() { hits.push(Hit::flat(HitId::Search, search_box)); }
    y += px(42.0);

    // Nombre y datos de la enfocada.
    if let Some(img) = focused {
        brush.SetColor(&color(ink(), 1.0));
        if super::show_names() { draw_text(target, &image_name(img), name_format, rect_f(left, y, right, y + px(24.0)), brush); }
        let mut details = Vec::new();
        if ctx.is_current(img) { details.push(crate::i18n::t("Current wallpaper")); }
        if img.is_favorite { details.push(crate::i18n::t("Favorite")); }
        if img.is_new { details.push(crate::i18n::t("New")); }
        if !details.is_empty() {
            brush.SetColor(&color(ink(), 0.55));
            draw_text(target, &details.join("  ·  "), small, rect_f(left, y + px(24.0), right, y + px(42.0)), brush);
        }
    }
    y += px(48.0);

    // Fichas de color
    let families = families_present(ctx.images, ctx.small);
    let selected = gv(|v| v.family.get());
    let d = px(16.0);
    let gap = px(9.0);
    let mut x = left + px(2.0);
    for f in families {
        let center = point(x + d / 2.0, y + d / 2.0);
        let hot = hovered(HitId::Chip(f));
        let faded = selected.is_some() && selected != Some(f);
        brush.SetColor(&color(f.swatch(), if faded { 0.4 } else { 1.0 }));
        let rad = d / 2.0 * if hot && selected != Some(f) { 1.15 } else { 1.0 };
        target.FillEllipse(&D2D1_ELLIPSE { point: center, radiusX: rad, radiusY: rad }, brush);
        if selected == Some(f) {
            brush.SetColor(&color(ink(), 0.9));
            let ring = d / 2.0 + px(3.5);
            target.DrawEllipse(&D2D1_ELLIPSE { point: center, radiusX: ring, radiusY: ring }, brush, px(1.6), None);
        }
        hits.push(Hit::flat(HitId::Chip(f), rect_f(x - gap / 2.0, y - px(4.0), x + d + gap / 2.0, y + d + px(4.0))));
        x += d + gap;
    }
    y += px(30.0);

    // Acciones
    let Some(img) = focused else { return; };
    let buttons = [
        (Button::Favorite, crate::i18n::t(if img.is_favorite { "Unfavorite" } else { "Favorite" })),
        (Button::Reveal, crate::i18n::t("Explorer")),
        (Button::Exclude, crate::i18n::t("Remove")),
    ];
    let gap = px(8.0);
    let w = (right - left - gap * 2.0) / 3.0;
    for (i, (button, text)) in buttons.iter().enumerate() {
        let bx = left + i as f32 * (w + gap);
        let rect = rect_f(bx, y, bx + w, y + px(30.0));
        frame_button(target, brush, label, rect, text, false, hovered(HitId::Button(*button, target_index)), px(8.0));
        hits.push(Hit::flat(HitId::Button(*button, target_index), rect));
    }
    y += px(38.0);
    let apply = rect_f(left, y, right, y + px(36.0));
    frame_button(target, brush, label, apply, &format!("\u{2713}  {}", crate::i18n::t("Apply")), true, hovered(HitId::Button(Button::Apply, target_index)), px(9.0));
    hits.push(Hit::flat(HitId::Button(Button::Apply, target_index), apply));
}

/// Columna derecha: flechas y filtros Todas / Favoritas / Nuevas en vertical.
unsafe fn draw_frame_column(renderer: &Renderer, target: &ID2D1RenderTarget, brush: &ID2D1SolidColorBrush, ctx: &Ctx, hits: &mut Vec<Hit>) {
    let geo = ctx.geo;
    let px = |v: f32| geo.px(v);
    let Some(label) = renderer.label.as_ref() else { return; };
    let o = geo.frame_rect();
    let (x0, x1) = (o.right + px(12.0), geo.w - px(12.0));
    let kind = gv(|v| v.kind.get());
    let kinds = [(Kind::All, crate::i18n::t("All")), (Kind::Favorites, crate::i18n::t("Favorites")), (Kind::New, crate::i18n::t("New"))];
    let (h, gap) = (px(30.0), px(8.0));
    let total = kinds.len() as f32 * (h + gap) - gap;
    let mut y = geo.h * 0.36 - total / 2.0;

    // Flecha arriba
    let chevron = |cy: f32, up: bool, hot: bool| {
        let cx = (x0 + x1) / 2.0;
        let (dx, dy) = (px(6.0), px(3.5) * if up { -1.0 } else { 1.0 });
        brush.SetColor(&color(ink(), if hot { 0.95 } else { 0.5 }));
        target.DrawLine(point(cx - dx, cy - dy), point(cx, cy + dy), brush, px(1.8), None);
        target.DrawLine(point(cx, cy + dy), point(cx + dx, cy - dy), brush, px(1.8), None);
    };
    let up = rect_f(x0, y - px(52.0), x1, y - px(16.0));
    chevron((up.top + up.bottom) / 2.0, true, hovered(HitId::Step(-1)));
    hits.push(Hit::flat(HitId::Step(-1), up));

    for (k, text) in kinds.iter() {
        let rect = rect_f(x0, y, x1, y + h);
        let selected = kind == *k;
        let hot = hovered(HitId::Filter(*k));
        if selected || hot {
            brush.SetColor(&color(if selected { ink() } else { field() }, 1.0));
            target.FillRoundedRectangle(&rounded(rect, h / 2.0), brush);
        }
        brush.SetColor(&if selected { color(paper(), 0.96) } else { color(ink(), 0.7) });
        draw_text(target, text, label, rect, brush);
        hits.push(Hit::flat(HitId::Filter(*k), rect));
        y += h + gap;
    }
    let down = rect_f(x0, y + px(8.0), x1, y + px(44.0));
    chevron((down.top + down.bottom) / 2.0, false, hovered(HitId::Step(1)));
    hits.push(Hit::flat(HitId::Step(1), down));
}

// --- Ventana ---

fn hit_at(x: f32, y: f32) -> Option<HitId> {
    gv(|v| v.hits.borrow().iter().find(|hit| hit.contains(x, y)).map(|hit| hit.id))
}

fn lparam_point(lparam: LPARAM) -> (f32, f32) {
    ((lparam.0 & 0xFFFF) as i16 as f32, ((lparam.0 >> 16) & 0xFFFF) as i16 as f32)
}

fn type_char(ch: char) {
    gv(|v| v.query.borrow_mut().push(ch));
    set_filter(None, None);
}

fn erase_char() {
    if gv(|v| v.query.borrow_mut().pop()).is_some() { set_filter(None, None); }
}

fn clear_query() {
    gv(|v| v.query.borrow_mut().clear());
    set_filter(None, None);
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
                    if super::vars(|v| v.menu_open.get()) || is_closing() { return LRESULT(0); }
                    // Clic en otro monitor: cerrar.
                    let mut pt = POINT::default();
                    let _ = GetCursorPos(&mut pt);
                    let m = monitor_rect();
                    let outside = pt.x < m.left || pt.x >= m.right || pt.y < m.top || pt.y >= m.bottom;
                    let clicked = GetAsyncKeyState(VK_LBUTTON.0 as i32) < 0 || GetAsyncKeyState(VK_RBUTTON.0 as i32) < 0;
                    if outside && (clicked || gv(|v| v.close_on_leave.get())) { super::hide_panel(); }
                }
                TILE_RELEASE_TIMER_ID => {
                    let _ = KillTimer(hwnd, TILE_RELEASE_TIMER_ID);
                    if !is_visible() { carousel::clear_tiles(); }
                }
                PREVIEW_TIMER_ID => {
                    let _ = KillTimer(hwnd, PREVIEW_TIMER_ID);
                    // El marco tapa el escritorio y ya enseña la imagen en grande.
                    if is_visible() && !is_closing() && gv(|v| v.style.get()) != ProStyle::Frame {
                        super::preview_focus(focused_image().and_then(super::image_path), monitor_rect());
                    }
                }
                _ => {}
            }
            LRESULT(0)
        }
        WM_APP_GALLERY_RENDER => {
            if is_visible() { render(hwnd); }
            LRESULT(0)
        }
        carousel::WM_APP_TILE_READY => {
            carousel::collect_ready();
            request_render();
            LRESULT(0)
        }
        WM_ACTIVATE => {
            // Otra ventana pasó a primer plano (Alt+Tab, clic en la barra de tareas).
            if (wparam.0 & 0xFFFF) == 0 && !super::vars(|v| v.menu_open.get()) && is_visible() && !is_closing() {
                super::hide_panel();
            }
            LRESULT(0)
        }
        WM_MOUSEMOVE => {
            let (x, y) = lparam_point(lparam);
            let hover = hit_at(x, y);
            if gv(|v| v.hover.replace(hover)) != hover {
                // En el mosaico, pasar por encima enfoca (y muestra la vista previa).
                if let (Some(HitId::Card(i)), false) = (hover, is_loop()) {
                    if gv(|v| v.target.replace(i)) != i { focus_changed(); }
                }
                request_render();
            }
            LRESULT(0)
        }
        WM_LBUTTONUP => {
            let (x, y) = lparam_point(lparam);
            match hit_at(x, y) {
                Some(HitId::Card(vi)) => {
                    if !is_loop() || vi == gv(|v| v.target.get()) {
                        if let Some(index) = image_of(vi) { apply(index); }
                    } else {
                        move_to(hwnd, vi);
                    }
                }
                Some(HitId::Button(button, vi)) => {
                    if let Some(index) = image_of(vi) { run_button(button, index); }
                }
                Some(HitId::Filter(kind)) => set_filter(Some(kind), None),
                Some(HitId::Chip(family)) => {
                    let current = gv(|v| v.family.get());
                    set_filter(None, Some(if current == Some(family) { None } else { Some(family) }));
                }
                Some(HitId::ClearSearch) => clear_query(),
                Some(HitId::Search | HitId::Backdrop) => {}
                Some(HitId::Step(delta)) => move_by(hwnd, delta),
                None => super::hide_panel(),
            }
            LRESULT(0)
        }
        WM_RBUTTONUP => {
            let (x, y) = lparam_point(lparam);
            if let Some(HitId::Card(vi)) = hit_at(x, y) {
                if let Some(index) = image_of(vi) { super::show_thumbnail_menu(hwnd, index); }
            }
            LRESULT(0)
        }
        WM_MOUSEWHEEL => {
            let delta = (wparam.0 >> 16) as i16 as i32;
            if is_loop() {
                move_by(hwnd, if delta < 0 { 1 } else { -1 });
            } else {
                let geo = Geo::current();
                let (_, ch) = geo.grid_cell();
                let step = (ch + geo.grid_gap()) * (-delta as f32 / 120.0);
                gv(|v| v.scroll_target.set((v.scroll_target.get() + step).clamp(0.0, geo.grid_max_scroll(view_len()))));
                gv(|v| v.last_step.set(Some(Instant::now())));
                start_frames(hwnd);
            }
            LRESULT(0)
        }
        WM_KEYDOWN => {
            let columns = gv(|v| v.columns.get()) as i64;
            match wparam.0 as u16 {
                0x1B => {
                    // Esc: primero borra la búsqueda, luego cierra.
                    if gv(|v| v.query.borrow().is_empty()) { super::hide_panel(); } else { clear_query(); }
                }
                0x25 => move_by(hwnd, -1),
                0x27 => move_by(hwnd, 1),
                0x26 if !is_loop() => move_by(hwnd, -columns),
                0x28 if !is_loop() => move_by(hwnd, columns),
                0x21 if !is_loop() => move_by(hwnd, -columns * 3),
                0x22 if !is_loop() => move_by(hwnd, columns * 3),
                0x24 => move_to(hwnd, 0),
                0x23 => move_to(hwnd, view_len() as i64 - 1),
                0x26 => move_by(hwnd, -1),
                0x28 => move_by(hwnd, 1),
                0x0D => if let Some(index) = focused_image() { apply(index); },
                0x09 => {
                    // Tab: siguiente filtro.
                    let next = match gv(|v| v.kind.get()) { Kind::All => Kind::Favorites, Kind::Favorites => Kind::New, Kind::New => Kind::All };
                    set_filter(Some(next), None);
                }
                0x5D => if let Some(index) = focused_image() { super::show_thumbnail_menu(hwnd, index); },
                _ => return DefWindowProcW(hwnd, msg, wparam, lparam),
            }
            LRESULT(0)
        }
        WM_CHAR => {
            match wparam.0 as u32 {
                0x08 => erase_char(),
                code if code >= 0x20 && code != 0x7F => {
                    if let Some(ch) = char::from_u32(code) { type_char(ch); }
                }
                _ => {}
            }
            LRESULT(0)
        }
        WM_SETCURSOR => {
            let pointer = matches!(gv(|v| v.hover.get()), Some(HitId::Card(_) | HitId::Button(..) | HitId::Filter(_) | HitId::Chip(_) | HitId::ClearSearch | HitId::Step(_)));
            if let Ok(cursor) = LoadCursorW(None, if pointer { IDC_HAND } else { IDC_ARROW }) { let _ = SetCursor(cursor); }
            LRESULT(1)
        }
        _ => DefWindowProcW(hwnd, msg, wparam, lparam),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn families_follow_the_hue() {
        assert_eq!(Family::of((0.9, 0.1, 0.1)), Family::Red);
        assert_eq!(Family::of((0.1, 0.8, 0.2)), Family::Green);
        assert_eq!(Family::of((0.1, 0.3, 0.9)), Family::Blue);
        assert_eq!(Family::of((0.5, 0.5, 0.5)), Family::Neutral);
        assert_eq!(Family::of((0.02, 0.02, 0.05)), Family::Neutral);
    }

    #[test]
    fn rotated_hits_follow_the_card() {
        let hit = Hit { id: HitId::Card(0), rect: rect_f(-10.0, -20.0, 10.0, 20.0), center: (100.0, 100.0), angle: std::f32::consts::FRAC_PI_2 };
        // Girada 90°: ahora es ancha y baja.
        assert!(hit.contains(115.0, 100.0));
        assert!(!hit.contains(100.0, 115.0));
    }

    #[test]
    fn layouts_fit_the_monitor() {
        for (w, h, s) in [(1920.0, 1080.0, 1.0), (2560.0, 1440.0, 1.5), (1366.0, 768.0, 1.0), (3840.0, 2160.0, 2.0)] {
            let geo = Geo { w, h, s };
            let focus = geo.show_rect(0.0);
            assert!(focus.left > 0.0 && focus.right < w && focus.top > geo.bar_top() + geo.bar_h());
            assert!(geo.show_cy() + geo.show_card_h() / 2.0 + geo.px(26.0 + 100.0) < h, "{w}x{h}: botones fuera");
            // Las tarjetas no se solapan.
            let rects: Vec<D2D_RECT_F> = (-3..=3).map(|i| geo.show_rect(i as f32 - 0.3)).collect();
            for pair in rects.windows(2) { assert!(pair[0].right <= pair[1].left + 0.5); }
            let (cw, _) = geo.grid_cell();
            let cols = geo.grid_columns() as f32;
            assert!(geo.grid_margin() * 2.0 + cw * cols + geo.grid_gap() * (cols - 1.0) <= w + 0.5);
            assert!(geo.fan_pivot_y() - geo.fan_radius() - geo.fan_card_h() * 0.6 > geo.bar_top() + geo.bar_h());
            // Los dos huecos del marco no se tocan.
            let o = geo.frame_rect();
            let (nw, nh) = geo.frame_top_notch();
            let (bw, bh) = geo.frame_bottom_notch();
            assert!(nw + bw + geo.frame_radius() * 2.0 < o.right - o.left, "{w}x{h}");
            assert!(nh + bh + geo.frame_radius() * 2.0 < o.bottom - o.top, "{w}x{h}");
        }
    }

    /// Pinta cada estilo sobre la primera imagen en BMP para revisarlo a ojo:
    /// `SWEPR_TEST_DIR=<carpeta> SWEPR_TEST_OUT=<prefijo> cargo test render_gallery_preview -- --ignored`
    #[test]
    #[ignore]
    fn render_gallery_preview() {
        if std::env::var("SWEPR_TEST_LIGHT").is_ok() { crate::winutil::set_theme(2); }
        use std::sync::{Arc, Mutex};
        let dir = std::env::var("SWEPR_TEST_DIR").expect("SWEPR_TEST_DIR");
        let out = std::env::var("SWEPR_TEST_OUT").expect("SWEPR_TEST_OUT");
        let mut paths: Vec<PathBuf> = std::fs::read_dir(dir).unwrap().flatten().map(|e| e.path())
            .filter(|p| crate::scanner::is_supported_image(p))
            .collect();
        paths.sort();
        paths.truncate(30);
        let images: Vec<ScannedImage> = paths.iter().enumerate().map(|(i, p)| ScannedImage {
            path: p.clone(),
            file_name: String::new(),
            is_new: i % 7 == 6,
            is_favorite: i % 5 == 3,
            modified: None,
        }).collect();
        let (w, h) = (1920, 1080);
        unsafe {
            use windows::Win32::System::Com::{CoInitializeEx, COINIT_APARTMENTTHREADED};
            let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
            let small = ThumbnailCache::new();
            for img in &images {
                if let Some(thumb) = crate::thumbs::load_cover_bitmap(&img.path, 256, 144) { small.insert(thumb); }
            }
            super::super::vars(|v| {
                *v.current_wallpaper.borrow_mut() = Some(images[2].path.clone());
                *v.thumb_cache.borrow_mut() = Some(small);
                *v.scanned.borrow_mut() = Some(Arc::new(Mutex::new(images.clone())));
            });
            gv(|v| {
                v.monitor.set(RECT { left: 0, top: 0, right: w, bottom: h });
                v.scale.set(1.0);
            });
            let mut renderer = Renderer::new(w, h, 1.0).expect("Direct2D");
            let backdrop = crate::thumbs::load_cover_bitmap(&images[0].path, w, h)
                .and_then(|thumb| super::super::d2d::gdi_to_d2d(&renderer.rt(), thumb.hbitmap));
            for (name, style, query, hover) in [
                ("showcase", ProStyle::Showcase, "", Some(HitId::Button(Button::Apply, 4))),
                ("fan", ProStyle::Fan, "", None),
                ("mosaic", ProStyle::Mosaic, "", Some(HitId::Card(9))),
                ("search", ProStyle::Mosaic, "a", None),
                ("frame", ProStyle::Frame, "", Some(HitId::Button(Button::Apply, 4))),
            ] {
                gv(|v| {
                    v.style.set(style);
                    v.open.set(1.0);
                    v.phase.set(Phase::Open);
                    v.pos.set(4.0);
                    v.target.set(4);
                    v.scroll.set(0.0);
                    v.hover.set(hover);
                    *v.query.borrow_mut() = query.to_string();
                });
                // Tarjetas grandes generadas aquí, sin el hilo de carga.
                let geo = Geo::current();
                let (tw, th) = geo.tile_size(style);
                carousel::set_layout(carousel::Layout { tile_w: tw, tile_h: th, slant: 0, gap: 0, pad_y: 0, margin_x: 0 });
                draw_frame(&mut renderer, HWND::default(), backdrop.as_ref());
                let bmp = windows::Win32::Graphics::Gdi::HBITMAP(renderer.dib.0);
                super::super::save_bmp(renderer.dc, bmp, renderer.width, renderer.height, &format!("{out}-{name}.bmp"));
            }
        }
    }
}
