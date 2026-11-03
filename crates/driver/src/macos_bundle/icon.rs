use super::{command, snapshot::write_new};
use std::path::Path;
use std::process::Command;

pub fn generate(bytes: &[u8], source: &Path, work: &Path, resources: &Path) -> Result<(), String> {
    if source.extension().is_some_and(|s| s == "icns") {
        if bytes.len() < 8
            || &bytes[..4] != b"icns"
            || u32::from_be_bytes(bytes[4..8].try_into().unwrap()) as usize != bytes.len()
        {
            return Err("invalid ICNS header or length".into());
        }
        write_new(&resources.join("Icon.icns"), bytes)?;
        // Decode an image, rather than accepting a container's format metadata alone.
        command(
            Command::new("/usr/bin/sips")
                .args(["-s", "format", "png"])
                .arg(resources.join("Icon.icns"))
                .arg("--out")
                .arg(work.join("validated-icon.png")),
        )?;
        return Ok(());
    }
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
    let iconset = work.join("App.iconset");
    std::fs::create_dir(&iconset).map_err(|e| e.to_string())?;
    for size in [16, 32, 128, 256, 512] {
        for scale in [1, 2] {
            let suffix = if scale == 2 { "@2x" } else { "" };
            let pixels = (size * scale).to_string();
            command(
                Command::new("/usr/bin/sips")
                    .args(["-z", &pixels, &pixels])
                    .arg(&input)
                    .arg("--out")
                    .arg(iconset.join(format!("icon_{size}x{size}{suffix}.png"))),
            )?;
        }
    }
    command(
        Command::new("/usr/bin/iconutil")
            .args(["-c", "icns"])
            .arg(&iconset)
            .arg("-o")
            .arg(resources.join("Icon.icns")),
    )?;
    Ok(())
}
