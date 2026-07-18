use ab_glyph::{FontRef, PxScale};
use anyhow::Result;
use image::{Rgba, RgbaImage};
use imageproc::drawing::draw_text_mut;

use crate::stats::{ActiveData, PaceInfo};

const W: u32 = 240;
const H: u32 = 240;

const BG: Rgba<u8> = Rgba([12, 12, 16, 255]);
const TEXT_PRIMARY: Rgba<u8> = Rgba([240, 240, 245, 255]);
const TEXT_DIM: Rgba<u8> = Rgba([113, 113, 122, 255]);
const TEXT_MUTED: Rgba<u8> = Rgba([161, 161, 170, 255]);
const BAR_TRACK: Rgba<u8> = Rgba([40, 40, 50, 255]);
const BAR_FILL_LEFT: Rgba<u8> = Rgba([59, 130, 246, 255]);
const BAR_FILL_RIGHT: Rgba<u8> = Rgba([6, 182, 212, 255]);
const PACE_OK: Rgba<u8> = Rgba([34, 197, 94, 255]);
const PACE_WARN: Rgba<u8> = Rgba([249, 115, 22, 255]);
const WARN_FILL_LEFT: Rgba<u8> = Rgba([234, 179, 8, 255]);
const WARN_FILL_RIGHT: Rgba<u8> = Rgba([249, 115, 22, 255]);
const DANGER_FILL: Rgba<u8> = Rgba([239, 68, 68, 255]);
const SEPARATOR: Rgba<u8> = Rgba([35, 35, 45, 255]);

const FONT_BYTES: &[u8] = include_bytes!("../fonts/Inter-Regular.ttf");
const FONT_BOLD_BYTES: &[u8] = include_bytes!("../fonts/Inter-Bold.ttf");

fn lerp_color(a: Rgba<u8>, b: Rgba<u8>, t: f32) -> Rgba<u8> {
    let t = t.clamp(0.0, 1.0);
    Rgba([
        (a[0] as f32 + (b[0] as f32 - a[0] as f32) * t) as u8,
        (a[1] as f32 + (b[1] as f32 - a[1] as f32) * t) as u8,
        (a[2] as f32 + (b[2] as f32 - a[2] as f32) * t) as u8,
        255,
    ])
}

fn bar_colors(usage_level: &str) -> (Rgba<u8>, Rgba<u8>) {
    match usage_level {
        "danger" | "over" => (DANGER_FILL, DANGER_FILL),
        "warn" => (WARN_FILL_LEFT, WARN_FILL_RIGHT),
        _ => (BAR_FILL_LEFT, BAR_FILL_RIGHT),
    }
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

fn draw_rounded_rect(img: &mut RgbaImage, x: i32, y: i32, w: u32, h: u32, r: u32, color: Rgba<u8>) {
    for px in 0..w {
        for py in 0..h {
            if is_inside_rounded(px, py, w, h, r) {
                let abs_x = x as u32 + px;
                let abs_y = y as u32 + py;
                if abs_x < W && abs_y < H {
                    img.put_pixel(abs_x, abs_y, color);
                }
            }
        }
    }
}

fn draw_gradient_bar(
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
        let abs_x = x as u32 + px;
        for py in 0..h {
            let abs_y = y as u32 + py;
            if is_inside_rounded(px, py, fill_w, h, corner_r) && abs_x < W && abs_y < H {
                img.put_pixel(abs_x, abs_y, color);
            }
        }
    }
}

fn blend_over(base: Rgba<u8>, over: Rgba<u8>) -> Rgba<u8> {
    let a = over[3] as f32 / 255.0;
    Rgba([
        (base[0] as f32 * (1.0 - a) + over[0] as f32 * a) as u8,
        (base[1] as f32 * (1.0 - a) + over[1] as f32 * a) as u8,
        (base[2] as f32 * (1.0 - a) + over[2] as f32 * a) as u8,
        255,
    ])
}

fn draw_pace_marker(
    img: &mut RgbaImage,
    bar_x: i32,
    bar_y: i32,
    bar_w: u32,
    bar_h: u32,
    expected_pct: f64,
    ok: bool,
) {
    let marker_x = bar_x + (bar_w as f64 * expected_pct.clamp(0.0, 100.0) / 100.0) as i32;
    let color = if ok { PACE_OK } else { PACE_WARN };
    let glow = if ok {
        Rgba([34, 197, 94, 80])
    } else {
        Rgba([249, 115, 22, 80])
    };

    for dx in 0..2i32 {
        for dy in -3..(bar_h as i32 + 3) {
            let px = marker_x + dx;
            let py = bar_y + dy;
            if px >= 0 && px < W as i32 && py >= 0 && py < H as i32 {
                img.put_pixel(px as u32, py as u32, color);
            }
        }
    }
    for dx in [-1i32, 2] {
        for dy in -2..(bar_h as i32 + 2) {
            let px = marker_x + dx;
            let py = bar_y + dy;
            if px >= 0 && px < W as i32 && py >= 0 && py < H as i32 {
                let existing = *img.get_pixel(px as u32, py as u32);
                img.put_pixel(px as u32, py as u32, blend_over(existing, glow));
            }
        }
    }
}

