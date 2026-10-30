use crate::log_info;
use crate::log_warn;
use std::path::Path;
use std::sync::{Arc, Mutex};
use windows::core::{Interface, PCWSTR};
use windows::Win32::Foundation::{HWND, LPARAM, WPARAM};
use windows::Win32::System::Com::{CoInitializeEx, CoUninitialize, COINIT_APARTMENTTHREADED};
use windows::Win32::UI::Shell::{IShellItem, IShellItemImageFactory, SHCreateItemFromParsingName, SIIGBF_RESIZETOFIT};
use windows::Win32::UI::WindowsAndMessaging::{PostMessageW, WM_APP};

pub const WM_APP_THUMB_READY: u32 = WM_APP + 20;

#[derive(Clone)]
pub struct Thumb {
    pub path_key: String,
    pub hbitmap: isize,
    pub width: i32,
    pub height: i32,
}

#[derive(Clone)]
pub struct ThumbnailCache {
    cache: Arc<Mutex<std::collections::HashMap<String, Thumb>>>,
}

impl ThumbnailCache {
    pub fn new() -> Self {
        Self {
            cache: Arc::new(Mutex::new(std::collections::HashMap::new())),
        }
    }

    pub fn get(&self, key: &str) -> Option<Thumb> {
        if let Ok(guard) = self.cache.lock() {
            guard.get(key).cloned()
        } else {
            None
        }
    }

    pub fn insert(&self, thumb: Thumb) {
        if let Ok(mut guard) = self.cache.lock() {
            guard.insert(thumb.path_key.clone(), thumb);
        }
    }

    /// Limpia la caché y libera los bitmaps GDI.
    pub fn clear(&self) {
        if let Ok(mut guard) = self.cache.lock() {
            for (_, thumb) in guard.drain() {
                unsafe {
                    if thumb.hbitmap != 0 {
                        let _ = windows::Win32::Graphics::Gdi::DeleteObject(windows::Win32::Graphics::Gdi::HGDIOBJ(thumb.hbitmap as *mut _));
                    }
                }
            }
        }
    }
}

impl Default for ThumbnailCache {
    fn default() -> Self {
        Self::new()
    }
}

impl Drop for ThumbnailCache {
    fn drop(&mut self) {
        if Arc::strong_count(&self.cache) == 1 {
            self.clear();
        }
    }
}

pub fn request_thumbnail<P: AsRef<Path>>(hwnd: HWND, path: P, size_px: i32, cache: ThumbnailCache) {
    let path_buf = path.as_ref().to_path_buf();
    let hwnd_val = hwnd.0 as isize;
    let key = path_buf.to_string_lossy().to_string();

    // Evitar regenerar si ya existe en la caché
    if cache.get(&key).is_some() {
        return;
    }

    let start = std::time::Instant::now();
    unsafe {
        let hr = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
        let should_uninit = hr.is_ok();

        let path_wide: Vec<u16> = path_buf
            .to_string_lossy()
            .encode_utf16()
            .chain(std::iter::once(0))
            .collect();
        let pcwstr = PCWSTR(path_wide.as_ptr());

        if let Ok(shell_item) = SHCreateItemFromParsingName::<PCWSTR, Option<&windows::Win32::System::Com::IBindCtx>, IShellItem>(pcwstr, None) {
            if let Ok(factory) = shell_item.cast::<IShellItemImageFactory>() {
                let size = windows::Win32::Foundation::SIZE { cx: size_px, cy: size_px };
                match factory.GetImage(size, SIIGBF_RESIZETOFIT) {
                    Ok(bmp) => {
                        let hbitmap = bmp.0 as isize;
                        if hbitmap != 0 {
                            let elapsed = start.elapsed().as_millis();
                            log_info!(
                                "Miniatura generada para {:?} en {} ms ({}x{})",
                                path_buf.file_name().unwrap_or_default(),
                                elapsed,
                                size_px,
                                size_px
                            );
                            let thumb = Thumb {
                                path_key: key.clone(),
                                hbitmap,
                                width: size_px,
                                height: size_px,
                            };
                            cache.insert(thumb);
                            let _ = PostMessageW(HWND(hwnd_val as *mut _), WM_APP_THUMB_READY, WPARAM(0), LPARAM(0));
                        } else {
                            log_warn!("GetImage devolvio bitmap invalido para {:?}", path_buf);
                        }
                    }
                    Err(e) => {
                        log_warn!("GetImage fallo para {:?}: {e:?}", path_buf);
                    }
                }
            } else {
                log_warn!("No se pudo obtener IShellItemImageFactory para {:?}", path_buf);
            }
        } else {
            log_warn!("SHCreateItemFromParsingName fallo para {:?}", path_buf);
        }

        if should_uninit {
            CoUninitialize();
        }
        let _ = hr;
    }
}
