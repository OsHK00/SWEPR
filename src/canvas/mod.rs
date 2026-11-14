//! Modo "Lienzo SWEPR": el fondo se dibuja en una ventana propia colocada detrás de los
//! iconos del escritorio, con Direct2D. Así el cambio empieza al instante y con una
//! transición suave sincronizada con el refresco de la pantalla.
//!
//! Hilos:
//! - Decodificador: abre la imagen con WIC y la escala al tamaño del monitor (puede tardar
//!   decenas de milisegundos, así que nunca se hace en el hilo del lienzo).
//! - Lienzo: dueño de la ventana y de Direct2D. Como su ventana es hija de una ventana del
//!   Explorador, este hilo comparte cola de entrada con él: nunca debe bloquearse, solo dibuja.
//!
//! Solo se dibuja durante las transiciones; con el fondo quieto no consume nada.

mod layout;

pub use layout::Area;
use layout::{decode_size, layout_rects};

use crate::config::{FitMode, Transition};
use crate::{log_error, log_info, log_warn};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::Arc;
use std::time::{Duration, Instant};
use windows::core::{w, PCWSTR};
use windows::Win32::Foundation::{BOOL, HWND, LPARAM, LRESULT, RECT, WPARAM};
use windows::Win32::Graphics::Direct2D::Common::{
    D2D1_ALPHA_MODE_PREMULTIPLIED, D2D1_ALPHA_MODE_UNKNOWN, D2D1_COLOR_F, D2D1_PIXEL_FORMAT, D2D_POINT_2F, D2D_SIZE_U,
};
use windows::Win32::Graphics::Direct2D::{
    D2D1CreateFactory, ID2D1Bitmap, ID2D1Factory, ID2D1HwndRenderTarget, ID2D1Layer, D2D1_ANTIALIAS_MODE_ALIASED,
    D2D1_ANTIALIAS_MODE_PER_PRIMITIVE, D2D1_ELLIPSE, D2D1_LAYER_OPTIONS_NONE, D2D1_LAYER_PARAMETERS,
    D2D1_BITMAP_INTERPOLATION_MODE_LINEAR, D2D1_BITMAP_PROPERTIES, D2D1_FACTORY_TYPE_SINGLE_THREADED,
    D2D1_FEATURE_LEVEL_DEFAULT, D2D1_HWND_RENDER_TARGET_PROPERTIES, D2D1_PRESENT_OPTIONS_NONE,
    D2D1_RENDER_TARGET_PROPERTIES, D2D1_RENDER_TARGET_TYPE_DEFAULT, D2D1_RENDER_TARGET_USAGE_NONE,
};
use windows::Win32::Graphics::Dxgi::Common::{DXGI_FORMAT_B8G8R8A8_UNORM, DXGI_FORMAT_UNKNOWN};
use windows::Win32::Graphics::Gdi::{EnumDisplayMonitors, GetMonitorInfoW, MapWindowPoints, HDC, HMONITOR, MONITORINFO};
use windows::Win32::System::Threading::GetCurrentThreadId;
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DestroyWindow, DispatchMessageW, EnumWindows, FindWindowExW, FindWindowW, GetClientRect,
    GetMessageW, GetWindowLongPtrW, PeekMessageW, PostThreadMessageW, RegisterClassW, SendMessageTimeoutW, SetLayeredWindowAttributes,
    SetParent, SetWindowLongPtrW, SetWindowPos, ShowWindow, TranslateMessage, GWL_STYLE, HTTRANSPARENT, LWA_ALPHA, MSG, PM_REMOVE,
    SMTO_NORMAL, SWP_NOACTIVATE, SW_SHOWNA, WM_APP, WM_NCHITTEST, WM_PAINT, WM_QUIT, WNDCLASSW, WS_CHILD, WS_EX_LAYERED,
    WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW, WS_POPUP,
};

/// Trazas de diagnóstico (solo en tests).
macro_rules! trace {
    ($($arg:tt)*) => {
        if cfg!(test) {
            eprintln!("[lienzo {:?}] {}", std::thread::current().name().unwrap_or("?"), format!($($arg)*));
        }
    };
}

/// Mensaje de hilo para despertar al lienzo cuando hay órdenes pendientes.
const WM_APP_CANVAS_WAKE: u32 = WM_APP + 70;
/// Mensajes atendidos por vuelta del bucle antes de mirar las órdenes y dibujar.
const MAX_MESSAGES_PER_TURN: usize = 32;
/// Mensaje de hilo: volver a colocar el lienzo justo detrás de los iconos.
const WM_APP_CANVAS_RESTACK: u32 = WM_APP + 71;
const CANVAS_CLASS: &str = "SWEPR_CanvasWindowClass";
/// D2DERR_RECREATE_TARGET: se perdió el dispositivo gráfico (driver reiniciado, escritorio remoto...).
const D2DERR_RECREATE_TARGET: i32 = 0x8899000Cu32 as i32;

/// Dónde se muestra una imagen.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Target {
    /// Todos los monitores (en "Extender", una sola imagen sobre todos).
    All,
    /// Solo el monitor con este rectángulo (coordenadas de pantalla).
    Monitor(RECT),
}

/// Imagen decodificada lista para subir a la GPU.
struct Frame {
    pixels: Vec<u8>,
    width: u32,
    height: u32,
    path: PathBuf,
    target: Target,
    fit: FitMode,
    duration: Duration,
    effect: Transition,
}

struct DecodeJob {
    path: PathBuf,
    target: Target,
    fit: FitMode,
    duration: Duration,
    effect: Transition,
    /// Tamaño del área que cubrirá la imagen (para no decodificar más píxeles de los necesarios).
    area_size: (u32, u32),
}

enum Command {
    Show(Frame),
    Quit,
}

