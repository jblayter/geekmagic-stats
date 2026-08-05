//! Model Usage screen. The Claude usage API doesn't break usage down per model,
//! so this scans the local Claude Code session logs (`~/.claude/projects/**/*.jsonl`)
//! and sums tokens per model over a rolling window — the same data the cost
//! tooling reads. Shows this week's total plus the top models by volume.

use std::collections::HashSet;
use std::env;
use std::fs;
use std::path::PathBuf;

use chrono::{DateTime, Duration, Utc};
use image::{Rgba, RgbaImage};
use serde::Deserialize;

use crate::draw;

const WINDOW_DAYS: i64 = 7;

/// Distinct gradient per rank so each model bar reads as its own series.
const PALETTE: &[(Rgba<u8>, Rgba<u8>)] = &[
    (Rgba([129, 140, 248, 255]), Rgba([167, 139, 250, 255])), // indigo → violet
    (Rgba([34, 211, 238, 255]), Rgba([16, 185, 129, 255])),   // cyan → emerald
    (Rgba([251, 146, 60, 255]), Rgba([250, 204, 21, 255])),    // orange → amber
    (Rgba([244, 114, 182, 255]), Rgba([232, 121, 249, 255])),  // pink → fuchsia
];

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
    messages: u64,
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

/// Sum tokens/messages per model over the last `WINDOW_DAYS`. Returns (models
/// sorted by tokens desc, total tokens).
fn gather() -> (Vec<ModelUsage>, u64) {
    let cutoff = Utc::now() - Duration::days(WINDOW_DAYS);

    let mut files = Vec::new();
    for dir in scan_dirs() {
        if dir.exists() {
            collect_jsonl(&dir, cutoff, &mut files);
        }
    }

    use std::collections::HashMap;
    let mut totals: HashMap<String, (u64, u64)> = HashMap::new(); // model -> (tokens, msgs)
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
            let e = totals.entry(model).or_insert((0, 0));
            e.0 += tokens;
            e.1 += 1;
        }
    }

    let total: u64 = totals.values().map(|(t, _)| *t).sum();
    let mut models: Vec<ModelUsage> = totals
        .into_iter()
        .map(|(model, (tokens, messages))| ModelUsage {
            model,
            tokens,
            messages,
        })
        .collect();
    models.sort_by(|a, b| b.tokens.cmp(&a.tokens));
    (models, total)
}

/// Map a raw model id to a short display name, e.g. claude-opus-4-8 -> "Opus 4.8".
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

/// Group digits with commas, e.g. 5216 -> "5,216".
fn thousands(n: u64) -> String {
    let s = n.to_string();
    let bytes = s.as_bytes();
    let mut out = String::new();
    for (i, c) in bytes.iter().enumerate() {
        if i > 0 && (bytes.len() - i) % 3 == 0 {
            out.push(',');
        }
        out.push(*c as char);
    }
    out
}

pub fn render_screen() -> RgbaImage {
    let font = draw::font();
    let font_bold = draw::font_bold();
    let mut img = draw::new_canvas();

    draw::draw_header(&mut img, &font, &font_bold, "Model Usage", &format!("{WINDOW_DAYS}d"));

    let mx = 16i32;
    let right_edge = draw::W as i32 - mx;
    let bar_w = (right_edge - mx) as u32;

    let (models, total) = gather();
    if total == 0 {
        draw::draw_text(&mut img, draw::TEXT_DIM, mx, 110, 15.0, &font, "No usage in last 7 days");
        return img;
    }

    // ── Hero: total tokens this week ──
    let msgs: u64 = models.iter().map(|m| m.messages).sum();
    draw::draw_text(&mut img, draw::TEXT_MUTED, mx, 44, 11.0, &font, "THIS WEEK");
    draw::draw_text(&mut img, draw::TEXT_PRIMARY, mx, 55, 32.0, &font_bold, &format_tokens(total));
    draw::draw_text(&mut img, draw::TEXT_DIM, mx + 2, 84, 12.0, &font, "tokens");
    draw::draw_text_right(
        &mut img,
        draw::TEXT_MUTED,
        right_edge,
        64,
        13.0,
        &font,
        &format!("{} msgs", thousands(msgs)),
    );

    // ── Ranked model bars ──
    draw::draw_rounded_rect(&mut img, mx, 104, bar_w, 1, 0, draw::SEPARATOR);
    draw::draw_text(&mut img, draw::TEXT_DIM, mx, 110, 11.0, &font, "BY MODEL");

    let max_tokens = models.first().map(|m| m.tokens).unwrap_or(1).max(1);
    let mut y = 128;
    for (i, m) in models.iter().take(4).enumerate() {
        let (l, r) = PALETTE[i % PALETTE.len()];
        draw::draw_circle(&mut img, mx + 3, y + 6, 3, l);
        draw::draw_text(&mut img, draw::TEXT_PRIMARY, mx + 12, y, 14.0, &font_bold, &friendly(&m.model));
        draw::draw_text_right(&mut img, draw::TEXT_MUTED, right_edge, y, 13.0, &font, &format_tokens(m.tokens));

        let frac = m.tokens as f32 / max_tokens as f32;
        draw::draw_gradient_bar(&mut img, mx, y + 16, bar_w, 6, frac, l, r, 3);
        y += 27;
    }

    img
}
