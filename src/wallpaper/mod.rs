use crate::{log_info, log_warn};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{channel, Sender};
use std::sync::Mutex;
use windows::core::PCWSTR;
use windows::Win32::System::Com::{CoCreateInstance, CoInitializeEx, CoUninitialize, CLSCTX_ALL, CLSCTX_INPROC_SERVER, COINIT_APARTMENTTHREADED, COINIT_MULTITHREADED};
use windows::Win32::UI::Shell::{DesktopWallpaper, IDesktopWallpaper, DESKTOP_WALLPAPER_POSITION};

// IActiveDesktop no viene en el crate `windows`: se declara aquí solo hasta los métodos que se usan
// (el orden debe coincidir con la vtable de shlobj.h). Aplicar el fondo con esta interfaz hace que
// Windows lo cambie con un fundido; lo anima el Explorador, no SWEPR.
const CLSID_ACTIVE_DESKTOP: windows::core::GUID = windows::core::GUID::from_u128(0x75048700_ef1f_11d0_9888_006097deacf9);
const AD_APPLY_SAVE: u32 = 0x1;
const AD_APPLY_HTMLGEN: u32 = 0x2;
const AD_APPLY_REFRESH: u32 = 0x4;
const AD_APPLY_FORCE: u32 = 0x8;
const AD_APPLY_BUFFERED_REFRESH: u32 = 0x10;

#[allow(non_snake_case)]
mod active_desktop {
    use windows::core::PCWSTR;

    #[repr(C)]
    pub struct WallpaperOpt {
        pub size: u32,
        pub style: u32,
    }

    #[windows_core::interface("f490eb00-1240-11d1-9888-006097deacf9")]
    pub unsafe trait IActiveDesktop: windows_core::IUnknown {
        pub fn ApplyChanges(&self, flags: u32) -> windows::core::HRESULT;
        pub fn GetWallpaper(&self, wallpaper: windows::core::PWSTR, capacity: u32, flags: u32) -> windows::core::HRESULT;
        pub fn SetWallpaper(&self, wallpaper: PCWSTR, reserved: u32) -> windows::core::HRESULT;
        pub fn GetWallpaperOptions(&self, options: *mut WallpaperOpt, reserved: u32) -> windows::core::HRESULT;
        pub fn SetWallpaperOptions(&self, options: *const WallpaperOpt, reserved: u32) -> windows::core::HRESULT;
    }
}
use active_desktop::{IActiveDesktop, WallpaperOpt};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WallpaperStyle {
    Center,
    Tile,
    Stretch,
    Fit,
    Fill,
    Span,
}

impl From<crate::config::FitMode> for WallpaperStyle {
    fn from(mode: crate::config::FitMode) -> Self {
        use crate::config::FitMode;
        match mode {
            FitMode::Center => WallpaperStyle::Center,
            FitMode::Tile => WallpaperStyle::Tile,
            FitMode::Stretch => WallpaperStyle::Stretch,
            FitMode::Fit => WallpaperStyle::Fit,
            FitMode::Fill => WallpaperStyle::Fill,
            FitMode::Span => WallpaperStyle::Span,
        }
    }
}

impl WallpaperStyle {
    fn to_dw_style(self) -> i32 {
        // DESKTOP_WALLPAPER_POSITION: Center=0, Tile=1, Stretch=2, Fit=3, Fill=4, Span=5
        match self {
            WallpaperStyle::Center => 0,
            WallpaperStyle::Tile => 1,
            WallpaperStyle::Stretch => 2,
            WallpaperStyle::Fit => 3,
            WallpaperStyle::Fill => 4,
            WallpaperStyle::Span => 5,
        }
    }

    /// Equivalente WPSTYLE_* de IActiveDesktop.
    fn to_wp_style(self) -> u32 {
        // WPSTYLE: Center=0, Tile=1, Stretch=2, KeepAspect=3, CropToFit=4, Span=5
        match self {
            WallpaperStyle::Center => 0,
            WallpaperStyle::Tile => 1,
            WallpaperStyle::Stretch => 2,
            WallpaperStyle::Fit => 3,
            WallpaperStyle::Fill => 4,
            WallpaperStyle::Span => 5,
        }
    }
}

