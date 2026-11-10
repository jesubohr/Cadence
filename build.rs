//! Embeds the app icon into the Windows executable.

use std::fs;
use std::path::PathBuf;

fn main() {
    println!("cargo:rerun-if-changed=assets/logo.ico");
    println!("cargo:rerun-if-changed=build.rs");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return;
    }
    let manifest = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").unwrap());
    let ico = manifest.join("assets").join("logo.ico");

    // gpui loads the window icon and the tray loads its icon from resource id 1.
    let rc_path = PathBuf::from(std::env::var("OUT_DIR").unwrap()).join("cadence.rc");
    let ico_text = ico.to_string_lossy().replace('\\', "/");
    fs::write(&rc_path, format!("1 ICON \"{ico_text}\"\n")).unwrap();
    embed_resource::compile(&rc_path, embed_resource::NONE)
        .manifest_optional()
        .unwrap();
}
