//! Reloj del escritorio: se dibuja en el lienzo encima del fondo y debajo de los iconos.
//! Estilos: grande, cápsula, analógico, con saludo, apilado y collage; el color se adapta a lo
//! claro u oscuro que sea el fondo en esa zona (salvo el collage, que lleva sus propios papeles).
//! Solo se repinta al cambiar el minuto (o el segundo).

use super::layout::Area;
use crate::config::{ClockConfig, ClockPosition, ClockSize, ClockStyle};
use std::collections::HashMap;
use windows::Foundation::Numerics::Matrix3x2;
use windows::Win32::Graphics::Direct2D::Common::{D2D1_COLOR_F, D2D1_FIGURE_BEGIN_FILLED, D2D1_FIGURE_END_CLOSED, D2D_POINT_2F, D2D_RECT_F};
use windows::Win32::Graphics::Direct2D::{
    ID2D1Factory, ID2D1RenderTarget, ID2D1SolidColorBrush, ID2D1StrokeStyle, D2D1_CAP_STYLE_ROUND, D2D1_DASH_STYLE_SOLID,
    D2D1_DRAW_TEXT_OPTIONS_NONE, D2D1_ELLIPSE, D2D1_LINE_JOIN_ROUND, D2D1_ROUNDED_RECT, D2D1_STROKE_STYLE_PROPERTIES,
};
use windows::Win32::Graphics::DirectWrite::{
    DWriteCreateFactory, IDWriteFactory, IDWriteTextFormat, DWRITE_FACTORY_TYPE_SHARED, DWRITE_FONT_STRETCH_NORMAL,
    DWRITE_FONT_STYLE_NORMAL, DWRITE_FONT_WEIGHT, DWRITE_FONT_WEIGHT_LIGHT, DWRITE_FONT_WEIGHT_BOLD, DWRITE_FONT_WEIGHT_NORMAL, DWRITE_FONT_WEIGHT_SEMI_LIGHT,
    DWRITE_MEASURING_MODE_NATURAL, DWRITE_TEXT_ALIGNMENT, DWRITE_TEXT_ALIGNMENT_CENTER, DWRITE_TEXT_ALIGNMENT_LEADING,
    DWRITE_TEXT_ALIGNMENT_TRAILING, DWRITE_TEXT_METRICS,
};

/// Hora local, sin depender de zonas horarias en Rust.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LocalTime {
    pub weekday: u16,
    pub day: u16,
    pub month: u16,
    pub hour: u16,
    pub minute: u16,
    pub second: u16,
}

impl LocalTime {
    pub fn now() -> Self {
        let t = unsafe { windows::Win32::System::SystemInformation::GetLocalTime() };
        Self { weekday: t.wDayOfWeek, day: t.wDay, month: t.wMonth, hour: t.wHour, minute: t.wMinute, second: t.wSecond }
    }
}

/// ¿Hay que repintar cada segundo (o basta cada minuto)?
pub fn ticks_every_second(settings: &ClockConfig) -> bool {
    settings.seconds
}

/// Textos del reloj: saludo (estilo con saludo), hora y fecha.
pub fn texts(settings: &ClockConfig, t: LocalTime) -> (Option<String>, String, Option<String>) {
    let time = if settings.hour24 {
        if settings.seconds { format!("{:02}:{:02}:{:02}", t.hour, t.minute, t.second) } else { format!("{:02}:{:02}", t.hour, t.minute) }
    } else {
        let hour = match t.hour % 12 { 0 => 12, h => h };
        let suffix = if t.hour < 12 { "AM" } else { "PM" };
        if settings.seconds { format!("{hour}:{:02}:{:02} {suffix}", t.minute, t.second) } else { format!("{hour}:{:02} {suffix}", t.minute) }
    };
    let date = settings.date.then(|| {
        let weekday = weekday_name(t);
        let month = month_name(t);
        crate::i18n::t("{weekday}, {month} {day}")
            .replace("{weekday}", &weekday)
            .replace("{month}", &month)
            .replace("{day}", &t.day.to_string())
    });
    let greeting = (settings.style == ClockStyle::Greeting).then(|| {
        crate::i18n::t(match t.hour {
            5..=11 => "Good morning",
            12..=19 => "Good afternoon",
            _ => "Good evening",
        })
    });
    (greeting, time, date)
}

/// Zona del fondo (rejilla de 3x3, por filas) sobre la que cae el reloj.
pub fn region(position: ClockPosition) -> usize {
    match position {
        ClockPosition::Center => 4,
        ClockPosition::TopLeft => 0,
        ClockPosition::TopRight => 2,
        ClockPosition::Left => 3,
        ClockPosition::Right => 5,
        ClockPosition::BottomLeft => 6,
        ClockPosition::BottomRight => 8,
    }
}

/// Brillo medio (0..1) de cada zona de una rejilla de 3x3 de una imagen BGRA.
pub fn region_luma(pixels: &[u8], width: u32, height: u32) -> [f32; 9] {
    let (w, h) = (width.max(1) as usize, height.max(1) as usize);
    let mut sums = [0f64; 9];
    let mut counts = [0u32; 9];
    // Muestreo: unas 64x36 muestras bastan.
    let step_x = (w / 64).max(1);
    let step_y = (h / 36).max(1);
    for y in (0..h).step_by(step_y) {
        for x in (0..w).step_by(step_x) {
            let p = (y * w + x) * 4;
            if p + 3 >= pixels.len() { continue; }
            let (b, g, r) = (pixels[p] as f64, pixels[p + 1] as f64, pixels[p + 2] as f64);
            let cell = (y * 3 / h) * 3 + x * 3 / w;
            sums[cell] += (0.2126 * r + 0.7152 * g + 0.0722 * b) / 255.0;
            counts[cell] += 1;
        }
    }
    let mut out = [0.5f32; 9];
    for i in 0..9 {
        if counts[i] > 0 { out[i] = (sums[i] / counts[i] as f64) as f32; }
    }
    out
}

fn color(rgb: (f32, f32, f32), a: f32) -> D2D1_COLOR_F { D2D1_COLOR_F { r: rgb.0, g: rgb.1, b: rgb.2, a } }

/// Fuentes del collage (familia y grosor): cada recorte con una, como letras de revista.
const COLLAGE_FONTS: [(&str, i32); 6] = [
    ("Arial Black", 900),
    ("Georgia", 700),
    ("Impact", 400),
    ("Segoe UI", 900),
    ("Times New Roman", 700),
    ("Bahnschrift", 700),
];
const PAPER: (f32, f32, f32) = (0.97, 0.97, 0.95);
const INK: (f32, f32, f32) = (0.05, 0.05, 0.06);
const RED: (f32, f32, f32) = (0.86, 0.06, 0.12);

