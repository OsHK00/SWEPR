//! Geometría del lienzo: dónde va cada imagen según el modo de ajuste y a qué tamaño decodificarla.

use crate::config::FitMode;
use windows::Win32::Graphics::Direct2D::Common::D2D_RECT_F;

/// Rectángulo en píxeles (coordenadas de cliente del lienzo).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Area {
    pub left: f32,
    pub top: f32,
    pub right: f32,
    pub bottom: f32,
}

impl Area {
    pub fn new(left: f32, top: f32, right: f32, bottom: f32) -> Self {
        Self { left, top, right, bottom }
    }
    pub fn width(&self) -> f32 { self.right - self.left }
    pub fn height(&self) -> f32 { self.bottom - self.top }
    pub fn to_d2d(self) -> D2D_RECT_F {
        D2D_RECT_F { left: self.left, top: self.top, right: self.right, bottom: self.bottom }
    }

    /// Rectángulo de `size` centrado en esta área.
    fn centered(&self, size: (f32, f32)) -> Area {
        let left = self.left + (self.width() - size.0) / 2.0;
        let top = self.top + (self.height() - size.1) / 2.0;
        Area::new(left.round(), top.round(), (left + size.0).round(), (top + size.1).round())
    }
}

/// Rectángulos donde dibujar una imagen de tamaño `image` en `area` (un monitor).
/// `virtual_area` es el conjunto de todos los monitores, que usa "Extender".
pub fn layout_rects(fit: FitMode, image: (f32, f32), area: Area, virtual_area: Area) -> Vec<Area> {
    let (iw, ih) = (image.0.max(1.0), image.1.max(1.0));
    match fit {
        FitMode::Fill => {
            let scale = (area.width() / iw).max(area.height() / ih);
            vec![area.centered((iw * scale, ih * scale))]
        }
        FitMode::Fit => {
            let scale = (area.width() / iw).min(area.height() / ih);
            vec![area.centered((iw * scale, ih * scale))]
        }
        FitMode::Stretch => vec![area],
        FitMode::Center => vec![area.centered((iw, ih))],
        FitMode::Tile => {
            let mut rects = Vec::new();
            let mut y = area.top;
            while y < area.bottom {
                let mut x = area.left;
                while x < area.right {
                    rects.push(Area::new(x, y, x + iw, y + ih));
                    x += iw;
                }
                y += ih;
            }
            rects
        }
        // Una imagen sobre todos los monitores (el recorte de cada monitor lo hace el lienzo).
        FitMode::Span => {
            let scale = (virtual_area.width() / iw).max(virtual_area.height() / ih);
            vec![virtual_area.centered((iw * scale, ih * scale))]
        }
    }
}

/// Tamaño al que decodificar una imagen de `image` píxeles para un área de `area` píxeles:
/// nunca más grande que el original ni que lo que se va a ver, para ahorrar memoria de GPU.
pub fn decode_size(fit: FitMode, image: (u32, u32), area: (u32, u32)) -> (u32, u32) {
    let (iw, ih) = (image.0.max(1) as f64, image.1.max(1) as f64);
    let (aw, ah) = (area.0.max(1) as f64, area.1.max(1) as f64);
    let scale = match fit {
        FitMode::Fill | FitMode::Span | FitMode::Stretch => (aw / iw).max(ah / ih),
        FitMode::Fit => (aw / iw).min(ah / ih),
        // A tamaño natural, pero con un tope (4 veces el área) para panorámicas enormes.
        FitMode::Center | FitMode::Tile => ((aw * ah * 4.0) / (iw * ih)).sqrt(),
    }
    .min(1.0);
    (((iw * scale).round() as u32).max(1), ((ih * scale).round() as u32).max(1))
}

#[cfg(test)]
mod tests {
    use super::*;

    const MONITOR: Area = Area { left: 0.0, top: 0.0, right: 1920.0, bottom: 1080.0 };

    #[test]
    fn fill_covers_and_fit_contains() {
        let fill = layout_rects(FitMode::Fill, (1000.0, 1000.0), MONITOR, MONITOR)[0];
        assert!(fill.left <= 0.0 && fill.right >= 1920.0 && fill.top <= 0.0 && fill.bottom >= 1080.0);
        assert_eq!(fill.width(), 1920.0);
        let fit = layout_rects(FitMode::Fit, (1000.0, 1000.0), MONITOR, MONITOR)[0];
        assert_eq!((fit.width(), fit.height()), (1080.0, 1080.0));
        assert_eq!(fit.left, 420.0);
    }

    #[test]
    fn tile_and_center_use_natural_size() {
        let tiles = layout_rects(FitMode::Tile, (1000.0, 600.0), MONITOR, MONITOR);
        assert_eq!(tiles.len(), 2 * 2);
        let center = layout_rects(FitMode::Center, (100.0, 50.0), MONITOR, MONITOR)[0];
        assert_eq!(center, Area::new(910.0, 515.0, 1010.0, 565.0));
    }

    #[test]
    fn span_uses_all_monitors() {
        let all = Area::new(0.0, 0.0, 3840.0, 1080.0);
        let second = Area::new(1920.0, 0.0, 3840.0, 1080.0);
        let rect = layout_rects(FitMode::Span, (3840.0, 1080.0), second, all)[0];
        assert_eq!(rect, all);
    }

    #[test]
    fn decode_size_never_upscales_and_limits_memory() {
        assert_eq!(decode_size(FitMode::Fill, (6000, 4000), (1920, 1080)), (1920, 1280));
        assert_eq!(decode_size(FitMode::Fit, (6000, 4000), (1920, 1080)), (1620, 1080));
        assert_eq!(decode_size(FitMode::Fill, (800, 600), (1920, 1080)), (800, 600));
        let (w, h) = decode_size(FitMode::Center, (20000, 20000), (1920, 1080));
        assert!((w as u64 * h as u64) <= 1920 * 1080 * 4 + 20000);
    }
}
