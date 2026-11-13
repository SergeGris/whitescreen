//! Where everything in the control window goes.
//!
//! Computed rather than loaded from a dialog resource: every length here is
//! in logical pixels and scaled by the window's DPI, and a resource template
//! would have had to be duplicated per scale factor. The widths that hold
//! text are measured rather than guessed, because the type ramp follows the
//! user's text-size setting.
//!
//! Nothing here paints. `layout` leaves a `Chrome` behind -- the card
//! rectangles, the labels, the preview -- and `draw` works from that.

use windows::Win32::Foundation::RECT;
use windows::Win32::Graphics::Gdi::{GetDC, HFONT, ReleaseDC};
use windows::Win32::UI::WindowsAndMessaging::{GetClientRect, MoveWindow};

use super::*;
use crate::theme::text_width;

/// WinUI's metrics, in logical pixels, scaled by the window's DPI.
pub(super) mod metric {
    /// The margin around the page.
    pub const PAGE: f32 = 20.0;
    /// The margin inside a card.
    pub const CARD: f32 = 16.0;
    /// Between cards.
    pub const GAP: f32 = 12.0;
    /// A control's height, and the gap between two rows of them.
    pub const ROW: f32 = 32.0;
    pub const ROW_GAP: f32 = 8.0;
    /// A monitor row, which carries two lines: the name and the detail.
    pub const MONITOR_ROW: f32 = 46.0;
    /// The tallest the live preview goes. Its width follows from the shape
    /// of the screen it stands for, so this is the only fixed dimension.
    pub const PREVIEW: f32 = 232.0;
    /// The monitor list down the left.
    pub const SIDEBAR: f32 = 296.0;
    /// A colour chip, and the shorter key-light chip.
    pub const CHIP_W: f32 = 46.0;
    pub const CHIP_H: f32 = 32.0;
    pub const CHIP_GAP: f32 = 8.0;
    pub const KELVIN_H: f32 = 26.0;
    /// A card's heading, and a line of caption text.
    pub const HEADING: f32 = 24.0;
    pub const CAPTION: f32 = 18.0;
    /// The page title at the top of the window.
    pub const TITLE: f32 = 30.0;
    /// The box of a checkbox.
    pub const CHECKBOX: f32 = 20.0;
    /// The narrowest a command button goes; a long label makes its own.
    pub const BUTTON_NARROW: f32 = 104.0;
    /// Below this the cards start to overlap, so the window will not go:
    /// the sidebar, a gap, and a colour card wide enough for six chips.
    pub const MIN_W: f32 = 800.0;
    pub const MIN_H: f32 = 820.0;
}

/// Text the window draws for itself, as opposed to a control's own label.
pub(super) struct Label {
    pub rect: RECT,
    pub(super) text: String,
    pub(super) role: Role,
}

#[derive(Clone, Copy)]
pub(super) enum Role {
    /// The page title: WinUI's Subtitle.
    Title,
    /// A card heading: Body Strong.
    Heading,
    /// A hint or a field label: Caption, in the secondary colour.
    Caption,
}

/// Where everything the window paints itself has ended up.
///
/// Recomputed by `layout` and used by both the erase and the paint handler,
/// so a themed child asking its parent for a background gets the same card
/// colour it is sitting on.
#[derive(Default)]
pub(super) struct Chrome {
    pub(super) preview: RECT,
    pub(super) cards: Vec<RECT>,
    pub(super) labels: Vec<Label>,
    /// The brightness readout, whose text comes from the session.
    pub(super) brightness: RECT,
}

/// A vertical cursor: the layout is columns of rows, and this hands them out.
struct Stack {
    x: i32,
    y: i32,
    width: i32,
}

impl Stack {
    fn row(&mut self, height: i32, gap: i32) -> RECT {
        let rect =
            RECT { left: self.x, top: self.y, right: self.x + self.width, bottom: self.y + height };
        self.y += height + gap;
        rect
    }
}

/// Split `rect` into a left part of `width` and the rest, `gap` apart.
fn split(rect: RECT, width: i32, gap: i32) -> (RECT, RECT) {
    (RECT { right: rect.left + width, ..rect }, RECT { left: rect.left + width + gap, ..rect })
}

/// Trim a rectangle inwards on every side.
pub(super) fn inset(rect: RECT, by: i32) -> RECT {
    RECT {
        left: rect.left + by,
        top: rect.top + by,
        right: rect.right - by,
        bottom: rect.bottom - by,
    }
}

/// How wide a checkbox has to be for a label `text` pixels wide: the box,
/// the gap WinUI puts after it, the text, and a little slack so the focus
/// ring has somewhere to go.
fn check_width(s: &State, text: i32) -> i32 {
    s.px(metric::CHECKBOX) + s.px(10.0) + text + s.px(8.0)
}

