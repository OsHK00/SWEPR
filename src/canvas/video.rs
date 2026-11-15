//! Fondos de vídeo (MP4, MOV, MKV, WebM...) para el lienzo, con Media Foundation.
//!
//! Un hilo por vídeo lee los fotogramas ya escalados al tamaño del monitor (el procesador de
//! vídeo de Media Foundation hace la conversión a BGRA y el escalado) y deja el último listo
//! para el lienzo, que lo copia al mapa de bits de la GPU igual que con los GIF. El vídeo se
//! repite sin fin, sin sonido, y se pausa en los mismos casos que las animaciones.
//!
//! Windows no admite vídeos como fondo: para el fondo real (y la pantalla de bloqueo) se guarda
//! el primer fotograma como JPEG en `%LOCALAPPDATA%\swepr\video-stills`, junto a un `.txt` con
//! la ruta del vídeo para poder reconocerlo al leer el fondo de Windows.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, Once};
use std::time::{Duration, Instant};
use windows::core::{HSTRING, PCWSTR};
use windows::Win32::Media::MediaFoundation::*;

use crate::{log_info, log_warn};

/// ¿Es un vídeo por su extensión?
pub fn is_video(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .is_some_and(|ext| crate::scanner::VIDEO_EXTENSIONS.iter().any(|video| ext.eq_ignore_ascii_case(video)))
}

fn startup() -> Result<(), String> {
    static START: Once = Once::new();
    static OK: AtomicBool = AtomicBool::new(false);
    START.call_once(|| match unsafe { MFStartup(MF_VERSION, MFSTARTUP_NOSOCKET) } {
        Ok(()) => OK.store(true, Ordering::Release),
        Err(e) => crate::log_error!("Media Foundation no disponible: {e}"),
    });
    if OK.load(Ordering::Acquire) { Ok(()) } else { Err("Media Foundation no disponible".into()) }
}

const VIDEO_STREAM: u32 = MF_SOURCE_READER_FIRST_VIDEO_STREAM.0 as u32;

/// Lector abierto con su tamaño de salida y la distancia entre filas.
struct Reader {
    reader: IMFSourceReader,
    size: (u32, u32),
    stride: i32,
}

/// Abre el vídeo para leer fotogramas BGRA de `size(nativo)` píxeles (redondeado a par).
unsafe fn open(path: &Path, size: impl Fn((u32, u32)) -> (u32, u32)) -> Result<Reader, String> {
    startup()?;
    let mut attributes: Option<IMFAttributes> = None;
    MFCreateAttributes(&mut attributes, 1).map_err(|e| e.to_string())?;
    let attributes = attributes.ok_or("sin atributos")?;
    // Conversión de color y escalado dentro de Media Foundation.
    attributes.SetUINT32(&MF_SOURCE_READER_ENABLE_ADVANCED_VIDEO_PROCESSING, 1).map_err(|e| e.to_string())?;
    let reader = MFCreateSourceReaderFromURL(&HSTRING::from(path.as_os_str()), &attributes).map_err(|e| format!("no se pudo abrir: {e}"))?;
    let _ = reader.SetStreamSelection(MF_SOURCE_READER_ALL_STREAMS.0 as u32, false);
    reader.SetStreamSelection(VIDEO_STREAM, true).map_err(|e| format!("sin pista de vídeo: {e}"))?;
    let native = reader.GetNativeMediaType(VIDEO_STREAM, 0).map_err(|e| format!("sin pista de vídeo: {e}"))?;
    let packed = native.GetUINT64(&MF_MT_FRAME_SIZE).map_err(|e| e.to_string())?;
    let native = ((packed >> 32) as u32, packed as u32);
    if native.0 == 0 || native.1 == 0 { return Err("tamaño de vídeo desconocido".into()); }
    let wanted = size(native);
    let wanted = ((wanted.0 & !1).max(2), (wanted.1 & !1).max(2));

    let output = |frame_size: Option<(u32, u32)>| -> windows::core::Result<IMFMediaType> {
        let media = MFCreateMediaType()?;
        media.SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Video)?;
        media.SetGUID(&MF_MT_SUBTYPE, &MFVideoFormat_RGB32)?;
        if let Some((w, h)) = frame_size {
            media.SetUINT64(&MF_MT_FRAME_SIZE, ((w as u64) << 32) | h as u64)?;
        }
        Ok(media)
    };
    // Si el procesador no sabe escalar a ese tamaño, al nativo (la GPU lo escala al dibujar).
    let scaled = output(Some(wanted)).and_then(|media| reader.SetCurrentMediaType(VIDEO_STREAM, None, &media));
    if scaled.is_err() {
        let media = output(None).map_err(|e| e.to_string())?;
        reader.SetCurrentMediaType(VIDEO_STREAM, None, &media).map_err(|e| format!("no se puede convertir a BGRA: {e}"))?;
    }
    let mut reader = Reader { reader, size: (0, 0), stride: 0 };
    reader.refresh_format()?;
    Ok(reader)
}

