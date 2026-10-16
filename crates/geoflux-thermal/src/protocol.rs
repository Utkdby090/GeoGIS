//! The pipe protocol between GeoGIS and the `geoflux-dirp` helper, the one program that loads
//! DJI's Thermal SDK.
//!
//! GeoGIS starts the helper once, with DJI's library folder, and keeps it while thermal photos
//! are being read. Both sides then exchange messages over the helper's stdin and stdout; the
//! helper writes nothing else there (its own diagnostics go to stderr).
//!
//! A message is:
//!
//! ```text
//! magic "GFD1" | header length: u32 LE | header: JSON | payload length: u32 LE | payload bytes
//! ```
//!
//! 1. GeoGIS sends [`Request::Hello`]; the helper answers [`Response::Hello`]. A different
//!    [`PROTOCOL_VERSION`] stops the helper from being used.
//! 2. For each photo GeoGIS sends [`Request::Measure`] with the JPEG as payload; the helper
//!    answers [`Response::Measured`] with the temperatures as payload (f32 LE, row by row), or
//!    [`Response::Failed`] with no payload. One photo at a time per helper; `id` matches them.
//! 3. [`Request::Shutdown`], or closing stdin, ends the helper.
//!
//! Both sides check every length against the limits below before allocating, and treat a
//! message that breaks the format as the end of the conversation.

use std::io::{Read, Write};

use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};

use crate::error::ThermalError;
use crate::model::{MAX_PIXELS, SceneOverrides, SceneParams, SceneSetting};

/// Bumped whenever a message changes shape.
pub const PROTOCOL_VERSION: u32 = 1;

pub const MAGIC: [u8; 4] = *b"GFD1";

/// The largest JSON header either side accepts.
pub const MAX_HEADER: u32 = 64 * 1024;

/// The largest photo GeoGIS sends (DJI R-JPEGs are 1–3 MB).
pub const MAX_JPEG: u32 = 64 << 20;

/// The largest temperature payload the helper may answer with.
pub const MAX_TEMPS: u32 = (MAX_PIXELS * 4) as u32;

/// GeoGIS to helper.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum Request {
    Hello {
        protocol: u32,
    },
    /// Payload: the JPEG.
    Measure {
        id: u64,
        overrides: SceneOverrides,
    },
    Shutdown,
}

/// Helper to GeoGIS.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum Response {
    Hello {
        protocol: u32,
        /// The helper's own version, for the About box and bug reports.
        helper: String,
        /// DJI's SDK version as the library reports it.
        sdk: String,
    },
    /// Payload: `width × height` f32 LE temperatures.
    Measured { id: u64, width: u32, height: u32, applied: SceneParams, refused: Vec<SceneSetting>, camera: String },
    /// No payload. `code` is DJI's return code; 0 means the helper's own check failed (an empty
    /// or oversized file) and `message` says why.
    Failed { id: u64, code: i32, message: String },
}

impl Response {
    /// A failure as the error the caller sees.
    pub fn failure(code: i32, message: &str) -> ThermalError {
        match code {
            0 => ThermalError::Damaged(message.to_string()),
            -7..=-4 => ThermalError::NotRadiometric,
            -64 => ThermalError::Unsupported(ThermalError::dji_code_meaning(code).into()),
            _ => ThermalError::Sdk { code, meaning: ThermalError::dji_code_meaning(code).into() },
        }
    }
}

/// Writes one message.
pub fn write_message<W: Write>(w: &mut W, header: &impl Serialize, payload: &[u8]) -> Result<(), ThermalError> {
    let head = serde_json::to_vec(header).map_err(|e| ThermalError::Protocol(e.to_string()))?;
    let head_len = u32::try_from(head.len()).ok().filter(|n| *n <= MAX_HEADER);
    let body_len = u32::try_from(payload.len()).ok();
    let (Some(head_len), Some(body_len)) = (head_len, body_len) else {
        return Err(ThermalError::Protocol("message too large".into()));
    };
    let io = |e: std::io::Error| ThermalError::Protocol(e.to_string());
    w.write_all(&MAGIC).map_err(io)?;
    w.write_all(&head_len.to_le_bytes()).map_err(io)?;
    w.write_all(&head).map_err(io)?;
    w.write_all(&body_len.to_le_bytes()).map_err(io)?;
    w.write_all(payload).map_err(io)?;
    w.flush().map_err(io)
}

