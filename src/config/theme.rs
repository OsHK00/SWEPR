//! Aspecto oscuro de la ventana de configuración, dibujado con GDI: tarjetas por sección,
//! casillas, desplegables y botones. Los controles siguen siendo los de Windows (teclado,
//! ratón y mensajes funcionan igual); solo se sustituye su pintado.

use std::sync::atomic::{AtomicIsize, Ordering};
use std::sync::Mutex;
use windows::Win32::Foundation::{COLORREF, HWND, LPARAM, LRESULT, POINT, RECT, WPARAM};
use windows::Win32::Graphics::Gdi::{
    BeginPaint, BitBlt, CreateCompatibleBitmap, CreateCompatibleDC, CreatePen, CreateSolidBrush, DeleteDC, DeleteObject,
    DrawTextW, EndPaint, FillRect, GetStockObject, InvalidateRect, Polyline, RoundRect, SelectObject, SetBkMode, SetTextColor,
    UpdateWindow, DT_CENTER, DT_END_ELLIPSIS, DT_LEFT, DT_SINGLELINE, DT_VCENTER, HBRUSH, HDC, HGDIOBJ, NULL_BRUSH, PAINTSTRUCT,
    PS_SOLID, SRCCOPY, TRANSPARENT,
};
use windows::Win32::UI::Input::KeyboardAndMouse::{GetFocus, IsWindowEnabled, TrackMouseEvent, TME_LEAVE, TRACKMOUSEEVENT};
use windows::Win32::UI::WindowsAndMessaging::{
    CallWindowProcW, GetClientRect, GetWindowLongPtrW, GetWindowLongW, GetWindowTextLengthW, GetWindowTextW, SendMessageW,
    SetWindowLongPtrW, BM_GETSTATE, BM_SETCHECK, BM_SETSTATE, CB_GETCURSEL, CB_GETDROPPEDSTATE, CB_GETLBTEXT, CB_GETLBTEXTLEN,
    CB_SETCURSEL, CB_SHOWDROPDOWN, GWLP_USERDATA, GWLP_WNDPROC, GWL_STYLE, WM_CAPTURECHANGED, WM_CHAR, WM_ENABLE,
    WM_ERASEBKGND, WM_KEYDOWN, WM_KEYUP, WM_KILLFOCUS, WM_LBUTTONDBLCLK, WM_LBUTTONDOWN, WM_LBUTTONUP, WM_MOUSEMOVE,
    WM_MOUSEWHEEL, WM_PAINT, WM_SETFOCUS, WM_SETTEXT, WNDPROC,
};

pub const COLOR_WINDOW_BG: COLORREF = COLORREF(0x001F1F1F);
pub const COLOR_CARD_BG: COLORREF = COLORREF(0x002A2A2A);
const COLOR_CARD_BORDER: COLORREF = COLORREF(0x00363636);
pub const COLOR_FIELD_BG: COLORREF = COLORREF(0x00363636);
const COLOR_FIELD_BORDER: COLORREF = COLORREF(0x00484848);
const COLOR_FIELD_BORDER_HOT: COLORREF = COLORREF(0x00606060);
pub const COLOR_TEXT: COLORREF = COLORREF(0x00F0F0F0);
pub const COLOR_LABEL: COLORREF = COLORREF(0x00C4C4C4);
pub const COLOR_MUTED: COLORREF = COLORREF(0x008C8C8C);
const COLOR_DISABLED: COLORREF = COLORREF(0x00686868);
/// Azul de acento (el mismo que marca el fondo actual en el panel).
const COLOR_ACCENT: COLORREF = COLORREF(0x00FFC24C);
const COLOR_ACCENT_HOT: COLORREF = COLORREF(0x00FFD17A);
const COLOR_ACCENT_PRESSED: COLORREF = COLORREF(0x00D9A33F);
const COLOR_ON_ACCENT: COLORREF = COLORREF(0x00101010);
const COLOR_BUTTON: COLORREF = COLORREF(0x003A3A3A);
const COLOR_BUTTON_HOT: COLORREF = COLORREF(0x00454545);
const COLOR_BUTTON_PRESSED: COLORREF = COLORREF(0x00303030);

