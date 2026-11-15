//! Piezas de Direct2D compartidas por las vistas dibujadas con él (notch y galería Pro).
//!
//! `Surface` es un destino de dibujo sobre un DIB de 32 bits con alfa premultiplicado que se
//! presenta con `UpdateLayeredWindow`: bordes suavizados y transparencia real por píxel.
//! Solo existe mientras la vista está abierta.

use std::collections::HashMap;
use windows::core::w;
use windows::Foundation::Numerics::Matrix3x2;
use windows::Win32::Foundation::{COLORREF, HWND, POINT, RECT, SIZE};
use windows::Win32::Graphics::Direct2D::Common::{
    D2D1_ALPHA_MODE_PREMULTIPLIED, D2D1_COLOR_F, D2D1_FIGURE_BEGIN_FILLED, D2D1_FIGURE_END_CLOSED, D2D1_PIXEL_FORMAT, D2D_POINT_2F,
    D2D_RECT_F, D2D_SIZE_U,
};
use windows::Win32::Graphics::Direct2D::{
    D2D1CreateFactory, ID2D1Bitmap, ID2D1BitmapBrush, ID2D1DCRenderTarget, ID2D1Factory, ID2D1Geometry, ID2D1Layer, ID2D1RenderTarget,
    ID2D1SolidColorBrush, D2D1_BITMAP_BRUSH_PROPERTIES, D2D1_BITMAP_INTERPOLATION_MODE_LINEAR, D2D1_BITMAP_PROPERTIES,
    D2D1_BRUSH_PROPERTIES, D2D1_DRAW_TEXT_OPTIONS_CLIP, D2D1_EXTEND_MODE_CLAMP, D2D1_FACTORY_TYPE_SINGLE_THREADED,
    D2D1_FEATURE_LEVEL_DEFAULT, D2D1_RENDER_TARGET_PROPERTIES, D2D1_RENDER_TARGET_TYPE_DEFAULT, D2D1_RENDER_TARGET_USAGE_NONE,
    D2D1_ROUNDED_RECT, D2D1_TEXT_ANTIALIAS_MODE_GRAYSCALE,
};
use windows::Win32::Graphics::DirectWrite::{
    DWriteCreateFactory, IDWriteFactory, IDWriteTextFormat, DWRITE_FACTORY_TYPE_SHARED, DWRITE_FONT_STRETCH_NORMAL,
    DWRITE_FONT_STYLE_NORMAL, DWRITE_FONT_WEIGHT, DWRITE_MEASURING_MODE_NATURAL, DWRITE_PARAGRAPH_ALIGNMENT_CENTER,
    DWRITE_TEXT_ALIGNMENT, DWRITE_TRIMMING, DWRITE_TRIMMING_GRANULARITY_CHARACTER, DWRITE_WORD_WRAPPING_NO_WRAP,
};
use windows::Win32::Graphics::Dxgi::Common::DXGI_FORMAT_B8G8R8A8_UNORM;
use windows::Win32::Graphics::Gdi::{
    CreateCompatibleDC, CreateDIBSection, DeleteDC, DeleteObject, GetObjectW, SelectObject, AC_SRC_ALPHA, AC_SRC_OVER, BITMAP, BITMAPINFO,
    BITMAPINFOHEADER, BLENDFUNCTION, DIB_RGB_COLORS, HBITMAP, HDC, HGDIOBJ,
};
use windows::Win32::UI::WindowsAndMessaging::{UpdateLayeredWindow, ULW_ALPHA};

pub type Rgb = (f32, f32, f32);

pub const COLOR_NEW: Rgb = (0.424, 0.796, 0.373);
pub const COLOR_FAVORITE: Rgb = (1.0, 0.784, 0.188);

pub fn color(rgb: Rgb, alpha: f32) -> D2D1_COLOR_F {
    D2D1_COLOR_F { r: rgb.0, g: rgb.1, b: rgb.2, a: alpha.clamp(0.0, 1.0) }
}
pub fn white(alpha: f32) -> D2D1_COLOR_F { color((1.0, 1.0, 1.0), alpha) }
pub fn black(alpha: f32) -> D2D1_COLOR_F { color((0.0, 0.0, 0.0), alpha) }

