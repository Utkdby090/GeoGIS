//! FLIR raw sensor counts to object temperature: the standard radiometric model (Planck curve plus
//! atmosphere, reflected-radiation and IR-window corrections), the same maths as ExifTool-based
//! tools and the Thermimage package. Ported from GeoFlux's `RadiometricCalibration`.

use crate::model::{PlanckConstants, SceneParams};

pub const KELVIN: f64 = 273.15;

/// Everything that doesn't depend on the pixel, worked out once per frame; [`Calibration::temp_c`]
/// is then a few arithmetic operations and one logarithm per pixel, with no allocation.
pub struct Calibration {
    p: PlanckConstants,
    object_scale: f64,
    background: f64,
}

impl Calibration {
    pub fn new(p: PlanckConstants, scene: &SceneParams) -> Self {
        let raw_of = |t: f64| p.r1 / (p.r2 * (((p.b) / (t + KELVIN)).exp() - p.f)) - p.o;
        let e = scene.emissivity.clamp(0.01, 1.0);
        let irt = if p.ir_window_transmission > 0.0 && p.ir_window_transmission <= 1.0 { p.ir_window_transmission } else { 1.0 };
        let at = scene.ambient_temp_c;
        let h2o = (scene.humidity_pct.clamp(0.0, 100.0) / 100.0)
            * (1.5587 + 0.06939 * at - 0.000_278_16 * at * at + 0.000_000_684_55 * at * at * at).exp();
        let d = (scene.distance_m.max(0.0) / 2.0).sqrt();
        let tau = p.atm_x * (-d * (p.atm_alpha1 + p.atm_beta1 * h2o.sqrt())).exp()
            + (1.0 - p.atm_x) * (-d * (p.atm_alpha2 + p.atm_beta2 * h2o.sqrt())).exp();
        // A broken atmosphere model must not divide by zero; no attenuation is the safe reading.
        let tau = if tau.is_finite() && tau > 0.0 { tau } else { 1.0 };
        let refl = raw_of(scene.reflected_temp_c);
        let atm = raw_of(at);
        let window = raw_of(p.ir_window_temp_c);
        let background = (1.0 - e) / e * refl
            + (1.0 - tau) / e / tau * atm
            + (1.0 - irt) / e / tau / irt * window
            + (1.0 - tau) / e / tau / irt / tau * atm;
        Self { p, object_scale: 1.0 / (e * tau * irt * tau), background }
    }

    /// °C for one raw value, or NaN when it is outside the curve.
    pub fn temp_c(&self, raw: u16) -> f32 {
        let p = &self.p;
        let obj = f64::from(raw) * self.object_scale - self.background;
        if obj + p.o <= 0.0 {
            return f32::NAN;
        }
        let x = p.r1 / (p.r2 * (obj + p.o)) + p.f;
        if x <= 0.0 {
            return f32::NAN;
        }
        let t = p.b / x.ln() - KELVIN;
        if t.is_finite() { t as f32 } else { f32::NAN }
    }
}
