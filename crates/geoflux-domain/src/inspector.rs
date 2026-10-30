//! The contract every automatic inspection meets, whether a fixed rule (the thermal hot-spot
//! rating) or a trained model, local or on GeoFlux Cloud. An [`Inspector`] looks at one photo and
//! returns [`Observation`]s; it never touches findings itself. The caller runs it off the UI
//! thread and hands each result to [`FindingSet::record_run`], which turns observations into
//! proposals a person reviews. Adding the AI health engine later is one more `Inspector`.
//!
//! Work is planned with [`plan`], which leaves out photos this inspector version has already
//! looked at, so re-running an inspection over a line is cheap and never duplicates findings.

use std::collections::HashSet;
use std::sync::atomic::{AtomicBool, Ordering};

use crate::asset::Properties;
use crate::finding::{FindingSet, InspectError, InspectorInfo, Observation, RunOutcome};
use crate::photo_id::PhotoId;
use crate::schema::{DomainSchema, ImageSlotDef};

/// Whether the user stopped the work. Inspectors check it between expensive steps.
pub trait Cancel: Sync {
    fn cancelled(&self) -> bool;
}

impl Cancel for AtomicBool {
    fn cancelled(&self) -> bool {
        self.load(Ordering::Relaxed)
    }
}

/// Never cancelled.
pub struct NoCancel;

impl Cancel for NoCancel {
    fn cancelled(&self) -> bool {
        false
    }
}

/// What an inspector is given for one photo.
pub struct InspectInput<'a> {
    pub photo: &'a PhotoId,
    /// Where the file is now (a desktop path, or the server's cached copy). The inspector reads
    /// it itself, so a large photo is only loaded by an inspector that needs it.
    pub path: &'a str,
    pub asset: &'a str,
    pub slot: &'a ImageSlotDef,
    /// The asset's form values (ambient temperature, emissivity…).
    pub properties: &'a Properties,
    pub schema: &'a DomainSchema,
}

/// An automatic inspection. Implementations are `Send + Sync` so one instance serves a worker
/// pool; they keep no per-photo state between calls.
pub trait Inspector: Send + Sync {
    fn info(&self) -> &InspectorInfo;

    /// Whether it looks at photos in this slot (a thermal rule only at thermal slots).
    fn wants(&self, slot: &ImageSlotDef) -> bool;

    /// Looks at one photo. Defect types must come from `input.schema`; observations that break the
    /// rules are dropped (and counted) when recorded, never stored. Return
    /// [`InspectError::NotApplicable`] for a photo it can't read by design, and
    /// [`InspectError::Cancelled`] as soon as `cancel` says so.
    fn inspect(&self, input: &InspectInput<'_>, cancel: &dyn Cancel) -> Result<Vec<Observation>, InspectError>;
}

/// One photo to inspect.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct InspectTask {
    pub asset: String,
    pub photo: PhotoId,
    pub path: String,
    pub slot: String,
}

