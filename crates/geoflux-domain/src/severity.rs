//! How urgent an asset's finding is, from its schema's [`SeverityRule`].

use crate::asset::{Properties, Severity};
use crate::schema::{SeverityRule, Thresholds};

impl Thresholds {
    /// The level an excess (e.g. a temperature rise in °C) reaches. Non-finite input is no finding.
    pub fn level(&self, excess: f64) -> Severity {
        if !excess.is_finite() {
            return Severity::None;
        }
        if excess >= self.critical {
            Severity::Critical
        } else if excess >= self.high {
            Severity::High
        } else if excess >= self.medium {
            Severity::Medium
        } else if excess >= self.low {
            Severity::Low
        } else {
            Severity::None
        }
    }
}

impl SeverityRule {
    /// Whether the properties match every `flaggedWhen` entry. A rule without entries flags nothing.
    pub fn is_flagged(&self, props: &Properties) -> bool {
        !self.flagged_when.is_empty()
            && self.flagged_when.iter().all(|m| props.get(&m.key).to_lowercase().contains(&m.contains.to_lowercase()))
    }

    /// The excess the rule rates: `rise`, or `measured − reference` when `rise` is blank or unreadable.
    pub fn excess(&self, props: &Properties) -> Option<f64> {
        crate::number(props.get(&self.rise)).or_else(|| {
            let measured = crate::number(props.get(self.measured.as_deref()?))?;
            let reference = crate::number(props.get(self.reference.as_deref()?))?;
            Some(measured - reference).filter(|v| v.is_finite())
        })
    }

    /// The severity of an asset with these properties. A flagged asset is never below LOW, and is
    /// MEDIUM when it has no usable numbers (it needs a look but cannot be rated).
    pub fn classify(&self, props: &Properties) -> Severity {
        let flagged = self.is_flagged(props);
        match self.excess(props) {
            None if flagged => Severity::Medium,
            None => Severity::None,
            Some(v) => {
                let level = self.thresholds.level(v);
                if flagged && level == Severity::None { Severity::Low } else { level }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::schema::FlagMatch;

    fn rule() -> SeverityRule {
        SeverityRule {
            rise: "rise".into(),
            measured: Some("fault".into()),
            reference: Some("ambient".into()),
            flagged_when: vec![
                FlagMatch { key: "status".into(), contains: "fault".into() },
                FlagMatch { key: "type".into(), contains: "fault".into() },
            ],
            thresholds: Thresholds::default(),
        }
    }

    fn props(pairs: &[(&str, &str)]) -> Properties {
        pairs.iter().copied().collect()
    }

    #[test]
    fn bands_follow_the_thresholds() {
        let t = Thresholds::default();
        assert_eq!(t.level(0.5), Severity::None);
        assert_eq!(t.level(1.0), Severity::Low);
        assert_eq!(t.level(4.0), Severity::Medium);
        assert_eq!(t.level(15.9), Severity::Medium);
        assert_eq!(t.level(16.0), Severity::High);
        assert_eq!(t.level(40.0), Severity::Critical);
        assert_eq!(t.level(f64::NAN), Severity::None);
    }

    #[test]
    fn rise_wins_then_measured_minus_reference() {
        let r = rule();
        assert_eq!(r.classify(&props(&[("rise", "20"), ("fault", "30"), ("ambient", "28")])), Severity::High);
        assert_eq!(r.classify(&props(&[("rise", ""), ("fault", "75"), ("ambient", "30")])), Severity::Critical);
        assert_eq!(r.classify(&props(&[("rise", "n/a"), ("fault", "32,5"), ("ambient", "30")])), Severity::Low);
    }

    #[test]
    fn flagged_assets_are_never_ignored() {
        let r = rule();
        let flagged = [("status", "Fault found"), ("type", "tower_fault")];
        assert_eq!(r.classify(&props(&flagged)), Severity::Medium);
        let mut small = flagged.to_vec();
        small.push(("rise", "0.2"));
        assert_eq!(r.classify(&props(&small)), Severity::Low);
        // Both entries must match: a fault status on a normal report type is not flagged.
        assert_eq!(r.classify(&props(&[("status", "fault"), ("type", "tower")])), Severity::None);
    }
}