/// Rectángulos de los monitores en coordenadas de pantalla, de izquierda a derecha.
pub fn monitor_rects() -> Vec<RECT> {
    unsafe extern "system" fn collect(monitor: HMONITOR, _: HDC, _: *mut RECT, data: LPARAM) -> BOOL {
        let rects = &mut *(data.0 as *mut Vec<RECT>);
        let mut info = MONITORINFO { cbSize: std::mem::size_of::<MONITORINFO>() as u32, ..Default::default() };
        if GetMonitorInfoW(monitor, &mut info).as_bool() {
            rects.push(info.rcMonitor);
        }
        BOOL(1)
    }
    let mut rects: Vec<RECT> = Vec::new();
    unsafe {
        let _ = EnumDisplayMonitors(None, None, Some(collect), LPARAM(&mut rects as *mut _ as isize));
    }
    rects.sort_by_key(|r| (r.left, r.top));
    rects
}

fn bounding(rects: &[RECT]) -> RECT {
    rects.iter().fold(rects.first().copied().unwrap_or_default(), |acc, r| RECT {
        left: acc.left.min(r.left),
        top: acc.top.min(r.top),
        right: acc.right.max(r.right),
        bottom: acc.bottom.max(r.bottom),
    })
}

/// Lienzo en ejecución. Al soltarlo se cierra la ventana y queda a la vista el fondo de Windows.
pub struct Canvas {
    commands: Sender<Command>,
    decoder: Sender<DecodeJob>,
    thread_id: u32,
    thread: Option<std::thread::JoinHandle<()>>,
    alive: Arc<AtomicBool>,
}

impl Canvas {
    /// Crea la ventana detrás de los iconos. Falla si no se encuentra el escritorio.
    pub fn start() -> Result<Self, String> {
        let (command_tx, command_rx) = channel::<Command>();
        let (decode_tx, decode_rx) = channel::<DecodeJob>();
        let (ready_tx, ready_rx) = channel::<Result<u32, String>>();
        let alive = Arc::new(AtomicBool::new(true));

        // Decodificador.
        let decoder_commands = command_tx.clone();
        std::thread::Builder::new()
            .name("swepr-canvas-decoder".into())
            .spawn(move || decoder_loop(decode_rx, decoder_commands))
            .map_err(|e| e.to_string())?;

        let thread_alive = alive.clone();
        let thread_decoder = decode_tx.clone();
        let thread = std::thread::Builder::new()
            .name("swepr-canvas".into())
            .spawn(move || {
                let mut canvas = match CanvasWindow::attach(thread_decoder) {
                    Ok(canvas) => canvas,
                    Err(e) => {
                        let _ = ready_tx.send(Err(e));
                        thread_alive.store(false, Ordering::Release);
                        return;
                    }
                };
                trace!("enganchado; avisando");
                let _ = ready_tx.send(Ok(unsafe { GetCurrentThreadId() }));
                canvas.run(command_rx);
                thread_alive.store(false, Ordering::Release);
            })
            .map_err(|e| e.to_string())?;

        trace!("esperando enganche");
        match ready_rx.recv_timeout(Duration::from_secs(5)) {
            Ok(Ok(thread_id)) => {
                log_info!("Lienzo: enganchado detrás de los iconos del escritorio.");
                Ok(Self { commands: command_tx, decoder: decode_tx, thread_id, thread: Some(thread), alive })
            }
            Ok(Err(e)) => {
                let _ = thread.join();
                Err(e)
            }
            Err(_) => Err("el lienzo no respondió a tiempo".into()),
        }
    }

    /// ¿Sigue la ventana viva? (se destruye si el Explorador se reinicia).
    pub fn is_alive(&self) -> bool {
        self.alive.load(Ordering::Acquire)
    }

    /// Muestra una imagen con una transición de `duration` (0 = sin transición).
    /// "Aleatoria" se resuelve aquí, para que todos los monitores usen el mismo efecto.
    pub fn show(&self, path: &Path, target: Target, fit: FitMode, duration: Duration, transition: Transition) {
        let effect = transition.resolve(crate::winutil::random_below);
        let monitors = monitor_rects();
        let area = match target {
            Target::Monitor(rect) => rect,
            Target::All if fit == FitMode::Span => bounding(&monitors),
            // El mismo bitmap sirve a todos los monitores: se decodifica para el mayor.
            Target::All => monitors.iter().copied().max_by_key(|r| (r.right - r.left) * (r.bottom - r.top)).unwrap_or_default(),
        };
        let area_size = ((area.right - area.left).max(1) as u32, (area.bottom - area.top).max(1) as u32);
        let _ = self.decoder.send(DecodeJob { path: path.to_path_buf(), target, fit, duration, effect, area_size });
    }
}

impl Drop for Canvas {
    fn drop(&mut self) {
        trace!("cerrando lienzo");
        let _ = self.commands.send(Command::Quit);
        unsafe { let _ = PostThreadMessageW(self.thread_id, WM_APP_CANVAS_WAKE, WPARAM(0), LPARAM(0)); }
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
        trace!("lienzo cerrado");
        log_info!("Lienzo: cerrado.");
    }
}

// --- Decodificación (WIC) ---

fn decoder_loop(jobs: Receiver<DecodeJob>, commands: Sender<Command>) {
    use windows::Win32::System::Com::{CoInitializeEx, COINIT_MULTITHREADED};
    unsafe { let _ = CoInitializeEx(None, COINIT_MULTITHREADED); }
    let factory = match unsafe { wic_factory() } {
        Ok(factory) => factory,
        Err(e) => {
            log_error!("Lienzo: no se pudo crear WIC: {e}");
            return;
        }
    };
    // Si llegan varias seguidas (atajos repetidos) solo cuenta la última de cada destino.
    while let Ok(first) = jobs.recv() {
        let mut latest: Vec<DecodeJob> = vec![first];
        while let Ok(job) = jobs.try_recv() {
            if job.target == Target::All {
                latest.clear();
            } else {
                latest.retain(|pending| pending.target != job.target);
            }
            latest.push(job);
        }
        for job in latest {
            let started = Instant::now();
            trace!("decodificando {:?}", job.path.file_name().unwrap_or_default());
            match unsafe { decode(&factory, &job) } {
                Ok((pixels, width, height)) => {
                    crate::log_debug!("Lienzo: {:?} decodificada a {width}x{height} en {} ms", job.path.file_name().unwrap_or_default(), started.elapsed().as_millis());
                    let frame = Frame { pixels, width, height, path: job.path, target: job.target, fit: job.fit, duration: job.duration, effect: job.effect };
                    if commands.send(Command::Show(frame)).is_err() {
                        return;
                    }
                    wake_canvas();
                }
                Err(e) => {
                    trace!("no se pudo decodificar {:?}: {e}", job.path);
                    log_warn!("Lienzo: no se pudo abrir {:?}: {e}", job.path);
                }
            }
        }
    }
}

