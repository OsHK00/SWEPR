//! Fondos animados (GIF y WebP) para el lienzo: se decodifican todos los fotogramas ya
//! compuestos, reducidos para no pasar de un presupuesto de memoria, y el lienzo los va
//! copiando a un único mapa de bits de la GPU según su duración.
//!
//! La animación se pausa sola con juegos o presentaciones a pantalla completa, con una
//! ventana maximizada que tapa el monitor, con la sesión bloqueada y con el ahorro de batería.

use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

/// Memoria máxima para los fotogramas de una animación (en RAM; la GPU solo guarda uno). Si no
/// caben, se reducen a la mitad hasta que quepan.
const BUDGET_BYTES: u64 = 160 * 1024 * 1024;
/// Fotogramas como mucho (las animaciones más largas se cortan aquí).
const MAX_FRAMES: usize = 1500;
/// Retardos menores que este se tratan como 100 ms, como hacen los navegadores.
const MIN_DELAY_MS: u64 = 20;

static ENABLED: AtomicBool = AtomicBool::new(true);

/// Reproducir o no los fondos animados (sin reproducción se ve el primer fotograma).
pub fn set_enabled(enabled: bool) { ENABLED.store(enabled, Ordering::Relaxed); }

pub fn enabled() -> bool { ENABLED.load(Ordering::Relaxed) }

/// ¿Puede ser animada por su extensión?
pub fn is_animated_format(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| e.eq_ignore_ascii_case("gif") || e.eq_ignore_ascii_case("webp"))
}

/// Fotogramas en BGRA premultiplicado, todos del mismo tamaño.
pub struct Animation {
    pub frames: Vec<Vec<u8>>,
    pub delays: Vec<Duration>,
    pub width: u32,
    pub height: u32,
}

fn open_frames(path: &Path) -> Result<image::Frames<'static>, String> {
    use image::AnimationDecoder;
    let file = std::io::BufReader::new(std::fs::File::open(path).map_err(|e| e.to_string())?);
    let is_gif = path.extension().and_then(|e| e.to_str()).is_some_and(|e| e.eq_ignore_ascii_case("gif"));
    if is_gif {
        Ok(image::codecs::gif::GifDecoder::new(file).map_err(|e| e.to_string())?.into_frames())
    } else {
        let decoder = image::codecs::webp::WebPDecoder::new(file).map_err(|e| e.to_string())?;
        Ok(decoder.into_frames())
    }
}

/// Decodifica una animación a lo sumo de `max_size` (ya calculado para el monitor), en una sola
/// pasada: si los fotogramas no caben en el presupuesto, se reducen a la mitad los ya guardados
/// y se sigue. `cancel` se consulta en cada fotograma (otra imagen pedida mientras tanto).
/// Con un solo fotograma devuelve igualmente una "animación" de uno.
pub fn decode(path: &Path, max_size: impl Fn((u32, u32)) -> (u32, u32), cancel: impl Fn() -> bool) -> Result<Animation, String> {
    let mut frames: Vec<Vec<u8>> = Vec::new();
    let mut delays = Vec::new();
    let mut size = (0u32, 0u32);
    for frame in open_frames(path)? {
        if cancel() { return Err("cancelada".into()); }
        let frame = frame.map_err(|e| e.to_string())?;
        let (numer, denom) = frame.delay().numer_denom_ms();
        let ms = if denom == 0 { 0 } else { (numer / denom) as u64 };
        delays.push(Duration::from_millis(if ms < MIN_DELAY_MS { 100 } else { ms }));
        let buffer = frame.into_buffer();
        let native = buffer.dimensions();
        if frames.is_empty() { size = max_size(native); }
        frames.push(resample(buffer.as_raw(), native, size, true));
        let bytes = size.0 as u64 * size.1 as u64 * 4 * frames.len() as u64;
        if bytes > BUDGET_BYTES && size.0 > 1 && size.1 > 1 {
            let half = (size.0 / 2, size.1 / 2);
            for stored in frames.iter_mut() {
                *stored = resample(stored, size, half, false);
            }
            size = half;
        }
        if frames.len() >= MAX_FRAMES { break; }
    }
    if frames.is_empty() || size.0 == 0 || size.1 == 0 {
        return Err("sin fotogramas".into());
    }
    Ok(Animation { frames, delays, width: size.0, height: size.1 })
}

/// Solo el primer fotograma (para GIF/WebP sin animar cuando WIC no sabe abrirlos).
pub fn first_frame(path: &Path, max_size: impl Fn((u32, u32)) -> (u32, u32)) -> Result<(Vec<u8>, u32, u32), String> {
    let frame = open_frames(path)?.next().ok_or("sin fotogramas")?.map_err(|e| e.to_string())?;
    let buffer = frame.into_buffer();
    let native = buffer.dimensions();
    let size = max_size(native);
    Ok((resample(buffer.as_raw(), native, size, true), size.0, size.1))
}