/// ¿Tema claro? (configuración o Windows).
pub fn light() -> bool { crate::winutil::light_theme() }
/// Primer plano de la interfaz: blanco en el tema oscuro, casi negro en el claro.
pub fn fg(alpha: f32) -> D2D1_COLOR_F { if light() { color((0.09, 0.09, 0.10), alpha) } else { white(alpha) } }
/// Fondo de la interfaz: negro en el tema oscuro, blanco roto en el claro.
pub fn bg(alpha: f32) -> D2D1_COLOR_F { if light() { color((0.955, 0.955, 0.95), alpha) } else { black(alpha) } }
/// Texto sobre el color de acento (el acento es claro en el tema oscuro y oscuro en el claro).
pub fn on_accent(alpha: f32) -> D2D1_COLOR_F { if light() { white(alpha) } else { color((0.04, 0.04, 0.05), alpha) } }
pub fn rect_f(left: f32, top: f32, right: f32, bottom: f32) -> D2D_RECT_F { D2D_RECT_F { left, top, right, bottom } }
pub fn point(x: f32, y: f32) -> D2D_POINT_2F { D2D_POINT_2F { x, y } }
pub fn lerp(a: f32, b: f32, t: f32) -> f32 { a + (b - a) * t }
pub fn ease_out_cubic(t: f32) -> f32 { 1.0 - (1.0 - t.clamp(0.0, 1.0)).powi(3) }
/// Sale un poco por encima de 1 antes de asentarse (efecto "muelle").
pub fn ease_out_back(t: f32) -> f32 {
    let t = t.clamp(0.0, 1.0);
    let c1 = 1.15;
    let c3 = c1 + 1.0;
    1.0 + c3 * (t - 1.0).powi(3) + c1 * (t - 1.0).powi(2)
}
pub fn smoothstep(t: f32) -> f32 {
    let t = t.clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}
pub fn rounded(rect: D2D_RECT_F, radius: f32) -> D2D1_ROUNDED_RECT { D2D1_ROUNDED_RECT { rect, radiusX: radius, radiusY: radius } }
pub fn inflate(rect: D2D_RECT_F, by: f32) -> D2D_RECT_F { rect_f(rect.left - by, rect.top - by, rect.right + by, rect.bottom + by) }
pub fn inside(rect: &D2D_RECT_F, x: f32, y: f32) -> bool { x >= rect.left && x < rect.right && y >= rect.top && y < rect.bottom }
pub fn width(rect: &D2D_RECT_F) -> f32 { rect.right - rect.left }
pub fn height(rect: &D2D_RECT_F) -> f32 { rect.bottom - rect.top }

/// Acento del sistema como color de Direct2D.
pub fn accent() -> Rgb {
    let (r, g, b) = crate::winutil::accent_rgb();
    (r as f32 / 255.0, g as f32 / 255.0, b as f32 / 255.0)
}

/// Destino de dibujo de Direct2D sobre un DIB para `UpdateLayeredWindow`.
pub struct Surface {
    pub factory: ID2D1Factory,
    pub target: ID2D1DCRenderTarget,
    pub layer: Option<ID2D1Layer>,
    pub dwrite: Option<IDWriteFactory>,
    pub dc: HDC,
    pub dib: HBITMAP,
    old: HGDIOBJ,
    pub width: i32,
    pub height: i32,
    /// Mapas de bits de Direct2D por imagen: (clave, si es la tarjeta grande) -> (bitmap, uso).
    bitmaps: HashMap<(String, bool), (ID2D1Bitmap, u64)>,
    pub tick: u64,
}

impl Drop for Surface {
    fn drop(&mut self) {
        unsafe {
            SelectObject(self.dc, self.old);
            let _ = DeleteObject(HGDIOBJ(self.dib.0));
            let _ = DeleteDC(self.dc);
        }
    }
}