unsafe fn wic_factory() -> windows::core::Result<windows::Win32::Graphics::Imaging::IWICImagingFactory> {
    use windows::Win32::System::Com::{CoCreateInstance, CLSCTX_INPROC_SERVER};
    CoCreateInstance(&windows::Win32::Graphics::Imaging::CLSID_WICImagingFactory, None, CLSCTX_INPROC_SERVER)
}

/// Abre la imagen y la devuelve en BGRA premultiplicado, escalada a lo necesario para el área.
unsafe fn decode(factory: &windows::Win32::Graphics::Imaging::IWICImagingFactory, job: &DecodeJob) -> windows::core::Result<(Vec<u8>, u32, u32)> {
    use windows::Win32::Foundation::GENERIC_READ;
    use windows::Win32::Graphics::Imaging::*;
    let wide: Vec<u16> = job.path.as_os_str().to_string_lossy().encode_utf16().chain(std::iter::once(0)).collect();
    let decoder = factory.CreateDecoderFromFilename(PCWSTR(wide.as_ptr()), None, GENERIC_READ, WICDecodeMetadataCacheOnDemand)?;
    let frame = decoder.GetFrame(0)?;
    let (mut width, mut height) = (0u32, 0u32);
    frame.GetSize(&mut width, &mut height)?;
    let (target_w, target_h) = decode_size(job.fit, (width, height), job.area_size);
    let source: IWICBitmapSource = if (target_w, target_h) != (width, height) {
        let scaler = factory.CreateBitmapScaler()?;
        scaler.Initialize(&frame, target_w, target_h, WICBitmapInterpolationModeFant)?;
        windows::core::Interface::cast(&scaler)?
    } else {
        windows::core::Interface::cast(&frame)?
    };
    let converter = factory.CreateFormatConverter()?;
    converter.Initialize(&source, &GUID_WICPixelFormat32bppPBGRA, WICBitmapDitherTypeNone, None, 0.0, WICBitmapPaletteTypeMedianCut)?;
    let stride = target_w * 4;
    let mut pixels = vec![0u8; (stride * target_h) as usize];
    converter.CopyPixels(std::ptr::null(), stride, &mut pixels)?;
    Ok((pixels, target_w, target_h))
}

// --- Ventana del lienzo ---

/// Hilo del lienzo activo (para despertarlo desde el decodificador).
static CANVAS_THREAD: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);

/// Vuelve a colocar el lienzo detrás de los iconos (tras cambiar el fondo real de Windows,
/// el Explorador puede reordenar sus capas). No hace nada si no hay lienzo.
pub fn restack() {
    let thread = CANVAS_THREAD.load(Ordering::Acquire);
    if thread != 0 {
        unsafe { let _ = PostThreadMessageW(thread, WM_APP_CANVAS_RESTACK, WPARAM(0), LPARAM(0)); }
    }
}

fn wake_canvas() {
    let thread = CANVAS_THREAD.load(Ordering::Acquire);
    if thread != 0 {
        unsafe { let _ = PostThreadMessageW(thread, WM_APP_CANVAS_WAKE, WPARAM(0), LPARAM(0)); }
    }
}

/// Cuánto se amplía la imagen nueva al empezar la transición "acercar" (6 %).
const ZOOM_START: f32 = 0.06;

/// Cómo se dibuja una imagen en un fotograma: opacidad, escala y desplazamiento.
#[derive(Clone, Copy)]
struct Placement {
    opacity: f32,
    scale: f32,
    offset_x: f32,
}

impl Default for Placement {
    fn default() -> Self {
        Self { opacity: 1.0, scale: 1.0, offset_x: 0.0 }
    }
}

/// Imagen ya en la GPU.
#[derive(Clone)]
struct Layer {
    bitmap: ID2D1Bitmap,
    size: (f32, f32),
    fit: FitMode,
    path: PathBuf,
}

/// Lo que se ve en un monitor.
struct Slot {
    /// Rectángulo del monitor en coordenadas de cliente del lienzo.
    area: Area,
    /// Rectángulo en pantalla (para emparejar con `Target::Monitor`).
    screen: RECT,
    current: Option<Layer>,
    previous: Option<Layer>,
    started: Instant,
    duration: Duration,
    effect: Transition,
}

impl Slot {
    /// Progreso de la transición (1 = terminada).
    fn progress(&self, now: Instant) -> f32 {
        if self.previous.is_none() || self.duration.is_zero() {
            return 1.0;
        }
        (now.duration_since(self.started).as_secs_f32() / self.duration.as_secs_f32()).min(1.0)
    }
}

struct CanvasWindow {
    hwnd: HWND,
    host: DesktopHost,
    factory: ID2D1Factory,
    target: Option<ID2D1HwndRenderTarget>,
    /// Capa para las máscaras (transición circular); depende del destino.
    layer: Option<ID2D1Layer>,
    slots: Vec<Slot>,
    /// Todos los monitores juntos, en coordenadas de cliente (para "Extender").
    virtual_area: Area,
    shown: bool,
    decoder: Sender<DecodeJob>,
}

