//! One measurement, with GeoFlux's hard-won rules: each override is clamped to the range the SDK
//! reports for the photo; when the SDK refuses them together, each is tried alone, and the ones
//! still refused keep the camera's value and are reported.

use geoflux_thermal::model::check_size;
use geoflux_thermal::{SceneOverrides, SceneParams, SceneSetting};

use crate::sdk::{Params, Photo, Range, Sdk};

pub struct Measured {
    pub width: u32,
    pub height: u32,
    pub temps: Vec<f32>,
    pub applied: SceneParams,
    pub refused: Vec<SceneSetting>,
}

/// Why a photo could not be measured: DJI's return code, or 0 with the helper's own reason.
pub struct Failure {
    pub code: i32,
    pub message: String,
}

impl From<i32> for Failure {
    fn from(code: i32) -> Self {
        Self { code, message: String::new() }
    }
}

fn field(p: &mut Params, s: SceneSetting) -> &mut f32 {
    match s {
        SceneSetting::Distance => &mut p.distance,
        SceneSetting::Humidity => &mut p.humidity,
        SceneSetting::Emissivity => &mut p.emissivity,
        SceneSetting::ReflectedTemp => &mut p.reflection,
        SceneSetting::AmbientTemp => &mut p.ambient_temp,
    }
}

fn limits(r: &Range, s: SceneSetting) -> (f32, f32) {
    match s {
        SceneSetting::Distance => r.distance,
        SceneSetting::Humidity => r.humidity,
        SceneSetting::Emissivity => r.emissivity,
        SceneSetting::ReflectedTemp => r.reflection,
        SceneSetting::AmbientTemp => r.ambient_temp,
    }
}

fn scene(p: &Params) -> SceneParams {
    SceneParams {
        emissivity: f64::from(p.emissivity),
        reflected_temp_c: f64::from(p.reflection),
        ambient_temp_c: f64::from(p.ambient_temp),
        distance_m: f64::from(p.distance),
        humidity_pct: f64::from(p.humidity),
    }
}

pub fn measure(sdk: &Sdk, jpeg: Vec<u8>, overrides: &SceneOverrides) -> Result<Measured, Failure> {
    if jpeg.len() < 4 || !jpeg.starts_with(&[0xFF, 0xD8]) {
        // Not a JPEG at all: DJI's "invalid R-JPEG" code, without handing it to the SDK.
        return Err(Failure::from(-7));
    }
    let photo = sdk.open(jpeg)?;
    let (w, h) = photo.resolution()?;
    let (w, h) = (u32::try_from(w).unwrap_or(0), u32::try_from(h).unwrap_or(0));
    check_size(w, h).map_err(|e| Failure { code: 0, message: e.to_string() })?;
    let mut temps = vec![0f32; w as usize * h as usize];
    let file = photo.params()?;

    let wanted: Vec<(SceneSetting, f64)> = overrides.iter().collect();
    let (applied, refused) = if wanted.is_empty() {
        photo.set_params(&file)?;
        photo.measure(&mut temps)?;
        (file, Vec::new())
    } else {
        settle(&photo, file, &wanted, &mut temps)?
    };
    Ok(Measured { width: w, height: h, temps, applied: scene(&applied), refused })
}

/// Applies the overrides as far as the SDK accepts them; `temps` holds the measurement for the
/// returned parameters.
fn settle(
    photo: &Photo<'_>,
    file: Params,
    wanted: &[(SceneSetting, f64)],
    temps: &mut [f32],
) -> Result<(Params, Vec<SceneSetting>), Failure> {
    let range = photo.range()?;
    let mut refused = Vec::new();
    let mut edits = Vec::new();
    for &(s, v) in wanted {
        let (lo, hi) = limits(&range, s);
        // A setting the camera can't change (a range of one value, like the H20T's ambient
        // temperature) is refused rather than silently ignored. This check also keeps `clamp`
        // below from panicking on a NaN or inverted range.
        if lo.is_nan() || hi.is_nan() || hi <= lo {
            refused.push(s);
            continue;
        }
        edits.push((s, (v as f32).clamp(lo, hi)));
    }
    let with = |base: Params, e: &[(SceneSetting, f32)]| {
        let mut p = base;
        for &(s, v) in e {
            *field(&mut p, s) = v;
        }
        p
    };
    let try_measure = |p: &Params, temps: &mut [f32]| photo.set_params(p).is_ok() && photo.measure(temps).is_ok();

    let all = with(file, &edits);
    if try_measure(&all, temps) {
        return Ok((all, refused));
    }
    // Together they were refused (an M4T can't measure at emissivity 0.1, an XT S won't take a
    // 1 m distance): keep each one the SDK accepts on its own.
    let mut applied = file;
    for &(s, v) in &edits {
        let candidate = with(applied, &[(s, v)]);
        if try_measure(&candidate, temps) {
            applied = candidate;
        } else {
            refused.push(s);
        }
    }
    // Leave the handle and `temps` consistent with what is reported.
    photo.set_params(&applied)?;
    photo.measure(temps)?;
    Ok((applied, refused))
}