impl Reader {
    /// Lee el tamaño y el paso de filas del formato de salida actual.
    unsafe fn refresh_format(&mut self) -> Result<(), String> {
        let current = self.reader.GetCurrentMediaType(VIDEO_STREAM).map_err(|e| e.to_string())?;
        let packed = current.GetUINT64(&MF_MT_FRAME_SIZE).map_err(|e| e.to_string())?;
        self.size = ((packed >> 32) as u32, packed as u32);
        self.stride = current.GetUINT32(&MF_MT_DEFAULT_STRIDE).map(|s| s as i32).unwrap_or(self.size.0 as i32 * 4);
        Ok(())
    }

    /// Lee el siguiente fotograma en `out` (BGRA opaco, filas seguidas de arriba abajo).
    /// Devuelve su instante en unidades de 100 ns, o None al final del vídeo.
    unsafe fn read(&mut self, out: &mut Vec<u8>) -> Result<Option<i64>, String> {
        loop {
            let mut flags = 0u32;
            let mut timestamp = 0i64;
            let mut sample: Option<IMFSample> = None;
            self.reader
                .ReadSample(VIDEO_STREAM, 0, None, Some(&mut flags), Some(&mut timestamp), Some(&mut sample))
                .map_err(|e| e.to_string())?;
            if flags & MF_SOURCE_READERF_ERROR.0 as u32 != 0 { return Err("error al decodificar".into()); }
            if flags & MF_SOURCE_READERF_ENDOFSTREAM.0 as u32 != 0 { return Ok(None); }
            // El decodificador concreta el formato al leer el primer fotograma (paso de filas,
            // a veces el tamaño): se vuelve a leer.
            if flags & MF_SOURCE_READERF_CURRENTMEDIATYPECHANGED.0 as u32 != 0 {
                self.refresh_format()?;
            }
            // Sin muestra: un hueco en la pista (se pide la siguiente).
            let Some(sample) = sample else { continue; };
            self.copy(&sample, out)?;
            return Ok(Some(timestamp));
        }
    }

    unsafe fn copy(&self, sample: &IMFSample, out: &mut Vec<u8>) -> Result<(), String> {
        let (w, h) = (self.size.0 as usize, self.size.1 as usize);
        let row = w * 4;
        out.resize(row * h, 0);
        let buffer = sample.ConvertToContiguousBuffer().map_err(|e| e.to_string())?;
        // Con IMF2DBuffer, la primera fila y su paso (negativo si está guardado de abajo arriba).
        if let Ok(buffer2d) = windows::core::Interface::cast::<IMF2DBuffer>(&buffer) {
            let mut scan0: *mut u8 = std::ptr::null_mut();
            let mut pitch = 0i32;
            buffer2d.Lock2D(&mut scan0, &mut pitch).map_err(|e| e.to_string())?;
            for y in 0..h {
                let src = scan0.offset(y as isize * pitch as isize);
                std::ptr::copy_nonoverlapping(src, out.as_mut_ptr().add(y * row), row);
            }
            let _ = buffer2d.Unlock2D();
        } else {
            let mut data: *mut u8 = std::ptr::null_mut();
            let mut length = 0u32;
            buffer.Lock(&mut data, None, Some(&mut length)).map_err(|e| e.to_string())?;
            let pitch = self.stride.unsigned_abs() as usize;
            if (length as usize) < pitch * (h - 1) + row {
                let _ = buffer.Unlock();
                return Err("fotograma incompleto".into());
            }
            for y in 0..h {
                // Paso negativo: la primera fila de la imagen es la última del búfer.
                let line = if self.stride < 0 { h - 1 - y } else { y };
                std::ptr::copy_nonoverlapping(data.add(line * pitch), out.as_mut_ptr().add(y * row), row);
            }
            let _ = buffer.Unlock();
        }
        // RGB32 deja el cuarto byte sin definir: opaco, para Direct2D (alfa premultiplicado).
        for pixel in out.chunks_exact_mut(4) {
            pixel[3] = 255;
        }
        Ok(())
    }