impl Surface {
    pub unsafe fn new(width: i32, height: i32) -> windows::core::Result<Self> {
        let factory: ID2D1Factory = D2D1CreateFactory(D2D1_FACTORY_TYPE_SINGLE_THREADED, None)?;
        let properties = D2D1_RENDER_TARGET_PROPERTIES {
            r#type: D2D1_RENDER_TARGET_TYPE_DEFAULT,
            pixelFormat: D2D1_PIXEL_FORMAT { format: DXGI_FORMAT_B8G8R8A8_UNORM, alphaMode: D2D1_ALPHA_MODE_PREMULTIPLIED },
            dpiX: 96.0,
            dpiY: 96.0,
            usage: D2D1_RENDER_TARGET_USAGE_NONE,
            minLevel: D2D1_FEATURE_LEVEL_DEFAULT,
        };
        let target = factory.CreateDCRenderTarget(&properties)?;
        target.SetTextAntialiasMode(D2D1_TEXT_ANTIALIAS_MODE_GRAYSCALE);
        let header = BITMAPINFOHEADER {
            biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
            biWidth: width,
            biHeight: -height,
            biPlanes: 1,
            biBitCount: 32,
            ..Default::default()
        };
        let info = BITMAPINFO { bmiHeader: header, ..Default::default() };
        let mut bits = std::ptr::null_mut();
        let dib = CreateDIBSection(None, &info, DIB_RGB_COLORS, &mut bits, None, 0)?;
        let dc = CreateCompatibleDC(None);
        let old = SelectObject(dc, HGDIOBJ(dib.0));
        target.BindDC(dc, &RECT { left: 0, top: 0, right: width, bottom: height })?;
        let layer = target.CreateLayer(None).ok();
        let dwrite = DWriteCreateFactory::<IDWriteFactory>(DWRITE_FACTORY_TYPE_SHARED).ok();
        Ok(Self { factory, target, layer, dwrite, dc, dib, old, width, height, bitmaps: HashMap::new(), tick: 0 })
    }

    pub fn rt(&self) -> ID2D1RenderTarget { self.target.clone().into() }

    pub unsafe fn format(&self, size: f32, weight: DWRITE_FONT_WEIGHT, align: DWRITE_TEXT_ALIGNMENT) -> Option<IDWriteTextFormat> {
        text_format(self.dwrite.as_ref()?, size, weight, align)
    }

    /// Mapa de bits de Direct2D de un bitmap GDI de la caché de miniaturas o tarjetas.
    pub unsafe fn bitmap(&mut self, key: &str, large: bool, hbitmap: isize) -> Option<ID2D1Bitmap> {
        let id = (key.to_string(), large);
        if let Some(entry) = self.bitmaps.get_mut(&id) {
            entry.1 = self.tick;
            return Some(entry.0.clone());
        }
        let bitmap = gdi_to_d2d(&self.rt(), hbitmap)?;
        self.bitmaps.insert(id, (bitmap.clone(), self.tick));
        Some(bitmap)
    }

    /// Olvida los mapas de bits que no se usaron en los últimos fotogramas.
    pub fn trim(&mut self, capacity: usize) {
        while self.bitmaps.len() > capacity {
            let Some(oldest) = self.bitmaps.iter().min_by_key(|(_, (_, tick))| *tick).map(|(key, _)| key.clone()) else { break; };
            self.bitmaps.remove(&oldest);
        }
    }

    /// Presenta el DIB en la ventana con una opacidad global.
    pub unsafe fn present(&self, hwnd: HWND, origin: POINT, alpha: u8) -> bool {
        let size = SIZE { cx: self.width, cy: self.height };
        let source = POINT::default();
        let blend = BLENDFUNCTION { BlendOp: AC_SRC_OVER as u8, BlendFlags: 0, SourceConstantAlpha: alpha, AlphaFormat: AC_SRC_ALPHA as u8 };
        UpdateLayeredWindow(hwnd, None, Some(&origin), Some(&size), self.dc, Some(&source), COLORREF(0), Some(&blend), ULW_ALPHA).is_ok()
    }
}

