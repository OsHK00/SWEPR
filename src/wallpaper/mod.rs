use crate::log_info;
use std::path::Path;
use windows::core::PCWSTR;
use windows::Win32::System::Com::{CoCreateInstance, CoInitializeEx, CoUninitialize, CLSCTX_ALL, COINIT_MULTITHREADED};
use windows::Win32::UI::Shell::{DesktopWallpaper, IDesktopWallpaper, DESKTOP_WALLPAPER_POSITION};

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
    pub fn set_wallpaper<P: AsRef<Path>>(&self, path: P, monitor_id: Option<&str>, style: WallpaperStyle) -> Result<(), String> {
        let wallpaper = self.wallpaper.as_ref().ok_or("IDesktopWallpaper no inicializado")?;
        let path_str = path.as_ref().to_string_lossy();
        let wide: Vec<u16> = path_str.encode_utf16().chain(std::iter::once(0)).collect();
        let pcwstr = PCWSTR(wide.as_ptr());

        unsafe {
            wallpaper
                .SetPosition(DESKTOP_WALLPAPER_POSITION(style.to_dw_style()))
                .map_err(|e| format!("SetPosition fallo: {e}"))?;

            if let Some(monitor) = monitor_id {
                let monitor_wide: Vec<u16> = monitor.encode_utf16().chain(std::iter::once(0)).collect();
                let monitor_pcwstr = PCWSTR(monitor_wide.as_ptr());
                wallpaper
                    .SetWallpaper(monitor_pcwstr, pcwstr)
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
