//! Copies the Natural Earth sample data (public domain) that GeoGIS embeds for its basemap
//! outline and World Atlas sample into `OUT_DIR`. The files are kept in this repo
//! (`crates/render/data`), so a build needs no network.
//!
//! Set `RUSTGIS_DATA_DIR` to build with the files from another folder.

use std::path::{Path, PathBuf};

const FILES: [&str; 5] = ["countries.geojson", "countries_chn.geojson", "cities.geojson", "rivers.geojson", "lakes.geojson"];

fn valid(p: &Path) -> bool {
    std::fs::read(p).is_ok_and(|b| b.len() > 2 && b.first() == Some(&b'{'))
}

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=data");
    println!("cargo:rerun-if-env-changed=RUSTGIS_DATA_DIR");
    let out = PathBuf::from(std::env::var_os("OUT_DIR").unwrap_or_default());
    let manifest = PathBuf::from(std::env::var_os("CARGO_MANIFEST_DIR").unwrap_or_default());
    let dir = std::env::var_os("RUSTGIS_DATA_DIR").map_or_else(|| manifest.join("data"), PathBuf::from);
    for f in FILES {
        let (src, dest) = (dir.join(f), out.join(f));
        if std::fs::copy(&src, &dest).is_err() || !valid(&dest) {
            panic!("{} is missing or not GeoJSON", src.display());
        }
    }
}