    /// Vuelve al principio para repetir.
    unsafe fn rewind(&self) -> Result<(), String> {
        let start = windows::core::PROPVARIANT::from(0i64);
        self.reader.SetCurrentPosition(&windows::core::GUID::zeroed(), &start).map_err(|e| e.to_string())
    }
}

/// El primer fotograma a `size(nativo)` píxeles (redondeado a par), en BGRA opaco.
pub fn first_frame(path: &Path, size: impl Fn((u32, u32)) -> (u32, u32)) -> Result<(Vec<u8>, u32, u32), String> {
    unsafe {
        let mut reader = open(path, size)?;
        let mut pixels = Vec::new();
        match reader.read(&mut pixels)? {
            Some(_) => Ok((pixels, reader.size.0, reader.size.1)),
            None => Err("el vídeo no tiene fotogramas".into()),
        }
    }
}

/// Lo que comparten el hilo del vídeo y el lienzo.
struct Shared {
    /// Último fotograma decodificado.
    frame: Mutex<Vec<u8>>,
    fresh: AtomicBool,
    paused: AtomicBool,
    stop: AtomicBool,
}

/// Reproducción de un vídeo en su propio hilo. Al soltarlo, el hilo termina.
pub struct Player {
    shared: Arc<Shared>,
}

/// Si va con más retraso que esto, se saltan fotogramas para ponerse al día.
const LATE: Duration = Duration::from_millis(60);
/// Aunque vaya con retraso, se enseña algún fotograma cada tanto.
const MAX_SKIP: Duration = Duration::from_millis(250);

impl Player {
    /// Empieza a reproducir `path` con fotogramas de exactamente `size` píxeles.
    pub fn start(path: &Path, size: (u32, u32)) -> Self {
        let shared = Arc::new(Shared {
            frame: Mutex::new(Vec::new()),
            fresh: AtomicBool::new(false),
            paused: AtomicBool::new(false),
            stop: AtomicBool::new(false),
        });
        let thread_shared = shared.clone();
        let path = path.to_path_buf();
        let spawned = std::thread::Builder::new().name("swepr-video".into()).spawn(move || {
            use windows::Win32::System::Com::{CoInitializeEx, CoUninitialize, COINIT_MULTITHREADED};
            unsafe { let _ = CoInitializeEx(None, COINIT_MULTITHREADED); }
            if let Err(e) = play(&path, size, &thread_shared) {
                log_warn!("Lienzo: el vídeo {:?} se queda quieto: {e}", path.file_name().unwrap_or_default());
            }
            unsafe { CoUninitialize(); }
        });
        if let Err(e) = spawned {
            log_warn!("Lienzo: no se pudo empezar el vídeo: {e}");
        }
        Self { shared }
    }

    pub fn set_paused(&self, paused: bool) {
        self.shared.paused.store(paused, Ordering::Relaxed);
    }

    /// Si hay un fotograma nuevo, se lo pasa a `use_frame`. Devuelve si lo había.
    pub fn take_frame(&self, use_frame: impl FnOnce(&[u8])) -> bool {
        if !self.shared.fresh.swap(false, Ordering::AcqRel) { return false; }
        let Ok(frame) = self.shared.frame.lock() else { return false; };
        if frame.is_empty() { return false; }
        use_frame(&frame);
        true
    }
}

impl Drop for Player {
    fn drop(&mut self) {
        self.shared.stop.store(true, Ordering::Release);
    }
}

