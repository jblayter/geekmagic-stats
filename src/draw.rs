//! Shared drawing chrome for GeekMagic screens: canvas, colors, fonts, and the
//! primitives (rounded rects, gradient bars, text alignment, header) that every
//! screen composes from. Keeps individual screen binaries lean.

use ab_glyph::{FontRef, PxScale};
use image::{Rgba, RgbaImage};
use imageproc::drawing::draw_text_mut;

pub const W: u32 = 240;
pub const H: u32 = 240;

// Palette — shared across screens so they read as one system.
pub const BG: Rgba<u8> = Rgba([12, 12, 16, 255]);
pub const PANEL_BG: Rgba<u8> = Rgba([22, 22, 30, 255]);
pub const TEXT_PRIMARY: Rgba<u8> = Rgba([240, 240, 245, 255]);
pub const TEXT_DIM: Rgba<u8> = Rgba([113, 113, 122, 255]);
pub const TEXT_MUTED: Rgba<u8> = Rgba([161, 161, 170, 255]);
pub const SEPARATOR: Rgba<u8> = Rgba([35, 35, 45, 255]);
pub const BAR_TRACK: Rgba<u8> = Rgba([40, 40, 50, 255]);

// Semantic status colors.
pub const OK_LEFT: Rgba<u8> = Rgba([59, 130, 246, 255]); // blue
pub const OK_RIGHT: Rgba<u8> = Rgba([6, 182, 212, 255]); // cyan
pub const GOOD_LEFT: Rgba<u8> = Rgba([34, 197, 94, 255]); // green
pub const GOOD_RIGHT: Rgba<u8> = Rgba([16, 185, 129, 255]);
pub const WARN_LEFT: Rgba<u8> = Rgba([234, 179, 8, 255]); // yellow
pub const WARN_RIGHT: Rgba<u8> = Rgba([249, 115, 22, 255]); // orange
pub const DANGER: Rgba<u8> = Rgba([239, 68, 68, 255]); // red
pub const NEUTRAL: Rgba<u8> = Rgba([113, 113, 122, 255]); // gray

const FONT_BYTES: &[u8] = include_bytes!("../fonts/Inter-Regular.ttf");
const FONT_BOLD_BYTES: &[u8] = include_bytes!("../fonts/Inter-Bold.ttf");

pub fn font() -> FontRef<'static> {
    FontRef::try_from_slice(FONT_BYTES).expect("bundled regular font is valid")
}

pub fn font_bold() -> FontRef<'static> {
    FontRef::try_from_slice(FONT_BOLD_BYTES).expect("bundled bold font is valid")
}

pub fn new_canvas() -> RgbaImage {
    RgbaImage::from_pixel(W, H, BG)
}

pub fn lerp_color(a: Rgba<u8>, b: Rgba<u8>, t: f32) -> Rgba<u8> {
    let t = t.clamp(0.0, 1.0);
    Rgba([
        (a[0] as f32 + (b[0] as f32 - a[0] as f32) * t) as u8,
        (a[1] as f32 + (b[1] as f32 - a[1] as f32) * t) as u8,
        (a[2] as f32 + (b[2] as f32 - a[2] as f32) * t) as u8,
        255,
    ])
}

fn is_inside_rounded(px: u32, py: u32, w: u32, h: u32, r: u32) -> bool {
    if r == 0 || w == 0 || h == 0 {
        return true;
    }
    let r = r.min(w / 2).min(h / 2);
    let corners = [
        (r, r),
        (w.saturating_sub(r + 1), r),
        (r, h.saturating_sub(r + 1)),
        (w.saturating_sub(r + 1), h.saturating_sub(r + 1)),
    ];
    for &(cx, cy) in &corners {
        let in_corner_x = if px <= cx {
            px < r
        } else {
            px > w.saturating_sub(r + 1)
        };
        let in_corner_y = if py <= cy {
            py < r
        } else {
            py > h.saturating_sub(r + 1)
        };
        if in_corner_x && in_corner_y {
            let dx = cx.abs_diff(px);
            let dy = cy.abs_diff(py);
            if dx * dx + dy * dy > r * r {
                return false;
            }
        }
    }
    true
}

pub fn draw_rounded_rect(
    img: &mut RgbaImage,
    x: i32,
    y: i32,
    w: u32,
    h: u32,
    r: u32,
    color: Rgba<u8>,
) {
    for px in 0..w {
        for py in 0..h {
            if is_inside_rounded(px, py, w, h, r) {
                let abs_x = x + px as i32;
                let abs_y = y + py as i32;
                if abs_x >= 0 && (abs_x as u32) < W && abs_y >= 0 && (abs_y as u32) < H {
                    img.put_pixel(abs_x as u32, abs_y as u32, color);
                }
            }
        }
    }
}

