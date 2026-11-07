use super::{command, snapshot::write_new};
use std::fs;
use std::path::Path;
use std::process::Command;

pub fn generate(bytes: &[u8], source: &Path, work: &Path, resources: &Path) -> Result<(), String> {
    let icon = resources.join("Icon.icns");
    if source.extension().is_some_and(|s| s == "icns") {
        if bytes.len() < 8
            || &bytes[..4] != b"icns"
            || u32::from_be_bytes(bytes[4..8].try_into().unwrap()) as usize != bytes.len()
        {
            return Err("invalid ICNS header or length".into());
        }
        write_new(&icon, bytes)?;
    } else {
        write_new(&icon, &png_icon(bytes, work)?)?;
    }
    // Decode the resource to validate both supplied and generated ICNS files.
    command(
        Command::new("/usr/bin/sips")
            .args(["-s", "format", "png"])
            .arg(&icon)
            .arg("--out")
            .arg(work.join("validated-icon.png")),
    )?;
    Ok(())
}

fn png_icon(bytes: &[u8], work: &Path) -> Result<Vec<u8>, String> {
    if bytes.len() < 24
        || &bytes[..8] != b"\x89PNG\r\n\x1a\n"
        || &bytes[12..16] != b"IHDR"
        || u32::from_be_bytes(bytes[16..20].try_into().unwrap()) != 1024
        || u32::from_be_bytes(bytes[20..24].try_into().unwrap()) != 1024
    {
        return Err("PNG app icon must be 1024x1024 pixels".into());
    }
    let input = work.join("icon.png");
    write_new(&input, bytes)?;
    // ICNS representation tags distinguish 1x and 2x images at the same pixel size.
    const REPRESENTATIONS: &[(&str, &[&[u8; 4]])] = &[
        ("16", &[b"icp4"]),
        ("32", &[b"icp5", b"ic11"]),
        ("64", &[b"ic12"]),
        ("128", &[b"ic07"]),
        ("256", &[b"ic08", b"ic13"]),
        ("512", &[b"ic09", b"ic14"]),
        ("1024", &[b"ic10"]),
    ];
    let mut icon = b"icns\0\0\0\0".to_vec();
    for &(pixels, tags) in REPRESENTATIONS {
        let resized;
        let png = if pixels == "1024" {
            bytes
        } else {
            let output = work.join(format!("icon-{pixels}.png"));
            command(
                Command::new("/usr/bin/sips")
                    .args(["-z", pixels, pixels])
                    .arg(&input)
                    .arg("--out")
                    .arg(&output),
            )?;
            resized = fs::read(output).map_err(|error| error.to_string())?;
            &resized
        };
        let length = u32::try_from(png.len() + 8).map_err(|_| "ICNS image is too large")?;
        for tag in tags {
            icon.extend_from_slice(*tag);
            icon.extend_from_slice(&length.to_be_bytes());
            icon.extend_from_slice(png);
        }
    }
    let length = u32::try_from(icon.len()).map_err(|_| "ICNS resource is too large")?;
    icon[4..8].copy_from_slice(&length.to_be_bytes());
    Ok(icon)
}