pub unsafe fn text_format(dwrite: &IDWriteFactory, size: f32, weight: DWRITE_FONT_WEIGHT, align: DWRITE_TEXT_ALIGNMENT) -> Option<IDWriteTextFormat> {
    let format = dwrite.CreateTextFormat(w!("Segoe UI Variable Text"), None, weight, DWRITE_FONT_STYLE_NORMAL, DWRITE_FONT_STRETCH_NORMAL, size, w!(""))
        .or_else(|_| dwrite.CreateTextFormat(w!("Segoe UI"), None, weight, DWRITE_FONT_STYLE_NORMAL, DWRITE_FONT_STRETCH_NORMAL, size, w!("")))
        .ok()?;
    let _ = format.SetTextAlignment(align);
    let _ = format.SetParagraphAlignment(DWRITE_PARAGRAPH_ALIGNMENT_CENTER);
    let _ = format.SetWordWrapping(DWRITE_WORD_WRAPPING_NO_WRAP);
    if let Ok(sign) = dwrite.CreateEllipsisTrimmingSign(&format) {
        let trimming = DWRITE_TRIMMING { granularity: DWRITE_TRIMMING_GRANULARITY_CHARACTER, delimiter: 0, delimiterCount: 0 };
        let _ = format.SetTrimming(&trimming, &sign);
    }
    Some(format)
}

/// Texto de una línea recortado al rectángulo.
pub unsafe fn draw_text(target: &ID2D1RenderTarget, text: &str, format: &IDWriteTextFormat, rect: D2D_RECT_F, brush: &ID2D1SolidColorBrush) {
    let wide: Vec<u16> = text.encode_utf16().collect();
    target.DrawText(&wide, format, &rect, brush, D2D1_DRAW_TEXT_OPTIONS_CLIP, DWRITE_MEASURING_MODE_NATURAL);
}

/// Ancho que ocupa un texto de una línea con este formato.
pub unsafe fn text_width(dwrite: &IDWriteFactory, format: &IDWriteTextFormat, text: &str) -> f32 {
    let wide: Vec<u16> = text.encode_utf16().collect();
    match dwrite.CreateTextLayout(&wide, format, 10_000.0, 100.0) {
        Ok(layout) => {
            let mut metrics = Default::default();
            if layout.GetMetrics(&mut metrics).is_ok() { metrics.widthIncludingTrailingWhitespace } else { 0.0 }
        }
        Err(_) => 0.0,
    }
}

/// Copia un DIB de 24 o 32 bits (de arriba abajo) a un mapa de bits BGRA opaco de Direct2D.
pub unsafe fn gdi_to_d2d(target: &ID2D1RenderTarget, hbitmap: isize) -> Option<ID2D1Bitmap> {
    let (pixels, width, height) = gdi_pixels(hbitmap)?;
    let properties = D2D1_BITMAP_PROPERTIES {
        pixelFormat: D2D1_PIXEL_FORMAT { format: DXGI_FORMAT_B8G8R8A8_UNORM, alphaMode: D2D1_ALPHA_MODE_PREMULTIPLIED },
        dpiX: 96.0,
        dpiY: 96.0,
    };
    target.CreateBitmap(D2D_SIZE_U { width: width as u32, height: height as u32 }, Some(pixels.as_ptr() as *const _), (width * 4) as u32, &properties).ok()
}

/// Píxeles BGRA opacos de un bitmap GDI de 24 o 32 bits: (píxeles, ancho, alto).
pub unsafe fn gdi_pixels(hbitmap: isize) -> Option<(Vec<u8>, usize, usize)> {
    let mut info = BITMAP::default();
    let read = GetObjectW(HGDIOBJ(hbitmap as *mut _), std::mem::size_of::<BITMAP>() as i32, Some(&mut info as *mut _ as *mut std::ffi::c_void));
    if read == 0 || info.bmBits.is_null() || info.bmWidth <= 0 || info.bmHeight == 0 { return None; }
    let (width, height) = (info.bmWidth as usize, info.bmHeight.unsigned_abs() as usize);
    let bytes_per_pixel = (info.bmBitsPixel / 8) as usize;
    if bytes_per_pixel < 3 { return None; }
    let stride = info.bmWidthBytes as usize;
    let source = std::slice::from_raw_parts(info.bmBits as *const u8, stride * height);
    let mut pixels = vec![255u8; width * height * 4];
    for y in 0..height {
        let row = &source[y * stride..y * stride + width * bytes_per_pixel];
        let out = &mut pixels[y * width * 4..(y + 1) * width * 4];
        for (src, dst) in row.chunks_exact(bytes_per_pixel).zip(out.chunks_exact_mut(4)) {
            dst[..3].copy_from_slice(&src[..3]);
        }
    }
    Some((pixels, width, height))
}

