//! Model usage screen. The Claude usage API doesn't break out per-model figures
//! (and Fable has no rate-limit window), so this scans the local Claude Code
//! session logs (`~/.claude/projects/**/*.jsonl`) and sums tokens per model over
//! a rolling window — the same data the cost tooling reads. Fable 5 is featured.

use std::collections::HashSet;
use std::env;
use std::fs;
use std::path::PathBuf;

use chrono::{DateTime, Duration, Utc};
use image::RgbaImage;
use serde::Deserialize;

use crate::draw;

const WINDOW_DAYS: i64 = 7;

#[derive(Deserialize)]
struct Entry {
    #[serde(rename = "type")]
    entry_type: Option<String>,
    timestamp: Option<String>,
    message: Option<Message>,
}

#[derive(Deserialize)]
struct Message {
    id: Option<String>,
    model: Option<String>,
    usage: Option<Usage>,
}

#[derive(Deserialize)]
struct Usage {
    input_tokens: Option<u64>,
    output_tokens: Option<u64>,
    cache_creation_input_tokens: Option<u64>,
    cache_read_input_tokens: Option<u64>,
}

struct ModelUsage {
    model: String,
    tokens: u64,
}

fn scan_dirs() -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    if let Ok(home) = env::var("HOME") {
        dirs.push(PathBuf::from(&home).join(".claude").join("projects"));
        dirs.push(PathBuf::from(&home).join(".config").join("claude").join("projects"));
    }
    dirs
}

/// Recursively collect `.jsonl` files modified within the window.
fn collect_jsonl(dir: &PathBuf, cutoff: DateTime<Utc>, out: &mut Vec<PathBuf>) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect_jsonl(&path, cutoff, out);
        } else if path.extension().and_then(|e| e.to_str()) == Some("jsonl") {
            // mtime pre-filter: skip files untouched during the window.
            if let Ok(meta) = entry.metadata() {
                if let Ok(modified) = meta.modified() {
                    let mt: DateTime<Utc> = modified.into();
                    if mt < cutoff {
                        continue;
                    }
                }
            }
            out.push(path);
        }
    }
}

/// Sum tokens per model over the last `WINDOW_DAYS`. Returns (models desc, total).
fn gather() -> (Vec<ModelUsage>, u64) {
    let cutoff = Utc::now() - Duration::days(WINDOW_DAYS);

    let mut files = Vec::new();
    for dir in scan_dirs() {
        if dir.exists() {
            collect_jsonl(&dir, cutoff, &mut files);
        }
    }

    use std::collections::HashMap;
    let mut totals: HashMap<String, u64> = HashMap::new(); // model -> tokens
    let mut seen_ids: HashSet<String> = HashSet::new();

    for path in files {
        let Ok(contents) = fs::read_to_string(&path) else {
            continue;
        };
        for line in contents.lines() {
            let entry: Entry = match serde_json::from_str(line) {
                Ok(e) => e,
                Err(_) => continue,
            };
            if entry.entry_type.as_deref() != Some("assistant") {
                continue;
            }
            // Per-message timestamp filter for an accurate window.
            if let Some(ts) = &entry.timestamp {
                if let Ok(dt) = DateTime::parse_from_rfc3339(ts) {
                    if dt.with_timezone(&Utc) < cutoff {
                        continue;
                    }
                }
            }
            let Some(message) = entry.message else { continue };
            let Some(usage) = message.usage else { continue };
            if let Some(id) = &message.id {
                if !seen_ids.insert(id.clone()) {
                    continue;
                }
            }
            let model = message.model.unwrap_or_else(|| "unknown".to_string());
            if model == "<synthetic>" {
                continue;
            }
            let tokens = usage.input_tokens.unwrap_or(0)
                + usage.output_tokens.unwrap_or(0)
                + usage.cache_creation_input_tokens.unwrap_or(0)
                + usage.cache_read_input_tokens.unwrap_or(0);
            *totals.entry(model).or_insert(0) += tokens;
        }
    }

    let total: u64 = totals.values().sum();
    let mut models: Vec<ModelUsage> = totals
        .into_iter()
        .map(|(model, tokens)| ModelUsage { model, tokens })
        .collect();
    models.sort_by(|a, b| b.tokens.cmp(&a.tokens));
    (models, total)
}

/// Map a raw model id to a short display name.
fn friendly(model: &str) -> String {
    let m = model.to_lowercase();
    let family = if m.contains("fable") {
        "Fable"
    } else if m.contains("opus") {
        "Opus"
    } else if m.contains("sonnet") {
        "Sonnet"
    } else if m.contains("haiku") {
        "Haiku"
    } else {
        return model.to_string();
    };
    // Pull the first "N-M" or "N" version chunk, e.g. claude-opus-4-8 -> 4.8.
    let ver: Vec<&str> = m
        .split('-')
        .skip_while(|p| !p.chars().next().map(|c| c.is_ascii_digit()).unwrap_or(false))
        .take(2)
        .collect();
    match ver.len() {
        0 => family.to_string(),
        1 => format!("{family} {}", ver[0]),
        _ => format!("{family} {}.{}", ver[0], ver[1]),
    }
}

