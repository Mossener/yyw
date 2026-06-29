use std::env;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::process::Command;

fn write_ico_from_png(source: &Path, target: &Path) -> io::Result<()> {
    let source_image = image::ImageReader::open(source)
        .map_err(io::Error::other)?
        .decode()
        .map_err(io::Error::other)?;

    let mut images = Vec::new();
    for size in [16u32, 32, 48, 256] {
        let image = source_image.resize(size, size, image::imageops::FilterType::Lanczos3);
        let mut png = Vec::new();
        image
            .write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png)
            .map_err(io::Error::other)?;
        images.push((size, png));
    }

    let image_offset = 6u32 + images.len() as u32 * 16u32;
    let mut offset = image_offset;
    let mut ico = Vec::new();
    ico.extend_from_slice(&0u16.to_le_bytes());
    ico.extend_from_slice(&1u16.to_le_bytes());
    ico.extend_from_slice(&(images.len() as u16).to_le_bytes());
    for (size, png) in &images {
        ico.push(if *size == 256 { 0 } else { *size as u8 });
        ico.push(if *size == 256 { 0 } else { *size as u8 });
        ico.push(0);
        ico.push(0);
        ico.extend_from_slice(&1u16.to_le_bytes());
        ico.extend_from_slice(&32u16.to_le_bytes());
        ico.extend_from_slice(&(png.len() as u32).to_le_bytes());
        ico.extend_from_slice(&offset.to_le_bytes());
        offset += png.len() as u32;
    }
    for (_, png) in images {
        ico.extend_from_slice(&png);
    }

    fs::write(target, ico)
}

fn find_windres() -> Option<PathBuf> {
    let path = env::var_os("PATH")?;
    env::split_paths(&path)
        .map(|dir| dir.join("windres.exe"))
        .find(|candidate| candidate.exists())
}

fn main() {
    if env::var("CARGO_CFG_WINDOWS").is_err() {
        return;
    }

    println!("cargo:rerun-if-changed=radian.png");
    println!("cargo:rerun-if-changed=build.rs");

    let out_dir = PathBuf::from(env::var_os("OUT_DIR").expect("OUT_DIR is set"));
    let icon = out_dir.join("yyw.ico");
    let rc = out_dir.join("yyw.rc");
    let res = out_dir.join("yyw.res");

    if let Err(err) = write_ico_from_png(Path::new("radian.png"), &icon) {
        println!("cargo:warning=failed to generate yyw icon: {err}");
        return;
    }

    let icon_path = icon.to_string_lossy().replace('\\', "/");
    if let Err(err) = fs::write(&rc, format!("1 ICON \"{icon_path}\"\n")) {
        println!("cargo:warning=failed to write yyw resource script: {err}");
        return;
    }

    let Some(windres) = find_windres() else {
        println!("cargo:warning=windres.exe not found; yyw.exe file icon was not embedded");
        return;
    };

    let status = Command::new(windres)
        .args(["-i"])
        .arg(&rc)
        .args(["-O", "res", "-o"])
        .arg(&res)
        .status();

    match status {
        Ok(status) if status.success() => {
            println!("cargo:rustc-link-arg-bin=yyw={}", res.display());
        }
        Ok(status) => {
            println!("cargo:warning=windres failed with status {status}");
        }
        Err(err) => {
            println!("cargo:warning=failed to run windres: {err}");
        }
    }
}