/// Marcas guardadas en GWLP_USERDATA de cada control.
const FLAG_HOT: isize = 1;
const FLAG_PRIMARY: isize = 2;
const FLAG_ON_CARD: isize = 4;

const BST_CHECKED: isize = 0x1;
const BST_PUSHED: isize = 0x4;

static BUTTON_PROC: AtomicIsize = AtomicIsize::new(0);
static COMBO_PROC: AtomicIsize = AtomicIsize::new(0);
static BRUSHES: Mutex<Vec<(u32, isize)>> = Mutex::new(Vec::new());

fn px(value: i32) -> i32 { super::window::px(value) }

/// Pincel sólido reutilizable (se crean pocos colores y viven lo que el proceso).
pub fn brush(color: COLORREF) -> HBRUSH {
    let mut brushes = match BRUSHES.lock() { Ok(b) => b, Err(_) => return HBRUSH::default() };
    if let Some((_, handle)) = brushes.iter().find(|(c, _)| *c == color.0) {
        return HBRUSH(*handle as *mut _);
    }
    let created = unsafe { CreateSolidBrush(color) };
    brushes.push((color.0, created.0 as isize));
    created
}

/// Rectángulo redondeado relleno con borde de 1 px.
pub unsafe fn round_rect(dc: HDC, rect: &RECT, radius: i32, fill: COLORREF, border: COLORREF) {
    let pen = CreatePen(PS_SOLID, 1, border);
    let old_pen = SelectObject(dc, pen);
    let old_brush = SelectObject(dc, brush(fill));
    let _ = RoundRect(dc, rect.left, rect.top, rect.right, rect.bottom, radius * 2, radius * 2);
    SelectObject(dc, old_brush);
    SelectObject(dc, old_pen);
    let _ = DeleteObject(pen);
}

unsafe fn polyline(dc: HDC, points: &[POINT], width: i32, color: COLORREF) {
    let pen = CreatePen(PS_SOLID, width, color);
    let old_pen = SelectObject(dc, pen);
    let old_brush = SelectObject(dc, GetStockObject(NULL_BRUSH));
    let _ = Polyline(dc, points);
    SelectObject(dc, old_brush);
    SelectObject(dc, old_pen);
    let _ = DeleteObject(pen);
}

pub unsafe fn draw_text(dc: HDC, text: &str, rect: &RECT, font: isize, color: COLORREF, flags: windows::Win32::Graphics::Gdi::DRAW_TEXT_FORMAT) {
    let old_font = (font != 0).then(|| SelectObject(dc, HGDIOBJ(font as *mut _)));
    let _ = SetBkMode(dc, TRANSPARENT);
    let _ = SetTextColor(dc, color);
    let mut wide: Vec<u16> = text.encode_utf16().collect();
    let mut rect = *rect;
    let _ = DrawTextW(dc, &mut wide, &mut rect, flags);
    if let Some(old) = old_font { SelectObject(dc, old); }
}

/// Fondo de un campo (texto, desplegable, atajo) sobre una tarjeta.
pub unsafe fn draw_field(dc: HDC, rect: &RECT, focused: bool, hot: bool) {
    let _ = FillRect(dc, rect, brush(COLOR_CARD_BG));
    let border = if focused { COLOR_ACCENT } else if hot { COLOR_FIELD_BORDER_HOT } else { COLOR_FIELD_BORDER };
    round_rect(dc, rect, px(4), COLOR_FIELD_BG, border);
}

/// Tarjeta de una sección con su título.
pub unsafe fn draw_card(dc: HDC, rect: &RECT, title: &str, title_font: isize) {
    round_rect(dc, rect, px(8), COLOR_CARD_BG, COLOR_CARD_BORDER);
    let title_rect = RECT { left: rect.left + px(16), top: rect.top + px(10), right: rect.right - px(16), bottom: rect.top + px(36) };
    draw_text(dc, title, &title_rect, title_font, COLOR_TEXT, DT_LEFT | DT_VCENTER | DT_SINGLELINE);
}

