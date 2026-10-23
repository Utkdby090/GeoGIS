//! Builds the web app (apps/geoflux-web/dist, made by `npm run build`) into the server binary, so
//! one file serves both. Without a dist folder the server still builds; its web pages then say
//! how to build the app.

use std::fmt::Write as _;
use std::path::{Path, PathBuf};

fn content_type(path: &Path) -> Option<&'static str> {
    Some(match path.extension()?.to_str()? {
        "html" => "text/html; charset=utf-8",
        "js" | "mjs" => "text/javascript; charset=utf-8",
        "css" => "text/css; charset=utf-8",
        "json" => "application/json",
        "webmanifest" => "application/manifest+json",
        "svg" => "image/svg+xml",
        "png" => "image/png",
        "ico" => "image/x-icon",
        "woff2" => "font/woff2",
        "txt" => "text/plain; charset=utf-8",
        // Source maps and anything unexpected stay out of the binary.
        _ => return None,
    })
}

fn walk(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    for e in entries.flatten() {
        let p = e.path();
        if p.is_dir() {
            walk(&p, out);
        } else {
            out.push(p);
        }
    }
}

fn main() {
    let manifest = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR"));
    let dist = manifest.join("../geoflux-web/dist");
    // A path that doesn't exist would make cargo run this on every build, so until dist exists
    // watch the app's folder, which changes when `npm run build` makes it.
    let watch = if dist.is_dir() { dist.clone() } else { manifest.join("../geoflux-web") };
    println!("cargo:rerun-if-changed={}", watch.display());
    let mut files = Vec::new();
    walk(&dist, &mut files);
    files.sort();
    let mut code = String::from(
        "/// The built web app: (path from the site root, content type, bytes).\npub static FILES: &[(&str, &str, &[u8])] = &[\n",
    );
    for f in &files {
        let Some(kind) = content_type(f) else { continue };
        let Ok(rel) = f.strip_prefix(&dist) else { continue };
        let rel = rel.components().map(|c| c.as_os_str().to_string_lossy()).collect::<Vec<_>>().join("/");
        let abs = f.canonicalize().unwrap_or_else(|_| f.clone());
        let _ = writeln!(code, "    ({:?}, {:?}, include_bytes!({:?})),", format!("/{rel}"), kind, abs.display().to_string());
    }
    code.push_str("];\n");
    let out = PathBuf::from(std::env::var("OUT_DIR").expect("OUT_DIR")).join("web_files.rs");
    std::fs::write(out, code).expect("writing web_files.rs");
}