unsafe extern "system" fn canvas_wnd_proc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    match msg {
        // Nunca recibe clics: van a los iconos del escritorio.
        WM_NCHITTEST => LRESULT(HTTRANSPARENT as isize),
        windows::Win32::UI::WindowsAndMessaging::WM_TIMER if wparam.0 == STACKING_TIMER_ID => {
            CHECK_STACKING.store(true, Ordering::Release);
            LRESULT(0)
        }
        WM_PAINT => {
            // Se repinta en el bucle del hilo; aquí solo se da por pintada la ventana.
            // BeginPaint/EndPaint vacían la región pendiente (ValidateRect no basta en una
            // ventana hija "layered" de otro proceso y WM_PAINT se repetía sin fin).
            let mut ps = windows::Win32::Graphics::Gdi::PAINTSTRUCT::default();
            let _ = windows::Win32::Graphics::Gdi::BeginPaint(hwnd, &mut ps);
            let _ = windows::Win32::Graphics::Gdi::EndPaint(hwnd, &ps);
            NEEDS_PAINT.store(true, Ordering::Release);
            LRESULT(0)
        }
        _ => DefWindowProcW(hwnd, msg, wparam, lparam),
    }
}

static NEEDS_PAINT: AtomicBool = AtomicBool::new(false);
/// Toca comprobar el orden de las capas (temporizador de la ventana del lienzo).
static CHECK_STACKING: AtomicBool = AtomicBool::new(false);
const STACKING_TIMER_ID: usize = 1;
const STACKING_CHECK_MS: u32 = 1000;

/// Dónde enganchar la ventana: padre, y ventana sobre la que debe quedar (la de los iconos).
struct DesktopHost {
    parent: HWND,
    insert_after: Option<HWND>,
    layered: bool,
}

/// Busca el sitio detrás de los iconos. Soporta las dos estructuras del escritorio:
/// - Windows 11 24H2 y posteriores: los iconos (SHELLDLL_DefView) y el fondo (WorkerW) son
///   hijos de Progman; el lienzo va entre ambos y debe ser una ventana "layered".
/// - Versiones anteriores: el fondo es una WorkerW de nivel superior justo detrás de la ventana
///   que contiene los iconos; el lienzo va dentro de esa WorkerW.
unsafe fn find_desktop_host() -> Result<DesktopHost, String> {
    let progman = FindWindowW(w!("Progman"), PCWSTR::null()).map_err(|_| "no se encontró Progman".to_string())?;
    trace!("Progman {:?}", progman);
    // Pedir al escritorio que cree la capa WorkerW (la misma petición que hace el fundido de Windows).
    let mut result = 0usize;
    let _ = SendMessageTimeoutW(progman, 0x052C, WPARAM(0xD), LPARAM(1), SMTO_NORMAL, 1000, Some(&mut result));
    let _ = SendMessageTimeoutW(progman, 0x052C, WPARAM(0), LPARAM(0), SMTO_NORMAL, 1000, Some(&mut result));

    if let Ok(defview) = FindWindowExW(progman, None, w!("SHELLDLL_DefView"), PCWSTR::null()) {
        if FindWindowExW(progman, None, w!("WorkerW"), PCWSTR::null()).is_ok() {
            return Ok(DesktopHost { parent: progman, insert_after: Some(defview), layered: true });
        }
    }

    unsafe extern "system" fn find(hwnd: HWND, data: LPARAM) -> BOOL {
        if FindWindowExW(hwnd, None, w!("SHELLDLL_DefView"), PCWSTR::null()).is_ok() {
            if let Ok(worker) = FindWindowExW(None, hwnd, w!("WorkerW"), PCWSTR::null()) {
                *(data.0 as *mut HWND) = worker;
                return BOOL(0);
            }
        }
        BOOL(1)
    }
    let mut worker = HWND::default();
    let _ = EnumWindows(Some(find), LPARAM(&mut worker as *mut _ as isize));
    if worker.is_invalid() {
        return Err("no se encontró la capa del fondo del escritorio (WorkerW)".into());
    }
    Ok(DesktopHost { parent: worker, insert_after: None, layered: false })
}

/// Coloca el lienzo cubriendo a su padre, justo debajo de los iconos.
unsafe fn place(hwnd: HWND, host: &DesktopHost, parent_rect: &RECT) {
    use windows::Win32::UI::WindowsAndMessaging::{HWND_BOTTOM, SWP_NOMOVE, SWP_NOSIZE};
    let _ = SetWindowPos(
        hwnd,
        host.insert_after.unwrap_or_default(),
        0, 0, parent_rect.right, parent_rect.bottom,
        SWP_NOACTIVATE,
    );
    if host.layered {
        // Escritorio 24H2+: la capa del fondo de Windows (WorkerW) va siempre al fondo, debajo
        // del lienzo. El Explorador la recrea al cambiar el fondo y la pone encima.
        let mut worker = FindWindowExW(host.parent, None, w!("WorkerW"), PCWSTR::null()).unwrap_or_default();
        while !worker.is_invalid() {
            let _ = SetWindowPos(worker, HWND_BOTTOM, 0, 0, 0, 0, SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE);
            worker = FindWindowExW(host.parent, worker, w!("WorkerW"), PCWSTR::null()).unwrap_or_default();
        }
    }
}

/// ¿Está el lienzo justo debajo de los iconos y sin ninguna capa del fondo encima?
unsafe fn is_stacked_correctly(hwnd: HWND, host: &DesktopHost) -> bool {
    use windows::Win32::UI::WindowsAndMessaging::{GetWindow, GW_HWNDPREV};
    let Some(icons) = host.insert_after else { return true; };
    GetWindow(hwnd, GW_HWNDPREV).map(|above| above == icons).unwrap_or(false)
}

impl CanvasWindow {
    fn restack(&self) {
        unsafe {
            let mut parent_rect = RECT::default();
            let _ = GetClientRect(self.host.parent, &mut parent_rect);
            place(self.hwnd, &self.host, &parent_rect);
        }
    }

    /// Comprobación periódica: si el Explorador movió las capas, se vuelven a ordenar.
    fn check_stacking(&self) {
        if !unsafe { is_stacked_correctly(self.hwnd, &self.host) } {
            log_info!("Lienzo: el Explorador reordenó el escritorio; se vuelve a colocar detrás de los iconos.");
            self.restack();
        }
    }

