//! `geekmagic-git` — render the Pull Request dashboard to a GeekMagic display.

use anyhow::{anyhow, Result};
use clap::Parser;

use geekmagic_common::{config, pr, upload};

#[derive(Parser)]
#[command(about = "Render a GitHub pull-request dashboard to a GeekMagic display")]
struct Args {
    #[arg(long)]
    host: Option<String>,

    /// Path to config file
    #[arg(long)]
    config: Option<String>,

    /// Save rendered image to this path instead of uploading
    #[arg(short, long)]
    output: Option<String>,

    /// Render representative sample data instead of querying GitHub
    #[arg(long)]
    demo: bool,
}

fn main() -> Result<()> {
    let args = Args::parse();
    let cfg = config::load(args.config.as_deref())?;

    // When uploading, verify the device is reachable before doing render work.
    let host = if args.output.is_some() {
        None
    } else {
        let host = args.host.or(cfg.host).ok_or_else(|| {
            anyhow!("missing host; pass --host, set host in config, or use --output")
        })?;
        if !upload::is_reachable(&host) {
            println!("Device {host} unreachable — skipping (not home?)");
            return Ok(());
        }
        Some(host)
    };

    let img = if args.demo {
        pr::render_demo()
    } else {
        pr::render_screen()
    };

    if let Some(path) = &args.output {
        img.save(path)?;
        println!("Saved to {path}");
        return Ok(());
    }

    let host = host.expect("host resolved when not saving to file");
    upload::upload_named(&host, "pr-dashboard.jpg", &img)?;
    println!("Pushed to {host}");
    Ok(())
}