/// Pinta en un lienzo en memoria y lo copia de una vez (sin parpadeo).
unsafe fn paint_buffered(hwnd: HWND, draw: impl FnOnce(HDC, &RECT)) {
    let mut ps = PAINTSTRUCT::default();
    let dc = BeginPaint(hwnd, &mut ps);
    let mut rect = RECT::default();
    let _ = GetClientRect(hwnd, &mut rect);
    let mem = CreateCompatibleDC(dc);
    let bmp = CreateCompatibleBitmap(dc, rect.right.max(1), rect.bottom.max(1));
    let old = SelectObject(mem, bmp);
    draw(mem, &rect);
    let _ = BitBlt(dc, 0, 0, rect.right, rect.bottom, mem, 0, 0, SRCCOPY);
    SelectObject(mem, old);
    let _ = DeleteObject(bmp);
    let _ = DeleteDC(mem);
    let _ = EndPaint(hwnd, &ps);
}

fn window_text(hwnd: HWND) -> String {
    unsafe {
        let len = GetWindowTextLengthW(hwnd);
        if len <= 0 { return String::new(); }
        let mut buffer = vec![0u16; len as usize + 1];
        let read = GetWindowTextW(hwnd, &mut buffer);
        String::from_utf16_lossy(&buffer[..read.max(0) as usize])
    }
}

unsafe fn flags(hwnd: HWND) -> isize { GetWindowLongPtrW(hwnd, GWLP_USERDATA) }

unsafe fn subclass(hwnd: HWND, original: &AtomicIsize, proc_: unsafe extern "system" fn(HWND, u32, WPARAM, LPARAM) -> LRESULT, initial_flags: isize) {
    SetWindowLongPtrW(hwnd, GWLP_USERDATA, initial_flags);
    let previous = SetWindowLongPtrW(hwnd, GWLP_WNDPROC, proc_ as usize as isize);
    // Todos los controles de una clase comparten el mismo procedimiento original.
    original.store(previous, Ordering::Relaxed);
}

unsafe fn call_original(original: &AtomicIsize, hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    let proc_: WNDPROC = std::mem::transmute(original.load(Ordering::Relaxed));
    CallWindowProcW(proc_, hwnd, msg, wparam, lparam)
}

/// Seguimiento del ratón encima del control (estado "hot").
unsafe fn track_hover(hwnd: HWND, msg: u32) {
    let current = flags(hwnd);
    if msg == WM_MOUSEMOVE && current & FLAG_HOT == 0 {
        SetWindowLongPtrW(hwnd, GWLP_USERDATA, current | FLAG_HOT);
        let mut track = TRACKMOUSEEVENT { cbSize: std::mem::size_of::<TRACKMOUSEEVENT>() as u32, dwFlags: TME_LEAVE, hwndTrack: hwnd, dwHoverTime: 0 };
        let _ = TrackMouseEvent(&mut track);
        let _ = InvalidateRect(hwnd, None, false);
    } else if msg == WM_MOUSELEAVE {
        SetWindowLongPtrW(hwnd, GWLP_USERDATA, current & !FLAG_HOT);
        let _ = InvalidateRect(hwnd, None, false);
    }
}

const WM_MOUSELEAVE: u32 = 0x02A3;

/// Mensajes tras los que el control clásico se pinta por su cuenta: se repinta encima al momento.
fn repaints_itself(msg: u32) -> bool {
    matches!(
        msg,
        BM_SETCHECK | BM_SETSTATE | WM_LBUTTONDOWN | WM_LBUTTONUP | WM_LBUTTONDBLCLK | WM_SETFOCUS | WM_KILLFOCUS | WM_ENABLE
            | WM_KEYDOWN | WM_KEYUP | WM_CHAR | WM_SETTEXT | WM_CAPTURECHANGED | WM_MOUSEWHEEL | CB_SETCURSEL | CB_SHOWDROPDOWN
    )
}

