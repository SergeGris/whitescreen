//! Embeds the side-by-side manifest into the executable.
//!
//! See `whitescreen.manifest`: it is what turns on the themed common
//! controls, and an app without it looks two decades old whatever it paints.
//!
//! The manifest is handed to the linker rather than compiled into a resource
//! script, so no `rc.exe` and no extra build dependency are needed. MSVC's
//! linker takes it directly; the GNU toolchain only accepts a compiled
//! resource, so there it goes through `windres` when that is available.

use std::path::PathBuf;

fn main() {
    let manifest_dir = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").unwrap());
    let manifest = manifest_dir.join("whitescreen.manifest");
    println!("cargo::rerun-if-changed={}", manifest.display());

    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return;
    }

    match std::env::var("CARGO_CFG_TARGET_ENV").as_deref() {
        Ok("msvc") => {
            // Link the manifest in directly. `/MANIFESTUAC:NO` stops the
            // linker adding a second `trustInfo` block of its own, which
            // would collide with the one in the file.
            println!("cargo::rustc-link-arg-bins=/MANIFEST:EMBED");
            println!(
                "cargo::rustc-link-arg-bins=/MANIFESTINPUT:{}",
                manifest.display()
            );
            println!("cargo::rustc-link-arg-bins=/MANIFESTUAC:NO");
        }
        Ok("gnu") => embed_with_windres(&manifest),
        _ => {}
    }
}

/// The GNU route: a one-line resource script compiled by `windres`.
///
/// Resource type 24 (`RT_MANIFEST`) at id 1 is where Windows looks for an
/// executable's manifest.
fn embed_with_windres(manifest: &std::path::Path) {
    let out = PathBuf::from(std::env::var("OUT_DIR").unwrap());
    let script = out.join("manifest.rc");
    let object = out.join("manifest.o");

    let path = manifest.to_string_lossy().replace('\\', "\\\\");
    if std::fs::write(&script, format!("1 24 \"{path}\"\n")).is_err() {
        println!("cargo::warning=could not write {}", script.display());
        return;
    }

    let windres = std::env::var("WINDRES").unwrap_or_else(|_| "windres".to_string());
    match std::process::Command::new(&windres)
        .arg(&script)
        .arg("-O")
        .arg("coff")
        .arg("-o")
        .arg(&object)
        .status()
    {
        Ok(status) if status.success() => {
            println!("cargo::rustc-link-arg-bins={}", object.display());
        }
        _ => println!(
            "cargo::warning={windres} is unavailable, so the executable will \
             have no manifest: the window will fall back to the unthemed \
             common controls"
        ),
    }
}