/// The tasks this inspector version still has to do: in slots it wants, not run before (a failed
/// run is retried), each (asset, photo) once. Reads the set's runs once, so planning a whole line
/// costs one pass, not one per photo.
pub fn plan(
    set: &FindingSet,
    inspector: &dyn Inspector,
    schema: &DomainSchema,
    photos: impl IntoIterator<Item = InspectTask>,
) -> Vec<InspectTask> {
    let info = inspector.info();
    let done: HashSet<(&str, &PhotoId)> = set
        .runs()
        .iter()
        .filter(|r| r.inspector == info.id && r.version == info.version && !matches!(r.outcome, RunOutcome::Failed { .. }))
        .map(|r| (r.asset.as_str(), &r.photo))
        .collect();
    let wanted: HashSet<&str> = schema.image_slots.iter().filter(|s| inspector.wants(s)).map(|s| s.id.as_str()).collect();
    let mut seen = HashSet::new();
    photos
        .into_iter()
        .filter(|t| wanted.contains(t.slot.as_str()))
        .filter(|t| !done.contains(&(t.asset.as_str(), &t.photo)))
        .filter(|t| seen.insert((t.asset.clone(), t.photo.clone())))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::defect::DefectId;
    use crate::finding::InspectorKind;

    struct HotSpots(InspectorInfo);

    impl Inspector for HotSpots {
        fn info(&self) -> &InspectorInfo {
            &self.0
        }
        fn wants(&self, slot: &ImageSlotDef) -> bool {
            slot.thermal
        }
        fn inspect(&self, input: &InspectInput<'_>, cancel: &dyn Cancel) -> Result<Vec<Observation>, InspectError> {
            if cancel.cancelled() {
                return Err(InspectError::Cancelled);
            }
            if input.path.ends_with(".png") {
                return Err(InspectError::NotApplicable("no thermal data".into()));
            }
            Ok(vec![Observation { defect: DefectId::parse("hotspot.joint"), ..Default::default() }])
        }
    }

    fn schema() -> DomainSchema {
        crate::builtin_schema("com.geo.telecom").expect("listed").expect("valid")
    }

    fn task(asset: &str, bytes: &[u8], slot: &str, path: &str) -> InspectTask {
        InspectTask { asset: asset.into(), photo: PhotoId::of_bytes(bytes), path: path.into(), slot: slot.into() }
    }

    fn run(set: &mut FindingSet, insp: &HotSpots, schema: &DomainSchema, tasks: Vec<InspectTask>, now: i64) -> usize {
        let todo = plan(set, insp, schema, tasks);
        for t in &todo {
            let slot = schema.image_slots.iter().find(|s| s.id == t.slot).expect("slot");
            let props = Properties::new();
            let input = InspectInput { photo: &t.photo, path: &t.path, asset: &t.asset, slot, properties: &props, schema };
            let result = insp.inspect(&input, &NoCancel);
            set.record_run(&t.asset, &t.photo, &t.slot, insp.info(), result, now, schema).expect("recorded");
        }
        todo.len()
    }

    #[test]
    fn plans_wanted_slots_once_and_skips_what_this_version_did() {
        let s = schema();
        let v1 = HotSpots(InspectorInfo { id: "test.hot".into(), version: "1".into(), kind: InspectorKind::Rule, label: "Hot".into() });
        let tasks = || {
            vec![
                task("1/0", b"a", "THERMAL", "a.jpg"),
                task("1/0", b"a", "THERMAL", "copy/a.jpg"),
                task("1/0", b"b", "RGB_ZOOM", "b.jpg"),
                task("2/0", b"c", "THERMAL", "c.png"),
            ]
        };
        let mut set = FindingSet::new();
        assert_eq!(run(&mut set, &v1, &s, tasks(), 10), 2, "the duplicate and the RGB photo are left out");
        assert_eq!(set.findings().len(), 1);
        assert!(matches!(set.runs()[1].outcome, RunOutcome::Skipped { .. }));
        assert_eq!(run(&mut set, &v1, &s, tasks(), 20), 0, "a second run of the same version does nothing");

        let v2 = HotSpots(InspectorInfo { version: "2".into(), ..v1.0.clone() });
        assert_eq!(run(&mut set, &v2, &s, tasks(), 30), 2, "a new version looks again");
        assert_eq!(set.findings().len(), 1, "its proposal replaced the undecided one of version 1");
        assert_eq!(set.findings()[0].source.inspector(), Some(("test.hot", "2")));
    }

    #[test]
    fn a_cancelled_photo_is_left_for_next_time() {
        let s = schema();
        let insp = HotSpots(InspectorInfo { id: "test.hot".into(), version: "1".into(), kind: InspectorKind::Model, label: "Hot".into() });
        let mut set = FindingSet::new();
        let t = task("1/0", b"a", "THERMAL", "a.jpg");
        let slot = s.image_slots.iter().find(|x| x.id == "THERMAL").expect("slot");
        let props = Properties::new();
        let input = InspectInput { photo: &t.photo, path: &t.path, asset: &t.asset, slot, properties: &props, schema: &s };
        let stop = AtomicBool::new(true);
        let result = insp.inspect(&input, &stop);
        let report = set.record_run(&t.asset, &t.photo, &t.slot, insp.info(), result, 1, &s).expect("recorded");
        assert_eq!(report.outcome, None);
        assert!(set.is_empty());
        assert_eq!(plan(&set, &insp, &s, vec![t]).len(), 1);
    }
}