fn draw_circle(img: &mut RgbaImage, cx: i32, cy: i32, r: i32, color: Rgba<u8>) {
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

fn format_duration(minutes: f64) -> String {
    let total = minutes.max(0.0).round() as u64;
    let days = total / 1440;
    let hours = (total % 1440) / 60;
    let mins = total % 60;
    if days > 0 {
        if hours == 0 {
            return format!("{days}d");
        }
        return format!("{days}d {hours}h");
    }
    if hours == 0 {
        return format!("{mins}m");
    }
    if mins == 0 {
        return format!("{hours}h");
    }
    format!("{hours}h {mins}m")
}

fn approx_text_width(text: &str, scale: f32) -> i32 {
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

fn draw_text_right(
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

fn format_updated_time(iso: &str) -> String {
    use chrono::{DateTime, Local};
    if let Ok(utc) = DateTime::parse_from_rfc3339(iso) {
        let local: DateTime<Local> = utc.with_timezone(&Local);
        // h:mm tt — 12-hour, no leading zero, AM/PM (e.g. "9:05 PM")
        return local.format("%-I:%M %p").to_string();
    }
    // Fallback: try extracting time part
    if let Some(t_pos) = iso.find('T') {
        let time_part = &iso[t_pos + 1..];
        if time_part.len() >= 5 {
            return time_part[..5].to_string();
        }
    }
    "??:??".to_string()
}

/// One usage bar on the combined screen.
struct Row {
    label: String,
    utilization: f64,
    usage_level: String,
    resets_in_minutes: Option<f64>,
    pace: Option<PaceInfo>,
    /// Extra note shown at bottom-left (e.g. Fable token volume).
    note: Option<String>,
}

/// Combined "Claude Code" screen: Session, Weekly, and Fable weekly-limit bars.
pub fn render_bars(data: &ActiveData, fable_tokens: u64) -> Result<RgbaImage> {
    let font = FontRef::try_from_slice(FONT_BYTES)?;
    let font_bold = FontRef::try_from_slice(FONT_BOLD_BYTES)?;
    let mut img = RgbaImage::from_pixel(W, H, BG);

    let mut rows: Vec<Row> = Vec::new();
    if let Some(w) = &data.five_hour {
        rows.push(Row {
            label: "Session".to_string(),
            utilization: w.utilization,
            usage_level: w.usage_level.clone(),
            resets_in_minutes: w.resets_in_minutes,
            pace: w.pace.clone(),
            note: None,
        });
    }
    if let Some(w) = &data.seven_day {
        rows.push(Row {
            label: "Weekly".to_string(),
            utilization: w.utilization,
            usage_level: w.usage_level.clone(),
            resets_in_minutes: w.resets_in_minutes,
            pace: w.pace.clone(),
            note: None,
        });
    }
    if let Some(f) = data.scoped.iter().find(|s| s.model.to_lowercase().contains("fable")) {
        let note = if fable_tokens > 0 {
            Some(format!("{} tok · 7d", crate::usage::format_tokens(fable_tokens)))
        } else {
            None
        };
        rows.push(Row {
            label: "Fable weekly".to_string(),
            utilization: f.utilization,
            usage_level: f.usage_level.clone(),
            resets_in_minutes: f.resets_in_minutes,
            pace: None,
            note,
        });
    }

    let mx = 16i32;
    let right_edge = (W as i32) - mx;
    let content_w = (right_edge - mx) as u32;

    // ── Header: "Claude Code" + updated time ──
    let header_y = 10;
    draw_text_mut(
        &mut img,
        TEXT_PRIMARY,
        mx,
        header_y,
        PxScale::from(17.0),
        &font_bold,
        "Claude Code",
    );
    let updated_text = if let Some(ts) = &data.updated_at {
        format!("ts: {}", format_updated_time(ts))
    } else {
        "—".to_string()
    };
    draw_text_right(&mut img, TEXT_DIM, right_edge, header_y + 1, 15.0, &font, &updated_text);
    draw_rounded_rect(&mut img, mx, 33, content_w, 1, 0, SEPARATOR);

    if rows.is_empty() {
        draw_text_mut(&mut img, TEXT_DIM, 60, 110, PxScale::from(16.0), &font, "No usage data");
        return Ok(img);
    }

    // ── Bars, split evenly over the remaining height ──
    let band_top = 36i32;
    let band_bottom = 228i32;
    let gap = 5i32;
    let n = rows.len() as i32;
    let sh = (band_bottom - band_top - (n - 1) * gap) / n;
    let bar_x = mx + 4;
    let bar_w = content_w - 8;
    let inner_right = right_edge - 4;

    for (i, row) in rows.iter().enumerate() {
        let by = band_top + i as i32 * (sh + gap);

        // Label + big percentage
        draw_text_mut(&mut img, TEXT_MUTED, bar_x, by + 8, PxScale::from(13.0), &font_bold, &row.label);
        let pct_text = format!("{}%", row.utilization.round() as i32);
        draw_text_right(&mut img, TEXT_PRIMARY, inner_right, by + 4, 26.0, &font_bold, &pct_text);

        // Progress bar + pace marker
        let bar_y = by + 32;
        let bar_h = 12u32;
        let (fill_l, fill_r) = bar_colors(&row.usage_level);
        draw_gradient_bar(
            &mut img,
            bar_x,
            bar_y,
            bar_w,
            bar_h,
            (row.utilization / 100.0) as f32,
            fill_l,
            fill_r,
            6,
        );
        if let Some(pace) = &row.pace {
            draw_pace_marker(&mut img, bar_x, bar_y, bar_w, bar_h, pace.expected_percent, pace.will_last_to_reset);
        }

        // Caption line: pace/note on the left, resets on the right
        let cap_y = bar_y + bar_h as i32 + 6;
        if let Some(pace) = &row.pace {
            let abs_delta = pace.delta_percent.abs().round() as i32;
            let (text, color) = if abs_delta <= 2 {
                ("on pace".to_string(), PACE_OK)
            } else if pace.delta_percent < 0.0 {
                (format!("{abs_delta}% reserve"), PACE_OK)
            } else {
                (format!("{abs_delta}% deficit"), PACE_WARN)
            };
            draw_circle(&mut img, bar_x + 3, cap_y + 6, 3, color);
            draw_text_mut(&mut img, color, bar_x + 11, cap_y, PxScale::from(12.0), &font, &text);
        } else if let Some(note) = &row.note {
            draw_text_mut(&mut img, PACE_OK, bar_x, cap_y, PxScale::from(12.0), &font, note);
        }

        if let Some(mins) = row.resets_in_minutes {
            let reset_text = format!("resets {}", format_duration(mins));
            draw_text_right(&mut img, TEXT_DIM, inner_right, cap_y, 12.0, &font, &reset_text);
        }
    }

    Ok(img)
}

/// A card shown when Claude stats can't be fetched (CLI error, not logged in…).
fn render_error(err: &crate::stats::StatsError) -> RgbaImage {
    let font = FontRef::try_from_slice(FONT_BYTES).expect("bundled font");
    let font_bold = FontRef::try_from_slice(FONT_BOLD_BYTES).expect("bundled bold font");
    let mut img = RgbaImage::from_pixel(W, H, BG);

    let mx = 16i32;
    let right_edge = W as i32 - mx;
    let content_w = (right_edge - mx) as u32;

    // Header
    draw_text_mut(
        &mut img,
        TEXT_PRIMARY,
        mx,
        10,
        PxScale::from(17.0),
        &font_bold,
        "Claude Code",
    );
    draw_text_right(&mut img, DANGER_FILL, right_edge, 11, 15.0, &font, "offline");
    draw_rounded_rect(&mut img, mx, 33, content_w, 1, 0, SEPARATOR);

    // A hollow warning glyph for a bit of visual weight.
    draw_circle(&mut img, W as i32 / 2, 110, 26, DANGER_FILL);
    draw_circle(&mut img, W as i32 / 2, 110, 22, BG);
    draw_text_mut(
        &mut img,
        DANGER_FILL,
        W as i32 / 2 - 3,
        96,
        PxScale::from(30.0),
        &font_bold,
        "!",
    );

    // Title + wrapped message.
    let title_w = approx_text_width(&err.title, 17.0);
    draw_text_mut(
        &mut img,
        TEXT_PRIMARY,
        W as i32 / 2 - title_w / 2,
        150,
        PxScale::from(17.0),
        &font_bold,
        &err.title,
    );
    for (i, line) in wrap_text(&err.message, 12.0, content_w as i32).iter().take(2).enumerate() {
        let lw = approx_text_width(line, 12.0);
        draw_text_mut(
            &mut img,
            TEXT_DIM,
            W as i32 / 2 - lw / 2,
            176 + i as i32 * 15,
            PxScale::from(12.0),
            &font,
            line,
        );
    }

    img
}

/// Greedy word-wrap into lines that fit `max_w` at `scale`.
fn wrap_text(text: &str, scale: f32, max_w: i32) -> Vec<String> {
    let mut lines = Vec::new();
    let mut current = String::new();
    for word in text.split_whitespace() {
        let candidate = if current.is_empty() {
            word.to_string()
        } else {
            format!("{current} {word}")
        };
        if approx_text_width(&candidate, scale) > max_w && !current.is_empty() {
            lines.push(current);
            current = word.to_string();
        } else {
            current = candidate;
        }
    }
    if !current.is_empty() {
        lines.push(current);
    }
    lines
}

/// Fetch Claude stats and render the bars, or an error card on failure.
pub fn render_screen() -> RgbaImage {
    match crate::stats::fetch_stats() {
        Ok(data) => {
            let fable_tokens = crate::usage::fable_tokens_7d();
            render_bars(&data, fable_tokens).unwrap_or_else(|_| {
                render_error(&crate::stats::StatsError {
                    title: "Render error".to_string(),
                    message: "Could not render stats".to_string(),
                })
            })
        }
        Err(e) => {
            eprintln!("Stats unavailable: {} — {}", e.title, e.message);
            render_error(&e)
        }
    }
}