/// Place every control and work out everything the window draws itself.
pub(super) fn place(hwnd: HWND, state: &RefCell<State>) {
    let moves = {
        let Ok(mut s) = state.try_borrow_mut() else {
            return;
        };
        // The same scaling the painter uses: `State::px`, borrowed here for
        // brevity because every length below goes through it.
        let px = |v: f32| s.px(v);

        let mut client = RECT::default();
        let _ = unsafe { GetClientRect(hwnd, &raw mut client) };

        // Control widths are measured rather than guessed: the type ramp
        // follows the user's text-size setting, so a width that fits
        // "Per-monitor colors" at 100% clips it at 125%.
        let dc = unsafe { GetDC(Some(hwnd)) };
        let measure = |label: &str, font: HFONT| {
            if dc.is_invalid() { 0 } else { text_width(dc, label, font) }
        };

        let pad = px(metric::PAGE);
        let gap = px(metric::GAP);
        let card_pad = px(metric::CARD);
        let row = px(metric::ROW);
        let row_gap = px(metric::ROW_GAP);
        let heading = px(metric::HEADING);
        let caption = px(metric::CAPTION);

        let mut chrome = Chrome::default();
        let mut moves: Vec<(HWND, RECT)> = Vec::new();
        let mut page = Stack {
            x: client.left + pad,
            y: client.top + pad,
            width: (client.right - client.left - pad * 2).max(px(400.0)),
        };

        // The page title, the way a WinUI page opens: a subtitle-sized name
        // and one line saying what the page is for.
        chrome.labels.push(Label {
            rect: page.row(px(metric::TITLE), 0),
            text: "White Screen".to_string(),
            role: Role::Title,
        });
        chrome.labels.push(Label {
            rect: page.row(caption, gap),
            text: "Solid colors and test patterns on every attached screen.".to_string(),
            role: Role::Caption,
        });

        // The preview is the shape of the screen it stands for, not the
        // shape of the card it sits in. A wedge judged in the wrong aspect
        // ratio has bands of the wrong width, and a grid the wrong pitch,
        // which is the whole of what the preview is for.
        let (aspect_w, aspect_h) = s.aimed_aspect();
        let tall = px(metric::PREVIEW);
        let mut preview_h = tall;
        let mut preview_w = (preview_h as f32 * aspect_w / aspect_h).round() as i32;
        if preview_w > page.width {
            preview_w = page.width;
            preview_h = (preview_w as f32 * aspect_h / aspect_w).round() as i32;
        }
        let band = page.row(preview_h, gap);
        chrome.preview = RECT {
            left: band.left + (page.width - preview_w) / 2,
            right: band.left + (page.width - preview_w) / 2 + preview_w,
            ..band
        };

        // The two-column row: the screens on the left, colour on the right.
        let screens = s.session.screens().len() as i32;
        let monitor_row = px(metric::MONITOR_ROW);
        let displays_h = card_pad * 2 + heading + caption + screens.max(1) * monitor_row;
        let colour_h = card_pad * 2
            + heading
            + px(metric::CHIP_H)
            + px(10.0)
            + caption
            + px(metric::KELVIN_H)
            + gap
            + row
            + row_gap
            + row;
        let columns = page.row(displays_h.max(colour_h), gap);
        let sidebar = px(metric::SIDEBAR).min(page.width / 2);
        let (displays_card, colour_card) = split(columns, sidebar, gap);
        chrome.cards.push(displays_card);
        chrome.cards.push(colour_card);

        // The screens.
        let mut column = Stack {
            x: displays_card.left + card_pad,
            y: displays_card.top + card_pad,
            width: sidebar - card_pad * 2,
        };
        chrome.labels.push(Label {
            rect: column.row(heading, 0),
            text: "Displays".to_string(),
            role: Role::Heading,
        });
        chrome.labels.push(Label {
            rect: column.row(caption, 0),
            text: "Which screens an overlay covers".to_string(),
            role: Role::Caption,
        });
        for control in s.controls.iter().filter(|c| c.kind == Kind::Monitor) {
            moves.push((control.hwnd, column.row(monitor_row, 0)));
        }

        // Colour.
        let mut column = Stack {
            x: colour_card.left + card_pad,
            y: colour_card.top + card_pad,
            width: colour_card.right - colour_card.left - card_pad * 2,
        };
        chrome.labels.push(Label {
            rect: column.row(heading, 0),
            text: "Color".to_string(),
            role: Role::Heading,
        });

        let chips = column.row(px(metric::CHIP_H), px(10.0));
        let chip_w = px(metric::CHIP_W);
        let chip_gap = px(metric::CHIP_GAP);
        for (i, control) in
            s.controls.iter().filter(|c| (ID_CHIP_FIRST..ID_CHIP_LAST).contains(&c.id)).enumerate()
        {
            let left = chips.left + i as i32 * (chip_w + chip_gap);
            moves.push((control.hwnd, RECT { left, right: left + chip_w, ..chips }));
        }
        if let Some(custom) = s.control(ID_CHIP_LAST) {
            let left = chips.left + CHIPS.len() as i32 * (chip_w + chip_gap);
            moves.push((
                custom.hwnd,
                RECT { left, right: left + px(metric::BUTTON_NARROW), ..chips },
            ));
        }

        chrome.labels.push(Label {
            rect: column.row(caption, 0),
            text: "Key light".to_string(),
            role: Role::Caption,
        });
        let kelvin_row = column.row(px(metric::KELVIN_H), gap);
        for (i, control) in s
            .controls
            .iter()
            .filter(|c| (ID_KELVIN_FIRST..=ID_KELVIN_LAST).contains(&c.id))
            .enumerate()
        {
            let left = kelvin_row.left + i as i32 * (chip_w + chip_gap);
            moves.push((control.hwnd, RECT { left, right: left + chip_w, ..kelvin_row }));
        }

        // "Applies to" and the per-monitor switch share a row.
        let aim_row = column.row(row, row_gap);
        let label_w = measure("Applies to", s.fonts.caption)
            .max(measure("Brightness", s.fonts.caption))
            + px(12.0);
        let (aim_label, rest) = split(aim_row, label_w, px(4.0));
        chrome.labels.push(Label {
            rect: aim_label,
            text: "Applies to".to_string(),
            role: Role::Caption,
        });
        let (combo, per_monitor) = split(rest, px(168.0), gap);
        if let Some(control) = s.control(ID_APPLIES_TO) {
            // A drop-down list clamps itself to the field height; the extra
            // is the room its list is allowed to use.
            moves.push((control.hwnd, RECT { bottom: combo.bottom + px(180.0), ..combo }));
        }
        if let Some(control) = s.control(ID_PER_MONITOR) {
            moves.push((control.hwnd, per_monitor));
        }

        // Brightness: the label, the slider, the readout.
        let bright_row = column.row(row, 0);
        let (bright_label, rest) = split(bright_row, label_w, px(4.0));
        chrome.labels.push(Label {
            rect: bright_label,
            text: "Brightness".to_string(),
            role: Role::Caption,
        });
        let readout = px(44.0);
        let (slider, value) = split(rest, (rest.right - rest.left - readout).max(px(80.0)), 0);
        if let Some(control) = s.control(ID_BRIGHTNESS) {
            moves.push((control.hwnd, slider));
        }
        chrome.brightness = value;

        // Pattern.
        let pattern_card = page.row(card_pad * 2 + heading + row, gap);
        chrome.cards.push(pattern_card);
        let mut column = Stack {
            x: pattern_card.left + card_pad,
            y: pattern_card.top + card_pad,
            width: pattern_card.right - pattern_card.left - card_pad * 2,
        };
        chrome.labels.push(Label {
            rect: column.row(heading, 0),
            text: "Pattern".to_string(),
            role: Role::Heading,
        });
        let fills = column.row(row, 0);
        let mut left = fills.left;
        for control in s.controls.iter().filter(|c| (ID_FILL_FIRST..=ID_FILL_LAST).contains(&c.id))
        {
            let width = check_width(&s, measure(&control.label, s.fonts.body));
            moves.push((control.hwnd, RECT { left, right: left + width, ..fills }));
            left += width + gap;
        }

        // Session.
        let session_card = page.row(card_pad * 2 + heading + row, gap);
        chrome.cards.push(session_card);
        let mut column = Stack {
            x: session_card.left + card_pad,
            y: session_card.top + card_pad,
            width: session_card.right - session_card.left - card_pad * 2,
        };
        chrome.labels.push(Label {
            rect: column.row(heading, 0),
            text: "Session".to_string(),
            role: Role::Heading,
        });
        let switches = column.row(row, 0);
        let mut left = switches.left;
        for id in [ID_CYCLE, ID_IDENTIFY] {
            let Some(control) = s.control(id) else {
                continue;
            };
            let width = check_width(&s, measure(&control.label, s.fonts.body));
            moves.push((control.hwnd, RECT { left, right: left + width, ..switches }));
            left += width + gap * 2;
        }

        // The commands go at the foot of the window, right-aligned, with
        // the default action last: the Windows order.
        let actions = RECT {
            left: page.x,
            top: (client.bottom - pad - row).max(page.y),
            right: page.x + page.width,
            bottom: (client.bottom - pad).max(page.y + row),
        };
        let mut right = actions.right;
        for id in [ID_SHOW, ID_HIDE] {
            let Some(control) = s.control(id) else {
                continue;
            };
            // Wide enough for the label, never narrower than WinUI's
            // minimum button, and free to grow for a longer translation.
            let width =
                (measure(&control.label, s.fonts.body) + px(32.0)).max(px(metric::BUTTON_NARROW));
            moves.push((control.hwnd, RECT { left: right - width, right, ..actions }));
            right -= width + row_gap;
        }

        if !dc.is_invalid() {
            unsafe { ReleaseDC(Some(hwnd), dc) };
        }
        // Where the last card ended, plus the row of commands and the
        // margin under it.
        s.natural = page.y + row + pad;
        s.chrome = chrome;
        moves
    };

    for (child, rect) in moves {
        let _ = unsafe {
            MoveWindow(
                child,
                rect.left,
                rect.top,
                (rect.right - rect.left).max(0),
                (rect.bottom - rect.top).max(0),
                true,
            )
        };
    }
}
