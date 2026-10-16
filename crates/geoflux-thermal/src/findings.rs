//! A measurement turned into form values, and form values turned into scene overrides.

use serde::{Deserialize, Serialize};

use crate::model::{Pixel, Region, SceneOverrides};

/// Which form fields the thermal view reads its scene settings from and writes its findings to.
/// The defaults are the telecom schema's keys; a schema can name others. A `None` field is
/// neither read nor written.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct FormBinding {
    pub fault_temp: Option<String>,
    pub rise_temp: Option<String>,
    pub emissivity: Option<String>,
    pub ambient_temp: Option<String>,
    pub humidity: Option<String>,
    pub distance: Option<String>,
}

impl Default for FormBinding {
    fn default() -> Self {
        Self {
            fault_temp: Some("faultTemp".into()),
            rise_temp: Some("riseTemp".into()),
            emissivity: Some("emissivity".into()),
            ambient_temp: Some("ambientTemp".into()),
            humidity: Some("humidity".into()),
            distance: None,
        }
    }
}

/// What the reference temperature, which the rise is measured over, comes from.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub enum Reference {
    /// The ambient temperature (the form's, or the camera's).
    Ambient(f32),
    /// The hottest pixel of a healthy, similar component the inspector boxed (the usual practice
    /// for a ΔT rating: the same clamp on the next phase).
    Component { region: Region, max_c: f32 },
}

impl Reference {
    pub fn temp_c(&self) -> f32 {
        match *self {
            Reference::Ambient(t) => t,
            Reference::Component { max_c, .. } => max_c,
        }
    }
}

/// The hot spot an inspector records for a tower.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct ThermalFinding {
    pub max_c: f32,
    pub at: Pixel,
    /// The box it was found in; `None` for the whole frame.
    pub region: Option<Region>,
    pub reference: Option<Reference>,
}

impl ThermalFinding {
    /// The rise over the reference, °C; `None` without one.
    pub fn rise_c(&self) -> Option<f32> {
        self.reference.map(|r| self.max_c - r.temp_c()).filter(|d| d.is_finite())
    }

    /// `(field key, text)` pairs for the bound form fields, one decimal, for `geoflux.setValues`
    /// (which validates them like typed input).
    pub fn form_values(&self, binding: &FormBinding) -> Vec<(String, String)> {
        let mut out = Vec::with_capacity(2);
        if let Some(k) = &binding.fault_temp
            && self.max_c.is_finite()
        {
            out.push((k.clone(), format!("{:.1}", self.max_c)));
        }
        if let (Some(k), Some(rise)) = (&binding.rise_temp, self.rise_c()) {
            out.push((k.clone(), format!("{rise:.1}")));
        }
        out
    }
}

impl FormBinding {
    /// The scene overrides the form's values give, read with `value(key)`. Text that isn't a
    /// finite number (empty, half-typed) is no override. Emissivity outside 0.01–1 and humidity
    /// outside 0–100 are ignored rather than clamped: they are typos, not intentions.
    pub fn overrides(&self, value: impl Fn(&str) -> Option<String>) -> SceneOverrides {
        let num = |key: &Option<String>| -> Option<f64> {
            let text = value(key.as_deref()?)?;
            let n: f64 = text.trim().replace(',', ".").parse().ok()?;
            n.is_finite().then_some(n)
        };
        SceneOverrides {
            emissivity: num(&self.emissivity).filter(|e| (0.01..=1.0).contains(e)),
            reflected_temp_c: None,
            ambient_temp_c: num(&self.ambient_temp),
            distance_m: num(&self.distance).filter(|d| *d >= 0.0),
            humidity_pct: num(&self.humidity).filter(|h| (0.0..=100.0).contains(h)),
        }
    }
}
