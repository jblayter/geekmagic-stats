//! CI Runners screen: self-hosted GitHub Actions runners running locally in
//! Docker, cross-referenced with the GitHub API for busy/idle status, active
//! jobs, and queue depth.

use std::collections::BTreeSet;
use std::process::Command;

use anyhow::{anyhow, Context, Result};
use image::RgbaImage;
use serde_json::Value;

use crate::draw;

/// A running self-hosted runner container discovered via Docker.
struct RunnerContainer {
    repo_url: String,
}

/// Why the CI screen couldn't gather data — some states get a dedicated card.
enum CiError {
    /// The `docker` CLI isn't installed.
    DockerMissing,
    /// Docker is installed but the daemon isn't reachable (Desktop not running).
    DockerDown,
    Other(String),
}

/// Run `docker ps`, distinguishing "not installed" from "daemon not running".
fn docker_ps() -> std::result::Result<String, CiError> {
    match Command::new("docker")
        .args(["ps", "--format", "{{.Names}}\t{{.Image}}"])
        .output()
    {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Err(CiError::DockerMissing),
        Err(e) => Err(CiError::Other(e.to_string())),
        Ok(out) if out.status.success() => Ok(String::from_utf8_lossy(&out.stdout).to_string()),
        Ok(out) => {
            let stderr = String::from_utf8_lossy(&out.stderr).to_lowercase();
            // The daemon-not-running message varies by platform/version.
            if stderr.contains("cannot connect to the docker daemon")
                || stderr.contains("is the docker daemon running")
                || stderr.contains("docker daemon")
            {
                Err(CiError::DockerDown)
            } else {
                Err(CiError::Other(
                    String::from_utf8_lossy(&out.stderr).trim().to_string(),
                ))
            }
        }
    }
}

fn docker_json(args: &[&str]) -> Result<String> {
    let out = Command::new("docker")
        .args(args)
        .output()
        .context("failed to run docker (is Docker installed and running?)")?;
    if !out.status.success() {
        return Err(anyhow!(
            "docker {} failed: {}",
            args.join(" "),
            String::from_utf8_lossy(&out.stderr).trim()
        ));
    }
    Ok(String::from_utf8_lossy(&out.stdout).to_string())
}

/// Discover running containers whose image looks like a GitHub Actions runner.
fn find_runner_containers() -> std::result::Result<Vec<RunnerContainer>, CiError> {
    // "<name>\t<image>" per line.
    let listing = docker_ps()?;
    let mut names = Vec::new();
    for line in listing.lines() {
        let mut parts = line.splitn(2, '\t');
        let name = parts.next().unwrap_or("").trim();
        let image = parts.next().unwrap_or("").trim();
        if image.contains("github-runner") || image.contains("actions-runner") {
            names.push(name.to_string());
        }
    }

    let mut runners = Vec::new();
    for name in names {
        let repo_url = docker_json(&[
            "inspect",
            &name,
            "--format",
            "{{range .Config.Env}}{{println .}}{{end}}",
        ])
        .ok()
        .and_then(|env| {
            env.lines()
                .find_map(|l| l.strip_prefix("REPO_URL=").map(|v| v.trim().to_string()))
        })
        .unwrap_or_default();
        runners.push(RunnerContainer { repo_url });
    }
    Ok(runners)
}