    fn attach(decoder: Sender<DecodeJob>) -> Result<Self, String> {
        unsafe {
            let host = find_desktop_host()?;
            trace!("host padre={:?} encima={:?} layered={}", host.parent, host.insert_after, host.layered);
            let hinstance = windows::Win32::System::LibraryLoader::GetModuleHandleW(None).map_err(|e| e.to_string())?;
            let class_name: Vec<u16> = CANVAS_CLASS.encode_utf16().chain(std::iter::once(0)).collect();
            let wc = WNDCLASSW {
                lpfnWndProc: Some(canvas_wnd_proc),
                hInstance: hinstance.into(),
                lpszClassName: PCWSTR(class_name.as_ptr()),
                ..Default::default()
            };
            let _ = RegisterClassW(&wc);

            let mut parent_rect = RECT::default();
            let _ = GetClientRect(host.parent, &mut parent_rect);
            let ex_style = WS_EX_NOACTIVATE | WS_EX_TOOLWINDOW | if host.layered { WS_EX_LAYERED } else { Default::default() };
            let hwnd = CreateWindowExW(
                ex_style,
                PCWSTR(class_name.as_ptr()),
                w!("SWEPR Canvas"),
                WS_POPUP,
                0, 0, parent_rect.right, parent_rect.bottom,
                None, None, hinstance, None,
            ).map_err(|e| format!("no se pudo crear la ventana del lienzo: {e}"))?;
            if host.layered {
                let _ = SetLayeredWindowAttributes(hwnd, windows::Win32::Foundation::COLORREF(0), 255, LWA_ALPHA);
            }
            trace!("ventana creada {:?}", hwnd);
            if SetParent(hwnd, host.parent).is_err() {
                let _ = DestroyWindow(hwnd);
                return Err("no se pudo colocar el lienzo en el escritorio".into());
            }
            let style = GetWindowLongPtrW(hwnd, GWL_STYLE);
            SetWindowLongPtrW(hwnd, GWL_STYLE, (style & !(WS_POPUP.0 as isize)) | WS_CHILD.0 as isize);
            trace!("SetParent hecho");
            place(hwnd, &host, &parent_rect);
            trace!("colocada {}x{}", parent_rect.right, parent_rect.bottom);

            let factory: ID2D1Factory = D2D1CreateFactory(D2D1_FACTORY_TYPE_SINGLE_THREADED, None)
                .map_err(|e| format!("Direct2D no disponible: {e}"))?;
            CANVAS_THREAD.store(GetCurrentThreadId(), Ordering::Release);
            let _ = windows::Win32::UI::WindowsAndMessaging::SetTimer(hwnd, STACKING_TIMER_ID, STACKING_CHECK_MS, None);
            let mut canvas = Self {
                hwnd,
                host,
                factory,
                target: None,
                layer: None,
                slots: Vec::new(),
                virtual_area: Area::default(),
                shown: false,
                decoder,
            };
            canvas.layout_slots();
            trace!("{} huecos; creando destino D2D", canvas.slots.len());
            canvas.create_target().map_err(|e| format!("no se pudo crear el destino Direct2D: {e}"))?;
            log_info!(
                "Lienzo: {} monitores, padre {:?} ({}).",
                canvas.slots.len(),
                canvas.host.parent,
                if canvas.host.layered { "Progman, escritorio 24H2+" } else { "WorkerW, escritorio clásico" }
            );
            Ok(canvas)
        }
    }

    /// Un hueco por monitor, en coordenadas de cliente del lienzo.
    fn layout_slots(&mut self) {
        let screens = monitor_rects();
        let to_client = |rect: RECT| unsafe {
            let mut points = [
                windows::Win32::Foundation::POINT { x: rect.left, y: rect.top },
                windows::Win32::Foundation::POINT { x: rect.right, y: rect.bottom },
            ];
            MapWindowPoints(None, self.hwnd, &mut points);
            Area::new(points[0].x as f32, points[0].y as f32, points[1].x as f32, points[1].y as f32)
        };
        self.virtual_area = to_client(bounding(&screens));
        self.slots = screens
            .into_iter()
            .map(|screen| Slot {
                area: to_client(screen),
                screen,
                current: None,
                previous: None,
                started: Instant::now(),
                duration: Duration::ZERO,
                effect: Transition::Fade,
            })
            .collect();
    }

    unsafe fn create_target(&mut self) -> windows::core::Result<()> {
        let mut rect = RECT::default();
        let _ = GetClientRect(self.hwnd, &mut rect);
        let properties = D2D1_RENDER_TARGET_PROPERTIES {
            r#type: D2D1_RENDER_TARGET_TYPE_DEFAULT,
            pixelFormat: D2D1_PIXEL_FORMAT { format: DXGI_FORMAT_UNKNOWN, alphaMode: D2D1_ALPHA_MODE_UNKNOWN },
            // 96 DPI: un DIP es un píxel, igual que las coordenadas de los monitores.
            dpiX: 96.0,
            dpiY: 96.0,
            usage: D2D1_RENDER_TARGET_USAGE_NONE,
            minLevel: D2D1_FEATURE_LEVEL_DEFAULT,
        };
        let hwnd_properties = D2D1_HWND_RENDER_TARGET_PROPERTIES {
            hwnd: self.hwnd,
            pixelSize: D2D_SIZE_U { width: rect.right.max(1) as u32, height: rect.bottom.max(1) as u32 },
            // Sin opciones: EndDraw espera al refresco de pantalla (animación sin saltos).
            presentOptions: D2D1_PRESENT_OPTIONS_NONE,
        };
        let target = self.factory.CreateHwndRenderTarget(&properties, &hwnd_properties)?;
        self.layer = target.CreateLayer(None).ok();
        self.target = Some(target);
        Ok(())
    }