/// Reduce a `to` píxeles promediando el área de origen de cada píxel (en dos pasadas: filas y
/// columnas). Con `from_rgba`, la entrada es RGBA normal y sale BGRA premultiplicado; si no,
/// entra y sale BGRA premultiplicado.
fn resample(src: &[u8], from: (u32, u32), to: (u32, u32), from_rgba: bool) -> Vec<u8> {
    let (fw, fh) = (from.0 as usize, from.1 as usize);
    let (tw, th) = (to.0.min(from.0).max(1) as usize, to.1.min(from.1).max(1) as usize);
    // Se promedia ya premultiplicado para que los bordes transparentes no se manchen.
    let premultiplied;
    let src: &[u8] = if from_rgba {
        premultiplied = premultiply_bgra(src);
        &premultiplied
    } else {
        src
    };
    if (tw, th) == (fw, fh) {
        return src.to_vec();
    }
    let spans = |src: usize, dst: usize| -> Vec<(usize, usize)> {
        (0..dst).map(|i| {
            let start = i * src / dst;
            (start, ((i + 1) * src).div_ceil(dst).max(start + 1).min(src))
        }).collect()
    };
    // Filas: cada fila de origen pasa a `tw` píxeles.
    let columns = spans(fw, tw);
    let mut rows = vec![0u8; tw * fh * 4];
    for y in 0..fh {
        let line = &src[y * fw * 4..(y + 1) * fw * 4];
        let out = &mut rows[y * tw * 4..(y + 1) * tw * 4];
        for (x, &(x0, x1)) in columns.iter().enumerate() {
            let (mut c0, mut c1, mut c2, mut c3) = (0u32, 0u32, 0u32, 0u32);
            for px in line[x0 * 4..x1 * 4].chunks_exact(4) {
                c0 += px[0] as u32;
                c1 += px[1] as u32;
                c2 += px[2] as u32;
                c3 += px[3] as u32;
            }
            let inv = reciprocal(x1 - x0);
            let o = &mut out[x * 4..x * 4 + 4];
            o[0] = scale(c0, inv);
            o[1] = scale(c1, inv);
            o[2] = scale(c2, inv);
            o[3] = scale(c3, inv);
        }
    }
    // Columnas: grupos de filas a `th` filas.
    let mut out = vec![0u8; tw * th * 4];
    let mut sum = vec![0u32; tw * 4];
    for (y, (y0, y1)) in spans(fh, th).into_iter().enumerate() {
        sum.iter_mut().for_each(|v| *v = 0);
        for row in rows[y0 * tw * 4..y1 * tw * 4].chunks_exact(tw * 4) {
            for (acc, v) in sum.iter_mut().zip(row) { *acc += *v as u32; }
        }
        let inv = reciprocal(y1 - y0);
        for (o, acc) in out[y * tw * 4..(y + 1) * tw * 4].iter_mut().zip(&sum) { *o = scale(*acc, inv); }
    }
    out
}

/// 1/n en coma fija (16 bits), para promediar multiplicando en vez de dividir.
fn reciprocal(n: usize) -> u32 { ((1u32 << 16) + n as u32 / 2) / n as u32 }

fn scale(sum: u32, reciprocal: u32) -> u8 { ((sum * reciprocal + (1 << 15)) >> 16).min(255) as u8 }

/// RGBA → BGRA premultiplicado.
fn premultiply_bgra(rgba: &[u8]) -> Vec<u8> {
    // c * a / 255 redondeado, sin dividir.
    let mul = |c: u8, a: u32| (((c as u32 * a + 128) * 257) >> 16) as u8;
    let mut out = vec![0u8; rgba.len()];
    for (o, px) in out.chunks_exact_mut(4).zip(rgba.chunks_exact(4)) {
        let a = px[3] as u32;
        o[0] = mul(px[2], a);
        o[1] = mul(px[1], a);
        o[2] = mul(px[0], a);
        o[3] = px[3];
    }
    out
}

/// Por qué conviene parar las animaciones ahora (None = pueden seguir): juegos o
/// presentaciones a pantalla completa, sesión bloqueada o ahorro de batería activado.
/// Con batería sin ahorro siguen: Windows activa el ahorro solo cuando queda poca.
pub fn pause_reason() -> Option<&'static str> {
    if !crate::winutil::is_notification_allowed() {
        return Some("pantalla completa o presentación");
    }
    unsafe {
        let mut power = windows::Win32::System::Power::SYSTEM_POWER_STATUS::default();
        // SystemStatusFlag 1 = ahorro de batería (o de energía) activado.
        if windows::Win32::System::Power::GetSystemPowerStatus(&mut power).is_ok() && power.SystemStatusFlag == 1 {
            return Some("ahorro de batería");
        }
        // Con la sesión bloqueada no hay ventana en primer plano.
        if windows::Win32::UI::WindowsAndMessaging::GetForegroundWindow().is_invalid() {
            return Some("sesión bloqueada");
        }
    }
    None
}

