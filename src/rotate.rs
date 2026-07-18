//! `geekmagic-all` — render every enabled screen and push them to the device as
//! a single rotating album (the display auto-cycles through them).

use std::thread;
use std::time::Duration;

use anyhow::{anyhow, Result};
use clap::Parser;
use image::RgbaImage;

use geekmagic_common::{ci, config, pr, render, sysinfo, upload, usage};

#[derive(Parser)]
#[command(about = "Render all screens and push them as a rotating album")]
struct Args {
    /// GeekMagic device IP address
    #[arg(long)]
    host: Option<String>,

    /// Path to config file
    #[arg(long)]
    config: Option<String>,

    /// Comma-separated screens in order (stats,git,sys,ci). Defaults to all.
    #[arg(long)]
    screens: Option<String>,

    /// On-device seconds per screen
    #[arg(long)]
    interval: Option<u64>,

    /// Re-render and re-push every N seconds
    #[arg(short, long)]
    daemon: Option<u64>,

    /// Save all screens as PNGs into this directory instead of uploading
    #[arg(short, long)]
    output_dir: Option<String>,
}

// "usage" is folded into "stats" now, but remains available on request.
const DEFAULT_SCREENS: &[&str] = &["stats", "git", "sys", "ci"];

/// Render one screen by name. Returns (device filename, image). `None` means the
/// screen has no data right now and should be skipped from the rotation.
fn render_screen(name: &str) -> Option<(String, RgbaImage)> {
    match name {
        "stats" => Some(("1-stats.jpg".to_string(), render::render_screen())),
        "git" | "pr" => Some(("3-pr-dashboard.jpg".to_string(), pr::render_screen())),
        "sys" | "vitals" => Some(("4-system-vitals.jpg".to_string(), sysinfo::render_screen())),
        "ci" | "runners" => Some(("5-ci-runners.jpg".to_string(), ci::render_screen())),
        "usage" | "models" | "fable" => {
            Some(("6-model-usage.jpg".to_string(), usage::render_screen()))
        }
        other => {
            eprintln!("Unknown screen '{other}', skipping");
            None
        }
    }
}

fn resolve_screens(args_screens: Option<String>, cfg_screens: Option<Vec<String>>) -> Vec<String> {
    if let Some(s) = args_screens {
        return s
            .split(',')
            .map(|x| x.trim().to_string())
            .filter(|x| !x.is_empty())
            .collect();
    }
    cfg_screens.unwrap_or_else(|| DEFAULT_SCREENS.iter().map(|s| s.to_string()).collect())
}

fn run_once(host: &str, screens: &[String], interval: u64, output_dir: &Option<String>) -> Result<()> {
    // When pushing to a device, bail early if it isn't reachable — this doubles
    // as an "am I home?" check and skips the (heavy) render work when away.
    if output_dir.is_none() && !upload::is_reachable(host) {
        let now = chrono::Local::now().format("%H:%M:%S");
        println!("[{now}] Device {host} unreachable — skipping (not home?)");
        return Ok(());
    }

    let rendered: Vec<(String, RgbaImage)> =
        screens.iter().filter_map(|s| render_screen(s)).collect();

    if rendered.is_empty() {
        return Err(anyhow!("no screens produced output"));
    }

    if let Some(dir) = output_dir {
        std::fs::create_dir_all(dir)?;
        for (name, img) in &rendered {
            // Local previews as PNG (JPEG can't hold RGBA); device gets JPEG on upload.
            let png_name = name.replace(".jpg", ".png");
            let path = format!("{dir}/{png_name}");
            img.save(&path)?;
            println!("Saved {path}");
        }
        return Ok(());
    }

    let album: Vec<(&str, &RgbaImage)> =
        rendered.iter().map(|(n, img)| (n.as_str(), img)).collect();
    upload::upload_album_every(host, &album, interval)?;

    let now = chrono::Local::now().format("%H:%M:%S");
    let names: Vec<&str> = screens.iter().map(|s| s.as_str()).collect();
    println!(
        "[{now}] Pushed {} screens ({}) to {host}, rotating every {interval}s",
        album.len(),
        names.join(", ")
    );
    Ok(())
}

fn main() -> Result<()> {
    let args = Args::parse();
    let cfg = config::load(args.config.as_deref())?;

    let screens = resolve_screens(args.screens, cfg.screens);
    let interval = args.interval.or(cfg.interval).unwrap_or(10);
    let output_dir = args.output_dir.clone();

    // Host only required when actually uploading.
    let host = if output_dir.is_some() {
        String::new()
    } else {
        args.host
            .or(cfg.host)
            .ok_or_else(|| anyhow!("missing host; pass --host, set host in config, or use --output-dir"))?
    };

    if let Some(refresh) = args.daemon.or(cfg.daemon) {
        let refresh = refresh.max(10);
        println!("Daemon mode: refreshing every {refresh}s to {host}");
        loop {
            if let Err(e) = run_once(&host, &screens, interval, &output_dir) {
                let now = chrono::Local::now().format("%H:%M:%S");
                eprintln!("[{now}] Error: {e}");
            }
            thread::sleep(Duration::from_secs(refresh));
        }
    } else {
        run_once(&host, &screens, interval, &output_dir)
    }
}