// ---------------------------------------------------------------- Botones y casillas

/// Casilla de verificación (BS_AUTOCHECKBOX) con el aspecto oscuro.
pub unsafe fn style_checkbox(hwnd: HWND) {
    subclass(hwnd, &BUTTON_PROC, button_proc, FLAG_ON_CARD);
}

/// Botón normal; `primary` lo pinta con el color de acento.
pub unsafe fn style_button(hwnd: HWND, primary: bool, on_card: bool) {
    let flags = if primary { FLAG_PRIMARY } else { 0 } | if on_card { FLAG_ON_CARD } else { 0 };
    subclass(hwnd, &BUTTON_PROC, button_proc, flags);
}

unsafe extern "system" fn button_proc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    match msg {
        WM_PAINT => {
            paint_buffered(hwnd, |dc, rect| paint_button(hwnd, dc, rect));
            return LRESULT(0);
        }
        WM_ERASEBKGND => return LRESULT(1),
        WM_MOUSEMOVE | WM_MOUSELEAVE => track_hover(hwnd, msg),
        _ => {}
    }
    let result = call_original(&BUTTON_PROC, hwnd, msg, wparam, lparam);
    if repaints_itself(msg) {
        let _ = InvalidateRect(hwnd, None, false);
        let _ = UpdateWindow(hwnd);
    }
    result
}

unsafe fn paint_button(hwnd: HWND, dc: HDC, rect: &RECT) {
    let flags = flags(hwnd);
    let hot = flags & FLAG_HOT != 0;
    let enabled = IsWindowEnabled(hwnd).as_bool();
    let state = SendMessageW(hwnd, BM_GETSTATE, WPARAM(0), LPARAM(0)).0;
    let text = window_text(hwnd);
    let font = super::window::ui_font();
    let background = if flags & FLAG_ON_CARD != 0 { COLOR_CARD_BG } else { COLOR_WINDOW_BG };
    let _ = FillRect(dc, rect, brush(background));

    let is_checkbox = GetWindowLongW(hwnd, GWL_STYLE) & 0xF == 3;
    if is_checkbox {
        let size = px(18);
        let top = rect.top + (rect.bottom - rect.top - size) / 2;
        let check_rect = RECT { left: rect.left, top, right: rect.left + size, bottom: top + size };
        let checked = state & BST_CHECKED != 0;
        let (fill, border) = match (enabled, checked) {
            (false, true) => (COLOR_DISABLED, COLOR_DISABLED),
            (false, false) => (COLOR_CARD_BG, COLOR_DISABLED),
            (true, true) => (if hot { COLOR_ACCENT_HOT } else { COLOR_ACCENT }, if hot { COLOR_ACCENT_HOT } else { COLOR_ACCENT }),
            (true, false) => (if hot { COLOR_FIELD_BG } else { COLOR_CARD_BG }, if hot { COLOR_LABEL } else { COLOR_MUTED }),
        };
        round_rect(dc, &check_rect, px(4), fill, border);
        if checked {
            let (x, y) = (check_rect.left, check_rect.top);
            let points = [
                POINT { x: x + size * 26 / 100, y: y + size * 52 / 100 },
                POINT { x: x + size * 43 / 100, y: y + size * 68 / 100 },
                POINT { x: x + size * 75 / 100, y: y + size * 34 / 100 },
            ];
            polyline(dc, &points, px(2).max(2), COLOR_ON_ACCENT);
        }
        let text_rect = RECT { left: check_rect.right + px(10), ..*rect };
        let color = if enabled { COLOR_TEXT } else { COLOR_DISABLED };
        draw_text(dc, &text, &text_rect, font, color, DT_LEFT | DT_VCENTER | DT_SINGLELINE | DT_END_ELLIPSIS);
        return;
    }

    let pressed = state & BST_PUSHED != 0;
    let primary = flags & FLAG_PRIMARY != 0;
    let (fill, border, text_color) = if primary {
        let fill = if pressed { COLOR_ACCENT_PRESSED } else if hot { COLOR_ACCENT_HOT } else { COLOR_ACCENT };
        (fill, fill, COLOR_ON_ACCENT)
    } else {
        let fill = if pressed { COLOR_BUTTON_PRESSED } else if hot { COLOR_BUTTON_HOT } else { COLOR_BUTTON };
        (fill, COLOR_FIELD_BORDER, COLOR_TEXT)
    };
    round_rect(dc, rect, px(4), fill, border);
    draw_text(dc, &text, rect, font, text_color, DT_CENTER | DT_VCENTER | DT_SINGLELINE);
}

