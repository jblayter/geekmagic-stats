//! System vitals screen: CPU load, memory pressure, and battery.

use std::process::Command;

use image::{Rgba, RgbaImage};

use crate::draw;

fn cmd_stdout(cmd: &str, args: &[&str]) -> Option<String> {
    let out = Command::new(cmd).args(args).output().ok()?;
    if !out.status.success() {
        return None;
    }
    Some(String::from_utf8_lossy(&out.stdout).to_string())
}

/// (load1, ncpu) — 1-minute load average and logical CPU count.
fn cpu() -> (f64, u32) {
    let load = cmd_stdout("sysctl", &["-n", "vm.loadavg"])
        .and_then(|s| {
            s.split_whitespace()
                .find_map(|tok| tok.trim_matches(|c| c == '{' || c == '}').parse::<f64>().ok())
        })
        .unwrap_or(0.0);
    let ncpu = cmd_stdout("sysctl", &["-n", "hw.ncpu"])
        .and_then(|s| s.trim().parse::<u32>().ok())
        .unwrap_or(1)
        .max(1);
    (load, ncpu)
}

/// Fraction of physical memory in use (0.0–1.0) and total bytes.
fn memory() -> (f32, u64) {
    let total = cmd_stdout("sysctl", &["-n", "hw.memsize"])
        .and_then(|s| s.trim().parse::<u64>().ok())
        .unwrap_or(0);
    // memory_pressure reports "System-wide memory free percentage: NN%".
    let free_pct = cmd_stdout("memory_pressure", &[])
        .and_then(|s| {
            s.lines()
                .find(|l| l.contains("free percentage"))
                .and_then(|l| l.rsplit(':').next())
                .map(|v| v.trim().trim_end_matches('%').to_string())
                .and_then(|v| v.parse::<f32>().ok())
        })
        .unwrap_or(50.0);
    ((100.0 - free_pct) / 100.0, total)
}

struct Battery {
    pct: u32,
    charging: bool,
    present: bool,
}

fn battery() -> Battery {
    let raw = cmd_stdout("pmset", &["-g", "batt"]).unwrap_or_default();
    let on_ac = raw.contains("AC Power");
    let pct = raw
        .split('%')
        .next()
        .and_then(|before| {
            let digits: String = before
                .chars()
                .rev()
                .take_while(|c| c.is_ascii_digit())
                .collect();
            digits.chars().rev().collect::<String>().parse::<u32>().ok()
        })
        .unwrap_or(0);
    let present = raw.contains("InternalBattery");
    let discharging = raw.contains("discharging");
    Battery {
        pct,
        // On AC and not actively discharging reads as plugged in.
        charging: present && on_ac && !discharging,
        present,
    }
}

/// e.g. "up 8d 19h" from `uptime`.
fn uptime_short() -> String {
    let raw = cmd_stdout("uptime", &[]).unwrap_or_default();
    if let Some(idx) = raw.find(" up ") {
        let end = &raw[idx + 4..];
        let mut days = 0u64;
        let mut hours = 0u64;
        if let Some(dpos) = end.find("day") {
            days = end[..dpos]
                .trim()
                .split_whitespace()
                .last()
                .and_then(|d| d.parse().ok())
                .unwrap_or(0);
            if let Some(after) = end[dpos..].find(',') {
                let hm = end[dpos + after + 1..].trim();
                hours = hm.split(':').next().and_then(|h| h.trim().parse().ok()).unwrap_or(0);
            }
        } else if let Some(colon) = end.find(':') {
            hours = end[..colon]
                .trim()
                .split_whitespace()
                .last()
                .and_then(|h| h.parse().ok())
                .unwrap_or(0);
        }
        if days > 0 {
            return format!("up {days}d {hours}h");
        }
        return format!("up {hours}h");
    }
    String::new()
}

/// (left, right) gradient colors for a "higher is worse" fraction.
fn load_colors(frac: f32) -> (Rgba<u8>, Rgba<u8>) {
    if frac >= 0.9 {
        (draw::DANGER, draw::DANGER)
    } else if frac >= 0.7 {
        (draw::WARN_LEFT, draw::WARN_RIGHT)
    } else {
        (draw::OK_LEFT, draw::OK_RIGHT)
    }
}

/// Battery colors — "higher is better", green when healthy or charging.
fn battery_colors(pct: u32, charging: bool) -> (Rgba<u8>, Rgba<u8>) {
    if charging || pct > 50 {
        (draw::GOOD_LEFT, draw::GOOD_RIGHT)
    } else if pct > 20 {
        (draw::WARN_LEFT, draw::WARN_RIGHT)
    } else {
        (draw::DANGER, draw::DANGER)
    }
}

fn format_gb(bytes: u64) -> String {
    format!("{:.0} GB", bytes as f64 / 1_000_000_000.0)
}

struct Row {
    label: String,
    value: String,
    caption: String,
    frac: f32,
    colors: (Rgba<u8>, Rgba<u8>),
}

fn build_rows() -> Vec<Row> {
    let (load1, ncpu) = cpu();
    let cpu_frac = (load1 / ncpu as f64) as f32;
    let cpu_row = Row {
        label: "CPU".to_string(),
        value: format!("{:.0}%", cpu_frac.clamp(0.0, 9.99) * 100.0),
        caption: format!("load {load1:.2} · {ncpu} cores"),
        frac: cpu_frac,
        colors: load_colors(cpu_frac),
    };

    let (mem_frac, mem_total) = memory();
    let mem_row = Row {
        label: "Memory".to_string(),
        value: format!("{:.0}%", mem_frac * 100.0),
        caption: if mem_total > 0 {
            format!(
                "{} used of {}",
                format_gb((mem_frac as f64 * mem_total as f64) as u64),
                format_gb(mem_total)
            )
        } else {
            String::new()
        },
        frac: mem_frac,
        colors: load_colors(mem_frac),
    };

    let bat = battery();
    let bat_row = Row {
        label: "Battery".to_string(),
        value: if bat.present { format!("{}%", bat.pct) } else { "AC".to_string() },
        caption: if !bat.present {
            "No battery".to_string()
        } else if bat.charging {
            "charging".to_string()
        } else {
            "on battery".to_string()
        },
        frac: if bat.present { bat.pct as f32 / 100.0 } else { 1.0 },
        colors: battery_colors(bat.pct, bat.charging),
    };

    vec![cpu_row, mem_row, bat_row]
}

pub fn render_screen() -> RgbaImage {
    let font = draw::font();
    let font_bold = draw::font_bold();
    let mut img = draw::new_canvas();

    let name = draw::machine_name("System");
    let uptime = uptime_short();
    draw::draw_header(&mut img, &font, &font_bold, &name, &uptime);

    let mx = 16i32;
    let right_edge = draw::W as i32 - mx;
    let bar_w = (right_edge - mx) as u32;

    let mut y = 48;
    for row in build_rows() {
        draw::draw_text(&mut img, draw::TEXT_PRIMARY, mx, y, 15.0, &font_bold, &row.label);
        draw::draw_text_right(&mut img, draw::TEXT_PRIMARY, right_edge, y, 15.0, &font, &row.value);
        draw::draw_gradient_bar(&mut img, mx, y + 21, bar_w, 12, row.frac, row.colors.0, row.colors.1, 6);
        if !row.caption.is_empty() {
            draw::draw_text(&mut img, draw::TEXT_DIM, mx, y + 37, 11.0, &font, &row.caption);
        }
        y += 58;
    }

    img
}
