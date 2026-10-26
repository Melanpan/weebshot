use std::env;
use std::fmt::Write as _;
use std::path::PathBuf;

fn main() {
    let root = PathBuf::from(env::var("CARGO_MANIFEST_DIR").unwrap());
    let out_dir = PathBuf::from(env::var("OUT_DIR").unwrap());

    let windows = env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows");

    // Windows: link the vendored libmpv (scripts/fetch-libmpv.ps1) and put the DLL next to
    // the built exe so `cargo run` works. Linux/macOS link the system libmpv.
    if windows {
        let mpv_dir = root.join("vendor/mpv");
        println!("cargo:rerun-if-changed=vendor/mpv");
        println!("cargo:rustc-link-search=native={}", mpv_dir.display());
        // OUT_DIR is target/<profile>/build/<pkg>-<hash>/out.
        if let Some(profile_dir) = out_dir.ancestors().nth(3) {
            let dll = mpv_dir.join("libmpv-2.dll");
            if dll.exists() {
                let _ = std::fs::copy(&dll, profile_dir.join("libmpv-2.dll"));
            }
        }
    }

    // The program icon (the mascot) for the .exe in Explorer/taskbar.
    println!("cargo:rerun-if-changed=assets/weebshot.rc");
    println!("cargo:rerun-if-changed=assets/icon.ico");
    if windows {
        embed_resource::compile(root.join("assets/weebshot.rc"), embed_resource::NONE)
            .manifest_optional()
            .unwrap();
    }

    // Embed the documentation screenshots (docs/images) for the in-app help.
    let images = root.join("docs/images");
    println!("cargo:rerun-if-changed=docs/images");
    let mut files: Vec<_> = std::fs::read_dir(&images)
        .map(|d| {
            d.filter_map(Result::ok)
                .map(|e| e.path())
                .filter(|p| {
                    matches!(
                        p.extension().and_then(|e| e.to_str()),
                        Some("png" | "jpg" | "jpeg")
                    )
                })
                .collect()
        })
        .unwrap_or_default();
    files.sort();
    let mut code = String::from("pub const DOC_IMAGES: &[(&str, &[u8])] = &[\n");
    for f in files {
        let name = f.file_name().unwrap().to_string_lossy();
        writeln!(
            code,
            "    ({name:?}, include_bytes!({:?})),",
            f.display().to_string()
        )
        .unwrap();
    }
    code += "];\n";
    std::fs::write(out_dir.join("doc_images.rs"), code).unwrap();
}
