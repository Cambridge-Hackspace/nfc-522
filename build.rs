use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::Command;

use flate2::write::GzEncoder;
use flate2::Compression;

fn main() {
    build_web_assets();

    // It is necessary to call this function once. Otherwise, some patches to the
    // runtime implemented by esp-idf-sys might not link properly.
    embuild::espidf::sysenv::output();
}

/// Build the captive-portal assets (Tailwind + DaisyUI), then minify + gzip them
/// into `OUT_DIR` so the firmware can `include_bytes!` the pre-compressed bytes.
/// The pipeline (purge → minify → gzip) keeps the embedded footprint tiny.
fn build_web_assets() {
    let manifest = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").unwrap());
    let web = manifest.join("web");
    let out = PathBuf::from(std::env::var("OUT_DIR").unwrap());

    // Re-run only when the web sources change (keeps incremental builds fast).
    for rel in [
        "src",
        "input.css",
        "tailwind.config.js",
        "package.json",
        "package-lock.json",
    ] {
        println!("cargo:rerun-if-changed=web/{rel}");
    }

    // Install JS deps on first build (requires `npm` + network once).
    if !web.join("node_modules").is_dir() {
        run(
            Command::new("npm").arg("ci").current_dir(&web),
            "npm ci (install web asset dependencies)",
        );
    }

    // Tailwind: purge against web/src/*.html and minify -> OUT_DIR/app.css.
    let css = out.join("app.css");
    let tailwind = web.join("node_modules/.bin/tailwindcss");
    run(
        Command::new(&tailwind)
            .current_dir(&web)
            .args(["-c", "tailwind.config.js", "-i", "input.css"])
            .arg("-o")
            .arg(&css)
            .arg("--minify"),
        "tailwindcss build",
    );

    // Gzip the CSS and both HTML pages; the device serves these with
    // `Content-Encoding: gzip`.
    gzip_to(&css, &out.join("app.css.gz"));
    gzip_to(&web.join("src/portal.html"), &out.join("portal.html.gz"));
    gzip_to(&web.join("src/saved.html"), &out.join("saved.html.gz"));
    gzip_to(&web.join("src/status.html"), &out.join("status.html.gz"));

    // Copy the latin-subset B612 Mono woff2 files (already compressed; served as-is).
    let fonts = web.join("node_modules/@fontsource/b612-mono/files");
    copy(
        &fonts.join("b612-mono-latin-400-normal.woff2"),
        &out.join("font-400.woff2"),
    );
    copy(
        &fonts.join("b612-mono-latin-700-normal.woff2"),
        &out.join("font-700.woff2"),
    );
}

/// Run a command, panicking with a clear message if it is missing or fails.
fn run(cmd: &mut Command, what: &str) {
    let status = cmd.status().unwrap_or_else(|e| {
        panic!("failed to run {what}: {e}. Is Node.js / npm installed and on PATH?")
    });
    assert!(status.success(), "{what} exited with {status}");
}

/// Gzip `src` into `dst` at maximum compression.
fn gzip_to(src: &Path, dst: &Path) {
    let data = fs::read(src).unwrap_or_else(|e| panic!("read {}: {e}", src.display()));
    let mut encoder = GzEncoder::new(Vec::new(), Compression::best());
    encoder.write_all(&data).unwrap();
    let compressed = encoder.finish().unwrap();
    fs::write(dst, compressed).unwrap_or_else(|e| panic!("write {}: {e}", dst.display()));
}

fn copy(src: &Path, dst: &Path) {
    fs::copy(src, dst)
        .unwrap_or_else(|e| panic!("copy {} -> {}: {e}", src.display(), dst.display()));
}
