//! A frame of temperatures and the settings it was measured with.

use serde::{Deserialize, Serialize};

use crate::error::ThermalError;

/// The most pixels a frame may have (4096×4096). Radiometric sensors top out near 1280×1024; the
/// limit stops a forged header from asking for gigabytes.
pub const MAX_PIXELS: u64 = 4096 * 4096;

/// Whose format a frame was read from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Vendor {
    Flir,
    Dji,
}

/// One scene setting a temperature depends on.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum SceneSetting {
    Emissivity,
    ReflectedTemp,
    AmbientTemp,
    Distance,
    Humidity,
}

impl std::fmt::Display for SceneSetting {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Emissivity => "emissivity",
            Self::ReflectedTemp => "reflected temperature",
            Self::AmbientTemp => "ambient temperature",
            Self::Distance => "distance",
            Self::Humidity => "humidity",
        })
    }
}

/// The scene settings a frame was actually measured with. Temperatures are °C, distance is metres,
/// humidity is percent (0–100), emissivity is 0–1.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct SceneParams {
    pub emissivity: f64,
    pub reflected_temp_c: f64,
    pub ambient_temp_c: f64,
    pub distance_m: f64,
    pub humidity_pct: f64,
}

impl SceneParams {
    pub fn get(&self, s: SceneSetting) -> f64 {
        match s {
            SceneSetting::Emissivity => self.emissivity,
            SceneSetting::ReflectedTemp => self.reflected_temp_c,
            SceneSetting::AmbientTemp => self.ambient_temp_c,
            SceneSetting::Distance => self.distance_m,
            SceneSetting::Humidity => self.humidity_pct,
        }
    }

    pub fn set(&mut self, s: SceneSetting, v: f64) {
        match s {
            SceneSetting::Emissivity => self.emissivity = v,
            SceneSetting::ReflectedTemp => self.reflected_temp_c = v,
            SceneSetting::AmbientTemp => self.ambient_temp_c = v,
            SceneSetting::Distance => self.distance_m = v,
            SceneSetting::Humidity => self.humidity_pct = v,
        }
    }
}

/// Scene values the inspector knows better than the camera did (the form's emissivity, ambient
/// temperature, humidity). `None` keeps the file's value; NaN and infinities count as `None`, so
/// a half-typed form field can't poison a measurement.
#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize, Deserialize)]
pub struct SceneOverrides {
    pub emissivity: Option<f64>,
    pub reflected_temp_c: Option<f64>,
    pub ambient_temp_c: Option<f64>,
    pub distance_m: Option<f64>,
    pub humidity_pct: Option<f64>,
}

impl SceneOverrides {
    /// The usable overrides, in the order DJI's SDK is asked to apply them.
    pub fn iter(&self) -> impl Iterator<Item = (SceneSetting, f64)> + '_ {
        [
            (SceneSetting::Distance, self.distance_m),
            (SceneSetting::Humidity, self.humidity_pct),
            (SceneSetting::Emissivity, self.emissivity),
            (SceneSetting::ReflectedTemp, self.reflected_temp_c),
            (SceneSetting::AmbientTemp, self.ambient_temp_c),
        ]
        .into_iter()
        .filter_map(|(s, v)| v.filter(|v| v.is_finite()).map(|v| (s, v)))
    }

    pub fn is_empty(&self) -> bool {
        self.iter().next().is_none()
    }

    /// `base` with these overrides applied.
    pub fn apply_to(&self, mut base: SceneParams) -> SceneParams {
        for (s, v) in self.iter() {
            base.set(s, v);
        }
        base
    }
}

/// FLIR's calibration constants (Planck curve and atmosphere model). DJI keeps its own secret, so
/// a DJI frame has none rather than a set of NaNs.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct PlanckConstants {
    pub r1: f64,
    pub r2: f64,
    pub b: f64,
    pub f: f64,
    pub o: f64,
    pub atm_alpha1: f64,
    pub atm_alpha2: f64,
    pub atm_beta1: f64,
    pub atm_beta2: f64,
    pub atm_x: f64,
    pub ir_window_temp_c: f64,
    pub ir_window_transmission: f64,
}

