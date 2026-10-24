//! Build glue: compile the Windows resource script (tray icon) into the
//! binary. Pure `std`, only touched on Windows targets; no crates.

use std::env;
use std::path::PathBuf;
use std::process::Command;

fn main() {
    if !cfg!(target_os = "windows") {
        return;
    }
    println!("cargo:rerun-if-changed=assets/icon.rc");
    println!("cargo:rerun-if-changed=assets/icon.ico");
    println!("cargo:rerun-if-changed=assets/app.manifest");
    println!("cargo:rerun-if-env-changed=WINDRES");
    let manifest = PathBuf::from(env::var("CARGO_MANIFEST_DIR").unwrap());
    let script = manifest.join("assets/icon.rc");
    let output = PathBuf::from(env::var("OUT_DIR").unwrap()).join("icon.coff.o");
    // `WINDRES` lets a build point at a resource compiler that is not named
    // `windres` and is not first on PATH (CI installs a prefixed MinGW one).
    let windres = env::var_os("WINDRES").unwrap_or_else(|| "windres".into());
    let baked = Command::new(&windres)
        .arg("-O")
        .arg("coff")
        .arg("--input")
        .arg(&script)
        .arg("--output")
        .arg(&output)
        .status();
    match baked {
        Ok(status) if status.success() => {
            println!("cargo:rustc-link-arg-bins={}", output.display());
        }
        Ok(status) => {
            notice(&format!("windres exited abnormally: {status}"));
        }
        Err(error) => {
            notice(&format!(
                "windres unavailable ({error}); exe keeps the default icon"
            ));
        }
    }
}

/// Build-setup problems are printed, never fatal: the icon is polish.
fn notice(message: &str) {
    eprintln!("build icon resource: {message}");
}