/// Convierte una cadena devuelta por COM (reservada con CoTaskMemAlloc) y la libera.
unsafe fn take_com_string(value: windows::core::PWSTR) -> Result<String, String> {
    if value.is_null() {
        return Err("cadena vacía".into());
    }
    let text = value.to_string().map_err(|e| e.to_string());
    windows::Win32::System::Com::CoTaskMemFree(Some(value.0 as *const _));
    text
}

/// Pone una imagen como fondo de la pantalla de bloqueo (en un hilo aparte: la API es asíncrona).
pub fn set_lock_screen(path: &Path) {
    let path = path.to_path_buf();
    std::thread::spawn(move || unsafe {
        let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
        let result = (|| -> windows::core::Result<()> {
            let file = windows::Storage::StorageFile::GetFileFromPathAsync(&windows::core::HSTRING::from(path.as_os_str()))?.get()?;
            windows::System::UserProfile::LockScreen::SetImageFileAsync(&windows::core::Interface::cast::<windows::Storage::IStorageFile>(&file)?)?.get()
        })();
        match result {
            Ok(()) => log_info!("Pantalla de bloqueo cambiada a {:?}", path),
            Err(e) => {
                crate::log_error!("No se pudo cambiar la pantalla de bloqueo: {e}");
                let text = crate::winutil::to_wide_chars(&format!("{}\n\n{e}", crate::i18n::t("Could not change the lock screen image.")));
                let title = crate::winutil::to_wide_chars("SWEPR");
                let _ = windows::Win32::UI::WindowsAndMessaging::MessageBoxW(
                    None,
                    crate::winutil::as_pcwstr(&text),
                    crate::winutil::as_pcwstr(&title),
                    windows::Win32::UI::WindowsAndMessaging::MB_ICONWARNING | windows::Win32::UI::WindowsAndMessaging::MB_SETFOREGROUND,
                );
            }
        }
        CoUninitialize();
    });
}

/// Hilo STA que aplica fondos con fundido. Si llegan varios seguidos (atajos repetidos),
/// solo se aplica el último.
static FADE_WORKER: Mutex<Option<Sender<(PathBuf, WallpaperStyle)>>> = Mutex::new(None);

fn fade_worker() -> Option<Sender<(PathBuf, WallpaperStyle)>> {
    let mut guard = FADE_WORKER.lock().ok()?;
    if guard.is_none() {
        let (sender, receiver) = channel::<(PathBuf, WallpaperStyle)>();
        std::thread::spawn(move || unsafe {
            let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
            while let Ok(mut request) = receiver.recv() {
                while let Ok(newer) = receiver.try_recv() { request = newer; }
                let (path, style) = request;
                match apply_with_fade(&path, style) {
                    Ok(()) => log_info!("Wallpaper aplicado con fundido y estilo {:?}", style),
                    Err(e) => {
                        log_warn!("Fundido no disponible ({e}); se aplica sin animación.");
                        let fallback = CoCreateInstance::<_, IDesktopWallpaper>(&DesktopWallpaper, None, CLSCTX_ALL)
                            .map_err(|e| e.to_string())
                            .and_then(|wallpaper| apply_instant(&wallpaper, &path, None, style));
                        if let Err(e) = fallback {
                            crate::log_error!("Error al aplicar wallpaper: {e}");
                        }
                    }
                }
            }
        });
        *guard = Some(sender);
    }
    guard.clone()
}

/// Petición de sincronización: imagen, monitor (None = todos), estilo y desde cuándo aplicarla.
type SyncRequest = (PathBuf, Option<String>, WallpaperStyle, std::time::Instant);

/// Hilo que aplica el fondo real de Windows sin animación mientras el lienzo lo tapa.
static SYNC_WORKER: Mutex<Option<Sender<SyncRequest>>> = Mutex::new(None);