/// Horizontal track with a gradient fill from `left_color` to `right_color`.
pub fn draw_gradient_bar(
    img: &mut RgbaImage,
    x: i32,
    y: i32,
    total_w: u32,
    h: u32,
    fill_frac: f32,
    left_color: Rgba<u8>,
    right_color: Rgba<u8>,
    corner_r: u32,
) {
    draw_rounded_rect(img, x, y, total_w, h, corner_r, BAR_TRACK);
    let fill_w = ((total_w as f32) * fill_frac.clamp(0.0, 1.0)) as u32;
    if fill_w == 0 {
        return;
    }
    for px in 0..fill_w {
        let t = if total_w > 1 {
            px as f32 / (total_w - 1) as f32
        } else {
            0.0
        };
        let color = lerp_color(left_color, right_color, t);
        let abs_x = x + px as i32;
        for py in 0..h {
            let abs_y = y + py as i32;
            if is_inside_rounded(px, py, fill_w, h, corner_r)
                && abs_x >= 0
                && (abs_x as u32) < W
                && abs_y >= 0
                && (abs_y as u32) < H
            {
                img.put_pixel(abs_x as u32, abs_y as u32, color);
            }
        }
    }
}

pub fn draw_circle(img: &mut RgbaImage, cx: i32, cy: i32, r: i32, color: Rgba<u8>) {
    for dx in -r..=r {
        for dy in -r..=r {
            if dx * dx + dy * dy <= r * r {
                let px = cx + dx;
                let py = cy + dy;
                if px >= 0 && px < W as i32 && py >= 0 && py < H as i32 {
                    img.put_pixel(px as u32, py as u32, color);
                }
            }
        }
    }
}

/// Rough advance-width estimate for centering/right-aligning without shaping.
pub fn approx_text_width(text: &str, scale: f32) -> i32 {
    let char_w = scale * 0.55;
    let mut w = 0.0f32;
    for ch in text.chars() {
        w += match ch {
            '.' | ':' | '!' | '|' | 'i' | 'l' | '1' => char_w * 0.55,
            'm' | 'w' | 'M' | 'W' => char_w * 1.25,
            ' ' => char_w * 0.6,
            '%' => char_w * 1.1,
            _ => char_w,
        };
    }
    w.ceil() as i32
}

pub fn draw_text(
    img: &mut RgbaImage,
    color: Rgba<u8>,
    x: i32,
    y: i32,
    scale: f32,
    font: &FontRef,
    text: &str,
) {
    draw_text_mut(img, color, x, y, PxScale::from(scale), font, text);
}

pub fn draw_text_right(
    img: &mut RgbaImage,
    color: Rgba<u8>,
    right_x: i32,
    y: i32,
    scale: f32,
    font: &FontRef,
    text: &str,
) {
    let w = approx_text_width(text, scale);
    draw_text_mut(img, color, right_x - w, y, PxScale::from(scale), font, text);
}

pub fn draw_text_centered(
    img: &mut RgbaImage,
    color: Rgba<u8>,
    center_x: i32,
    y: i32,
    scale: f32,
    font: &FontRef,
    text: &str,
) {
    let w = approx_text_width(text, scale);
    draw_text_mut(
        img,
        color,
        center_x - w / 2,
        y,
        PxScale::from(scale),
        font,
        text,
    );
}

/// Truncate `text` with an ellipsis so it fits within `max_w` at `scale`.
pub fn truncate_to_width(text: &str, scale: f32, max_w: i32) -> String {
    if approx_text_width(text, scale) <= max_w {
        return text.to_string();
    }
    let mut out = String::new();
    for ch in text.chars() {
        let candidate = format!("{out}{ch}…");
        if approx_text_width(&candidate, scale) > max_w {
            break;
        }
        out.push(ch);
    }
    if out.is_empty() {
        "…".to_string()
    } else {
        format!("{out}…")
    }
}

/// Standard header: bold `title` on the left (shrinking to fit), dim `right`
/// text on the right, and a separator line beneath. Returns the y just below
/// the separator so callers know where content starts.
pub fn draw_header(
    img: &mut RgbaImage,
    font: &FontRef,
    font_bold: &FontRef,
    title: &str,
    right: &str,
) -> i32 {
    let mx = 16i32;
    let right_edge = W as i32 - mx;
    let header_y = 10;

    draw_text_right(img, TEXT_DIM, right_edge, header_y + 1, 15.0, font, right);

    let avail = right_edge - mx - approx_text_width(right, 15.0) - 8;
    let mut scale = 17.0f32;
    while scale > 10.0 && approx_text_width(title, scale) > avail {
        scale -= 1.0;
    }
    draw_text(img, TEXT_PRIMARY, mx, header_y, scale, font_bold, title);

    let content_w = (right_edge - mx) as u32;
    draw_rounded_rect(img, mx, 33, content_w, 1, 0, SEPARATOR);
    35
}

/// The computer's name as shown in Finder / Sharing (e.g. "John's MacBook Pro"),
/// falling back to the hostname, then to `fallback`.
pub fn machine_name(fallback: &str) -> String {
    use std::process::Command;
    let from_cmd = |cmd: &str, args: &[&str]| -> Option<String> {
        let out = Command::new(cmd).args(args).output().ok()?;
        if !out.status.success() {
            return None;
        }
        let s = String::from_utf8_lossy(&out.stdout).trim().to_string();
        if s.is_empty() {
            None
        } else {
            Some(s)
        }
    };
    from_cmd("scutil", &["--get", "ComputerName"])
        .or_else(|| from_cmd("hostname", &[]))
        .unwrap_or_else(|| fallback.to_string())
}
