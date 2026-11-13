//! Painting the control window.
//!
//! Every face in this window is drawn here rather than by the theme, which
//! is what makes dark mode correct: a themed Win32 checkbox draws its own
//! label in the *theme's* colour, which is black whatever the app wants, so
//! an app that follows the system into dark mode and does not own its pixels
//! ends up with black text on a near-black card.
//!
//! It is also what gets WinUI's geometry -- 4 px corners on a control, 8 px
//! on a card, a 20 px check box -- and the hover, pressed and focus states a
//! modern Windows app has.

use windows::Win32::Foundation::{LPARAM, RECT, WPARAM};
use windows::Win32::Graphics::Gdi::{
    CreatePen, CreateRoundRectRgn, CreateSolidBrush, DT_CENTER, DT_END_ELLIPSIS, DT_LEFT, DT_RIGHT,
    DT_SINGLELINE, DT_VCENTER, DeleteObject, Ellipse, FillRect, HDC, PS_SOLID, SelectClipRgn,
    SelectObject,
};
use windows::Win32::UI::Controls::{
    CDDS_ITEMPREPAINT, CDDS_PREPAINT, CDIS_SELECTED, CDRF_DODEFAULT, CDRF_NOTIFYITEMDRAW,
    CDRF_SKIPDEFAULT, DRAWITEMSTRUCT, NMCUSTOMDRAW, ODS_COMBOBOXEDIT, ODS_FOCUS, ODS_SELECTED,
    ODT_COMBOBOX, TBCD_CHANNEL, TBCD_THUMB, TBCD_TICS, TBM_GETTHUMBRECT,
};
use windows::Win32::UI::WindowsAndMessaging::{
    CB_GETLBTEXT, CB_GETLBTEXTLEN, GetClientRect, SendMessageW,
};

use super::layout::{inset, metric};
use super::*;
use crate::paint::{colorref, paint};
use crate::theme::{RADIUS_CARD, RADIUS_CONTROL, focus_ring, glyph, round_rect, text};

/// Whether an owner-draw state flag is set.
fn has(
    state: windows::Win32::UI::Controls::ODS_FLAGS,
    flag: windows::Win32::UI::Controls::ODS_FLAGS,
) -> bool {
    state.0 & flag.0 != 0
}

/// The page and the cards: everything behind the controls.
pub(super) fn paint_background(hwnd: HWND, state: &RefCell<State>, dc: HDC) {
    let Ok(s) = state.try_borrow() else { return };
    let mut client = RECT::default();
    let _ = unsafe { GetClientRect(hwnd, &raw mut client) };

    unsafe {
        let brush = CreateSolidBrush(colorref(s.palette.base));
        FillRect(dc, &raw const client, brush);
        let _ = DeleteObject(brush.into());
    }

    let radius = s.px(RADIUS_CARD);
    for card in &s.chrome.cards {
        round_rect(dc, *card, radius, Some(s.palette.card), Some(s.palette.card_stroke));
    }
}

/// The preview, the headings and the captions.
pub(super) fn paint_foreground(state: &RefCell<State>, dc: HDC) {
    let Ok(s) = state.try_borrow() else { return };

    // The preview, with the corner radius every other surface has. GDI will
    // not round a fill, so the painter is clipped to the rounded shape
    // instead.
    let preview = s.chrome.preview;
    if preview.right > preview.left && preview.bottom > preview.top {
        let radius = s.px(RADIUS_CARD);
        unsafe {
            let region = CreateRoundRectRgn(
                preview.left,
                preview.top,
                preview.right + 1,
                preview.bottom + 1,
                radius * 2,
                radius * 2,
            );
            SelectClipRgn(dc, Some(region));
            // What the chips are editing, which is the aimed-at monitor's
            // own colour when "Applies to" names one.
            let color = s.aimed_color().scaled(s.session.brightness());
            paint(dc, preview, color, s.session.fill(), s.scale, 0);
            SelectClipRgn(dc, None);
            let _ = DeleteObject(region.into());
        }
        // A hairline so a white preview has an edge on a white card.
        round_rect(dc, preview, s.px(RADIUS_CARD), None, Some(s.palette.card_stroke));
    }

    for label in &s.chrome.labels {
        let (font, color) = match label.role {
            Role::Title => (s.fonts.heading, s.palette.text),
            Role::Heading => (s.fonts.strong, s.palette.text),
            Role::Caption => (s.fonts.caption, s.palette.text_secondary),
        };
        text(
            dc,
            label.rect,
            &label.text,
            font,
            color,
            DT_SINGLELINE | DT_VCENTER | DT_LEFT | DT_END_ELLIPSIS,
        );
    }

    let percent = format!("{}%", (s.session.brightness() * 100.0).round() as i32);
    text(
        dc,
        s.chrome.brightness,
        &percent,
        s.fonts.caption,
        s.palette.text_secondary,
        DT_SINGLELINE | DT_VCENTER | DT_RIGHT,
    );
}