/// A pixel position, `x` across and `y` down from the top left.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Pixel {
    pub x: u32,
    pub y: u32,
}

/// An inclusive pixel rectangle. Corners may come in any order; it is clipped to the frame when
/// used.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Region {
    pub a: Pixel,
    pub b: Pixel,
}

/// Statistics over the valid pixels of a frame or region.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct ThermalStats {
    pub min_c: f32,
    pub max_c: f32,
    pub mean_c: f32,
    pub min_at: Pixel,
    /// The hottest pixel (the first one found, scanning row by row, when several tie).
    pub max_at: Pixel,
    pub valid_pixels: u64,
}

/// Temperatures in °C, row by row from the top left. Pixels the sensor could not convert are NaN.
///
/// The fields are private so every frame keeps `temps.len() == width × height` and fits
/// [`MAX_PIXELS`]; build one with [`ThermalFrame::new`].
#[derive(Debug, Clone, PartialEq)]
pub struct ThermalFrame {
    width: u32,
    height: u32,
    temps: Vec<f32>,
    vendor: Vendor,
    scene: SceneParams,
    planck: Option<PlanckConstants>,
    refused: Vec<SceneSetting>,
}

impl ThermalFrame {
    /// Checks the size against the data and [`MAX_PIXELS`].
    pub fn new(width: u32, height: u32, temps: Vec<f32>, vendor: Vendor, scene: SceneParams) -> Result<Self, ThermalError> {
        check_size(width, height)?;
        if temps.len() as u64 != u64::from(width) * u64::from(height) {
            return Err(ThermalError::Damaged("its temperature data does not match its size".into()));
        }
        Ok(Self { width, height, temps, vendor, scene, planck: None, refused: Vec::new() })
    }

    /// Records the FLIR constants the frame was converted with.
    pub fn with_planck(mut self, planck: PlanckConstants) -> Self {
        self.planck = Some(planck);
        self
    }

    /// Records overrides the camera refused (their camera values were used instead).
    pub fn with_refused(mut self, refused: Vec<SceneSetting>) -> Self {
        self.refused = refused;
        self
    }

    pub fn width(&self) -> u32 {
        self.width
    }
    pub fn height(&self) -> u32 {
        self.height
    }
    pub fn temps(&self) -> &[f32] {
        &self.temps
    }
    pub fn vendor(&self) -> Vendor {
        self.vendor
    }
    /// The settings the temperatures were measured with (the overrides that took effect included).
    pub fn scene(&self) -> &SceneParams {
        &self.scene
    }
    pub fn planck(&self) -> Option<&PlanckConstants> {
        self.planck.as_ref()
    }
    /// Overrides the camera refused; shown to the inspector so a form value that had no effect
    /// isn't mistaken for one that did.
    pub fn refused(&self) -> &[SceneSetting] {
        &self.refused
    }

    /// The temperature at `p`, or `None` outside the frame or where there is no valid reading.
    pub fn temperature_at(&self, p: Pixel) -> Option<f32> {
        if p.x >= self.width || p.y >= self.height {
            return None;
        }
        let i = p.y as usize * self.width as usize + p.x as usize;
        self.temps.get(i).copied().filter(|t| !t.is_nan())
    }