/// Pone `path` como fondo real de Windows en segundo plano (sin fundido), pasado `delay`. Lo usa
/// el lienzo para que la vista de tareas, el color de énfasis y otras apps vean la misma imagen,
/// y para que el escritorio quede bien si SWEPR se cierra. Se espera a que acabe la transición:
/// al cambiar el fondo, el Explorador recrea su capa y podría tapar el efecto a medias.
pub fn sync_in_background(path: &Path, monitor_id: Option<String>, style: WallpaperStyle, delay: std::time::Duration) {
    let Ok(mut guard) = SYNC_WORKER.lock() else { return; };
    if guard.is_none() {
        let (sender, receiver) = channel::<SyncRequest>();
        std::thread::spawn(move || unsafe {
            let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
            let Ok(wallpaper) = CoCreateInstance::<_, IDesktopWallpaper>(&DesktopWallpaper, None, CLSCTX_ALL) else {
                crate::log_error!("Sincronización del fondo: IDesktopWallpaper no disponible.");
                return;
            };
            while let Ok(first) = receiver.recv() {
                // Solo la última petición de cada monitor (una para "todos" anula las demás),
                // esperando a que pase el plazo de la más reciente.
                let mut deadline = first.3;
                let mut pending: Vec<SyncRequest> = vec![first];
                loop {
                    let wait = deadline.saturating_duration_since(std::time::Instant::now());
                    match receiver.recv_timeout(wait) {
                        Ok(request) => {
                            deadline = deadline.max(request.3);
                            if request.1.is_none() {
                                pending.clear();
                            } else {
                                pending.retain(|p| p.1 != request.1);
                            }
                            pending.push(request);
                        }
                        Err(std::sync::mpsc::RecvTimeoutError::Timeout) => break,
                        Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => return,
                    }
                }
                for (path, monitor, style, _) in pending {
                    if let Err(e) = apply_instant(&wallpaper, &path, monitor.as_deref(), style) {
                        crate::log_error!("Sincronización del fondo fallida: {e}");
                    }
                }
                // Cambiar el fondo puede reordenar las capas del escritorio.
                crate::canvas::restack();
            }
        });
        *guard = Some(sender);
    }
    if let Some(sender) = guard.as_ref() {
        let _ = sender.send((path.to_path_buf(), monitor_id, style, std::time::Instant::now() + delay));
    }
}

/// Aplica el fondo a todos los monitores con el fundido nativo de Windows.
unsafe fn apply_with_fade(path: &Path, style: WallpaperStyle) -> Result<(), String> {
    use windows::Win32::UI::WindowsAndMessaging::{FindWindowW, SendMessageTimeoutW, SMTO_NORMAL};
    // Pedir al escritorio que cree su capa WorkerW: sin ella Windows cambia el fondo de golpe.
    if let Ok(progman) = FindWindowW(windows::core::w!("Progman"), None) {
        let _ = SendMessageTimeoutW(progman, 0x052C, windows::Win32::Foundation::WPARAM(0), windows::Win32::Foundation::LPARAM(0), SMTO_NORMAL, 1000, None);
    }
    let desktop: IActiveDesktop = CoCreateInstance(&CLSID_ACTIVE_DESKTOP, None, CLSCTX_INPROC_SERVER)
        .map_err(|e| format!("CoCreateInstance(ActiveDesktop) fallo: {e}"))?;
    let options = WallpaperOpt { size: std::mem::size_of::<WallpaperOpt>() as u32, style: style.to_wp_style() };
    desktop.SetWallpaperOptions(&options, 0).ok().map_err(|e| format!("SetWallpaperOptions fallo: {e}"))?;
    let wide: Vec<u16> = path.to_string_lossy().encode_utf16().chain(std::iter::once(0)).collect();
    desktop.SetWallpaper(PCWSTR(wide.as_ptr()), 0).ok().map_err(|e| format!("SetWallpaper fallo: {e}"))?;
    desktop
        .ApplyChanges(AD_APPLY_SAVE | AD_APPLY_HTMLGEN | AD_APPLY_REFRESH | AD_APPLY_FORCE | AD_APPLY_BUFFERED_REFRESH)
        .ok()
        .map_err(|e| format!("ApplyChanges fallo: {e}"))
}