/// Where an owner-drawn face goes, what it sits on, and which of its states
/// it is in. Bundled because every face needs all six and a function taking
/// them one at a time reads as a puzzle.
struct Face {
    dc: HDC,
    rect: RECT,
    /// The colour of the surface underneath, which the focus ring needs and
    /// the face clears itself to.
    surface: Rgba,
    hot: bool,
    pressed: bool,
    focused: bool,
}

/// Paint one owner-drawn control.
pub(super) fn draw_item(state: &RefCell<State>, item: &DRAWITEMSTRUCT) {
    let Ok(s) = state.try_borrow() else { return };
    if item.CtlType == ODT_COMBOBOX {
        return draw_combo_item(&s, item);
    }
    let Some(control) = s.controls.iter().find(|c| c.hwnd == item.hwndItem) else {
        return;
    };

    let face = Face {
        dc: item.hDC,
        rect: item.rcItem,
        // Clear to the surface underneath first. A `BS_OWNERDRAW` button is
        // never erased for us, and none of these faces cover their whole
        // rectangle, so without this the ring from the last hover stays put.
        surface: if control.on_card { s.palette.card } else { s.palette.base },
        hot: control.hot,
        pressed: has(item.itemState, ODS_SELECTED),
        focused: has(item.itemState, ODS_FOCUS),
    };
    unsafe {
        let brush = CreateSolidBrush(colorref(face.surface));
        FillRect(face.dc, &raw const face.rect, brush);
        let _ = DeleteObject(brush.into());
    }

    match control.kind {
        Kind::Swatch(color) => draw_swatch(&s, &face, color, s.aimed_color() == color),
        Kind::Push { accent } => draw_push(&s, &face, &control.label, accent),
        Kind::Check => draw_check(&s, &face, control, s.checked(control.id), None),
        Kind::Monitor => draw_check(
            &s,
            &face,
            control,
            s.checked(control.id),
            Some(s.monitor_swatch(control.id)),
        ),
        Kind::Combo | Kind::Slider => {}
    }
}

/// A colour chip.
///
/// The chip is the colour, so the states cannot be shown by tinting it: a
/// hover is a ring, a press is a smaller chip, and the chip that matches the
/// colour now in force wears the accent ring.
fn draw_swatch(s: &State, face: &Face, color: Rgba, current: bool) {
    let Face { dc, rect, .. } = *face;
    let radius = s.px(RADIUS_CONTROL + 2.0);
    let body = if face.pressed { inset(rect, s.px(2.0)) } else { inset(rect, s.px(1.0)) };
    // A hairline that contrasts with the fill, so a white chip does not
    // vanish into a light card -- the same rule as the GTK swatches.
    let edge = if color.luma() > 0.5 { Rgba::new(0, 0, 0) } else { Rgba::new(255, 255, 255) };
    let edge = crate::theme::blend(color, edge, 0.35);
    round_rect(dc, body, radius, Some(color), Some(edge));

    if current {
        round_rect(dc, rect, radius + s.px(1.0), None, Some(s.palette.accent));
        round_rect(dc, inset(rect, s.px(1.0)), radius, None, Some(s.palette.accent));
    } else if face.hot {
        round_rect(dc, rect, radius + s.px(1.0), None, Some(s.palette.control_stroke));
    }
    if face.focused {
        focus_ring(dc, rect, radius + s.px(1.0), &s.palette, face.surface);
    }
}

/// A command button.
fn draw_push(s: &State, face: &Face, label: &str, accent: bool) {
    let Face { dc, rect, hot, pressed, .. } = *face;
    let radius = s.px(RADIUS_CONTROL);
    let (fill, stroke, ink) = if accent {
        (s.palette.accent_for(hot, pressed), None, s.palette.on_accent)
    } else {
        (s.palette.control_for(hot, pressed), Some(s.palette.control_stroke), s.palette.text)
    };
    round_rect(dc, rect, radius, Some(fill), stroke);
    text(
        dc,
        rect,
        label,
        s.fonts.body,
        ink,
        DT_SINGLELINE | DT_VCENTER | DT_CENTER | DT_END_ELLIPSIS,
    );
    if face.focused {
        focus_ring(dc, rect, radius, &s.palette, face.surface);
    }
}