    /// Bucle del hilo: atiende mensajes y órdenes, y dibuja mientras haya transiciones.
    fn run(&mut self, commands: Receiver<Command>) {
        let mut msg = MSG::default();
        loop {
            let animating = self.is_animating();
            unsafe {
                if animating {
                    for _ in 0..MAX_MESSAGES_PER_TURN {
                        if !PeekMessageW(&mut msg, None, 0, 0, PM_REMOVE).as_bool() { break; }
                        if msg.message == WM_QUIT { return self.close(); }
                        self.dispatch(&msg);
                    }
                } else {
                    // Sin animación se duerme hasta el próximo mensaje u orden.
                    if !GetMessageW(&mut msg, None, 0, 0).as_bool() { return self.close(); }
                    self.dispatch(&msg);
                    for _ in 0..MAX_MESSAGES_PER_TURN {
                        if !PeekMessageW(&mut msg, None, 0, 0, PM_REMOVE).as_bool() { break; }
                        if msg.message == WM_QUIT { return self.close(); }
                        self.dispatch(&msg);
                    }
                }
                if !windows::Win32::UI::WindowsAndMessaging::IsWindow(self.hwnd).as_bool() {
                    // El Explorador se reinició y se llevó la ventana consigo.
                    log_warn!("Lienzo: la ventana desapareció (¿se reinició el Explorador?).");
                    return;
                }
            }
            if CHECK_STACKING.swap(false, Ordering::AcqRel) {
                self.check_stacking();
            }
            let mut dirty = NEEDS_PAINT.swap(false, Ordering::AcqRel);
            while let Ok(command) = commands.try_recv() {
                match command {
                    Command::Quit => {
                        trace!("orden de salir");
                        return self.close();
                    }
                    Command::Show(frame) => {
                        self.show(frame);
                        dirty = true;
                    }
                }
            }
            if dirty || self.is_animating() {
                let started = Instant::now();
                self.render();
                trace!("fotograma en {} ms", started.elapsed().as_millis());
                // Si el refresco no frena EndDraw (pantalla apagada, escritorio tapado), no quemar CPU.
                let spent = started.elapsed();
                if self.is_animating() && spent < Duration::from_millis(6) {
                    std::thread::sleep(Duration::from_millis(8) - spent);
                }
            }
        }
    }

    fn dispatch(&self, msg: &MSG) {
        if msg.hwnd.is_invalid() && msg.message == WM_APP_CANVAS_RESTACK {
            self.restack();
            return;
        }
        unsafe {
            let _ = TranslateMessage(msg);
            DispatchMessageW(msg);
        }
    }

    fn is_animating(&self) -> bool {
        let now = Instant::now();
        self.slots.iter().any(|slot| slot.progress(now) < 1.0)
    }

    fn show(&mut self, frame: Frame) {
        trace!("mostrar {:?} {}x{}", frame.path.file_name().unwrap_or_default(), frame.width, frame.height);
        let Some(target) = self.target.clone() else { return; };
        let properties = D2D1_BITMAP_PROPERTIES {
            pixelFormat: D2D1_PIXEL_FORMAT { format: DXGI_FORMAT_B8G8R8A8_UNORM, alphaMode: D2D1_ALPHA_MODE_PREMULTIPLIED },
            dpiX: 96.0,
            dpiY: 96.0,
        };
        let bitmap = unsafe {
            target.CreateBitmap(
                D2D_SIZE_U { width: frame.width, height: frame.height },
                Some(frame.pixels.as_ptr() as *const _),
                frame.width * 4,
                &properties,
            )
        };
        let bitmap = match bitmap {
            Ok(bitmap) => bitmap,
            Err(e) => {
                log_error!("Lienzo: no se pudo subir la imagen a la GPU: {e}");
                return;
            }
        };
        drop(frame.pixels);
        let layer = Layer { bitmap, size: (frame.width as f32, frame.height as f32), fit: frame.fit, path: frame.path };
        let now = Instant::now();
        let overlap = |a: &RECT, b: &RECT| {
            let w = (a.right.min(b.right) - a.left.max(b.left)).max(0) as i64;
            let h = (a.bottom.min(b.bottom) - a.top.max(b.top)).max(0) as i64;
            w * h
        };
        let chosen = match frame.target {
            Target::All => None,
            Target::Monitor(rect) => self.slots.iter().enumerate()
                .max_by_key(|(_, slot)| overlap(&slot.screen, &rect))
                .filter(|(_, slot)| overlap(&slot.screen, &rect) > 0)
                .map(|(index, _)| index),
        };
        if matches!(frame.target, Target::Monitor(_)) && chosen.is_none() {
            log_warn!("Lienzo: ningún monitor coincide con el destino; se ignora la imagen.");
            return;
        }
        for (index, slot) in self.slots.iter_mut().enumerate() {
            if chosen.is_some_and(|chosen| chosen != index) { continue; }
            // Si llega otra imagen a mitad de transición, se parte de la que ya estaba entrando.
            slot.previous = if frame.duration.is_zero() { None } else { slot.current.take() };
            slot.current = Some(layer.clone());
            slot.started = now;
            slot.duration = frame.duration;
            slot.effect = frame.effect;
        }
    }

    fn render(&mut self) {
        let Some(target) = self.target.clone() else { return; };
        let now = Instant::now();
        unsafe {
            target.BeginDraw();
            target.Clear(Some(&D2D1_COLOR_F { r: 0.0, g: 0.0, b: 0.0, a: 1.0 }));
            for slot in &self.slots {
                let clip = slot.area.to_d2d();
                target.PushAxisAlignedClip(&clip, D2D1_ANTIALIAS_MODE_ALIASED);
                let progress = slot.progress(now);
                // Curva suave (entra y sale despacio).
                let eased = progress * progress * (3.0 - 2.0 * progress);
                match (slot.previous.as_ref().filter(|_| progress < 1.0), &slot.current) {
                    (Some(previous), Some(current)) => self.draw_transition(&target, slot, previous, current, eased),
                    (None, Some(current)) => self.draw_layer(&target, current, slot.area, Placement::default()),
                    _ => {}
                }
                target.PopAxisAlignedClip();
            }
            match target.EndDraw(None, None) {
                Ok(()) => {}
                Err(e) if e.code().0 == D2DERR_RECREATE_TARGET => {
                    log_warn!("Lienzo: dispositivo gráfico perdido, se recrea.");
                    self.recover_device();
                    return;
                }
                Err(e) => log_warn!("Lienzo: error al dibujar: {e}"),
            }
        }
        // Las transiciones terminadas sueltan la imagen anterior (memoria de GPU).
        let mut released = false;
        for slot in self.slots.iter_mut() {
            if slot.previous.is_some() && slot.progress(now) >= 1.0 {
                slot.previous = None;
                released = true;
            }
        }
        if released && !self.is_animating() {
            self.trim();
        }
        if !self.shown {
            self.shown = true;
            unsafe { let _ = ShowWindow(self.hwnd, SW_SHOWNA); }
        }
    }