/// Reads one message whose payload may be at most `max_payload` bytes. A closed pipe at a message
/// boundary is `Ok(None)`; anything else that isn't a whole, well-formed message is an error.
pub fn read_message<R: Read, H: DeserializeOwned>(r: &mut R, max_payload: u32) -> Result<Option<(H, Vec<u8>)>, ThermalError> {
    let io = |e: std::io::Error| ThermalError::Protocol(e.to_string());
    let mut magic = [0u8; 4];
    match r.read_exact(&mut magic) {
        Ok(()) => {}
        Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => return Ok(None),
        Err(e) => return Err(io(e)),
    }
    if magic != MAGIC {
        return Err(ThermalError::Protocol("not a GeoFlux helper message".into()));
    }
    let head_len = read_u32(r).map_err(io)?;
    if head_len > MAX_HEADER {
        return Err(ThermalError::Protocol("header too large".into()));
    }
    let head = read_exact_vec(r, head_len).map_err(io)?;
    let header: H = serde_json::from_slice(&head).map_err(|e| ThermalError::Protocol(e.to_string()))?;
    let body_len = read_u32(r).map_err(io)?;
    if body_len > max_payload {
        return Err(ThermalError::Protocol("payload too large".into()));
    }
    let payload = read_exact_vec(r, body_len).map_err(io)?;
    Ok(Some((header, payload)))
}

/// Temperatures as payload bytes (f32 LE).
pub fn temps_to_bytes(temps: &[f32], out: &mut Vec<u8>) {
    out.clear();
    out.reserve(temps.len() * 4);
    for t in temps {
        out.extend_from_slice(&t.to_le_bytes());
    }
}

/// Payload bytes as temperatures; `None` unless exactly `count` values.
pub fn temps_from_bytes(bytes: &[u8], count: u64) -> Option<Vec<f32>> {
    if bytes.len() as u64 != count.checked_mul(4)? {
        return None;
    }
    Some(bytes.chunks_exact(4).map(|c| f32::from_le_bytes([c[0], c[1], c[2], c[3]])).collect())
}

fn read_u32<R: Read>(r: &mut R) -> std::io::Result<u32> {
    let mut b = [0u8; 4];
    r.read_exact(&mut b)?;
    Ok(u32::from_le_bytes(b))
}

/// Reads `len` bytes, growing the buffer as data arrives rather than trusting `len` up front.
fn read_exact_vec<R: Read>(r: &mut R, len: u32) -> std::io::Result<Vec<u8>> {
    let mut v = Vec::new();
    let got = r.take(u64::from(len)).read_to_end(&mut v)?;
    if got as u64 != u64::from(len) {
        return Err(std::io::ErrorKind::UnexpectedEof.into());
    }
    Ok(v)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_message_round_trips_and_a_forged_length_is_refused() {
        let mut wire = Vec::new();
        let req = Request::Measure { id: 7, overrides: SceneOverrides { emissivity: Some(0.95), ..Default::default() } };
        write_message(&mut wire, &req, b"\xFF\xD8jpeg").unwrap();
        let (back, payload): (Request, Vec<u8>) = read_message(&mut wire.as_slice(), MAX_JPEG).unwrap().unwrap();
        assert_eq!(back, req);
        assert_eq!(payload, b"\xFF\xD8jpeg");
        assert!(read_message::<_, Request>(&mut [].as_slice(), MAX_JPEG).unwrap().is_none());

        // A payload length past the limit is refused before anything is read or allocated.
        let mut forged = wire.clone();
        let at = 8 + u32::from_le_bytes([wire[4], wire[5], wire[6], wire[7]]) as usize;
        forged[at..at + 4].copy_from_slice(&u32::MAX.to_le_bytes());
        assert!(matches!(read_message::<_, Request>(&mut forged.as_slice(), MAX_JPEG), Err(ThermalError::Protocol(_))));
        // So is a truncated message.
        assert!(read_message::<_, Request>(&mut &wire[..wire.len() - 1], MAX_JPEG).is_err());
    }

    #[test]
    fn temperatures_survive_the_payload_and_a_wrong_count_does_not() {
        let mut bytes = Vec::new();
        temps_to_bytes(&[21.5, f32::NAN, -3.0], &mut bytes);
        let back = temps_from_bytes(&bytes, 3).unwrap();
        assert_eq!(back[0], 21.5);
        assert!(back[1].is_nan());
        assert!(temps_from_bytes(&bytes, 4).is_none());
        assert!(temps_from_bytes(&bytes, u64::MAX).is_none());
    }
}