    /// Statistics over `region` (clipped to the frame) or the whole frame; `None` when no pixel in
    /// it has a valid reading.
    pub fn stats(&self, region: Option<Region>) -> Option<ThermalStats> {
        let (x0, y0, x1, y1) = match region {
            Some(r) => (r.a.x.min(r.b.x), r.a.y.min(r.b.y), r.a.x.max(r.b.x), r.a.y.max(r.b.y)),
            None => (0, 0, u32::MAX, u32::MAX),
        };
        if x0 >= self.width || y0 >= self.height {
            return None;
        }
        let (x1, y1) = (x1.min(self.width - 1), y1.min(self.height - 1));
        let w = self.width as usize;
        let mut s = ThermalStats {
            min_c: f32::INFINITY,
            max_c: f32::NEG_INFINITY,
            mean_c: 0.0,
            min_at: Pixel { x: 0, y: 0 },
            max_at: Pixel { x: 0, y: 0 },
            valid_pixels: 0,
        };
        let mut sum = 0.0f64;
        for y in y0..=y1 {
            let start = y as usize * w;
            let Some(row) = self.temps.get(start + x0 as usize..=start + x1 as usize) else { continue };
            for (dx, &t) in row.iter().enumerate() {
                if t.is_nan() {
                    continue;
                }
                let at = Pixel { x: x0 + dx as u32, y };
                if t < s.min_c {
                    s.min_c = t;
                    s.min_at = at;
                }
                if t > s.max_c {
                    s.max_c = t;
                    s.max_at = at;
                }
                sum += f64::from(t);
                s.valid_pixels += 1;
            }
        }
        (s.valid_pixels > 0).then(|| ThermalStats { mean_c: (sum / s.valid_pixels as f64) as f32, ..s })
    }
}

/// Refuses an empty frame or one over [`MAX_PIXELS`], before anything is allocated for it.
pub fn check_size(width: u32, height: u32) -> Result<(), ThermalError> {
    if width == 0 || height == 0 {
        return Err(ThermalError::Damaged("it reports an empty image size".into()));
    }
    if u64::from(width) * u64::from(height) > MAX_PIXELS {
        return Err(ThermalError::TooLarge { width, height });
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scene() -> SceneParams {
        SceneParams { emissivity: 0.95, reflected_temp_c: 20.0, ambient_temp_c: 25.0, distance_m: 5.0, humidity_pct: 50.0 }
    }

    #[test]
    fn stats_skip_unreadable_pixels_and_clip_regions() {
        let f = ThermalFrame::new(3, 2, vec![10.0, f32::NAN, 30.0, 20.0, 50.0, 40.0], Vendor::Flir, scene()).unwrap();
        let s = f.stats(None).unwrap();
        assert_eq!((s.min_c, s.max_c, s.valid_pixels), (10.0, 50.0, 5));
        assert_eq!(s.max_at, Pixel { x: 1, y: 1 });
        // Corners in any order, clipped to the frame.
        let r = Region { a: Pixel { x: 99, y: 0 }, b: Pixel { x: 2, y: 0 } };
        assert_eq!(f.stats(Some(r)).unwrap().max_c, 30.0);
        assert!(f.stats(Some(Region { a: Pixel { x: 5, y: 5 }, b: Pixel { x: 9, y: 9 } })).is_none());
        assert_eq!(f.temperature_at(Pixel { x: 1, y: 0 }), None);
        assert_eq!(f.temperature_at(Pixel { x: 3, y: 0 }), None);
    }

    #[test]
    fn sizes_are_checked_before_anything_is_kept() {
        assert!(matches!(ThermalFrame::new(2, 2, vec![0.0; 3], Vendor::Dji, scene()), Err(ThermalError::Damaged(_))));
        assert!(matches!(check_size(u32::MAX, u32::MAX), Err(ThermalError::TooLarge { .. })));
        assert!(check_size(0, 10).is_err());
    }

    #[test]
    fn half_typed_overrides_are_ignored() {
        let o = SceneOverrides { emissivity: Some(f64::NAN), humidity_pct: Some(60.0), ..Default::default() };
        assert_eq!(o.iter().collect::<Vec<_>>(), vec![(SceneSetting::Humidity, 60.0)]);
        assert_eq!(o.apply_to(scene()).emissivity, 0.95);
    }
}