/// A checkbox, and for a monitor row the screen's colour beside it.
fn draw_check(s: &State, face: &Face, control: &Control, checked: bool, swatch: Option<Rgba>) {
    let Face { dc, rect, hot, pressed, .. } = *face;
    let label = control.label.as_str();
    let size = s.px(metric::CHECKBOX);
    let middle = (rect.top + rect.bottom) / 2;
    let box_rect = RECT {
        left: rect.left,
        top: middle - size / 2,
        right: rect.left + size,
        bottom: middle + size / 2,
    };
    let radius = s.px(RADIUS_CONTROL);

    if checked {
        round_rect(dc, box_rect, radius, Some(s.palette.accent_for(hot, pressed)), None);
        text(
            dc,
            box_rect,
            glyph::CHECK,
            s.fonts.icon,
            s.palette.on_accent,
            DT_SINGLELINE | DT_VCENTER | DT_CENTER,
        );
    } else {
        round_rect(
            dc,
            box_rect,
            radius,
            Some(s.palette.control_for(hot, pressed)),
            Some(s.palette.control_stroke),
        );
    }

    let mut label_rect = RECT { left: box_rect.right + s.px(10.0), ..rect };
    if let Some(color) = swatch {
        // The screen's colour at the right-hand end of the row.
        let chip = s.px(18.0);
        let chip_rect = RECT {
            left: rect.right - chip,
            top: middle - chip / 2,
            right: rect.right,
            bottom: middle + chip / 2,
        };
        let edge = crate::theme::blend(
            color,
            if color.luma() > 0.5 { Rgba::new(0, 0, 0) } else { Rgba::new(255, 255, 255) },
            0.35,
        );
        round_rect(dc, chip_rect, s.px(RADIUS_CONTROL), Some(color), Some(edge));
        label_rect.right = chip_rect.left - s.px(8.0);
    }
    if control.detail.is_empty() {
        text(
            dc,
            label_rect,
            label,
            s.fonts.body,
            s.palette.text,
            DT_SINGLELINE | DT_VCENTER | DT_LEFT | DT_END_ELLIPSIS,
        );
    } else {
        // Two lines: what the screen is, and where it is plugged in.
        text(
            dc,
            RECT { bottom: middle, ..label_rect },
            label,
            s.fonts.body,
            s.palette.text,
            DT_SINGLELINE | DT_VCENTER | DT_LEFT | DT_END_ELLIPSIS,
        );
        text(
            dc,
            RECT { top: middle, ..label_rect },
            &control.detail,
            s.fonts.caption,
            s.palette.text_secondary,
            DT_SINGLELINE | DT_VCENTER | DT_LEFT | DT_END_ELLIPSIS,
        );
    }

    if face.focused {
        focus_ring(dc, rect, radius, &s.palette, face.surface);
    }
}

/// One row of the drop-down, or its closed field.
fn draw_combo_item(s: &State, item: &DRAWITEMSTRUCT) {
    let hwnd = item.hwndItem;
    // -1 is "nothing selected", which only reads as negative once the
    // unsigned field has been cast back.
    let index = item.itemID as i32;
    if index < 0 {
        return;
    }
    // The text belongs to the control, so it has to be asked for it.
    let length =
        unsafe { SendMessageW(hwnd, CB_GETLBTEXTLEN, Some(WPARAM(index as usize)), None) }.0;
    if length <= 0 {
        return;
    }
    let mut buffer = vec![0u16; length as usize + 1];
    unsafe {
        SendMessageW(
            hwnd,
            CB_GETLBTEXT,
            Some(WPARAM(index as usize)),
            Some(LPARAM(buffer.as_mut_ptr() as isize)),
        );
    }
    let label = String::from_utf16_lossy(&buffer[..length as usize]);

    let field = has(item.itemState, ODS_COMBOBOXEDIT);
    let highlighted = !field && has(item.itemState, ODS_SELECTED);
    let (fill, ink) = if highlighted {
        (Some(s.palette.accent), s.palette.on_accent)
    } else if field {
        // The themed frame is still the system's, but the inside of it is
        // ours to clear: leaving it to the theme would leave the last
        // selection's text underneath this one.
        (Some(s.palette.control), s.palette.text)
    } else {
        (Some(s.palette.card), s.palette.text)
    };

    if let Some(fill) = fill {
        unsafe {
            let brush = CreateSolidBrush(colorref(fill));
            FillRect(item.hDC, &raw const item.rcItem, brush);
            let _ = DeleteObject(brush.into());
        }
    }
    let inner = RECT { left: item.rcItem.left + s.px(8.0), ..item.rcItem };
    text(
        item.hDC,
        inner,
        &label,
        s.fonts.body,
        ink,
        DT_SINGLELINE | DT_VCENTER | DT_LEFT | DT_END_ELLIPSIS,
    );
}