/// Pincel con la imagen escalada para cubrir `rect`, centrada.
pub unsafe fn cover_brush(target: &ID2D1RenderTarget, bitmap: &ID2D1Bitmap, rect: D2D_RECT_F, alpha: f32) -> Option<ID2D1BitmapBrush> {
    let size = bitmap.GetSize();
    if size.width <= 0.0 || size.height <= 0.0 { return None; }
    let (w, h) = (width(&rect), height(&rect));
    let scale = (w / size.width).max(h / size.height);
    let offset_x = rect.left + (w - size.width * scale) / 2.0;
    let offset_y = rect.top + (h - size.height * scale) / 2.0;
    let properties = D2D1_BITMAP_BRUSH_PROPERTIES {
        extendModeX: D2D1_EXTEND_MODE_CLAMP,
        extendModeY: D2D1_EXTEND_MODE_CLAMP,
        interpolationMode: D2D1_BITMAP_INTERPOLATION_MODE_LINEAR,
    };
    let brush_properties = D2D1_BRUSH_PROPERTIES {
        opacity: alpha,
        transform: Matrix3x2 { M11: scale, M12: 0.0, M21: 0.0, M22: scale, M31: offset_x, M32: offset_y },
    };
    target.CreateBitmapBrush(bitmap, Some(&properties), Some(&brush_properties)).ok()
}

/// Rellena la forma con la imagen escalada para cubrirla, centrada.
pub unsafe fn fill_cover(target: &ID2D1RenderTarget, bitmap: &ID2D1Bitmap, shape: &D2D1_ROUNDED_RECT, alpha: f32) {
    if let Some(brush) = cover_brush(target, bitmap, shape.rect, alpha) {
        target.FillRoundedRectangle(shape, &brush);
    }
}

/// Rellena una geometría cualquiera con la imagen escalada para cubrir `bounds`.
pub unsafe fn fill_cover_geometry(target: &ID2D1RenderTarget, bitmap: &ID2D1Bitmap, geometry: &ID2D1Geometry, bounds: D2D_RECT_F, alpha: f32) {
    if let Some(brush) = cover_brush(target, bitmap, bounds, alpha) {
        target.FillGeometry(geometry, &brush, None);
    }
}

/// Sombra suave bajo un rectángulo redondeado: capas cada vez más anchas y tenues.
pub unsafe fn soft_shadow(target: &ID2D1RenderTarget, brush: &ID2D1SolidColorBrush, rect: D2D_RECT_F, radius: f32, spread: f32, offset_y: f32, alpha: f32) {
    let steps = 6;
    for i in (1..=steps).rev() {
        let grow = spread * i as f32 / steps as f32;
        brush.SetColor(&black(alpha / steps as f32));
        let r = rect_f(rect.left, rect.top + offset_y, rect.right, rect.bottom + offset_y);
        target.FillRoundedRectangle(&rounded(inflate(r, grow), radius + grow), brush);
    }
}

/// Paralelogramo (tarjeta inclinada): la parte de arriba se desplaza `slant` hacia la derecha.
pub unsafe fn slanted_geometry(factory: &ID2D1Factory, rect: D2D_RECT_F, slant: f32) -> Option<ID2D1Geometry> {
    let geometry = factory.CreatePathGeometry().ok()?;
    let sink = geometry.Open().ok()?;
    let half = slant / 2.0;
    sink.BeginFigure(point(rect.left + half, rect.top), D2D1_FIGURE_BEGIN_FILLED);
    sink.AddLine(point(rect.right + half, rect.top));
    sink.AddLine(point(rect.right - half, rect.bottom));
    sink.AddLine(point(rect.left - half, rect.bottom));
    sink.EndFigure(D2D1_FIGURE_END_CLOSED);
    sink.Close().ok()?;
    Some(geometry.into())
}

pub unsafe fn draw_star(factory: &ID2D1Factory, target: &ID2D1RenderTarget, brush: &ID2D1SolidColorBrush, center: D2D_POINT_2F, radius: f32) {
    let Ok(geometry) = factory.CreatePathGeometry() else { return; };
    let Ok(sink) = geometry.Open() else { return; };
    let vertex = |i: usize| {
        let r = if i.is_multiple_of(2) { radius } else { radius * 0.45 };
        let angle = -std::f32::consts::FRAC_PI_2 + i as f32 * std::f32::consts::PI / 5.0;
        point(center.x + r * angle.cos(), center.y + r * angle.sin())
    };
    sink.BeginFigure(vertex(0), D2D1_FIGURE_BEGIN_FILLED);
    for i in 1..10 { sink.AddLine(vertex(i)); }
    sink.EndFigure(D2D1_FIGURE_END_CLOSED);
    if sink.Close().is_ok() {
        target.FillGeometry(&geometry, brush, None);
    }
}

