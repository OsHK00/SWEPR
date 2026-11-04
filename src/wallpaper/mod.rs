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
}

impl WallpaperStyle {
    pub fn from_str(s: &str) -> Self {
        match s.to_ascii_lowercase().as_str() {
            "center" => WallpaperStyle::Center,
            "tile" => WallpaperStyle::Tile,
            "stretch" => WallpaperStyle::Stretch,
            "fit" => WallpaperStyle::Fit,
            _ => WallpaperStyle::Fill,
        }
    }
    
    fn to_dw_style(self) -> i32 {
        // DESKTOP_WALLPAPER_POSITION: Center=0, Tile=1, Stretch=2, Fit=3, Fill=4
        match self {
            WallpaperStyle::Center => 0,
            WallpaperStyle::Tile => 1,
            WallpaperStyle::Stretch => 2,
            WallpaperStyle::Fit => 3,
            WallpaperStyle::Fill => 4,
        }
    }

    /// Equivalente WPSTYLE_* de IActiveDesktop.
    fn to_wp_style(self) -> u32 {
        // WPSTYLE: Center=0, Tile=1, Stretch=2, KeepAspect=3, CropToFit=4
        match self {
            WallpaperStyle::Center => 0,
            WallpaperStyle::Tile => 1,
            WallpaperStyle::Stretch => 2,
            WallpaperStyle::Fit => 3,
            WallpaperStyle::Fill => 4,
        }
    }
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

#[derive(Clone)]
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
            let path_str = path_pwstr.to_string().map_err(|e| format!("{e}"))?;
            Ok(std::path::PathBuf::from(path_str))
        }
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
