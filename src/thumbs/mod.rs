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
            if let Some(old) = guard.insert(thumb.path_key.clone(), thumb) {
                // Liberar el bitmap GDI sustituido para evitar fugas de memoria GDI
                unsafe {
                    if old.hbitmap != 0 {
                        let _ = windows::Win32::Graphics::Gdi::DeleteObject(
                            windows::Win32::Graphics::Gdi::HGDIOBJ(old.hbitmap as *mut _),
                        );
                    }
                }
            }
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

/// Genera un bitmap de 24 bits de exactamente `width`x`height` que cubre el área recortando
/// el centro de la imagen. Solo se guarda el recorte, sin el bitmap intermedio del Shell.
/// Requiere COM inicializado en el hilo que llama.
pub unsafe fn load_cover_bitmap(path: &Path, width: i32, height: i32) -> Option<Thumb> {
    use windows::Win32::Graphics::Gdi::{
        CreateCompatibleDC, CreateDIBSection, DeleteDC, DeleteObject, GetObjectW, SelectObject, SetBrushOrgEx,
        SetStretchBltMode, StretchBlt, BITMAP, BITMAPINFO, BITMAPINFOHEADER, DIB_RGB_COLORS, HALFTONE, HGDIOBJ, SRCCOPY,
    };
    if width <= 0 || height <= 0 { return None; }
    let path_wide: Vec<u16> = path.to_string_lossy().encode_utf16().chain(std::iter::once(0)).collect();
    let shell_item = SHCreateItemFromParsingName::<PCWSTR, Option<&windows::Win32::System::Com::IBindCtx>, IShellItem>(
        PCWSTR(path_wide.as_ptr()),
        None,
    ).ok()?;
    let factory = shell_item.cast::<IShellItemImageFactory>().ok()?;
    // Caja ancha: las imágenes horizontales se ajustan por alto y luego se recortan a lo ancho.
    let request = windows::Win32::Foundation::SIZE { cx: width.max(height * 3), cy: height };
    let source = match factory.GetImage(request, SIIGBF_RESIZETOFIT) {
        Ok(bmp) => bmp,
        Err(e) => {
            log_warn!("GetImage (carrusel) fallo para {:?}: {e:?}", path);
            return None;
        }
    };
    let mut info = BITMAP::default();
    let read = GetObjectW(HGDIOBJ(source.0), std::mem::size_of::<BITMAP>() as i32, Some(&mut info as *mut _ as *mut std::ffi::c_void));
    if read == 0 || info.bmWidth <= 0 || info.bmHeight <= 0 {
        let _ = DeleteObject(HGDIOBJ(source.0));
        return None;
    }
    let (src_w, src_h) = (info.bmWidth, info.bmHeight.abs());
    let (crop_x, crop_y, crop_w, crop_h) = if src_w * height > src_h * width {
        let crop_w = (src_h * width / height).max(1);
        ((src_w - crop_w) / 2, 0, crop_w, src_h)
    } else {
        let crop_h = (src_w * height / width).max(1);
        (0, (src_h - crop_h) / 2, src_w, crop_h)
    };

    let header = BITMAPINFOHEADER {
        biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
        biWidth: width,
        biHeight: -height,
        biPlanes: 1,
        biBitCount: 24,
        ..Default::default()
    };
    let bitmap_info = BITMAPINFO { bmiHeader: header, ..Default::default() };
    let mut bits = std::ptr::null_mut();
    let target = CreateDIBSection(None, &bitmap_info, DIB_RGB_COLORS, &mut bits, None, 0).ok()?;

    let src_dc = CreateCompatibleDC(None);
    let dst_dc = CreateCompatibleDC(None);
    let old_src = SelectObject(src_dc, HGDIOBJ(source.0));
    let old_dst = SelectObject(dst_dc, HGDIOBJ(target.0));
    let _ = SetStretchBltMode(dst_dc, HALFTONE);
    let _ = SetBrushOrgEx(dst_dc, 0, 0, None);
    let _ = StretchBlt(dst_dc, 0, 0, width, height, src_dc, crop_x, crop_y, crop_w, crop_h, SRCCOPY);
    SelectObject(src_dc, old_src);
    SelectObject(dst_dc, old_dst);
    let _ = DeleteDC(src_dc);
    let _ = DeleteDC(dst_dc);
    let _ = DeleteObject(HGDIOBJ(source.0));

    Some(Thumb {
        path_key: path.to_string_lossy().to_string(),
        hbitmap: target.0 as isize,
        width,
        height,
    })
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
        // Solo desinicializar COM si esta llamada lo inicializó (hr == S_OK, 0).
        // Si ya estaba inicializado (S_FALSE, 1), el hilo superior gestiona la desinicialización.
        let should_uninit = hr.0 == 0;

        let path_wide: Vec<u16> = path_buf
            .to_string_lossy()
            .encode_utf16()
            .chain(std::iter::once(0))
            .collect();
        let pcwstr = PCWSTR(path_wide.as_ptr());

        if let Ok(shell_item) = SHCreateItemFromParsingName::<PCWSTR, Option<&windows::Win32::System::Com::IBindCtx>, IShellItem>(pcwstr, None) {
            if let Ok(factory) = shell_item.cast::<IShellItemImageFactory>() {
                let thumb_w = size_px;
                let thumb_h = (size_px as f32 * 9.0 / 16.0).round() as i32;
                let size = windows::Win32::Foundation::SIZE { cx: thumb_w, cy: thumb_h };
                match factory.GetImage(size, SIIGBF_RESIZETOFIT) {
                    Ok(bmp) => {
                        let hbitmap = bmp.0 as isize;
                        if hbitmap != 0 {
                            let mut bitmap = windows::Win32::Graphics::Gdi::BITMAP::default();
                            let bitmap_size = std::mem::size_of::<windows::Win32::Graphics::Gdi::BITMAP>() as i32;
                            let actual_size = windows::Win32::Graphics::Gdi::GetObjectW(
                                windows::Win32::Graphics::Gdi::HGDIOBJ(bmp.0),
                                bitmap_size,
                                Some(&mut bitmap as *mut _ as *mut std::ffi::c_void),
                            );
                            let actual_w = if actual_size > 0 { bitmap.bmWidth } else { thumb_w };
                            let actual_h = if actual_size > 0 { bitmap.bmHeight } else { thumb_h };
                            let elapsed = start.elapsed().as_millis();
                            log_info!(
                                "Miniatura generada para {:?} en {} ms ({}x{})",
                                path_buf.file_name().unwrap_or_default(),
                                elapsed,
                                actual_w,
                                actual_h
                            );
                            let thumb = Thumb {
                                path_key: key.clone(),
                                hbitmap,
                                width: actual_w,
                                height: actual_h,
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