fn play(path: &Path, size: (u32, u32), shared: &Shared) -> Result<(), String> {
    let mut reader = unsafe { open(path, |_| size)? };
    log_info!("Lienzo: reproduciendo {:?} a {}x{}", path.file_name().unwrap_or_default(), size.0, size.1);
    let stopped = || shared.stop.load(Ordering::Acquire);
    let mut buffer = Vec::new();
    let mut clock = Instant::now();
    let mut first: Option<i64> = None;
    let mut last_shown = Instant::now();
    let mut frames_this_loop = 0usize;
    while !stopped() {
        // En pausa no se decodifica; al seguir, el reloj se corre lo que duró la pausa.
        if shared.paused.load(Ordering::Relaxed) {
            let since = Instant::now();
            while shared.paused.load(Ordering::Relaxed) && !stopped() {
                std::thread::sleep(Duration::from_millis(100));
            }
            clock += since.elapsed();
            continue;
        }
        let Some(timestamp) = (unsafe { reader.read(&mut buffer)? }) else {
            if frames_this_loop == 0 { return Err("el vídeo no tiene fotogramas".into()); }
            unsafe { reader.rewind()?; }
            first = None;
            frames_this_loop = 0;
            continue;
        };
        // El mapa de bits de la GPU es de `size`: un fotograma de otro tamaño no cabe.
        if reader.size != size {
            return Err(format!("sale a {}x{} en vez de {}x{}", reader.size.0, reader.size.1, size.0, size.1));
        }
        frames_this_loop += 1;
        let base = *first.get_or_insert_with(|| {
            // Cada vuelta empieza ahora (sin correr para recuperar lo que tardó en volver).
            clock = Instant::now();
            timestamp
        });
        let due = clock + Duration::from_nanos((timestamp - base).max(0) as u64 * 100);
        let now = Instant::now();
        if now > due + LATE && now.duration_since(last_shown) < MAX_SKIP {
            continue;
        }
        // Espera a su momento en tramos cortos, para atender a tiempo una pausa o el final.
        while !stopped() && !shared.paused.load(Ordering::Relaxed) {
            let left = due.saturating_duration_since(Instant::now());
            if left.is_zero() { break; }
            std::thread::sleep(left.min(Duration::from_millis(50)));
        }
        if stopped() { break; }
        if let Ok(mut frame) = shared.frame.lock() {
            std::mem::swap(&mut *frame, &mut buffer);
        }
        shared.fresh.store(true, Ordering::Release);
        last_shown = Instant::now();
        super::wake_canvas();
    }
    Ok(())
}

// --- Fotograma fijo para Windows ---

fn stills_dir() -> Option<PathBuf> {
    std::env::var("LOCALAPPDATA").ok().map(|dir| PathBuf::from(dir).join("swepr").join("video-stills"))
}

/// Cuántos fotogramas fijos se guardan como mucho (los más antiguos se borran).
const MAX_STILLS: usize = 40;

/// Lo que se le puede dar a Windows como fondo: la propia imagen, o el primer fotograma de un
/// vídeo guardado como JPEG (si no se puede, la ruta tal cual y que Windows decida).
pub fn windows_image(path: &Path) -> PathBuf {
    if !is_video(path) { return path.to_path_buf(); }
    match still_of(path) {
        Ok(still) => still,
        Err(e) => {
            log_warn!("No se pudo sacar el fotograma fijo de {:?}: {e}", path);
            path.to_path_buf()
        }
    }
}

/// El vídeo del que salió un fotograma fijo de SWEPR (si `path` es uno y el vídeo sigue ahí).
pub fn video_of_still(path: &Path) -> Option<PathBuf> {
    let dir = stills_dir()?;
    if !crate::winutil::same_path(path.parent()?, &dir) { return None; }
    let video = PathBuf::from(std::fs::read_to_string(path.with_extension("txt")).ok()?.trim());
    video.exists().then_some(video)
}

fn still_of(path: &Path) -> Result<PathBuf, String> {
    use std::hash::{Hash, Hasher};
    let dir = stills_dir().ok_or("sin %LOCALAPPDATA%")?;
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    // El nombre cambia si el vídeo cambia (fecha o tamaño).
    let meta = std::fs::metadata(path).map_err(|e| e.to_string())?;
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    path.to_string_lossy().to_lowercase().hash(&mut hasher);
    meta.len().hash(&mut hasher);
    meta.modified().ok().hash(&mut hasher);
    let still = dir.join(format!("{:016x}.jpg", hasher.finish()));
    if still.exists() { return Ok(still); }
    let (pixels, width, height) = first_frame(path, |native| native)?;
    unsafe { write_jpeg(&still, &pixels, width, height).map_err(|e| e.to_string())?; }
    std::fs::write(still.with_extension("txt"), path.to_string_lossy().as_bytes()).map_err(|e| e.to_string())?;
    log_info!("Fotograma fijo de {:?} guardado para Windows", path.file_name().unwrap_or_default());
    prune(&dir);
    Ok(still)
}

