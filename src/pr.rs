//! Pull Request dashboard screen: gathers open PRs you authored and PRs awaiting
//! your review from GitHub via the `gh` CLI, plus CI status for the most recent
//! few, and renders a glanceable card.

use std::process::Command;

use anyhow::{anyhow, Context, Result};
use image::{Rgba, RgbaImage};
use serde::Deserialize;

use crate::draw;

#[derive(Deserialize)]
struct Repo {
    #[serde(rename = "nameWithOwner")]
    name_with_owner: String,
}

#[derive(Deserialize)]
struct Pr {
    number: u64,
    title: String,
    repository: Repo,
    url: String,
    #[serde(rename = "isDraft", default)]
    is_draft: bool,
    #[serde(rename = "updatedAt", default)]
    updated_at: String,
}

#[derive(Clone, Copy, PartialEq)]
enum CiStatus {
    Pass,
    Fail,
    Pending,
    None,
}

fn gh_json<T: for<'de> Deserialize<'de>>(args: &[&str]) -> Result<T> {
    let out = Command::new("gh")
        .args(args)
        .output()
        .context("failed to run gh (is the GitHub CLI installed?)")?;
    if !out.status.success() {
        return Err(anyhow!(
            "gh {} failed: {}",
            args.join(" "),
            String::from_utf8_lossy(&out.stderr).trim()
        ));
    }
    serde_json::from_slice(&out.stdout).context("failed to parse gh JSON output")
}

fn fetch_authored() -> Result<Vec<Pr>> {
    let mut prs: Vec<Pr> = gh_json(&[
        "search",
        "prs",
        "--author=@me",
        "--state=open",
        "--limit=30",
        "--json",
        "number,title,repository,url,isDraft,updatedAt",
    ])?;
    // Most recently updated first.
    prs.sort_by(|a, b| b.updated_at.cmp(&a.updated_at));
    Ok(prs)
}

fn fetch_review_count() -> Result<usize> {
    let prs: Vec<serde_json::Value> = gh_json(&[
        "search",
        "prs",
        "--review-requested=@me",
        "--state=open",
        "--limit=50",
        "--json",
        "number",
    ])?;
    Ok(prs.len())
}

#[derive(Deserialize)]
struct Check {
    bucket: String,
}

/// Aggregate CI status for a single PR. Failures win, then pending, then pass.
fn fetch_ci(url: &str) -> CiStatus {
    let out = Command::new("gh")
        .args(["pr", "checks", url, "--json", "bucket"])
        .output();
    let Ok(out) = out else {
        return CiStatus::None;
    };
    // `gh pr checks` exits non-zero when checks are failing OR when there are
    // none; distinguish by parsing whatever JSON came back.
    let checks: Vec<Check> = match serde_json::from_slice(&out.stdout) {
        Ok(c) => c,
        Err(_) => return CiStatus::None,
    };
    if checks.is_empty() {
        return CiStatus::None;
    }
    let mut pending = false;
    for c in &checks {
        match c.bucket.as_str() {
            "fail" | "cancel" => return CiStatus::Fail,
            "pending" => pending = true,
            _ => {}
        }
    }
    if pending {
        CiStatus::Pending
    } else {
        CiStatus::Pass
    }
}

fn ci_color(status: CiStatus) -> Rgba<u8> {
    match status {
        CiStatus::Pass => draw::GOOD_LEFT,
        CiStatus::Fail => draw::DANGER,
        CiStatus::Pending => draw::WARN_LEFT,
        CiStatus::None => draw::NEUTRAL,
    }
}

const LIST_LIMIT: usize = 3;

struct Dashboard {
    authored: Vec<Pr>,
    authored_total: usize,
    review_count: usize,
    ci: Vec<CiStatus>,
}

fn gather() -> Result<Dashboard> {
    let authored = fetch_authored()?;
    let review_count = fetch_review_count().unwrap_or(0);
    let authored_total = authored.len();

    let shown: Vec<Pr> = authored.into_iter().take(LIST_LIMIT).collect();
    let ci = shown.iter().map(|pr| fetch_ci(&pr.url)).collect();

    Ok(Dashboard {
        authored: shown,
        authored_total,
        review_count,
        ci,
    })
}