/// Cómo es un recorte del collage.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Kind {
    /// Papel blanco, letra negra.
    Paper,
    /// Papel negro, letra blanca y borde blanco.
    Ink,
    /// Papel rojo, letra blanca.
    Red,
    /// Solo la letra (en la franja, o los dos puntos).
    Bare,
}

struct Cutout {
    text: String,
    /// Centro, en coordenadas del bloque.
    center: (f32, f32),
    size: (f32, f32),
    px: f32,
    font: usize,
    kind: Kind,
    /// Giro en grados.
    angle: f32,
    seed: u32,
}

/// Número pseudoaleatorio estable entre 0 y 1 (mismo resultado para la misma semilla).
fn noise(seed: u32) -> f32 {
    let mut x = seed.wrapping_mul(0x9E37_79B9) ^ 0x85EB_CA6B;
    x ^= x >> 15;
    x = x.wrapping_mul(0x2C1B_3C6D);
    x ^= x >> 12;
    x = x.wrapping_mul(0x297A_2D39);
    x ^= x >> 15;
    x as f32 / u32::MAX as f32
}

/// Como `noise`, entre -1 y 1.
fn jitter(seed: u32) -> f32 { noise(seed) * 2.0 - 1.0 }

/// Giro de `degrees` alrededor de (cx, cy).
fn rotation(degrees: f32, cx: f32, cy: f32) -> Matrix3x2 {
    let (s, c) = degrees.to_radians().sin_cos();
    Matrix3x2 { M11: c, M12: s, M21: -s, M22: c, M31: cx - cx * c + cy * s, M32: cy - cx * s - cy * c }
}

/// Primero `a`, después `b`.
fn mul(a: &Matrix3x2, b: &Matrix3x2) -> Matrix3x2 {
    Matrix3x2 {
        M11: a.M11 * b.M11 + a.M12 * b.M21,
        M12: a.M11 * b.M12 + a.M12 * b.M22,
        M21: a.M21 * b.M11 + a.M22 * b.M21,
        M22: a.M21 * b.M12 + a.M22 * b.M22,
        M31: a.M31 * b.M11 + a.M32 * b.M21 + b.M31,
        M32: a.M31 * b.M12 + a.M32 * b.M22 + b.M32,
    }
}

unsafe fn fill_polygon(target: &ID2D1RenderTarget, factory: &ID2D1Factory, brush: &ID2D1SolidColorBrush, points: &[D2D_POINT_2F], fill: D2D1_COLOR_F) {
    let Ok(geometry) = factory.CreatePathGeometry() else { return; };
    let Ok(sink) = geometry.Open() else { return; };
    sink.BeginFigure(points[0], D2D1_FIGURE_BEGIN_FILLED);
    sink.AddLines(&points[1..]);
    sink.EndFigure(D2D1_FIGURE_END_CLOSED);
    if sink.Close().is_err() { return; }
    brush.SetColor(&fill);
    target.FillGeometry(&geometry, brush, None);
}

/// Nombre del día de la semana, traducido.
fn weekday_name(t: LocalTime) -> String {
    const DAYS: [&str; 7] = ["Sunday", "Monday", "Tuesday", "Wednesday", "Thursday", "Friday", "Saturday"];
    crate::i18n::t(DAYS[(t.weekday as usize) % 7])
}

/// Nombre del mes, traducido.
fn month_name(t: LocalTime) -> String {
    const MONTHS: [&str; 12] = ["January", "February", "March", "April", "May", "June", "July", "August", "September", "October", "November", "December"];
    crate::i18n::t(MONTHS[(t.month.clamp(1, 12) - 1) as usize])
}

const FONT: &str = "Segoe UI Variable Display";

