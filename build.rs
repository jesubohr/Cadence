//! Embeds the app icon into the Windows executable.

use std::fs;
use std::path::PathBuf;

include!("src/icon.rs");

const SIZES: [u32; 5] = [16, 24, 32, 48, 64];

/// Builds an .ico file with one uncompressed 32-bit image per size.
fn ico() -> Vec<u8> {
    let images: Vec<Vec<u8>> = SIZES
        .iter()
        .map(|&size| {
            let rgba = rgba(size);
            let mut image = Vec::new();
            // BITMAPINFOHEADER. The height counts the image and the mask.
            image.extend_from_slice(&40u32.to_le_bytes());
            image.extend_from_slice(&size.to_le_bytes());
            image.extend_from_slice(&(size * 2).to_le_bytes());
            image.extend_from_slice(&1u16.to_le_bytes());
            image.extend_from_slice(&32u16.to_le_bytes());
            image.extend_from_slice(&[0u8; 24]);
            // Pixels are stored bottom-up as BGRA.
            for row in (0..size).rev() {
                for col in 0..size {
                    let i = ((row * size + col) * 4) as usize;
                    image.extend_from_slice(&[rgba[i + 2], rgba[i + 1], rgba[i], rgba[i + 3]]);
                }
            }
            // The AND mask is unused with an alpha channel, but must exist.
            image.extend(std::iter::repeat_n(
                0u8,
                (size.div_ceil(32) * 4 * size) as usize,
            ));
            image
        })
        .collect();

    let mut file = Vec::new();
    file.extend_from_slice(&[0, 0, 1, 0]);
    file.extend_from_slice(&(SIZES.len() as u16).to_le_bytes());
    let mut offset = 6 + 16 * SIZES.len() as u32;
    for (&size, image) in SIZES.iter().zip(&images) {
        file.extend_from_slice(&[size as u8, size as u8, 0, 0]);
        file.extend_from_slice(&1u16.to_le_bytes());
        file.extend_from_slice(&32u16.to_le_bytes());
        file.extend_from_slice(&(image.len() as u32).to_le_bytes());
        file.extend_from_slice(&offset.to_le_bytes());
        offset += image.len() as u32;
    }
    for image in images {
        file.extend(image);
    }
    file
}

fn main() {
    println!("cargo:rerun-if-changed=src/icon.rs");
    println!("cargo:rerun-if-changed=build.rs");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return;
    }
    let out = PathBuf::from(std::env::var("OUT_DIR").unwrap());
    let ico_path = out.join("cadence.ico");
    fs::write(&ico_path, ico()).unwrap();

    // gpui loads the window icon from resource id 1.
    let rc_path = out.join("cadence.rc");
    let ico_text = ico_path.to_string_lossy().replace('\\', "/");
    fs::write(&rc_path, format!("1 ICON \"{ico_text}\"\n")).unwrap();
    embed_resource::compile(&rc_path, embed_resource::NONE)
        .manifest_optional()
        .unwrap();
}