/// Color dominante de una miniatura: el tono más presente entre los píxeles con color,
/// o gris/negro/blanco si la imagen casi no tiene saturación. Devuelve (r, g, b) de 0 a 1.
pub unsafe fn dominant_color(hbitmap: isize) -> Option<Rgb> {
    let (pixels, width, height) = gdi_pixels(hbitmap)?;
    Some(dominant_of(&pixels, width, height))
}

pub fn dominant_of(pixels: &[u8], width: usize, height: usize) -> Rgb {
    const BUCKETS: usize = 12;
    let mut weight = [0f32; BUCKETS];
    let mut sum = [(0f32, 0f32, 0f32); BUCKETS];
    let (mut gray_w, mut gray) = (0f32, (0f32, 0f32, 0f32));
    let step = ((width * height) / 4096).max(1);
    for px in pixels.chunks_exact(4).step_by(step) {
        let (b, g, r) = (px[0] as f32 / 255.0, px[1] as f32 / 255.0, px[2] as f32 / 255.0);
        let (h, s, v) = hsv(r, g, b);
        if s < 0.18 || v < 0.12 {
            gray_w += 1.0;
            gray = (gray.0 + r, gray.1 + g, gray.2 + b);
            continue;
        }
        let i = ((h / 360.0 * BUCKETS as f32) as usize).min(BUCKETS - 1);
        let w = s * v;
        weight[i] += w;
        sum[i] = (sum[i].0 + r * w, sum[i].1 + g * w, sum[i].2 + b * w);
    }
    let (best, best_w) = weight.iter().copied().enumerate().fold((0, 0f32), |acc, (i, w)| if w > acc.1 { (i, w) } else { acc });
    // Con poco color frente a lo gris, la imagen se considera neutra.
    if best_w <= 0.0 || best_w < gray_w * 0.08 {
        let n = gray_w.max(1.0);
        return (gray.0 / n, gray.1 / n, gray.2 / n);
    }
    (sum[best].0 / best_w, sum[best].1 / best_w, sum[best].2 / best_w)
}

/// (tono 0..360, saturación 0..1, valor 0..1)
pub fn hsv(r: f32, g: f32, b: f32) -> (f32, f32, f32) {
    let max = r.max(g).max(b);
    let min = r.min(g).min(b);
    let delta = max - min;
    let h = if delta <= 0.0 {
        0.0
    } else if max == r {
        60.0 * (((g - b) / delta).rem_euclid(6.0))
    } else if max == g {
        60.0 * ((b - r) / delta + 2.0)
    } else {
        60.0 * ((r - g) / delta + 4.0)
    };
    let s = if max <= 0.0 { 0.0 } else { delta / max };
    (h, s, max)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn solid(r: u8, g: u8, b: u8) -> Vec<u8> { (0..64 * 64).flat_map(|_| [b, g, r, 255]).collect() }

    #[test]
    fn dominant_color_picks_the_main_hue() {
        let (r, g, b) = dominant_of(&solid(200, 30, 30), 64, 64);
        assert!(r > 0.7 && g < 0.2 && b < 0.2);
        let (r, g, b) = dominant_of(&solid(20, 60, 220), 64, 64);
        assert!(b > 0.8 && r < 0.2 && g < 0.3);
        // Gris: sin tono.
        let (r, g, b) = dominant_of(&solid(128, 128, 128), 64, 64);
        assert!((r - g).abs() < 0.01 && (g - b).abs() < 0.01);
    }

    #[test]
    fn hsv_matches_primaries() {
        assert_eq!(hsv(1.0, 0.0, 0.0).0, 0.0);
        assert!((hsv(0.0, 1.0, 0.0).0 - 120.0).abs() < 0.01);
        assert!((hsv(0.0, 0.0, 1.0).0 - 240.0).abs() < 0.01);
    }
}