    /// Al quedar quieto, devuelve al sistema la memoria que Direct2D y el driver guardan en caché
    /// (en gráficas integradas sale de la RAM, así que se nota).
    fn trim(&self) {
        use windows::core::Interface;
        let Some(target) = self.target.as_ref() else { return; };
        unsafe {
            let Ok(context) = target.cast::<windows::Win32::Graphics::Direct2D::ID2D1DeviceContext>() else { return; };
            let Ok(device) = context.GetDevice() else { return; };
            device.ClearResources(0);
            if let Ok(device2) = device.cast::<windows::Win32::Graphics::Direct2D::ID2D1Device2>() {
                if let Ok(dxgi) = device2.GetDxgiDevice() {
                    if let Ok(dxgi3) = dxgi.cast::<windows::Win32::Graphics::Dxgi::IDXGIDevice3>() {
                        dxgi3.Trim();
                    }
                }
            }
        }
        trace!("memoria de la GPU recortada");
    }

    unsafe fn draw_layer(&self, target: &ID2D1HwndRenderTarget, layer: &Layer, area: Area, placement: Placement) {
        let (cx, cy) = ((area.left + area.right) / 2.0, (area.top + area.bottom) / 2.0);
        for rect in layout_rects(layer.fit, layer.size, area, self.virtual_area) {
            // Escala alrededor del centro del monitor y desplazamiento horizontal.
            let scaled = Area::new(
                cx + (rect.left - cx) * placement.scale + placement.offset_x,
                cy + (rect.top - cy) * placement.scale,
                cx + (rect.right - cx) * placement.scale + placement.offset_x,
                cy + (rect.bottom - cy) * placement.scale,
            );
            target.DrawBitmap(&layer.bitmap, Some(&scaled.to_d2d()), placement.opacity, D2D1_BITMAP_INTERPOLATION_MODE_LINEAR, None);
        }
    }

    /// Un fotograma de la transición de `previous` a `current` (`t` va de 0 a 1, ya suavizado).
    unsafe fn draw_transition(&self, target: &ID2D1HwndRenderTarget, slot: &Slot, previous: &Layer, current: &Layer, t: f32) {
        let area = slot.area;
        let still = Placement::default();
        match slot.effect {
            Transition::Fade | Transition::Random => {
                self.draw_layer(target, previous, area, still);
                self.draw_layer(target, current, area, Placement { opacity: t, ..still });
            }
            Transition::Slide => {
                let width = area.width();
                self.draw_layer(target, previous, area, Placement { offset_x: -width * t, ..still });
                self.draw_layer(target, current, area, Placement { offset_x: width * (1.0 - t), ..still });
            }
            Transition::Zoom => {
                self.draw_layer(target, previous, area, still);
                self.draw_layer(target, current, area, Placement { opacity: t, scale: 1.0 + ZOOM_START * (1.0 - t), ..still });
            }
            Transition::Wipe => {
                self.draw_layer(target, previous, area, still);
                let edge = area.left + area.width() * t;
                target.PushAxisAlignedClip(&Area::new(area.left, area.top, edge, area.bottom).to_d2d(), D2D1_ANTIALIAS_MODE_ALIASED);
                self.draw_layer(target, current, area, still);
                target.PopAxisAlignedClip();
            }
            Transition::Circle => {
                self.draw_layer(target, previous, area, still);
                let center = D2D_POINT_2F { x: (area.left + area.right) / 2.0, y: (area.top + area.bottom) / 2.0 };
                // Al final el círculo cubre hasta las esquinas.
                let radius = (area.width().hypot(area.height()) / 2.0) * t;
                let geometry = self.factory.CreateEllipseGeometry(&D2D1_ELLIPSE { point: center, radiusX: radius, radiusY: radius });
                match (geometry, self.layer.as_ref()) {
                    (Ok(geometry), Some(layer)) => {
                        let parameters = D2D1_LAYER_PARAMETERS {
                            contentBounds: area.to_d2d(),
                            geometricMask: std::mem::ManuallyDrop::new(Some(geometry.into())),
                            maskAntialiasMode: D2D1_ANTIALIAS_MODE_PER_PRIMITIVE,
                            maskTransform: windows::Foundation::Numerics::Matrix3x2::identity(),
                            opacity: 1.0,
                            opacityBrush: std::mem::ManuallyDrop::new(None),
                            layerOptions: D2D1_LAYER_OPTIONS_NONE,
                        };
                        target.PushLayer(&parameters, layer);
                        self.draw_layer(target, current, area, still);
                        target.PopLayer();
                        // Devolver la referencia a la geometría que guardaba la estructura.
                        drop(std::mem::ManuallyDrop::into_inner(parameters.geometricMask));
                    }
                    // Sin capa (no debería pasar): fundido.
                    _ => self.draw_layer(target, current, area, Placement { opacity: t, ..still }),
                }
            }
        }
    }

    /// Tras perder el dispositivo: nuevo destino y volver a pedir las imágenes visibles.
    fn recover_device(&mut self) {
        self.target = None;
        self.layer = None;
        let mut pending: HashMap<PathBuf, (Vec<RECT>, FitMode)> = HashMap::new();
        for slot in self.slots.iter_mut() {
            slot.previous = None;
            if let Some(layer) = slot.current.take() {
                pending.entry(layer.path).or_insert_with(|| (Vec::new(), layer.fit)).0.push(slot.screen);
            }
        }
        if let Err(e) = unsafe { self.create_target() } {
            log_error!("Lienzo: no se pudo recrear Direct2D: {e}");
            return;
        }
        for (path, (screens, fit)) in pending {
            for screen in screens {
                let size = ((screen.right - screen.left).max(1) as u32, (screen.bottom - screen.top).max(1) as u32);
                let _ = self.decoder.send(DecodeJob { path: path.clone(), target: Target::Monitor(screen), fit, duration: Duration::ZERO, effect: Transition::Fade, area_size: size });
            }
        }
    }