/// Aplica el fondo sin animación con IDesktopWallpaper (todos los monitores o uno concreto).
unsafe fn apply_instant(wallpaper: &IDesktopWallpaper, path: &Path, monitor_id: Option<&str>, style: WallpaperStyle) -> Result<(), String> {
    let wide: Vec<u16> = path.to_string_lossy().encode_utf16().chain(std::iter::once(0)).collect();
    let pcwstr = PCWSTR(wide.as_ptr());
    wallpaper
        .SetPosition(DESKTOP_WALLPAPER_POSITION(style.to_dw_style()))
        .map_err(|e| format!("SetPosition fallo: {e}"))?;
    if let Some(monitor) = monitor_id {
        let monitor_wide: Vec<u16> = monitor.encode_utf16().chain(std::iter::once(0)).collect();
        wallpaper
            .SetWallpaper(PCWSTR(monitor_wide.as_ptr()), pcwstr)
            .map_err(|e| format!("SetWallpaper(monit) fallo: {e}"))?;
        log_info!("Wallpaper aplicado al monitor {} con estilo {:?}", monitor, style);
    } else {
        wallpaper
            .SetWallpaper(PCWSTR(std::ptr::null()), pcwstr)
            .map_err(|e| format!("SetWallpaper(todos) fallo: {e}"))?;
        log_info!("Wallpaper aplicado a todos los monitores con estilo {:?}", style);
    }
    Ok(())
}

/// Envoltorio de IDesktopWallpaper. No es `Clone` a propósito: cada instancia equilibra
/// su propio CoInitializeEx con un CoUninitialize al destruirse.
pub struct WallpaperManager {
    wallpaper: Option<IDesktopWallpaper>,
    com_initialized: bool,
}

impl WallpaperManager {
    pub fn new() -> Result<Self, String> {
        unsafe {
            let hr = CoInitializeEx(None, COINIT_MULTITHREADED);
            if hr.is_err() {
                return Err(format!("CoInitializeEx fallo: {:?}", hr));
            }
            // Solo desinicializar si esta instancia inicializo COM (S_OK). S_FALSE = ya estaba inicializado.
            let com_initialized = hr.0 == 0;

            let wallpaper: IDesktopWallpaper = CoCreateInstance(&DesktopWallpaper, None, CLSCTX_ALL)
                .map_err(|e| format!("CoCreateInstance(DesktopWallpaper) fallo: {e}"))?;

            log_info!("IDesktopWallpaper inicializado correctamente.");
            Ok(Self {
                wallpaper: Some(wallpaper),
                com_initialized,
            })
        }
    }

    /// Aplicar imagen a un monitor especifico o a todos los monitores.
    /// Con `fade`, a todos los monitores con el fundido de Windows (en segundo plano).
    pub fn set_wallpaper<P: AsRef<Path>>(&self, path: P, monitor_id: Option<&str>, style: WallpaperStyle, fade: bool) -> Result<(), String> {
        if fade && monitor_id.is_none() {
            let sender = fade_worker().ok_or("No se pudo iniciar el hilo de fundido")?;
            return sender.send((path.as_ref().to_path_buf(), style)).map_err(|e| format!("Hilo de fundido no disponible: {e}"));
        }
        let wallpaper = self.wallpaper.as_ref().ok_or("IDesktopWallpaper no inicializado")?;
        unsafe { apply_instant(wallpaper, path.as_ref(), monitor_id, style) }
    }

    /// Obtiene la ruta del fondo de pantalla actualmente activo en Windows.
    pub fn get_current_wallpaper(&self) -> Result<std::path::PathBuf, String> {
        let wallpaper = self.wallpaper.as_ref().ok_or("IDesktopWallpaper no inicializado")?;
        // Consulta IDesktopWallpaper con monitor NULL para obtener el fondo principal
        unsafe {
            let path_pwstr = wallpaper
                .GetWallpaper(PCWSTR(std::ptr::null()))
                .map_err(|e| format!("GetWallpaper fallo: {e}"))?;
            let path_str = take_com_string(path_pwstr)?;
            if path_str.is_empty() {
                return Err("Sin fondo de pantalla común a todos los monitores".into());
            }
            Ok(std::path::PathBuf::from(path_str))
        }
    }