unsafe fn write_jpeg(path: &Path, pixels: &[u8], width: u32, height: u32) -> windows::core::Result<()> {
    use windows::Win32::Foundation::GENERIC_WRITE;
    use windows::Win32::Graphics::Imaging::*;
    use windows::Win32::System::Com::{CoCreateInstance, CLSCTX_INPROC_SERVER};
    let factory: IWICImagingFactory = CoCreateInstance(&CLSID_WICImagingFactory, None, CLSCTX_INPROC_SERVER)?;
    let bitmap = factory.CreateBitmapFromMemory(width, height, &GUID_WICPixelFormat32bppBGR, width * 4, pixels)?;
    let stream = factory.CreateStream()?;
    let wide: Vec<u16> = path.as_os_str().to_string_lossy().encode_utf16().chain(std::iter::once(0)).collect();
    stream.InitializeFromFilename(PCWSTR(wide.as_ptr()), GENERIC_WRITE.0)?;
    let encoder = factory.CreateEncoder(&GUID_ContainerFormatJpeg, std::ptr::null())?;
    encoder.Initialize(&stream, WICBitmapEncoderNoCache)?;
    let mut frame: Option<IWICBitmapFrameEncode> = None;
    let mut options = None;
    encoder.CreateNewFrame(&mut frame, &mut options)?;
    let frame = frame.ok_or_else(|| windows::core::Error::from(windows::Win32::Foundation::E_FAIL))?;
    frame.Initialize(options.as_ref())?;
    frame.SetSize(width, height)?;
    let mut format = GUID_WICPixelFormat24bppBGR;
    frame.SetPixelFormat(&mut format)?;
    // WriteSource convierte al formato que acepte el codificador.
    frame.WriteSource(&bitmap, std::ptr::null())?;
    frame.Commit()?;
    encoder.Commit()
}

/// Borra los fotogramas fijos más antiguos si hay demasiados.
fn prune(dir: &Path) {
    let Ok(entries) = std::fs::read_dir(dir) else { return; };
    let mut stills: Vec<(std::time::SystemTime, PathBuf)> = entries
        .filter_map(|entry| entry.ok())
        .map(|entry| entry.path())
        .filter(|path| path.extension().is_some_and(|e| e.eq_ignore_ascii_case("jpg")))
        .filter_map(|path| Some((std::fs::metadata(&path).ok()?.modified().ok()?, path)))
        .collect();
    if stills.len() <= MAX_STILLS { return; }
    stills.sort();
    for (_, path) in stills.iter().take(stills.len() - MAX_STILLS) {
        let _ = std::fs::remove_file(path);
        let _ = std::fs::remove_file(path.with_extension("txt"));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recognizes_video_extensions() {
        assert!(is_video(Path::new(r"C:\a\b.MP4")));
        assert!(is_video(Path::new("x.webm")));
        assert!(!is_video(Path::new("x.gif")));
        assert!(!is_video(Path::new("x")));
    }

    /// Decodifica un vídeo real: primer fotograma y unos segundos de reproducción.
    /// `SWEPR_VIDEO_FILE=ruta.mp4 cargo test --release video_probe -- --ignored --nocapture`
    /// (en debug, copiar los píxeles es tan lento que se saltan casi todos los fotogramas).
    #[test]
    #[ignore]
    fn video_probe() {
        use windows::Win32::System::Com::{CoInitializeEx, COINIT_MULTITHREADED};
        unsafe { let _ = CoInitializeEx(None, COINIT_MULTITHREADED); }
        let path = PathBuf::from(std::env::var("SWEPR_VIDEO_FILE").expect("SWEPR_VIDEO_FILE"));
        let started = Instant::now();
        let size = |native| super::super::layout::decode_size(crate::config::FitMode::Fill, native, (1920, 1080));
        let (pixels, w, h) = first_frame(&path, size).unwrap();
        eprintln!("primer fotograma {w}x{h} en {} ms", started.elapsed().as_millis());
        assert_eq!(pixels.len(), (w * h * 4) as usize);
        let player = Player::start(&path, (w, h));
        let started = Instant::now();
        let mut shown = 0;
        while started.elapsed() < Duration::from_secs(3) {
            if player.take_frame(|frame| assert_eq!(frame.len(), pixels.len())) { shown += 1; }
            std::thread::sleep(Duration::from_millis(5));
        }
        eprintln!("{shown} fotogramas en 3 s");
        assert!(shown > 10);
        let still = windows_image(&path);
        eprintln!("fijo: {still:?}");
        assert_eq!(video_of_still(&still).as_deref(), Some(path.as_path()));
    }
}