/// Custom-draw the brightness trackbar.
///
/// The control stays a trackbar -- keyboard, accessibility, the lot -- and
/// only its pixels change: a 4 px rail, accent up to the thumb, and a round
/// thumb with an accent core, which is the Fluent slider.
pub(super) fn draw_slider(state: &RefCell<State>, draw: *const NMCUSTOMDRAW) -> u32 {
    let draw = unsafe { &*draw };
    if draw.dwDrawStage == CDDS_PREPAINT {
        return CDRF_NOTIFYITEMDRAW;
    }
    if draw.dwDrawStage != CDDS_ITEMPREPAINT {
        return CDRF_DODEFAULT;
    }
    let Ok(s) = state.try_borrow() else {
        return CDRF_DODEFAULT;
    };

    match draw.dwItemSpec as u32 {
        v if v == TBCD_TICS => CDRF_SKIPDEFAULT,
        v if v == TBCD_CHANNEL => {
            let height = s.px(4.0).max(2);
            let middle = (draw.rc.top + draw.rc.bottom) / 2;
            let rail = RECT {
                left: draw.rc.left,
                top: middle - height / 2,
                right: draw.rc.right,
                bottom: middle - height / 2 + height,
            };
            let radius = height / 2;
            round_rect(draw.hdc, rail, radius, Some(s.palette.control_stroke), None);

            // The part behind the thumb carries the accent. The thumb's
            // own rectangle is the honest place to stop: the channel is
            // wider than the distance the thumb's centre travels.
            let mut thumb = RECT::default();
            unsafe {
                SendMessageW(
                    draw.hdr.hwndFrom,
                    TBM_GETTHUMBRECT,
                    None,
                    Some(LPARAM(&raw mut thumb as isize)),
                );
            }
            let edge = if thumb.right > thumb.left && thumb.left >= draw.rc.left {
                (thumb.left + thumb.right) / 2
            } else {
                // No thumb rectangle to be had: fall back to the value.
                let span = (rail.right - rail.left).max(1);
                let fraction = ((s.session.brightness() * 100.0).round() as i32 - BRIGHTNESS_MIN)
                    .clamp(0, BRIGHTNESS_MAX - BRIGHTNESS_MIN)
                    as f32
                    / (BRIGHTNESS_MAX - BRIGHTNESS_MIN) as f32;
                rail.left + (span as f32 * fraction) as i32
            };
            let filled = RECT { right: edge, ..rail };
            if filled.right > filled.left {
                round_rect(draw.hdc, filled, radius, Some(s.palette.accent), None);
            }
            CDRF_SKIPDEFAULT
        }
        v if v == TBCD_THUMB => {
            let pressed = draw.uItemState.contains(CDIS_SELECTED);
            let rect = draw.rc;
            let middle = ((rect.left + rect.right) / 2, (rect.top + rect.bottom) / 2);
            let outer = ((rect.bottom - rect.top).min(rect.right - rect.left) / 2).max(s.px(9.0));
            // WinUI shrinks the core while the thumb is held.
            let core = if pressed { outer / 2 } else { (outer * 2) / 3 };
            unsafe {
                let pen = CreatePen(PS_SOLID, 1, colorref(s.palette.control_stroke));
                let brush = CreateSolidBrush(colorref(s.palette.card));
                let old_pen = SelectObject(draw.hdc, pen.into());
                let old_brush = SelectObject(draw.hdc, brush.into());
                let _ = Ellipse(
                    draw.hdc,
                    middle.0 - outer,
                    middle.1 - outer,
                    middle.0 + outer,
                    middle.1 + outer,
                );
                SelectObject(draw.hdc, old_brush);
                let _ = DeleteObject(brush.into());

                let core_brush = CreateSolidBrush(colorref(s.palette.accent));
                let old_brush = SelectObject(draw.hdc, core_brush.into());
                let core_pen = CreatePen(PS_SOLID, 1, colorref(s.palette.accent));
                let discarded = SelectObject(draw.hdc, core_pen.into());
                let _ = Ellipse(
                    draw.hdc,
                    middle.0 - core,
                    middle.1 - core,
                    middle.0 + core,
                    middle.1 + core,
                );
                SelectObject(draw.hdc, discarded);
                SelectObject(draw.hdc, old_brush);
                SelectObject(draw.hdc, old_pen);
                let _ = DeleteObject(core_pen.into());
                let _ = DeleteObject(core_brush.into());
                let _ = DeleteObject(pen.into());
            }
            CDRF_SKIPDEFAULT
        }
        _ => CDRF_DODEFAULT,
    }
}
