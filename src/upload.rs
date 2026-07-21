use std::io::Cursor;
use std::net::{TcpStream, ToSocketAddrs};
use std::time::Duration;

use anyhow::{Context, Result};
use image::RgbaImage;
use reqwest::blocking::multipart;

/// Quick check that the device is on the network and accepting connections.
/// Because the display only lives on the home LAN, a successful probe doubles
/// as an "am I home?" check. `host` may be "ip" or "ip:port" (defaults to :80).
pub fn is_reachable(host: &str) -> bool {
    let addr = if host.contains(':') {
        host.to_string()
    } else {
        format!("{host}:80")
    };
    // A device with Wi-Fi power-saving (and especially one just coming out of a
    // display-off/night-mode window) can take several seconds to wake its radio
    // and answer. Retry a handful of times: the repeated SYNs act as wake-up
    // traffic, and a later attempt succeeds once the radio is up. Worst case
    // (genuinely away) this blocks ~10s, which is fine against a 60s cycle.
    let Ok(mut addrs) = addr.to_socket_addrs() else {
        return false;
    };
    let Some(sa) = addrs.next() else {
        return false;
    };
    for _ in 0..5 {
        if TcpStream::connect_timeout(&sa, Duration::from_millis(2000)).is_ok() {
            return true;
        }
    }
    false
}

fn encode_jpeg(img: &RgbaImage) -> Result<Vec<u8>> {
    let rgb = image::DynamicImage::ImageRgba8(img.clone()).into_rgb8();
    let mut jpeg_buf = Cursor::new(Vec::new());
    rgb.write_to(&mut jpeg_buf, image::ImageFormat::Jpeg)?;
    Ok(jpeg_buf.into_inner())
}

fn upload_file(
    client: &reqwest::blocking::Client,
    base: &str,
    filename: &str,
    jpeg_bytes: Vec<u8>,
) -> Result<()> {
    let part = multipart::Part::bytes(jpeg_bytes)
        .file_name(filename.to_string())
        .mime_str("image/jpeg")?;
    let form = multipart::Form::new().part("file", part);

    let resp = client
        .post(format!("{base}/doUpload?dir=/image/"))
        .multipart(form)
        .send();

    match resp {
        Ok(_) => {}
        Err(e) => {
            let msg = e.to_string();
            if msg.contains("Duplicate Content-Length")
                || msg.contains("Data after")
                || msg.contains("invalid content-length")
            {
            } else {
                return Err(e).context("upload failed");
            }
        }
    }
    Ok(())
}

fn make_client() -> Result<reqwest::blocking::Client> {
    Ok(reqwest::blocking::Client::builder()
        .timeout(std::time::Duration::from_secs(30))
        .build()?)
}

/// Upload a single image under `filename` and display it. `filename` should end
/// in `.jpg` (e.g. "stats.jpg", "pr-dashboard.jpg") so multiple screens can live
/// on the device without clobbering each other.
pub fn upload_named(host: &str, filename: &str, img: &RgbaImage) -> Result<()> {
    let base = format!("http://{host}");
    let client = make_client()?;

    upload_file(&client, &base, filename, encode_jpeg(img)?)?;

    client
        .get(format!("{base}/set?theme=3"))
        .send()
        .context("failed to set theme")?;
    client
        .get(format!("{base}/set?img=/image//{filename}"))
        .send()
        .context("failed to set image")?;

    Ok(())
}

pub fn upload_and_display(host: &str, img: &RgbaImage) -> Result<()> {
    upload_named(host, "stats.jpg", img)
}

pub fn upload_album(host: &str, images: &[(&str, &RgbaImage)]) -> Result<()> {
    upload_album_every(host, images, 10)
}

/// Like [`upload_album`], but with a configurable on-device autoplay interval
/// (seconds) between screens.
pub fn upload_album_every(
    host: &str,
    images: &[(&str, &RgbaImage)],
    interval_secs: u64,
) -> Result<()> {
    let base = format!("http://{host}");
    let client = make_client()?;

    // Clear existing images
    let resp = client.get(format!("{base}/filelist?dir=/image/")).send()?;
    let body = resp.text().unwrap_or_default();
    for line in body.lines() {
        let name = line.trim();
        if !name.is_empty() && name.ends_with(".jpg") {
            let _ = client.get(format!("{base}/del?path=/image//{name}")).send();
        }
    }

    for (filename, img) in images {
        upload_file(&client, &base, filename, encode_jpeg(img)?)?;
    }

    client
        .get(format!("{base}/set?theme=3"))
        .send()
        .context("failed to set theme")?;
    if let Some((first, _)) = images.first() {
        client
            .get(format!("{base}/set?img=/image//{first}"))
            .send()
            .context("failed to set image")?;
    }

    // Enable autoplay with the requested interval
    let interval = interval_secs.max(1);
    client
        .get(format!("{base}/set?i_i={interval}&autoplay=1"))
        .send()
        .context("failed to enable autoplay")?;

    Ok(())
}
