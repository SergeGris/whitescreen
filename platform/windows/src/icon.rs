//! The application icon, drawn at run time.
//!
//! A portable single executable has nowhere to keep a `.ico`: an external
//! file would defeat the point, and compiling one in would mean a resource
//! script and `rc.exe` in the build. So the icon is computed instead -- a
//! rounded accent tile with a white panel on it, which is what this app
//! does to a screen -- at whatever sizes the shell asks for, in the user's
//! own accent colour.
//!
//! Without this the taskbar and Alt+Tab show the generic "no icon"
//! placeholder, which is the single clearest sign of an unfinished app.

use windows::Win32::Foundation::HWND;
use windows::Win32::Foundation::{LPARAM, WPARAM};
use windows::Win32::Graphics::Gdi::{
    BI_RGB, BITMAPINFO, BITMAPINFOHEADER, CreateBitmap, CreateDIBSection, DIB_RGB_COLORS,
    DeleteObject, HBITMAP,
};
use windows::Win32::UI::WindowsAndMessaging::GetSystemMetrics;
use windows::Win32::UI::WindowsAndMessaging::{
    CreateIconIndirect, DestroyIcon, HICON, ICON_BIG, ICON_SMALL, ICONINFO, SM_CXICON, SM_CXSMICON,
    SendMessageW, WM_SETICON,
};

use whitescreen_core::Rgba;

/// Icons belonging to a window, destroyed with it.
///
/// Windows does not take ownership of an icon handed to `WM_SETICON`, so
/// these have to outlive the window and be released after it.
pub struct Icons {
    large: Option<HICON>,
    small: Option<HICON>,
}

impl Icons {
    /// Draw the pair the shell wants and give them to `hwnd`.
    pub fn attach(hwnd: HWND, accent: Rgba) -> Self {
        let metric = |index, fallback| {
            let value = unsafe { GetSystemMetrics(index) };
            if value > 0 { value } else { fallback }
        };
        let large = render(metric(SM_CXICON, 32), accent);
        let small = render(metric(SM_CXSMICON, 16), accent);

        for (which, icon) in [(ICON_BIG, large), (ICON_SMALL, small)] {
            if let Some(icon) = icon {
                unsafe {
                    SendMessageW(
                        hwnd,
                        WM_SETICON,
                        Some(WPARAM(which as usize)),
                        Some(LPARAM(icon.0 as isize)),
                    );
                }
            }
        }
        Self { large, small }
    }
}

impl Drop for Icons {
    fn drop(&mut self) {
        for icon in [self.large.take(), self.small.take()].into_iter().flatten() {
            let _ = unsafe { DestroyIcon(icon) };
        }
    }
}

/// One square icon, `size` pixels on a side.
///
/// `None` if GDI would not give us a bitmap, in which case the window keeps
/// the stock icon: an app with no icon is a great deal better than one that
/// fails to start.
fn render(size: i32, accent: Rgba) -> Option<HICON> {
    if size <= 0 {
        return None;
    }
    let pixels = draw(size as usize, accent);

    // Top-down 32-bit BGRA, which is what an alpha icon's colour bitmap is.
    let info = BITMAPINFO {
        bmiHeader: BITMAPINFOHEADER {
            biSize: size_of::<BITMAPINFOHEADER>() as u32,
            biWidth: size,
            biHeight: -size,
            biPlanes: 1,
            biBitCount: 32,
            biCompression: BI_RGB.0,
            ..Default::default()
        },
        ..Default::default()
    };

    let mut bits: *mut core::ffi::c_void = std::ptr::null_mut();
    let color =
        unsafe { CreateDIBSection(None, &raw const info, DIB_RGB_COLORS, &raw mut bits, None, 0) }
            .ok()?;
    if bits.is_null() {
        let _ = unsafe { DeleteObject(color.into()) };
        return None;
    }
    // Safety: the section is exactly `size * size` 32-bit pixels, which is
    // the length of `pixels`.
    unsafe {
        std::ptr::copy_nonoverlapping(pixels.as_ptr(), bits.cast::<u32>(), pixels.len());
    }

    // A 32-bit icon carries its own alpha, so the AND mask only has to
    // exist. Zeroed bits are "show the colour bitmap everywhere".
    let mask_bits = vec![0u8; (size as usize).div_ceil(16) * 2 * size as usize];
    let mask: HBITMAP = unsafe { CreateBitmap(size, size, 1, 1, Some(mask_bits.as_ptr().cast())) };

    let info = ICONINFO {
        fIcon: windows::core::BOOL(1),
        xHotspot: 0,
        yHotspot: 0,
        hbmMask: mask,
        hbmColor: color,
    };
    let icon = unsafe { CreateIconIndirect(&raw const info) }.ok();

    // CreateIconIndirect copies both bitmaps; they are ours to free.
    unsafe {
        let _ = DeleteObject(color.into());
        let _ = DeleteObject(mask.into());
    }
    icon
}

/// The artwork: an accent tile, rounded, with a white panel inside it.
///
/// Returned premultiplied, which is what GDI composites an alpha icon as.
/// The edges are antialiased by sampling each pixel on a 4x4 grid, because
/// a 16 px icon with hard corners looks broken next to every other icon on
/// the taskbar.
fn draw(size: usize, accent: Rgba) -> Vec<u32> {
    let n = size as f32;
    let radius = n * 0.22;
    let inset = (n * 0.19).max(1.0);
    let inner_radius = (radius - inset * 0.5).max(0.0);

    let mut pixels = vec![0u32; size * size];
    for y in 0..size {
        for x in 0..size {
            let (mut outer, mut inner) = (0.0f32, 0.0f32);
            // 4x4 supersampling, so an edge pixel gets a fraction rather
            // than a staircase.
            for sy in 0..4 {
                for sx in 0..4 {
                    let px = x as f32 + (sx as f32 + 0.5) / 4.0;
                    let py = y as f32 + (sy as f32 + 0.5) / 4.0;
                    if inside(px, py, 0.0, 0.0, n, n, radius) {
                        outer += 1.0 / 16.0;
                    }
                    if inside(px, py, inset, inset, n - inset, n - inset, inner_radius) {
                        inner += 1.0 / 16.0;
                    }
                }
            }
            if outer <= 0.0 {
                continue;
            }
            // White panel over the accent tile, the whole thing faded out
            // by the tile's own coverage at the corners.
            let blend = |tile: u8| {
                let tile = f32::from(tile);
                let value = tile + (255.0 - tile) * inner;
                (value * outer).round().clamp(0.0, 255.0) as u8
            };
            let alpha = (outer * 255.0).round().clamp(0.0, 255.0) as u8;
            pixels[y * size + x] = u32::from(blend(accent.b))
                | (u32::from(blend(accent.g)) << 8)
                | (u32::from(blend(accent.r)) << 16)
                | (u32::from(alpha) << 24);
        }
    }
    pixels
}

/// Whether a point is inside a rounded rectangle.
fn inside(x: f32, y: f32, left: f32, top: f32, right: f32, bottom: f32, radius: f32) -> bool {
    if x < left || y < top || x >= right || y >= bottom {
        return false;
    }
    let radius = radius.min((right - left) / 2.0).min((bottom - top) / 2.0);
    if radius <= 0.0 {
        return true;
    }
    // Outside the corner boxes it is a plain rectangle; inside one, it is
    // the distance to that corner's centre that decides.
    let cx = x.clamp(left + radius, right - radius);
    let cy = y.clamp(top + radius, bottom - radius);
    let (dx, dy) = (x - cx, y - cy);
    dx * dx + dy * dy <= radius * radius
}