/// Total Fable tokens over the window, for the combined stats screen.
pub fn fable_tokens_7d() -> u64 {
    let (models, _) = gather();
    models
        .iter()
        .find(|m| m.model.to_lowercase().contains("fable"))
        .map(|m| m.tokens)
        .unwrap_or(0)
}

pub fn format_tokens(n: u64) -> String {
    let f = n as f64;
    if f >= 1e9 {
        format!("{:.1}B", f / 1e9)
    } else if f >= 1e6 {
        format!("{:.1}M", f / 1e6)
    } else if f >= 1e3 {
        format!("{:.0}K", f / 1e3)
    } else {
        n.to_string()
    }
}

fn format_reset(minutes: f64) -> String {
    let total = minutes.max(0.0).round() as u64;
    let days = total / 1440;
    let hours = (total % 1440) / 60;
    if days > 0 {
        format!("resets {days}d {hours}h")
    } else if hours > 0 {
        format!("resets {hours}h")
    } else {
        format!("resets {}m", total % 60)
    }
}

/// (left, right) bar colors for a rate-limit gauge (low = good).
fn level_colors(level: &str) -> (image::Rgba<u8>, image::Rgba<u8>) {
    match level {
        "danger" | "over" => (draw::DANGER, draw::DANGER),
        "warn" => (draw::WARN_LEFT, draw::WARN_RIGHT),
        _ => (draw::OK_LEFT, draw::OK_RIGHT),
    }
}

pub fn render_screen() -> RgbaImage {
    let font = draw::font();
    let font_bold = draw::font_bold();
    let mut img = draw::new_canvas();

    draw::draw_header(&mut img, &font, &font_bold, "Fable 5 Usage", &format!("{WINDOW_DAYS}d"));

    let mx = 16i32;
    let right_edge = draw::W as i32 - mx;
    let bar_w = (right_edge - mx) as u32;

    // Weekly rate-limit gauge for Fable, straight from the API.
    let fable_weekly = crate::stats::fetch_stats()
        .ok()
        .and_then(|d| d.scoped.into_iter().find(|s| s.model.to_lowercase().contains("fable")));

    // ── Hero: Fable weekly limit ──
    draw::draw_text(&mut img, draw::GOOD_LEFT, mx, 42, 12.0, &font_bold, "WEEKLY LIMIT");
    if let Some(fw) = &fable_weekly {
        if let Some(mins) = fw.resets_in_minutes {
            draw::draw_text_right(&mut img, draw::TEXT_MUTED, right_edge, 43, 12.0, &font, &format_reset(mins));
        }
        draw::draw_text(&mut img, draw::TEXT_PRIMARY, mx, 55, 34.0, &font_bold, &format!("{:.0}%", fw.utilization));
        let remaining = (100.0 - fw.utilization).max(0.0);
        draw::draw_text_right(&mut img, draw::TEXT_MUTED, right_edge, 68, 13.0, &font, &format!("{:.0}% left", remaining));
        let (l, r) = level_colors(&fw.usage_level);
        draw::draw_gradient_bar(&mut img, mx, 96, bar_w, 10, (fw.utilization / 100.0) as f32, l, r, 5);
    } else {
        draw::draw_text(&mut img, draw::TEXT_DIM, mx, 58, 15.0, &font, "No weekly Fable limit reported");
    }

    // ── Token volume by model (from local logs) ──
    let (models, total) = gather();
    draw::draw_rounded_rect(&mut img, mx, 116, bar_w, 1, 0, draw::SEPARATOR);
    draw::draw_text(&mut img, draw::TEXT_DIM, mx, 122, 11.0, &font, "TOKENS BY MODEL · 7d");

    if total == 0 {
        draw::draw_text(&mut img, draw::TEXT_DIM, mx, 150, 13.0, &font, "No token usage in last 7 days");
        return img;
    }

    let max_tokens = models.first().map(|m| m.tokens).unwrap_or(1).max(1);
    let mut y = 140;
    for m in models.iter().take(3) {
        let is_fable = m.model.to_lowercase().contains("fable");
        let name_color = if is_fable { draw::GOOD_LEFT } else { draw::TEXT_PRIMARY };
        draw::draw_text(&mut img, name_color, mx, y, 13.0, &font_bold, &friendly(&m.model));
        draw::draw_text_right(&mut img, draw::TEXT_MUTED, right_edge, y, 13.0, &font, &format_tokens(m.tokens));

        let frac = m.tokens as f32 / max_tokens as f32;
        let (l, r) = if is_fable {
            (draw::GOOD_LEFT, draw::GOOD_RIGHT)
        } else {
            (draw::OK_LEFT, draw::OK_RIGHT)
        };
        draw::draw_gradient_bar(&mut img, mx, y + 15, bar_w, 4, frac, l, r, 2);
        y += 28;
    }

    img
}