// ---------------------------------------------------------------- Desplegables

/// Desplegable (CBS_DROPDOWNLIST) con el aspecto oscuro. La lista que se abre usa
/// WM_CTLCOLORLISTBOX de la ventana padre.
pub unsafe fn style_combo(hwnd: HWND) {
    // Alto del campo cerrado: el resto del control lo añade Windows (~6 px).
    let _ = SendMessageW(hwnd, windows::Win32::UI::WindowsAndMessaging::CB_SETITEMHEIGHT, WPARAM(usize::MAX), LPARAM(px(26) as isize));
    subclass(hwnd, &COMBO_PROC, combo_proc, FLAG_ON_CARD);
}

unsafe extern "system" fn combo_proc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    match msg {
        WM_PAINT => {
            paint_buffered(hwnd, |dc, rect| paint_combo(hwnd, dc, rect));
            return LRESULT(0);
        }
        WM_ERASEBKGND => return LRESULT(1),
        WM_MOUSEMOVE | WM_MOUSELEAVE => track_hover(hwnd, msg),
        _ => {}
    }
    let result = call_original(&COMBO_PROC, hwnd, msg, wparam, lparam);
    if repaints_itself(msg) {
        let _ = InvalidateRect(hwnd, None, false);
        let _ = UpdateWindow(hwnd);
    }
    result
}

unsafe fn paint_combo(hwnd: HWND, dc: HDC, rect: &RECT) {
    let enabled = IsWindowEnabled(hwnd).as_bool();
    let dropped = SendMessageW(hwnd, CB_GETDROPPEDSTATE, WPARAM(0), LPARAM(0)).0 != 0;
    let focused = GetFocus() == hwnd || dropped;
    draw_field(dc, rect, focused, flags(hwnd) & FLAG_HOT != 0);

    let selected = SendMessageW(hwnd, CB_GETCURSEL, WPARAM(0), LPARAM(0)).0;
    let mut text = String::new();
    if selected >= 0 {
        let len = SendMessageW(hwnd, CB_GETLBTEXTLEN, WPARAM(selected as usize), LPARAM(0)).0;
        if len > 0 {
            let mut buffer = vec![0u16; len as usize + 1];
            let _ = SendMessageW(hwnd, CB_GETLBTEXT, WPARAM(selected as usize), LPARAM(buffer.as_mut_ptr() as isize));
            text = String::from_utf16_lossy(&buffer[..len as usize]);
        }
    }
    let arrow_space = px(30);
    let text_rect = RECT { left: rect.left + px(10), right: rect.right - arrow_space, ..*rect };
    let color = if enabled { COLOR_TEXT } else { COLOR_DISABLED };
    draw_text(dc, &text, &text_rect, super::window::ui_font(), color, DT_LEFT | DT_VCENTER | DT_SINGLELINE | DT_END_ELLIPSIS);

    // Flecha en forma de "v".
    let cx = rect.right - arrow_space / 2;
    let cy = (rect.top + rect.bottom) / 2;
    let w = px(4);
    let points = [POINT { x: cx - w, y: cy - w / 2 }, POINT { x: cx, y: cy + w / 2 }, POINT { x: cx + w, y: cy - w / 2 }];
    polyline(dc, &points, px(1).max(1) + 1, if enabled { COLOR_LABEL } else { COLOR_DISABLED });
}