/// Rectángulo (en pantalla) del monitor que tapa por completo la ventana en primer plano
/// (maximizada o a pantalla completa), si la hay. El escritorio y la barra de tareas no cuentan.
pub fn covered_monitor() -> Option<windows::Win32::Foundation::RECT> {
    use windows::Win32::Graphics::Gdi::{GetMonitorInfoW, MonitorFromWindow, MONITORINFO, MONITOR_DEFAULTTONULL};
    use windows::Win32::UI::WindowsAndMessaging::{GetClassNameW, GetForegroundWindow, GetWindowRect, IsIconic, IsZoomed};
    unsafe {
        let window = GetForegroundWindow();
        if window.is_invalid() || IsIconic(window).as_bool() { return None; }
        let mut class = [0u16; 64];
        let len = GetClassNameW(window, &mut class).max(0) as usize;
        let class = String::from_utf16_lossy(&class[..len]);
        if matches!(class.as_str(), "Progman" | "WorkerW" | "Shell_TrayWnd" | "Shell_SecondaryTrayWnd") {
            return None;
        }
        let monitor = MonitorFromWindow(window, MONITOR_DEFAULTTONULL);
        if monitor.is_invalid() { return None; }
        let mut info = MONITORINFO { cbSize: std::mem::size_of::<MONITORINFO>() as u32, ..Default::default() };
        if !GetMonitorInfoW(monitor, &mut info).as_bool() { return None; }
        let mut rect = windows::Win32::Foundation::RECT::default();
        let _ = GetWindowRect(window, &mut rect);
        let work = info.rcWork;
        let covers = rect.left <= work.left && rect.top <= work.top && rect.right >= work.right && rect.bottom >= work.bottom;
        (IsZoomed(window).as_bool() || covers).then_some(info.rcMonitor)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn downscale_averages_and_premultiplies() {
        // 2x1 RGBA: rojo opaco y transparente → 1 píxel rojo a media opacidad, premultiplicado.
        let rgba = [255, 0, 0, 255, 0, 255, 0, 0];
        let out = resample(&rgba, (2, 1), (1, 1), true);
        assert_eq!(out, vec![0, 0, 128, 128]);
        // Mismo tamaño: solo cambia el orden de los canales.
        let out = resample(&[10, 20, 30, 255], (1, 1), (1, 1), true);
        assert_eq!(out, vec![30, 20, 10, 255]);
        // Ya en BGRA: solo se promedia.
        let out = resample(&[10, 20, 30, 40, 30, 40, 50, 60], (2, 1), (1, 1), false);
        assert_eq!(out, vec![20, 30, 40, 50]);
    }

    #[test]
    fn decodes_every_gif_frame_with_its_delay() {
        use image::codecs::gif::GifEncoder;
        use image::{Delay, Frame, Rgba, RgbaImage};
        let path = std::env::temp_dir().join(format!("swepr-anim-{}.gif", std::process::id()));
        {
            let file = std::fs::File::create(&path).unwrap();
            let mut encoder = GifEncoder::new(file);
            for (color, ms) in [([255, 0, 0, 255], 50), ([0, 0, 255, 255], 0)] {
                let image = RgbaImage::from_pixel(8, 4, Rgba(color));
                encoder.encode_frame(Frame::from_parts(image, 0, 0, Delay::from_numer_denom_ms(ms, 1))).unwrap();
            }
        }
        let animation = decode(&path, |native| native, || false).unwrap();
        let _ = std::fs::remove_file(&path);
        assert_eq!((animation.width, animation.height), (8, 4));
        assert_eq!(animation.frames.len(), 2);
        assert_eq!(animation.delays, vec![Duration::from_millis(50), Duration::from_millis(100)]);
        // Primer fotograma rojo, segundo azul (BGRA).
        assert_eq!(&animation.frames[0][..4], &[0, 0, 255, 255]);
        assert_eq!(&animation.frames[1][..4], &[255, 0, 0, 255]);
    }
}

#[cfg(test)]
mod probe {
    /// Tiempo y memoria al decodificar una animación real para un monitor 1920x1080.
    /// `SWEPR_ANIM_FILE=ruta.gif cargo test anim_decode_probe -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn anim_decode_probe() {
        let path = std::path::PathBuf::from(std::env::var("SWEPR_ANIM_FILE").expect("SWEPR_ANIM_FILE"));
        let started = std::time::Instant::now();
        let animation = super::decode(&path, |native| super::super::layout::decode_size(crate::config::FitMode::Fill, native, (1920, 1080)), || false).unwrap();
        let bytes: usize = animation.frames.iter().map(|f| f.len()).sum();
        eprintln!(
            "{} fotogramas a {}x{} en {} ms, {} MB",
            animation.frames.len(), animation.width, animation.height, started.elapsed().as_millis(), bytes / (1024 * 1024)
        );
    }
}