fn render(data: &Dashboard) -> RgbaImage {
    let font = draw::font();
    let font_bold = draw::font_bold();
    let mut img = draw::new_canvas();

    let right = format!("{} open", data.authored_total);
    draw::draw_header(&mut img, &font, &font_bold, "Pull Requests", &right);

    let mx = 16i32;
    let right_edge = draw::W as i32 - mx;

    // ── Summary tiles: Mine / To review ──
    let tile_y = 44;
    draw::draw_text(&mut img, draw::TEXT_MUTED, mx + 10, tile_y, 12.0, &font, "Mine");
    draw::draw_text(
        &mut img,
        draw::TEXT_PRIMARY,
        mx + 10,
        tile_y + 14,
        26.0,
        &font_bold,
        &data.authored_total.to_string(),
    );

    let col2 = 132i32;
    let review_color = if data.review_count > 0 {
        draw::WARN_LEFT
    } else {
        draw::TEXT_PRIMARY
    };
    draw::draw_text(&mut img, draw::TEXT_MUTED, col2, tile_y, 12.0, &font, "To review");
    draw::draw_text(
        &mut img,
        review_color,
        col2,
        tile_y + 14,
        26.0,
        &font_bold,
        &data.review_count.to_string(),
    );

    draw::draw_rounded_rect(&mut img, mx, 90, (right_edge - mx) as u32, 1, 0, draw::SEPARATOR);

    // ── Recent PR list ──
    draw::draw_text(&mut img, draw::TEXT_DIM, mx, 98, 12.0, &font, "RECENT");

    if data.authored.is_empty() {
        draw::draw_text(&mut img, draw::TEXT_DIM, mx, 140, 15.0, &font, "No open PRs 🎉");
        return img;
    }

    let mut y = 118;
    for (i, pr) in data.authored.iter().enumerate() {
        let status = data.ci.get(i).copied().unwrap_or(CiStatus::None);
        draw::draw_circle(&mut img, mx + 5, y + 8, 4, ci_color(status));

        let text_x = mx + 18;
        let repo_line = format!("{} #{}", pr.repository.name_with_owner, pr.number);
        let repo_line = draw::truncate_to_width(&repo_line, 12.0, right_edge - text_x);
        draw::draw_text(&mut img, draw::TEXT_MUTED, text_x, y, 12.0, &font, &repo_line);

        let title = if pr.is_draft {
            format!("[draft] {}", pr.title)
        } else {
            pr.title.clone()
        };
        let title = draw::truncate_to_width(&title, 14.0, right_edge - text_x);
        let title_color = if pr.is_draft {
            draw::TEXT_DIM
        } else {
            draw::TEXT_PRIMARY
        };
        draw::draw_text(&mut img, title_color, text_x, y + 15, 14.0, &font_bold, &title);

        y += 38;
    }

    img
}

/// Render an error card so callers show something rather than crashing.
fn render_error(msg: &str) -> RgbaImage {
    let font = draw::font();
    let font_bold = draw::font_bold();
    let mut img = draw::new_canvas();
    draw::draw_header(&mut img, &font, &font_bold, "Pull Requests", "");
    draw::draw_text(&mut img, draw::DANGER, 16, 100, 15.0, &font_bold, "Unavailable");
    let msg = draw::truncate_to_width(msg, 12.0, draw::W as i32 - 32);
    draw::draw_text(&mut img, draw::TEXT_DIM, 16, 122, 12.0, &font, &msg);
    img
}

/// Gather PR data and render the screen, falling back to an error card.
pub fn render_screen() -> RgbaImage {
    match gather() {
        Ok(data) => render(&data),
        Err(e) => {
            eprintln!("Error gathering PR data: {e}");
            render_error(&e.to_string())
        }
    }
}

/// Render with representative sample data — for docs/screenshots and previewing
/// the layout without a GitHub connection.
pub fn render_demo() -> RgbaImage {
    let mk = |repo: &str, number: u64, title: &str| Pr {
        number,
        title: title.to_string(),
        repository: Repo {
            name_with_owner: repo.to_string(),
        },
        url: String::new(),
        is_draft: false,
        updated_at: String::new(),
    };
    let data = Dashboard {
        authored: vec![
            mk("acme/webapp", 128, "Add OAuth login flow"),
            mk("acme/api", 77, "Fix rate limiter edge case"),
            mk("acme/cli", 9, "Bump dependencies"),
        ],
        authored_total: 4,
        review_count: 2,
        ci: vec![CiStatus::Pass, CiStatus::Fail, CiStatus::None],
    };
    render(&data)
}