fn gh_api(path: &str) -> Result<Value> {
    let out = Command::new("gh")
        .args(["api", path])
        .output()
        .context("failed to run gh")?;
    if !out.status.success() {
        return Err(anyhow!(
            "gh api {path} failed: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        ));
    }
    serde_json::from_slice(&out.stdout).context("failed to parse gh api JSON")
}

/// owner/repo → short repo name.
fn repo_short(owner_repo: &str) -> String {
    owner_repo.rsplit('/').next().unwrap_or(owner_repo).to_string()
}

/// Strip a REPO_URL like https://github.com/Owner/Repo to "Owner/Repo".
fn repo_slug(url: &str) -> Option<String> {
    let trimmed = url.trim_end_matches('/');
    let after = trimmed.split("github.com/").nth(1)?;
    let mut it = after.split('/');
    let owner = it.next()?;
    let repo = it.next()?;
    if owner.is_empty() || repo.is_empty() {
        None
    } else {
        Some(format!("{owner}/{repo}"))
    }
}

struct ActiveRun {
    name: String,
    repo: String,
    branch: String,
    elapsed_min: i64,
    /// true = a job is executing on a runner; false = waiting in the queue.
    running: bool,
}

struct CiData {
    containers_up: usize,
    repos: Vec<String>,
    runners_online: usize,
    runners_busy: usize,
    queued: usize,
    active: Vec<ActiveRun>,
}

fn minutes_since(iso: &str) -> i64 {
    use chrono::{DateTime, Local, Utc};
    if let Ok(dt) = DateTime::parse_from_rfc3339(iso) {
        let now: DateTime<Utc> = Local::now().with_timezone(&Utc);
        return (now - dt.with_timezone(&Utc)).num_minutes().max(0);
    }
    0
}

fn gather() -> std::result::Result<CiData, CiError> {
    let containers = find_runner_containers()?;
    let containers_up = containers.len();

    let repos: BTreeSet<String> = containers
        .iter()
        .filter_map(|c| repo_slug(&c.repo_url))
        .collect();
    let repos: Vec<String> = repos.into_iter().collect();

    let mut runners_online = 0;
    let mut runners_busy = 0;
    let mut queued = 0;
    let mut active = Vec::new();

    for repo in &repos {
        if let Ok(v) = gh_api(&format!("repos/{repo}/actions/runners")) {
            if let Some(list) = v.get("runners").and_then(|r| r.as_array()) {
                for r in list {
                    if r.get("status").and_then(|s| s.as_str()) == Some("online") {
                        runners_online += 1;
                    }
                    if r.get("busy").and_then(|b| b.as_bool()) == Some(true) {
                        runners_busy += 1;
                    }
                }
            }
        }

        let parse_runs = |v: &Value, running: bool, out: &mut Vec<ActiveRun>| {
            if let Some(runs) = v.get("workflow_runs").and_then(|r| r.as_array()) {
                for run in runs {
                    out.push(ActiveRun {
                        name: run
                            .get("name")
                            .and_then(|n| n.as_str())
                            .unwrap_or("workflow")
                            .to_string(),
                        repo: repo_short(repo),
                        branch: run
                            .get("head_branch")
                            .and_then(|b| b.as_str())
                            .unwrap_or("")
                            .to_string(),
                        elapsed_min: run
                            .get("created_at")
                            .and_then(|c| c.as_str())
                            .map(minutes_since)
                            .unwrap_or(0),
                        running,
                    });
                }
            }
        };

        // Runs actively executing on a runner.
        if let Ok(v) = gh_api(&format!(
            "repos/{repo}/actions/runs?status=in_progress&per_page=10"
        )) {
            parse_runs(&v, true, &mut active);
        }

        // Queued runs — with busy runners these are usually already executing;
        // GitHub just rolls the run status up to "queued". Show their branches.
        if let Ok(v) = gh_api(&format!(
            "repos/{repo}/actions/runs?status=queued&per_page=10"
        )) {
            queued += v.get("total_count").and_then(|c| c.as_u64()).unwrap_or(0) as usize;
            parse_runs(&v, false, &mut active);
        }
    }

    // GitHub often reports a run as "queued" even while a busy runner executes
    // it. Reconcile: if fewer runs look "running" than there are busy runners,
    // promote the most-recently-created queued runs so the display matches how
    // many runners are actually working.
    if runners_busy > 0 {
        let running_now = active.iter().filter(|r| r.running).count();
        if running_now < runners_busy {
            let mut queued_idx: Vec<usize> = active
                .iter()
                .enumerate()
                .filter(|(_, r)| !r.running)
                .map(|(i, _)| i)
                .collect();
            // Most recently created first (smallest elapsed).
            queued_idx.sort_by_key(|&i| active[i].elapsed_min);
            for &i in queued_idx.iter().take(runners_busy - running_now) {
                active[i].running = true;
            }
        }
    }

    // Running first, then longest-waiting.
    active.sort_by(|a, b| {
        b.running
            .cmp(&a.running)
            .then(b.elapsed_min.cmp(&a.elapsed_min))
    });

    Ok(CiData {
        containers_up,
        repos: repos.iter().map(|r| repo_short(r)).collect(),
        runners_online,
        runners_busy,
        queued,
        active,
    })
}

fn format_elapsed(min: i64) -> String {
    if min >= 60 {
        format!("{}h{}m", min / 60, min % 60)
    } else {
        format!("{min}m")
    }
}

fn tile(img: &mut RgbaImage, x: i32, label: &str, value: &str, value_color: image::Rgba<u8>) {
    let font = draw::font();
    let font_bold = draw::font_bold();
    draw::draw_text(img, draw::TEXT_MUTED, x, 44, 11.0, &font, label);
    draw::draw_text(img, value_color, x, 57, 24.0, &font_bold, value);
}

fn render(data: &CiData) -> RgbaImage {
    let font = draw::font();
    let font_bold = draw::font_bold();
    let mut img = draw::new_canvas();

    let header_right = match data.repos.len() {
        0 => String::new(),
        1 => data.repos[0].clone(),
        n => format!("{n} repos"),
    };
    draw::draw_header(&mut img, &font, &font_bold, "CI Runners", &header_right);

    let mx = 16i32;
    let right_edge = draw::W as i32 - mx;

    // ── Tiles: Online / Busy / Queued ──
    let online_color = if data.runners_online == 0 {
        draw::DANGER
    } else if data.runners_online < data.containers_up {
        draw::WARN_LEFT
    } else {
        draw::GOOD_LEFT
    };
    tile(
        &mut img,
        mx + 4,
        "ONLINE",
        &format!("{}/{}", data.runners_online, data.containers_up.max(data.runners_online)),
        online_color,
    );
    tile(
        &mut img,
        92,
        "BUSY",
        &data.runners_busy.to_string(),
        if data.runners_busy > 0 { draw::OK_LEFT } else { draw::TEXT_PRIMARY },
    );
    tile(
        &mut img,
        168,
        "QUEUED",
        &data.queued.to_string(),
        if data.queued > 0 { draw::WARN_LEFT } else { draw::TEXT_PRIMARY },
    );

    draw::draw_rounded_rect(&mut img, mx, 90, (right_edge - mx) as u32, 1, 0, draw::SEPARATOR);
    draw::draw_text(&mut img, draw::TEXT_DIM, mx, 98, 12.0, &font, "ACTIVE JOBS");

    if data.active.is_empty() {
        let msg = if data.containers_up == 0 {
            "No runner containers".to_string()
        } else if data.runners_busy > 0 {
            // Runners report busy but no run surfaced yet (endpoints polled
            // separately and briefly disagree as a run spins up).
            format!("{} runner(s) working…", data.runners_busy)
        } else {
            "Idle — no jobs running".to_string()
        };
        let color = if data.runners_busy > 0 { draw::TEXT_MUTED } else { draw::TEXT_DIM };
        draw::draw_text(&mut img, color, mx, 138, 15.0, &font, &msg);
        return img;
    }

    let single_repo = data.repos.len() <= 1;
    let mut y = 118;
    for run in data.active.iter().take(3) {
        // Green = executing on a runner, amber = waiting in the queue.
        let dot = if run.running { draw::GOOD_LEFT } else { draw::WARN_LEFT };
        draw::draw_circle(&mut img, mx + 5, y + 8, 4, dot);

        let elapsed = format_elapsed(run.elapsed_min);
        draw::draw_text_right(&mut img, draw::TEXT_MUTED, right_edge, y, 13.0, &font, &elapsed);

        // Headline: the branch under test (fall back to the workflow name).
        let text_x = mx + 18;
        let headline = if run.branch.is_empty() {
            run.name.clone()
        } else {
            run.branch.clone()
        };
        let head_max = right_edge - text_x - draw::approx_text_width(&elapsed, 13.0) - 6;
        let headline = draw::truncate_to_width(&headline, 14.0, head_max);
        draw::draw_text(&mut img, draw::TEXT_PRIMARY, text_x, y, 14.0, &font_bold, &headline);

        // Secondary: workflow name (+ repo when several are in play), and a
        // "queued" marker for runs still waiting.
        let mut caption = if single_repo || run.branch.is_empty() {
            run.name.clone()
        } else {
            format!("{}  ·  {}", run.name, run.repo)
        };
        if !run.running {
            caption = format!("{caption}  ·  queued");
        }
        let caption = draw::truncate_to_width(&caption, 11.0, right_edge - text_x);
        draw::draw_text(&mut img, draw::TEXT_DIM, text_x, y + 16, 11.0, &font, &caption);

        y += 38;
    }

    img
}

/// A centered notice card (used for Docker states and errors).
fn render_notice(headline: &str, sub: &str, color: image::Rgba<u8>) -> RgbaImage {
    let font = draw::font();
    let font_bold = draw::font_bold();
    let mut img = draw::new_canvas();
    draw::draw_header(&mut img, &font, &font_bold, "CI Runners", "");
    draw::draw_text_centered(&mut img, color, draw::W as i32 / 2, 108, 18.0, &font_bold, headline);
    if !sub.is_empty() {
        let sub = draw::truncate_to_width(sub, 12.0, draw::W as i32 - 24);
        draw::draw_text_centered(&mut img, draw::TEXT_DIM, draw::W as i32 / 2, 136, 12.0, &font, &sub);
    }
    img
}

pub fn render_screen() -> RgbaImage {
    match gather() {
        Ok(data) => render(&data),
        Err(CiError::DockerMissing) => {
            render_notice("Docker not installed", "Runners need Docker", draw::TEXT_MUTED)
        }
        Err(CiError::DockerDown) => {
            render_notice("Docker isn't running", "Start Docker Desktop", draw::WARN_LEFT)
        }
        Err(CiError::Other(e)) => {
            eprintln!("Error gathering CI data: {e}");
            render_notice("Unavailable", &e, draw::DANGER)
        }
    }
}