pub struct ClockPainter {
    dwrite: IDWriteFactory,
    formats: HashMap<(&'static str, u32, i32, i32), IDWriteTextFormat>,
    round: Option<ID2D1StrokeStyle>,
    /// Proporciones de cada fuente (ascendente, altura de cifras), para no consultarlas cada vez.
    ratios: HashMap<(&'static str, i32), (f32, f32)>,
}

impl ClockPainter {
    pub fn new(factory: &ID2D1Factory) -> Option<Self> {
        unsafe {
            let dwrite: IDWriteFactory = DWriteCreateFactory(DWRITE_FACTORY_TYPE_SHARED).ok()?;
            let properties = D2D1_STROKE_STYLE_PROPERTIES {
                startCap: D2D1_CAP_STYLE_ROUND,
                endCap: D2D1_CAP_STYLE_ROUND,
                dashCap: D2D1_CAP_STYLE_ROUND,
                lineJoin: D2D1_LINE_JOIN_ROUND,
                miterLimit: 1.0,
                dashStyle: D2D1_DASH_STYLE_SOLID,
                dashOffset: 0.0,
            };
            let round = factory.CreateStrokeStyle(&properties, None).ok();
            Some(Self { dwrite, formats: HashMap::new(), round, ratios: HashMap::new() })
        }
    }

    fn format(&mut self, size: f32, weight: DWRITE_FONT_WEIGHT, align: DWRITE_TEXT_ALIGNMENT) -> Option<IDWriteTextFormat> {
        self.format_with(FONT, size, weight, align)
    }

    fn format_with(&mut self, family: &'static str, size: f32, weight: DWRITE_FONT_WEIGHT, align: DWRITE_TEXT_ALIGNMENT) -> Option<IDWriteTextFormat> {
        let key = (family, size.round() as u32, weight.0, align.0);
        if let Some(format) = self.formats.get(&key) { return Some(format.clone()); }
        let family_w: Vec<u16> = family.encode_utf16().chain(std::iter::once(0)).collect();
        let format = unsafe {
            let format = self.dwrite.CreateTextFormat(windows::core::PCWSTR(family_w.as_ptr()), None, weight, DWRITE_FONT_STYLE_NORMAL, DWRITE_FONT_STRETCH_NORMAL, size.round().max(1.0), windows::core::w!("")).ok()?;
            let _ = format.SetTextAlignment(align);
            let _ = format.SetWordWrapping(windows::Win32::Graphics::DirectWrite::DWRITE_WORD_WRAPPING_NO_WRAP);
            format
        };
        // Pocas combinaciones; se limpia si crecen (cambios de tamaño o de monitor).
        if self.formats.len() > 64 { self.formats.clear(); }
        self.formats.insert(key, format.clone());
        Some(format)
    }

    fn measure(&self, text: &str, format: &IDWriteTextFormat) -> (f32, f32) {
        let wide: Vec<u16> = text.encode_utf16().collect();
        unsafe {
            let Ok(layout) = self.dwrite.CreateTextLayout(&wide, format, 10000.0, 10000.0) else { return (0.0, 0.0); };
            let mut metrics = DWRITE_TEXT_METRICS::default();
            if layout.GetMetrics(&mut metrics).is_err() { return (0.0, 0.0); }
            (metrics.widthIncludingTrailingWhitespace, metrics.height)
        }
    }

    /// Dibuja el reloj en `area` (un monitor, en coordenadas del lienzo). `luma`: brillo del
    /// fondo en la zona del reloj (0 negro, 1 blanco).
    pub fn draw(&mut self, target: &ID2D1RenderTarget, factory: &ID2D1Factory, area: Area, settings: &ClockConfig, luma: f32, now: LocalTime) {
        let Ok(brush) = (unsafe { target.CreateSolidColorBrush(&color((1.0, 1.0, 1.0), 1.0), None) }) else { return; };
        let light_bg = luma > 0.6;
        let scale = (area.height() / 1080.0).max(0.5);
        let size = match settings.size { ClockSize::Small => 0.72, ClockSize::Medium => 1.0, ClockSize::Large => 1.4 } * scale;
        let (greeting, time, date) = texts(settings, now);
        match settings.style {
            ClockStyle::Analog => self.draw_analog(target, factory, &brush, area, settings, light_bg, size, now, date),
            ClockStyle::Pill => self.draw_pill(target, &brush, area, settings, light_bg, size, &time, date),
            ClockStyle::Stacked => self.draw_stacked(target, &brush, area, settings, light_bg, size, now, date),
            ClockStyle::Collage => self.draw_collage(target, factory, &brush, area, settings, size, &time, now),
            ClockStyle::Large | ClockStyle::Greeting => self.draw_large(target, &brush, area, settings, light_bg, size, greeting, &time, date),
        }
    }

    /// Proporciones de la fuente respecto a su tamaño: (ascendente, altura de las mayúsculas y cifras).
    fn font_ratios(&self, family: &str, weight: DWRITE_FONT_WEIGHT) -> (f32, f32) {
        let fallback = (0.93, 0.70);
        unsafe {
            let mut collection = None;
            if self.dwrite.GetSystemFontCollection(&mut collection, false).is_err() { return fallback; }
            let Some(collection) = collection else { return fallback; };
            let family_w: Vec<u16> = family.encode_utf16().chain(std::iter::once(0)).collect();
            let mut index = 0u32;
            let mut exists = windows::Win32::Foundation::BOOL(0);
            if collection.FindFamilyName(windows::core::PCWSTR(family_w.as_ptr()), &mut index, &mut exists).is_err() || !exists.as_bool() { return fallback; }
            let Ok(font_family) = collection.GetFontFamily(index) else { return fallback; };
            let Ok(font) = font_family.GetFirstMatchingFont(weight, DWRITE_FONT_STRETCH_NORMAL, DWRITE_FONT_STYLE_NORMAL) else { return fallback; };
            let mut metrics = windows::Win32::Graphics::DirectWrite::DWRITE_FONT_METRICS::default();
            font.GetMetrics(&mut metrics);
            let em = metrics.designUnitsPerEm.max(1) as f32;
            (metrics.ascent as f32 / em, metrics.capHeight as f32 / em)
        }
    }

    /// Collage de recortes de revista: cada cifra en su propio recorte inclinado, con otra
    /// fuente; blanco, negro y un toque rojo, con sombra dura. Debajo, el día de la semana en
    /// una franja negra sesgada y el día del mes con el mes en dos recortes más.
    #[allow(clippy::too_many_arguments)]
    fn draw_collage(&mut self, target: &ID2D1RenderTarget, factory: &ID2D1Factory, brush: &ID2D1SolidColorBrush, area: Area, settings: &ClockConfig, size: f32, time: &str, t: LocalTime) {
        let unit = 200.0 * size;
        // Semilla por minuto del día: los recortes cambian de forma con la hora, no a cada repintado.
        let base_seed = 7919;

        // Fila de la hora: cifras sueltas; los dos puntos en rojo sin recorte; segundos y AM/PM más pequeños.
        let mut row: Vec<Cutout> = Vec::new();
        let mut x = 0.0f32;
        let mut colons = 0;
        let mut tokens: Vec<String> = Vec::new();
        for c in time.chars() {
            match tokens.last_mut() {
                Some(last) if c.is_alphabetic() && last.chars().all(char::is_alphabetic) => last.push(c),
                _ => tokens.push(c.to_string()),
            }
        }
        let digit_count = tokens.iter().filter(|tok| tok.chars().all(|c| c.is_ascii_digit())).count().max(1);
        let red_digit = (noise(base_seed + t.hour as u32 * 61 + t.minute as u32) * digit_count as f32) as usize % digit_count;
        let mut digit_index = 0;
        for (i, token) in tokens.iter().enumerate() {
            let seed = base_seed.wrapping_add(i as u32 * 131).wrapping_add(token.chars().next().map(|c| c as u32).unwrap_or(0) * 17);
            if token == " " { x += unit * 0.12; continue; }
            if token == ":" {
                colons += 1;
                let px = unit * if colons > 1 { 0.6 } else { 0.85 };
                let (w, _) = self.cutout_size(":", px, 0);
                row.push(Cutout { text: ":".into(), center: (x + w * 0.5, unit * 0.5), size: (w, unit * 0.6), px, font: 0, kind: Kind::Bare, angle: jitter(seed) * 6.0, seed });
                x += w * 0.9;
                continue;
            }
            let small = colons > 1 || token.chars().all(char::is_alphabetic);
            let scale = if small { 0.6 } else { 1.0 };
            let px = unit * 0.8 * scale * (1.0 + 0.2 * jitter(seed + 3));
            let font = (noise(seed + 5) * COLLAGE_FONTS.len() as f32) as usize % COLLAGE_FONTS.len();
            let (w, h) = self.cutout_size(token, px, font);
            let kind = if token.chars().all(|c| c.is_ascii_digit()) {
                digit_index += 1;
                if digit_index - 1 == red_digit { Kind::Red } else if noise(seed + 9) < 0.3 { Kind::Ink } else { Kind::Paper }
            } else {
                Kind::Red
            };
            // Los pequeños se apoyan abajo, como recortes pegados al lado.
            let cy = if small { unit - h * 0.45 } else { unit * 0.5 + jitter(seed + 11) * unit * 0.08 };
            row.push(Cutout { text: token.clone(), center: (x + w * 0.5, cy), size: (w, h), px, font, kind, angle: jitter(seed + 13) * 10.0, seed });
            x += w * 0.94;
        }
        let row_w = x;

        // Día de la semana en una franja y, al lado, el día y el mes.
        let mut banner: Option<(Vec<Cutout>, D2D_RECT_F)> = None;
        let mut date_cuts: Vec<Cutout> = Vec::new();
        if settings.date {
            let band_h = unit * 0.46;
            let band_left = row_w * 0.16;
            let band_top = unit * 1.06;
            let mut letters = Vec::new();
            let mut lx = band_left + band_h * 0.45;
            for (i, letter) in weekday_name(t).to_uppercase().chars().enumerate() {
                let seed = base_seed.wrapping_add(1000 + i as u32 * 37 + letter as u32);
                let font = (noise(seed) * COLLAGE_FONTS.len() as f32) as usize % COLLAGE_FONTS.len();
                let px = band_h * 0.66 * (1.0 + 0.14 * jitter(seed + 1));
                let text = letter.to_string();
                let (w, h) = self.cutout_size(&text, px, font);
                let kind = if noise(seed + 2) < 0.22 { Kind::Paper } else { Kind::Bare };
                let w = if kind == Kind::Bare { w * 0.78 } else { w };
                letters.push(Cutout { text, center: (lx + w * 0.5, band_top + band_h * 0.5 + jitter(seed + 3) * band_h * 0.06), size: (w, h * 0.9), px, font, kind, angle: jitter(seed + 4) * 8.0, seed });
                lx += w * 0.92;
            }
            let band = D2D_RECT_F { left: band_left, top: band_top, right: lx + band_h * 0.45, bottom: band_top + band_h };
            // Día del mes (rojo) y mes abreviado, solapando el final de la franja.
            let day_seed = base_seed + 2000 + t.day as u32;
            let day = t.day.to_string();
            let (dw, dh) = self.cutout_size(&day, unit * 0.46, 1);
            let day_x = band.right - dw * 0.15;
            date_cuts.push(Cutout { text: day, center: (day_x + dw * 0.5, band_top + band_h * 0.42), size: (dw, dh), px: unit * 0.46, font: 1, kind: Kind::Red, angle: jitter(day_seed) * 6.0 - 4.0, seed: day_seed });
            let month: String = month_name(t).chars().take(3).collect::<String>().to_uppercase();
            let (mw, mh) = self.cutout_size(&month, unit * 0.3, 0);
            let month_x = day_x + dw * 0.85;
            date_cuts.push(Cutout { text: month, center: (month_x + mw * 0.5, band_top + band_h * 0.62), size: (mw, mh), px: unit * 0.3, font: 0, kind: Kind::Paper, angle: jitter(day_seed + 1) * 6.0 + 3.0, seed: day_seed + 1 });
            banner = Some((letters, band));
        }

        // Caja total (con margen por las inclinaciones) y colocación en el monitor.
        let mut right = row_w;
        let mut bottom = unit;
        if let Some((_, band)) = &banner { bottom = bottom.max(band.bottom); right = right.max(band.right); }
        for cut in &date_cuts { right = right.max(cut.center.0 + cut.size.0 * 0.5); bottom = bottom.max(cut.center.1 + cut.size.1 * 0.5); }
        let margin = unit * 0.16;
        let (px0, py0) = Self::place(area, settings.position, right + margin * 2.0, bottom + margin * 2.0, size);
        let (ox, oy) = (px0 + margin, py0 + margin);

        unsafe {
            let mut saved = Matrix3x2::identity();
            target.GetTransform(&mut saved);
            // Todo el bloque, un poco inclinado.
            let group = mul(&rotation(-7.0, right * 0.5, bottom * 0.5), &mul(&Matrix3x2::translation(ox, oy), &saved));
            for cut in &row {
                self.draw_cutout(target, factory, brush, cut, &group);
            }
            if let Some((letters, band)) = &banner {
                let band_group = mul(&rotation(3.0, (band.left + band.right) * 0.5, (band.top + band.bottom) * 0.5), &group);
                target.SetTransform(&band_group);
                let skew = (band.bottom - band.top) * 0.35;
                let corners = |grow: f32, dx: f32, dy: f32| -> [D2D_POINT_2F; 4] {
                    [
                        D2D_POINT_2F { x: band.left + skew - grow + dx, y: band.top - grow + dy },
                        D2D_POINT_2F { x: band.right + grow + dx, y: band.top - grow * 0.6 + dy },
                        D2D_POINT_2F { x: band.right - skew + grow + dx, y: band.bottom + grow + dy },
                        D2D_POINT_2F { x: band.left - grow + dx, y: band.bottom + grow * 0.6 + dy },
                    ]
                };
                let shadow = unit * 0.05;
                fill_polygon(target, factory, brush, &corners(unit * 0.035, shadow, shadow * 1.2), color((0.0, 0.0, 0.0), 0.9));
                fill_polygon(target, factory, brush, &corners(unit * 0.035, 0.0, 0.0), color(PAPER, 1.0));
                fill_polygon(target, factory, brush, &corners(0.0, 0.0, 0.0), color(INK, 1.0));
                for cut in letters {
                    self.draw_cutout(target, factory, brush, cut, &band_group);
                }
            }
            for cut in &date_cuts {
                self.draw_cutout(target, factory, brush, cut, &group);
            }
            target.SetTransform(&saved);
        }
    }

    /// Ancho y alto del recorte de `text` a `px` con la fuente `font` del collage.
    fn cutout_size(&mut self, text: &str, px: f32, font: usize) -> (f32, f32) {
        let (family, weight) = COLLAGE_FONTS[font % COLLAGE_FONTS.len()];
        let Some(format) = self.format_with(family, px, DWRITE_FONT_WEIGHT(weight), DWRITE_TEXT_ALIGNMENT_CENTER) else { return (px, px); };
        let (w, _) = self.measure(text, &format);
        let (_, cap) = self.cached_ratios(family, DWRITE_FONT_WEIGHT(weight));
        (w + px * 0.3, cap * px + px * 0.36)
    }

    /// Un recorte: sombra dura, borde, cara y su letra, girado sobre su centro.
    unsafe fn draw_cutout(&mut self, target: &ID2D1RenderTarget, factory: &ID2D1Factory, brush: &ID2D1SolidColorBrush, cut: &Cutout, group: &Matrix3x2) {
        let (cx, cy) = cut.center;
        target.SetTransform(&mul(&rotation(cut.angle, cx, cy), group));
        let (w, h) = cut.size;
        let (face, ink, edge) = match cut.kind {
            Kind::Paper => (PAPER, INK, INK),
            Kind::Ink => (INK, PAPER, PAPER),
            Kind::Red => (RED, PAPER, INK),
            Kind::Bare => (INK, if cut.text == ":" { RED } else { PAPER }, INK),
        };
        if cut.kind != Kind::Bare {
            // Esquinas un poco irregulares, como cortadas a tijera (siempre igual para la misma semilla).
            let rough = w.min(h) * 0.07;
            let quad = |grow: f32, dx: f32, dy: f32| -> [D2D_POINT_2F; 4] {
                let corner = |i: u32, sx: f32, sy: f32| D2D_POINT_2F {
                    x: cx + sx * (w * 0.5 + grow) + jitter(cut.seed + 20 + i) * rough + dx,
                    y: cy + sy * (h * 0.5 + grow) + jitter(cut.seed + 30 + i) * rough + dy,
                };
                [corner(0, -1.0, -1.0), corner(1, 1.0, -1.0), corner(2, 1.0, 1.0), corner(3, -1.0, 1.0)]
            };
            let border = h * 0.07;
            let shadow = h * 0.08;
            fill_polygon(target, factory, brush, &quad(border, shadow, shadow * 1.25), color((0.0, 0.0, 0.0), 0.9));
            fill_polygon(target, factory, brush, &quad(border, 0.0, 0.0), color(edge, 1.0));
            fill_polygon(target, factory, brush, &quad(0.0, 0.0, 0.0), color(face, 1.0));
        }
        let (family, weight) = COLLAGE_FONTS[cut.font % COLLAGE_FONTS.len()];
        let weight = DWRITE_FONT_WEIGHT(weight);
        let Some(format) = self.format_with(family, cut.px, weight, DWRITE_TEXT_ALIGNMENT_CENTER) else { return; };
        let (ascent, cap) = self.cached_ratios(family, weight);
        // Centra la altura de las mayúsculas y cifras en el recorte.
        let top = cy - (ascent - cap * 0.5) * cut.px;
        let rect = D2D_RECT_F { left: cx - w * 2.0, top, right: cx + w * 2.0, bottom: top + cut.px * 1.6 };
        let wide: Vec<u16> = cut.text.encode_utf16().collect();
        if cut.kind == Kind::Bare {
            // Sin recorte: contorno oscuro para que se lea sobre la franja y sobre el fondo.
            brush.SetColor(&color((0.0, 0.0, 0.0), 0.9));
            let o = cut.px * 0.04;
            for (dx, dy) in [(-o, 0.0), (o, 0.0), (0.0, -o), (0.0, o), (o, o * 1.6)] {
                let r = D2D_RECT_F { left: rect.left + dx, right: rect.right + dx, top: rect.top + dy, bottom: rect.bottom + dy };
                target.DrawText(&wide, &format, &r, brush, D2D1_DRAW_TEXT_OPTIONS_NONE, DWRITE_MEASURING_MODE_NATURAL);
            }
        }
        brush.SetColor(&color(ink, 1.0));
        target.DrawText(&wide, &format, &rect, brush, D2D1_DRAW_TEXT_OPTIONS_NONE, DWRITE_MEASURING_MODE_NATURAL);
    }

    fn cached_ratios(&mut self, family: &'static str, weight: DWRITE_FONT_WEIGHT) -> (f32, f32) {
        if let Some(r) = self.ratios.get(&(family, weight.0)) { return *r; }
        let r = self.font_ratios(family, weight);
        self.ratios.insert((family, weight.0), r);
        r
    }

    /// Cifras gruesas en dos filas (horas y minutos; los segundos, en una tercera más tenue),
    /// muy juntas, con la fecha pequeña debajo.
    #[allow(clippy::too_many_arguments)]
    fn draw_stacked(&mut self, target: &ID2D1RenderTarget, brush: &ID2D1SolidColorBrush, area: Area, settings: &ClockConfig, light_bg: bool, size: f32, t: LocalTime, date: Option<String>) {
        let align = Self::align(settings.position);
        let px = 260.0 * size;
        let weight = DWRITE_FONT_WEIGHT_BOLD;
        let (Some(big), Some(small)) = (self.format(px, weight, align), self.format(px * 0.11, DWRITE_FONT_WEIGHT_NORMAL, align)) else { return; };
        let (ascent, cap) = self.font_ratios(FONT, weight);
        let hour = if settings.hour24 { format!("{:02}", t.hour) } else { (match t.hour % 12 { 0 => 12, h => h }).to_string() };
        let mut rows: Vec<(String, f32)> = vec![(hour, 1.0), (format!("{:02}", t.minute), 1.0)];
        if settings.seconds { rows.push((format!("{:02}", t.second), 0.55)); }
        let mut caption: Vec<String> = Vec::new();
        if !settings.hour24 { caption.push(if t.hour < 12 { "AM".into() } else { "PM".into() }); }
        if let Some(d) = date { caption.push(d); }
        let caption = (!caption.is_empty()).then(|| caption.join(" · "));
        let digit_h = cap * px;
        let gap = px * 0.13;
        let caption_h = if caption.is_some() { px * 0.26 } else { 0.0 };
        let width = rows.iter().map(|(text, _)| self.measure(text, &big).0).fold(0.0, f32::max)
            .max(caption.as_ref().map(|c| self.measure(c, &small).0).unwrap_or(0.0));
        let height = rows.len() as f32 * digit_h + (rows.len() - 1) as f32 * gap + caption_h;
        let (x, y) = Self::place(area, settings.position, width, height, size);
        let (left, right) = match settings.position {
            ClockPosition::Center => (area.left, area.right),
            _ => (x, x + width),
        };
        let mut top = y;
        for (text, alpha) in &rows {
            // La caja del texto empieza por encima de las cifras (ascendente menos altura de cifra).
            let box_top = top - (ascent - cap) * px;
            Self::shadowed_text(target, brush, text, &big, D2D_RECT_F { left, top: box_top, right, bottom: box_top + px * 1.4 }, light_bg, *alpha, 3.0 * size);
            top += digit_h + gap;
        }
        if let Some(c) = caption {
            let top = top - gap + px * 0.1;
            Self::shadowed_text(target, brush, &c, &small, D2D_RECT_F { left, top, right, bottom: top + px * 0.2 }, light_bg, 0.9, 1.2 * size);
        }
    }

    /// Margen al borde del monitor y punto de anclaje del bloque de `w` x `h`.
    fn place(area: Area, position: ClockPosition, w: f32, h: f32, scale: f32) -> (f32, f32) {
        let margin = area.height() * 0.07;
        // Abajo deja sitio para la barra de tareas.
        let bottom = area.bottom - margin - 40.0 * scale;
        match position {
            ClockPosition::Center => ((area.left + area.right - w) / 2.0, area.top + area.height() * 0.42 - h / 2.0),
            ClockPosition::TopLeft => (area.left + margin, area.top + margin),
            ClockPosition::TopRight => (area.right - margin - w, area.top + margin),
            ClockPosition::Left => (area.left + margin, area.top + (area.height() - h) / 2.0),
            ClockPosition::Right => (area.right - margin - w, area.top + (area.height() - h) / 2.0),
            ClockPosition::BottomLeft => (area.left + margin, bottom - h),
            ClockPosition::BottomRight => (area.right - margin - w, bottom - h),
        }
    }

    fn align(position: ClockPosition) -> DWRITE_TEXT_ALIGNMENT {
        match position {
            ClockPosition::Center => DWRITE_TEXT_ALIGNMENT_CENTER,
            ClockPosition::TopLeft | ClockPosition::Left | ClockPosition::BottomLeft => DWRITE_TEXT_ALIGNMENT_LEADING,
            ClockPosition::TopRight | ClockPosition::Right | ClockPosition::BottomRight => DWRITE_TEXT_ALIGNMENT_TRAILING,
        }
    }

    fn ink(light_bg: bool) -> (f32, f32, f32) { if light_bg { (0.08, 0.08, 0.10) } else { (1.0, 1.0, 1.0) } }

    /// Texto con una sombra suave (dos pasadas) para leerse sobre cualquier fondo.
    #[allow(clippy::too_many_arguments)]
    fn shadowed_text(target: &ID2D1RenderTarget, brush: &ID2D1SolidColorBrush, text: &str, format: &IDWriteTextFormat, rect: D2D_RECT_F, light_bg: bool, alpha: f32, offset: f32) {
        let wide: Vec<u16> = text.encode_utf16().collect();
        let shadow = if light_bg { (1.0, 1.0, 1.0) } else { (0.0, 0.0, 0.0) };
        unsafe {
            for (dy, a) in [(offset * 2.0, 0.16), (offset, 0.28)] {
                brush.SetColor(&color(shadow, a * alpha));
                let r = D2D_RECT_F { top: rect.top + dy, bottom: rect.bottom + dy, ..rect };
                target.DrawText(&wide, format, &r, brush, D2D1_DRAW_TEXT_OPTIONS_NONE, DWRITE_MEASURING_MODE_NATURAL);
            }
            brush.SetColor(&color(Self::ink(light_bg), alpha));
            target.DrawText(&wide, format, &rect, brush, D2D1_DRAW_TEXT_OPTIONS_NONE, DWRITE_MEASURING_MODE_NATURAL);
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn draw_large(&mut self, target: &ID2D1RenderTarget, brush: &ID2D1SolidColorBrush, area: Area, settings: &ClockConfig, light_bg: bool, size: f32, greeting: Option<String>, time: &str, date: Option<String>) {
        let align = Self::align(settings.position);
        let time_px = 150.0 * size;
        let (Some(time_format), Some(small), Some(greet)) = (
            self.format(time_px, DWRITE_FONT_WEIGHT_LIGHT, align),
            self.format(time_px * 0.19, DWRITE_FONT_WEIGHT_NORMAL, align),
            self.format(time_px * 0.26, DWRITE_FONT_WEIGHT_SEMI_LIGHT, align),
        ) else { return; };
        let mut lines: Vec<(String, IDWriteTextFormat, f32, f32)> = Vec::new();
        if let Some(g) = greeting { lines.push((g, greet.clone(), time_px * 0.34, 0.92)); }
        // La hora ocupa menos que su caja de texto: se junta con lo de arriba y lo de abajo.
        lines.push((time.to_string(), time_format.clone(), time_px * 1.08, 1.0));
        if let Some(d) = date { lines.push((d, small.clone(), time_px * 0.3, 0.9)); }
        let width = lines.iter().map(|(text, format, _, _)| self.measure(text, format).0).fold(0.0, f32::max);
        let height: f32 = lines.iter().map(|(_, _, h, _)| *h).sum();
        let (x, mut y) = Self::place(area, settings.position, width, height, size);
        let (left, right) = match settings.position {
            ClockPosition::Center => (area.left, area.right),
            _ => (x, x + width),
        };
        for (text, format, h, alpha) in &lines {
            let (_, text_h) = self.measure(text, format);
            // Centrar el texto en su franja.
            let top = y + (h - text_h) / 2.0;
            Self::shadowed_text(target, brush, text, format, D2D_RECT_F { left, top, right, bottom: top + text_h }, light_bg, *alpha, 1.5 * size);
            y += h;
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn draw_pill(&mut self, target: &ID2D1RenderTarget, brush: &ID2D1SolidColorBrush, area: Area, settings: &ClockConfig, light_bg: bool, size: f32, time: &str, date: Option<String>) {
        let time_px = 64.0 * size;
        let (Some(time_format), Some(small)) = (
            self.format(time_px, DWRITE_FONT_WEIGHT_SEMI_LIGHT, DWRITE_TEXT_ALIGNMENT_LEADING),
            self.format(time_px * 0.3, DWRITE_FONT_WEIGHT_NORMAL, DWRITE_TEXT_ALIGNMENT_LEADING),
        ) else { return; };
        let (tw, th) = self.measure(time, &time_format);
        let (dw, dh) = date.as_ref().map(|d| self.measure(d, &small)).unwrap_or((0.0, 0.0));
        let pad_x = time_px * 0.5;
        let pad_y = time_px * 0.22;
        let w = tw.max(dw) + pad_x * 2.0;
        let h = th + dh + pad_y * 2.0 - if date.is_some() { time_px * 0.08 } else { 0.0 };
        let (x, y) = Self::place(area, settings.position, w, h, size);
        let rect = D2D_RECT_F { left: x, top: y, right: x + w, bottom: y + h };
        let radius = (h / 2.0).min(time_px * 0.7);
        let shape = D2D1_ROUNDED_RECT { rect, radiusX: radius, radiusY: radius };
        unsafe {
            // Sombra muy suave y cristal translúcido.
            for i in 1..=4 {
                let grow = i as f32 * 3.0 * size;
                brush.SetColor(&color((0.0, 0.0, 0.0), 0.05));
                let r = D2D_RECT_F { left: rect.left - grow * 0.5, top: rect.top + grow * 0.3, right: rect.right + grow * 0.5, bottom: rect.bottom + grow };
                target.FillRoundedRectangle(&D2D1_ROUNDED_RECT { rect: r, radiusX: radius + grow, radiusY: radius + grow }, brush);
            }
            brush.SetColor(&if light_bg { color((1.0, 1.0, 1.0), 0.55) } else { color((0.06, 0.06, 0.08), 0.42) });
            target.FillRoundedRectangle(&shape, brush);
            brush.SetColor(&if light_bg { color((0.0, 0.0, 0.0), 0.08) } else { color((1.0, 1.0, 1.0), 0.16) });
            target.DrawRoundedRectangle(&shape, brush, 1.2 * size, None);
            let ink = Self::ink(light_bg);
            let left = x + pad_x;
            brush.SetColor(&color(ink, 1.0));
            let wide: Vec<u16> = time.encode_utf16().collect();
            let time_top = y + pad_y - time_px * 0.04;
            target.DrawText(&wide, &time_format, &D2D_RECT_F { left, top: time_top, right: left + tw + 4.0, bottom: time_top + th }, brush, D2D1_DRAW_TEXT_OPTIONS_NONE, DWRITE_MEASURING_MODE_NATURAL);
            if let Some(d) = date {
                brush.SetColor(&color(ink, 0.72));
                let wide: Vec<u16> = d.encode_utf16().collect();
                let top = time_top + th - time_px * 0.08;
                target.DrawText(&wide, &small, &D2D_RECT_F { left, top, right: left + dw + 4.0, bottom: top + dh }, brush, D2D1_DRAW_TEXT_OPTIONS_NONE, DWRITE_MEASURING_MODE_NATURAL);
            }
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn draw_analog(&mut self, target: &ID2D1RenderTarget, _factory: &ID2D1Factory, brush: &ID2D1SolidColorBrush, area: Area, settings: &ClockConfig, light_bg: bool, size: f32, t: LocalTime, date: Option<String>) {
        let radius = 115.0 * size;
        let small = self.format(radius * 0.17, DWRITE_FONT_WEIGHT_NORMAL, DWRITE_TEXT_ALIGNMENT_CENTER);
        let date_h = if date.is_some() { radius * 0.45 } else { 0.0 };
        let (x, y) = Self::place(area, settings.position, radius * 2.0, radius * 2.0 + date_h, size);
        let center = D2D_POINT_2F { x: x + radius, y: y + radius };
        let ink = Self::ink(light_bg);
        let round = self.round.clone();
        let line = |from: f32, to: f32, angle_deg: f32, width: f32, rgb: (f32, f32, f32), a: f32| unsafe {
            let angle = (angle_deg - 90.0).to_radians();
            let (c, s) = (angle.cos(), angle.sin());
            brush.SetColor(&color(rgb, a));
            target.DrawLine(
                D2D_POINT_2F { x: center.x + c * from, y: center.y + s * from },
                D2D_POINT_2F { x: center.x + c * to, y: center.y + s * to },
                brush,
                width,
                round.as_ref(),
            );
        };
        unsafe {
            // Sombra suave y esfera translúcida.
            for i in 1..=4 {
                let grow = i as f32 * 3.0 * size;
                brush.SetColor(&color((0.0, 0.0, 0.0), 0.05));
                let r = radius + grow;
                target.FillEllipse(&D2D1_ELLIPSE { point: D2D_POINT_2F { x: center.x, y: center.y + grow * 0.6 }, radiusX: r, radiusY: r }, brush);
            }
            brush.SetColor(&if light_bg { color((1.0, 1.0, 1.0), 0.5) } else { color((0.06, 0.06, 0.08), 0.38) });
            target.FillEllipse(&D2D1_ELLIPSE { point: center, radiusX: radius, radiusY: radius }, brush);
            brush.SetColor(&color(ink, 0.35));
            target.DrawEllipse(&D2D1_ELLIPSE { point: center, radiusX: radius, radiusY: radius }, brush, 1.5 * size, None);
        }
        // Marcas: las de las 12, 3, 6 y 9 más largas.
        for i in 0..12 {
            let major = i % 3 == 0;
            let outer = radius * 0.86;
            let inner = if major { radius * 0.72 } else { radius * 0.79 };
            line(inner, outer, i as f32 * 30.0, if major { 4.0 } else { 2.0 } * size, ink, if major { 0.9 } else { 0.55 });
        }
        let (h, m, s) = (t.hour as f32 % 12.0, t.minute as f32, t.second as f32);
        let minute_angle = (m + if settings.seconds { s / 60.0 } else { 0.0 }) * 6.0;
        line(-radius * 0.08, radius * 0.48, (h + m / 60.0) * 30.0, 7.0 * size, ink, 1.0);
        line(-radius * 0.08, radius * 0.7, minute_angle, 4.5 * size, ink, 1.0);
        let (r, g, b) = crate::winutil::accent_rgb();
        let accent = (r as f32 / 255.0, g as f32 / 255.0, b as f32 / 255.0);
        if settings.seconds {
            line(-radius * 0.16, radius * 0.8, s * 6.0, 2.0 * size, accent, 1.0);
        }
        unsafe {
            brush.SetColor(&color(if settings.seconds { accent } else { ink }, 1.0));
            let dot = 6.0 * size;
            target.FillEllipse(&D2D1_ELLIPSE { point: center, radiusX: dot, radiusY: dot }, brush);
        }
        if let (Some(d), Some(format)) = (date, small) {
            let top = center.y + radius + radius * 0.1;
            Self::shadowed_text(target, brush, &d, &format, D2D_RECT_F { left: center.x - radius * 3.0, top, right: center.x + radius * 3.0, bottom: top + radius * 0.35 }, light_bg, 0.92, 1.2 * size);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(hour: u16, minute: u16, second: u16) -> LocalTime {
        LocalTime { weekday: 6, day: 15, month: 11, hour, minute, second }
    }

    #[test]
    fn formats_12_and_24_hours() {
        let mut settings = ClockConfig { date: false, ..Default::default() };
        assert_eq!(texts(&settings, at(9, 5, 7)).1, "09:05");
        settings.seconds = true;
        assert_eq!(texts(&settings, at(9, 5, 7)).1, "09:05:07");
        settings.hour24 = false;
        settings.seconds = false;
        assert_eq!(texts(&settings, at(0, 30, 0)).1, "12:30 AM");
        assert_eq!(texts(&settings, at(14, 32, 0)).1, "2:32 PM");
        assert_eq!(texts(&settings, at(12, 0, 0)).1, "12:00 PM");
    }

    #[test]
    fn greeting_follows_the_hour() {
        let settings = ClockConfig { style: ClockStyle::Greeting, ..Default::default() };
        assert_eq!(texts(&settings, at(8, 0, 0)).0, Some(crate::i18n::t("Good morning")));
        assert_eq!(texts(&settings, at(15, 0, 0)).0, Some(crate::i18n::t("Good afternoon")));
        assert_eq!(texts(&settings, at(23, 0, 0)).0, Some(crate::i18n::t("Good evening")));
        assert_eq!(texts(&ClockConfig::default(), at(8, 0, 0)).0, None);
    }

    #[test]
    fn luma_grid_splits_the_image() {
        // 3x3 píxeles: blanco arriba a la izquierda, negro el resto.
        let mut pixels = vec![0u8; 3 * 3 * 4];
        pixels[..4].copy_from_slice(&[255, 255, 255, 255]);
        let luma = region_luma(&pixels, 3, 3);
        assert!((luma[0] - 1.0).abs() < 0.01);
        assert!(luma[4] < 0.01 && luma[8] < 0.01);
    }

    /// Pinta los estilos del reloj sobre una imagen y los guarda como BMP para revisarlos:
    /// `SWEPR_TEST_IMAGE=<imagen> SWEPR_TEST_OUT=<prefijo> cargo test render_clock_preview -- --ignored`
    #[test]
    #[ignore]
    fn render_clock_preview() {
        use crate::panel::d2d::Surface;
        let image = std::env::var("SWEPR_TEST_IMAGE").expect("SWEPR_TEST_IMAGE");
        let out = std::env::var("SWEPR_TEST_OUT").expect("SWEPR_TEST_OUT");
        let (w, h) = (1920, 1080);
        unsafe {
            use windows::Win32::System::Com::{CoInitializeEx, COINIT_APARTMENTTHREADED};
            let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
            let surface = Surface::new(w, h).expect("Direct2D");
            let target = surface.rt();
            // Se decodifica como en el lienzo (relleno a 1920x1080) y se recorta el centro.
            let factory = super::super::wic_factory().unwrap();
            let job = super::super::DecodeJob {
                path: std::path::PathBuf::from(&image), target: super::super::Target::All, fit: crate::config::FitMode::Fill,
                duration: std::time::Duration::ZERO, effect: crate::config::Transition::Fade, area_size: (w as u32, h as u32),
            };
            let (pixels, iw, ih) = super::super::decode(&factory, &job).unwrap();
            let properties = windows::Win32::Graphics::Direct2D::D2D1_BITMAP_PROPERTIES {
                pixelFormat: windows::Win32::Graphics::Direct2D::Common::D2D1_PIXEL_FORMAT {
                    format: windows::Win32::Graphics::Dxgi::Common::DXGI_FORMAT_B8G8R8A8_UNORM,
                    alphaMode: windows::Win32::Graphics::Direct2D::Common::D2D1_ALPHA_MODE_PREMULTIPLIED,
                },
                dpiX: 96.0,
                dpiY: 96.0,
            };
            let backdrop = target.CreateBitmap(windows::Win32::Graphics::Direct2D::Common::D2D_SIZE_U { width: iw, height: ih }, Some(pixels.as_ptr() as *const _), iw * 4, &properties).unwrap();
            let luma = region_luma(&pixels, iw, ih);
            let mut painter = ClockPainter::new(&surface.factory).expect("DirectWrite");
            let area = Area::new(0.0, 0.0, w as f32, h as f32);
            let now = at(16, 42, 9);
            let cases = [
                ("large", ClockConfig { enabled: true, style: ClockStyle::Large, position: ClockPosition::Center, ..Default::default() }),
                ("pill", ClockConfig { enabled: true, style: ClockStyle::Pill, position: ClockPosition::TopRight, ..Default::default() }),
                ("analog", ClockConfig { enabled: true, style: ClockStyle::Analog, position: ClockPosition::BottomRight, seconds: true, ..Default::default() }),
                ("greeting", ClockConfig { enabled: true, style: ClockStyle::Greeting, position: ClockPosition::BottomLeft, size: ClockSize::Large, hour24: false, ..Default::default() }),
            ];
            let cases: Vec<(&str, ClockConfig)> = cases.into_iter().chain([
                ("stacked", ClockConfig { enabled: true, style: ClockStyle::Stacked, position: ClockPosition::Center, ..Default::default() }),
                ("stacked-left", ClockConfig { enabled: true, style: ClockStyle::Stacked, position: ClockPosition::Left, seconds: true, ..Default::default() }),
                ("collage", ClockConfig { enabled: true, style: ClockStyle::Collage, position: ClockPosition::Center, ..Default::default() }),
                ("collage-corner", ClockConfig { enabled: true, style: ClockStyle::Collage, position: ClockPosition::BottomRight, hour24: false, seconds: true, ..Default::default() }),
            ]).collect();
            for (name, settings) in cases {
                target.BeginDraw();
                let (sw, sh) = (iw as f32, ih as f32);
                let k = (w as f32 / sw).max(h as f32 / sh);
                let (dw, dh) = (sw * k, sh * k);
                let dest = D2D_RECT_F { left: (w as f32 - dw) / 2.0, top: (h as f32 - dh) / 2.0, right: (w as f32 + dw) / 2.0, bottom: (h as f32 + dh) / 2.0 };
                target.DrawBitmap(&backdrop, Some(&dest), 1.0, windows::Win32::Graphics::Direct2D::D2D1_BITMAP_INTERPOLATION_MODE_LINEAR, None);
                painter.draw(&target, &surface.factory, area, &settings, luma[region(settings.position)], now);
                target.EndDraw(None, None).unwrap();
                crate::panel::save_bmp(surface.dc, surface.dib, w, h, &format!("{out}-{name}.bmp"));
            }
        }
    }
}