    fn close(&mut self) {
        CANVAS_THREAD.store(0, Ordering::Release);
        self.slots.clear();
        self.layer = None;
        self.target = None;
        unsafe {
            if windows::Win32::UI::WindowsAndMessaging::IsWindow(self.hwnd).as_bool() {
                let _ = DestroyWindow(self.hwnd);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Demo en el escritorio real: muestra dos imágenes con fundido y retira el lienzo.
    /// No cambia el fondo de Windows.
    /// `SWEPR_CANVAS_A=a.png SWEPR_CANVAS_B=b.png cargo test canvas_live_demo -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn canvas_live_demo() {
        std::thread::spawn(|| {
            std::thread::sleep(Duration::from_secs(20));
            eprintln!("VIGILANTE: la demo tardó demasiado; se cierra el proceso.");
            std::process::exit(3);
        });
        let a = PathBuf::from(std::env::var("SWEPR_CANVAS_A").expect("SWEPR_CANVAS_A"));
        let b = PathBuf::from(std::env::var("SWEPR_CANVAS_B").expect("SWEPR_CANVAS_B"));
        let canvas = Canvas::start().expect("no se pudo enganchar el lienzo");
        let fade = Duration::from_millis(1000);
        canvas.show(&a, Target::All, FitMode::Fill, Duration::ZERO, Transition::Fade);
        std::thread::sleep(Duration::from_millis(2500));
        canvas.show(&b, Target::All, FitMode::Fill, fade, Transition::Fade);
        std::thread::sleep(Duration::from_millis(3000));
        canvas.show(&a, Target::All, FitMode::Fill, fade, Transition::Fade);
        std::thread::sleep(Duration::from_millis(3000));
        assert!(canvas.is_alive(), "la ventana del lienzo desapareció");
    }
}

#[cfg(test)]
mod memory_probe {
    use super::*;

    fn private_mb() -> u64 {
        use windows::Win32::System::ProcessStatus::{GetProcessMemoryInfo, PROCESS_MEMORY_COUNTERS};
        use windows::Win32::System::Threading::GetCurrentProcess;
        let mut counters = PROCESS_MEMORY_COUNTERS::default();
        unsafe {
            let _ = GetProcessMemoryInfo(GetCurrentProcess(), &mut counters, std::mem::size_of::<PROCESS_MEMORY_COUNTERS>() as u32);
        }
        counters.PagefileUsage as u64 / (1024 * 1024)
    }

    /// Memoria privada en cada paso del lienzo. `SWEPR_CANVAS_DIR=<carpeta con imágenes>`.
    #[test]
    #[ignore]
    fn canvas_memory_probe() {
        std::thread::spawn(|| {
            std::thread::sleep(Duration::from_secs(30));
            eprintln!("VIGILANTE: se cierra el proceso.");
            std::process::exit(3);
        });
        let dir = std::env::var("SWEPR_CANVAS_DIR").expect("SWEPR_CANVAS_DIR");
        let mut images: Vec<PathBuf> = std::fs::read_dir(dir).unwrap().flatten().map(|e| e.path()).collect();
        images.sort();
        eprintln!("MEM inicio: {} MB", private_mb());
        let canvas = Canvas::start().expect("lienzo");
        std::thread::sleep(Duration::from_millis(300));
        eprintln!("MEM lienzo enganchado (Direct2D listo): {} MB", private_mb());
        for (i, image) in images.iter().enumerate() {
            canvas.show(image, Target::All, FitMode::Fill, Duration::from_millis(400), Transition::Fade);
            std::thread::sleep(Duration::from_millis(1200));
            eprintln!("MEM tras imagen {}: {} MB", i + 1, private_mb());
        }
        std::thread::sleep(Duration::from_millis(1500));
        eprintln!("MEM en reposo: {} MB", private_mb());
        drop(canvas);
        std::thread::sleep(Duration::from_millis(300));
        eprintln!("MEM lienzo cerrado: {} MB", private_mb());
    }
}

#[cfg(test)]
mod decode_probe {
    use super::*;

    /// Memoria privada al decodificar con WIC sin lienzo. `SWEPR_CANVAS_DIR=<carpeta>`.
    #[test]
    #[ignore]
    fn decode_memory_probe() {
        use windows::Win32::System::Com::{CoInitializeEx, COINIT_MULTITHREADED};
        let mb = || {
            use windows::Win32::System::ProcessStatus::{GetProcessMemoryInfo, PROCESS_MEMORY_COUNTERS};
            use windows::Win32::System::Threading::GetCurrentProcess;
            let mut c = PROCESS_MEMORY_COUNTERS::default();
            unsafe { let _ = GetProcessMemoryInfo(GetCurrentProcess(), &mut c, std::mem::size_of::<PROCESS_MEMORY_COUNTERS>() as u32); }
            c.PagefileUsage / (1024 * 1024)
        };
        let dir = std::env::var("SWEPR_CANVAS_DIR").expect("SWEPR_CANVAS_DIR");
        let images: Vec<PathBuf> = std::fs::read_dir(dir).unwrap().flatten().map(|e| e.path()).collect();
        unsafe { let _ = CoInitializeEx(None, COINIT_MULTITHREADED); }
        eprintln!("DEC inicio: {} MB", mb());
        let factory = unsafe { wic_factory() }.unwrap();
        eprintln!("DEC WIC creado: {} MB", mb());
        for (i, path) in images.iter().enumerate() {
            let job = DecodeJob { path: path.clone(), target: Target::All, fit: FitMode::Fill, duration: Duration::ZERO, effect: Transition::Fade, area_size: (1920, 1080) };
            let (pixels, w, h) = unsafe { decode(&factory, &job) }.unwrap();
            eprintln!("DEC imagen {} ({w}x{h}, {} MB de píxeles): {} MB", i + 1, pixels.len() / (1024 * 1024), mb());
            drop(pixels);
            eprintln!("DEC  tras soltar píxeles: {} MB", mb());
        }
        drop(factory);
        eprintln!("DEC WIC liberado: {} MB", mb());
    }
}