    /// Monitores conectados (identificador y rectángulo), de izquierda a derecha.
    pub fn monitors(&self) -> Vec<(String, windows::Win32::Foundation::RECT)> {
        let Some(wallpaper) = self.wallpaper.as_ref() else { return Vec::new(); };
        let mut monitors: Vec<(i32, i32, String, windows::Win32::Foundation::RECT)> = Vec::new();
        unsafe {
            let count = wallpaper.GetMonitorDevicePathCount().unwrap_or(0);
            for index in 0..count {
                let Ok(raw) = wallpaper.GetMonitorDevicePathAt(index) else { continue; };
                let Ok(id) = take_com_string(raw) else { continue; };
                let wide: Vec<u16> = id.encode_utf16().chain(std::iter::once(0)).collect();
                // Los monitores desconectados siguen en la lista pero no tienen rectángulo.
                if let Ok(rect) = wallpaper.GetMonitorRECT(PCWSTR(wide.as_ptr())) {
                    if rect.right > rect.left {
                        monitors.push((rect.left, rect.top, id, rect));
                    }
                }
            }
        }
        monitors.sort_by(|a, b| (a.0, a.1).cmp(&(b.0, b.1)));
        monitors.into_iter().map(|(_, _, id, rect)| (id, rect)).collect()
    }

    /// Fondo que Windows tiene puesto en un monitor concreto.
    pub fn wallpaper_of(&self, monitor_id: &str) -> Option<PathBuf> {
        let wallpaper = self.wallpaper.as_ref()?;
        let wide: Vec<u16> = monitor_id.encode_utf16().chain(std::iter::once(0)).collect();
        unsafe {
            let raw = wallpaper.GetWallpaper(PCWSTR(wide.as_ptr())).ok()?;
            take_com_string(raw).ok().filter(|path| !path.is_empty()).map(PathBuf::from)
        }
    }

    /// Identificador del monitor cuyo rectángulo coincide con `rect` (el del monitor del panel).
    pub fn monitor_id_for_rect(&self, rect: windows::Win32::Foundation::RECT) -> Option<String> {
        let wallpaper = self.wallpaper.as_ref()?;
        unsafe {
            let count = wallpaper.GetMonitorDevicePathCount().ok()?;
            for index in 0..count {
                let Ok(raw) = wallpaper.GetMonitorDevicePathAt(index) else { continue; };
                let Ok(id) = take_com_string(raw) else { continue; };
                let wide: Vec<u16> = id.encode_utf16().chain(std::iter::once(0)).collect();
                if let Ok(monitor_rect) = wallpaper.GetMonitorRECT(PCWSTR(wide.as_ptr())) {
                    if monitor_rect == rect {
                        return Some(id);
                    }
                }
            }
        }
        None
    }
}

impl Drop for WallpaperManager {
    fn drop(&mut self) {
        unsafe {
            if self.com_initialized {
                CoUninitialize();
                log_info!("COM finalizado (CoUninitialize).");
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Comprueba que IActiveDesktop responde en este Windows: lee el fondo y su estilo y los
    /// vuelve a guardar sin cambios (no altera el escritorio). `cargo test -- --ignored`.
    #[test]
    #[ignore]
    fn active_desktop_roundtrip_is_noop() {
        unsafe {
            let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
            let desktop: IActiveDesktop = CoCreateInstance(&CLSID_ACTIVE_DESKTOP, None, CLSCTX_INPROC_SERVER).unwrap();
            let mut buffer = [0u16; 1024];
            desktop.GetWallpaper(windows::core::PWSTR(buffer.as_mut_ptr()), buffer.len() as u32, 0).ok().unwrap();
            let mut options = WallpaperOpt { size: std::mem::size_of::<WallpaperOpt>() as u32, style: 0 };
            desktop.GetWallpaperOptions(&mut options, 0).ok().unwrap();
            let len = buffer.iter().position(|&c| c == 0).unwrap_or(0);
            println!("Fondo actual: {} (estilo {})", String::from_utf16_lossy(&buffer[..len]), options.style);
            desktop.SetWallpaperOptions(&options, 0).ok().unwrap();
            desktop.SetWallpaper(PCWSTR(buffer.as_ptr()), 0).ok().unwrap();
            // Sin ApplyChanges: no se guarda ni se refresca nada.
        }
    }
}
