//! `geoflux-dirp --sdk <folder>`: GeoGIS's thermal helper. It loads DJI's Thermal SDK from the
//! folder, answers the [`geoflux_thermal::protocol`] over stdin and stdout, and exits when stdin
//! closes or GeoGIS says so. Diagnostics go to stderr; stdout carries protocol messages only.
//!
//! GeoGIS runs it in its own process so that a crash inside DJI's library, on a damaged photo,
//! ends this program and not the app. GeoGIS then starts a fresh one for the next photo.

#[cfg(not(target_arch = "wasm32"))]
mod measure;
#[cfg(not(target_arch = "wasm32"))]
mod sdk;

#[cfg(target_arch = "wasm32")]
fn main() {}

#[cfg(not(target_arch = "wasm32"))]
fn main() -> std::process::ExitCode {
    use std::io::{BufReader, BufWriter};

    use geoflux_thermal::protocol::{self, MAX_JPEG, Request, Response};

    let mut args = std::env::args().skip(1);
    let dir = match (args.next().as_deref(), args.next()) {
        (Some("--sdk"), Some(dir)) => std::path::PathBuf::from(dir),
        _ => {
            eprintln!("usage: geoflux-dirp --sdk <folder with libdirp>");
            return std::process::ExitCode::from(2);
        }
    };
    let sdk = match sdk::Sdk::load(&dir) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("geoflux-dirp: {e}");
            return std::process::ExitCode::from(3);
        }
    };
    let mut input = BufReader::new(std::io::stdin().lock());
    let mut output = BufWriter::new(std::io::stdout().lock());
    let mut payload = Vec::new();
    loop {
        let msg = match protocol::read_message::<_, Request>(&mut input, MAX_JPEG) {
            Ok(Some(m)) => m,
            Ok(None) => return std::process::ExitCode::SUCCESS,
            Err(e) => {
                eprintln!("geoflux-dirp: {e}");
                return std::process::ExitCode::from(4);
            }
        };
        let sent = match msg {
            (Request::Hello { .. }, _) => {
                let hello = Response::Hello {
                    protocol: protocol::PROTOCOL_VERSION,
                    helper: env!("CARGO_PKG_VERSION").to_string(),
                    sdk: "DJI Thermal SDK".to_string(),
                };
                protocol::write_message(&mut output, &hello, &[])
            }
            (Request::Measure { id, overrides }, jpeg) => match measure::measure(&sdk, jpeg, &overrides) {
                Ok(m) => {
                    protocol::temps_to_bytes(&m.temps, &mut payload);
                    let head = Response::Measured {
                        id,
                        width: m.width,
                        height: m.height,
                        applied: m.applied,
                        refused: m.refused,
                        camera: String::new(),
                    };
                    protocol::write_message(&mut output, &head, &payload)
                }
                Err(f) => protocol::write_message(&mut output, &Response::Failed { id, code: f.code, message: f.message }, &[]),
            },
            (Request::Shutdown, _) => return std::process::ExitCode::SUCCESS,
        };
        if let Err(e) = sent {
            eprintln!("geoflux-dirp: {e}");
            return std::process::ExitCode::from(5);
        }
    }
}
