//! Projects opened or saved lately, newest first, kept with the app's preferences so the Project
//! page can reopen one with a click. Only paths are kept (the browser has none, so its list stays
//! empty).

use std::path::PathBuf;

use serde_json::{Value, json};

use crate::RustGisApp;

const PREF: &str = "recent_projects";
/// How many are kept.
pub const MAX: usize = 10;
/// The longest path kept (a longer one in the preferences was put there by hand).
const MAX_PATH: usize = 4096;

#[derive(Default)]
pub struct Recent {
    /// Newest first.
    pub paths: Vec<String>,
    loaded: bool,
    /// The project path last noted, so only a change touches the list.
    noted: Option<PathBuf>,
}

/// The kept list read back: strings only, no blanks, no repeats, at most [`MAX`] (the
/// preferences are a file anyone could have edited).
fn parse(text: &str) -> Vec<String> {
    let v: Value = serde_json::from_str(text).unwrap_or(Value::Null);
    let mut out: Vec<String> = Vec::new();
    for p in v.as_array().into_iter().flatten().filter_map(Value::as_str) {
        let p = p.trim();
        if !p.is_empty() && p.len() <= MAX_PATH && !out.iter().any(|o| o == p) {
            out.push(p.to_string());
        }
        if out.len() == MAX {
            break;
        }
    }
    out
}

/// Puts `path` first, once.
fn push_front(paths: &mut Vec<String>, path: String) {
    paths.retain(|p| *p != path);
    paths.insert(0, path);
    paths.truncate(MAX);
}

fn load(app: &mut RustGisApp) {
    if app.ui.recent.loaded {
        return;
    }
    app.ui.recent.loaded = true;
    if let Some(text) = app.services.load_pref.as_ref().and_then(|load| load(PREF)) {
        app.ui.recent.paths = parse(&text);
    }
}

fn save(app: &RustGisApp) {
    if let Some(save) = &app.services.save_pref {
        save(PREF, &json!(app.ui.recent.paths).to_string());
    }
}

/// Notes the project's file when it changed (opened, saved for the first time or saved as).
/// Cheap when it didn't: called after every command.
pub fn note(app: &mut RustGisApp) {
    if app.ui.recent.loaded && app.ui.recent.noted == app.session.path {
        return;
    }
    load(app);
    if app.ui.recent.noted == app.session.path {
        return;
    }
    app.ui.recent.noted.clone_from(&app.session.path);
    if let Some(p) = &app.session.path {
        let p = p.display().to_string();
        if p.len() <= MAX_PATH {
            push_front(&mut app.ui.recent.paths, p);
            save(app);
        }
    }
}

/// Opens a recent project. One whose file is gone leaves the list, with a note.
pub fn open(app: &mut RustGisApp, path: &str) {
    match app.run("project.open", json!({"path": path})) {
        Ok(v) => {
            // "Save changes?" may be asked first: the page stays until it is answered.
            if v.get("waiting").is_none() {
                app.ui.backstage = false;
            }
            app.map.invalidate();
        }
        Err(e) => {
            #[cfg(not(target_arch = "wasm32"))]
            if !std::path::Path::new(path).exists() {
                remove(app, path);
                app.dialogs.error =
                    Some(crate::i18n::fmt("{path} is no longer there; it was taken off the recent list.", &[("path", path)]));
                return;
            }
            app.dialogs.error = Some(e);
        }
    }
}

/// Takes a project off the list.
pub fn remove(app: &mut RustGisApp, path: &str) {
    let before = app.ui.recent.paths.len();
    app.ui.recent.paths.retain(|p| p != path);
    if app.ui.recent.paths.len() != before {
        save(app);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_keeps_clean_unique_paths_only() {
        let text = r#"["C:/a.rgis", "", "  ", 7, null, "C:/a.rgis", "/b.rgis", {"x": 1}]"#;
        assert_eq!(parse(text), ["C:/a.rgis", "/b.rgis"]);
        assert!(parse("not json").is_empty());
        assert!(parse(r#"{"paths": ["/a"]}"#).is_empty());
        let many: Vec<String> = (0..30).map(|i| format!("/p{i}.rgis")).collect();
        assert_eq!(parse(&json!(many).to_string()).len(), MAX);
        assert!(parse(&json!(["x".repeat(MAX_PATH + 1)]).to_string()).is_empty());
    }

    #[test]
    fn newest_first_without_repeats() {
        let mut v = vec!["/a".to_string(), "/b".to_string()];
        push_front(&mut v, "/b".into());
        assert_eq!(v, ["/b", "/a"]);
        for i in 0..20 {
            push_front(&mut v, format!("/n{i}"));
        }
        assert_eq!(v.len(), MAX);
        assert_eq!(v[0], "/n19");
    }
}
