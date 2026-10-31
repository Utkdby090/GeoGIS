//! Runs frames of the whole UI (no GPU) through every tab, pane, dialog and theme.

use egui_kittest::kittest::Queryable;
use rustgis_ui_egui::RustGisApp;
use serde_json::json;

fn harness() -> egui_kittest::Harness<'static, RustGisApp> {
    egui_kittest::Harness::builder().with_size(egui::vec2(1400.0, 860.0)).with_max_steps(64).build_ui_state(
        |ui, app: &mut RustGisApp| {
            let ctx = ui.ctx().clone();
            app.logic(&ctx);
            app.ui(ui);
        },
        RustGisApp::with_sample(),
    )
}

#[test]
fn every_tab_pane_and_dialog_renders() {
    let mut h = harness();
    for _ in 0..4 {
        h.step();
    }
    for tab in ["Home", "Data", "Style", "Edit", "Analyze", "View", "Publish", "Plugins", "Settings", "Help"] {
        h.state_mut().ui.tab = tab.into();
        h.step();
        h.step();
    }
    for (id, p) in [
        ("ui.pane.symbology", json!(null)),
        ("ui.pane.geoprocessing", json!(null)),
        ("ui.gp.open", json!({"tool": "buffer"})),
        ("ui.pane.history", json!(null)),
        ("ui.pane.details", json!(null)),
        ("ui.pane.catalog", json!(null)),
        ("table.open", json!(null)),
        ("ui.dialog", json!({"name": "selectByAttributes"})),
        ("ui.dialog", json!({"name": "goToXY"})),
        ("ui.dialog", json!({"name": "calculateField"})),
        ("ui.commandSearch", json!(null)),
        ("ui.about", json!(null)),
        ("ui.shortcuts", json!(null)),
        ("ui.theme", json!({"dark": true})),
        ("ui.backstage", json!({"open": true})),
    ] {
        h.state_mut().run(id, p).unwrap_or_else(|e| panic!("{id}: {e}"));
        h.step();
        h.step();
    }
    h.state_mut().ui.backstage = false;
    // Every language, the collapsed and peeked ribbon.
    for l in rustgis_ui_egui::i18n::Lang::all() {
        h.state_mut().run("ui.language", json!({"code": l.code()})).unwrap();
        h.step();
        h.step();
    }
    h.state_mut().run("ui.language", json!({"code": "auto"})).unwrap();
    assert!(h.state_mut().run("ui.language", json!({"code": "xx"})).is_err());
    h.state_mut().run("ui.ribbon", json!({"minimized": true})).unwrap();
    h.state_mut().ui.ribbon_peek = true;
    h.step();
    h.state_mut().run("ui.ribbon", json!({"minimized": false})).unwrap();
    h.step();
    for kind in ["unique", "graduated", "single"] {
        h.state_mut().run("layer.symbology", json!({"layer": "Countries", "type": kind, "field": "continent"})).ok();
        h.step();
    }
    h.state_mut().run("layer.select", json!({"layer": "Cities"})).unwrap();
    h.state_mut().run("layer.symbology", json!({"type": "proportional", "field": "pop_max"})).unwrap();
    h.state_mut().run("project.new", json!(null)).unwrap();
    for _ in 0..4 {
        h.step();
    }
    // The symbology edits above are unsaved: New Project asks first.
    h.get_by_label("Don't Save").click();
    for _ in 0..4 {
        h.step();
    }
    assert!(h.state().session.map().layers.is_empty());
}

/// Plugins are loaded for the whole process, so the tests that look at them take turns.
static PLUGINS: std::sync::Mutex<()> = std::sync::Mutex::new(());

#[test]
fn a_ribbon_wider_than_the_window_scrolls_with_arrow_buttons() {
    let mut h = harness();
    h.set_size(egui::vec2(900.0, 600.0));
    h.state_mut().ui.tab = "View".into();
    for _ in 0..4 {
        h.step();
    }
    assert!(h.query_by_label("Scroll the ribbon left").is_none());
    h.get_by_label("Scroll the ribbon right").click();
    for _ in 0..4 {
        h.step();
    }
    let _ = h.get_by_label("Scroll the ribbon left");
    // A window wide enough for every group shows no arrows.
    h.set_size(egui::vec2(3000.0, 600.0));
    h.state_mut().ui.tab = "Help".into();
    for _ in 0..4 {
        h.step();
    }
    assert!(h.query_by_label("Scroll the ribbon right").is_none());
    assert!(h.query_by_label("Scroll the tabs right").is_none());
}

#[test]
fn a_tab_strip_wider_than_the_window_scrolls_with_arrow_buttons() {
    let mut h = harness();
    h.set_size(egui::vec2(560.0, 500.0));
    for _ in 0..4 {
        h.step();
    }
    assert!(h.query_by_label("Scroll the tabs left").is_none());
    h.get_by_label("Scroll the tabs right").click();
    h.step();
    h.step();
    let _ = h.get_by_label("Scroll the tabs left");
    // Scrolling to the end reaches the last tab.
    for _ in 0..20 {
        let Some(right) = h.query_by_label("Scroll the tabs right") else { break };
        right.click();
        h.step();
        h.step();
    }
    assert!(h.query_by_label("Scroll the tabs right").is_none());
    h.get_by_label("Help").click();
    h.step();
    h.step();
    assert_eq!(h.state().ui.tab, "Help");
    // A tab opened off screen scrolls into view, so the left arrow goes away.
    h.state_mut().ui.tab = "Home".into();
    for _ in 0..4 {
        h.step();
    }
    assert!(h.query_by_label("Scroll the tabs left").is_none());
}

#[test]
fn plugins_install_from_a_file_after_a_prompt() {
    let _turn = PLUGINS.lock().unwrap_or_else(|e| e.into_inner());
    let dir = std::env::temp_dir().join(format!("rustgis-ui-plugins-{}", std::process::id()));
    rustgis_ui_egui::rustgis_engine::plugins::store::set_dir(Some(dir.join("installed")));
    std::fs::create_dir_all(&dir).unwrap();
    let file = dir.join("ui-test.rgplugin");
    std::fs::write(&file, rustgis_ui_egui::rustgis_engine::plugins::testing::package("ui-test", &["edit"])).unwrap();
    let mut h = harness();
    for _ in 0..4 {
        h.step();
    }
    h.state_mut().run("ui.installPlugin", json!({"path": file.display().to_string()})).unwrap();
    h.step();
    h.step();
    assert!(h.state().plugins.confirm.is_some(), "asks first");
    assert!(rustgis_ui_egui::rustgis_engine::plugins::get("ui-test").is_none());
    h.get_by_label("Install").click();
    h.step();
    h.step();
    assert!(rustgis_ui_egui::rustgis_engine::plugins::get("ui-test").is_some());
    // Its commands are on the Plugins tab, and run from there.
    h.state_mut().ui.tab = "Plugins".into();
    h.step();
    h.step();
    h.get_by_label("Hello").click();
    h.step();
    // Its tools: a button on the Plugins tab, and listed in the Tools pane.
    h.get_by_label("Make Points").click();
    h.step();
    h.step();
    assert_eq!(h.state().gp.tool, Some("plugin:ui-test/m"));
    h.state_mut().gp.tool = None;
    h.step();
    h.step();
    // On the ribbon and in the pane's Plugins section.
    assert_eq!(h.query_all_by_label("Select Two").count(), 2);
    for tab in ["installed", "browse", "settings"] {
        // An unreachable registry is an error in the window, not a crash.
        h.state_mut().run("ui.plugins", json!({"open": true, "tab": tab, "registry": "http://localhost:9/registry.json"})).unwrap();
        h.step();
        h.step();
    }
    // The Tools pane has an Uninstall of its own when a GeoLibre runner is installed on this
    // machine: closed, the only one is the plugin's.
    h.state_mut().ui.right = None;
    h.state_mut().run("ui.plugins", json!({"open": true, "tab": "installed"})).unwrap();
    h.step();
    h.get_by_label("Uninstall").click();
    h.step();
    assert!(rustgis_ui_egui::rustgis_engine::plugins::get("ui-test").is_none());
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn plugins_can_have_a_tab_of_their_own() {
    let _turn = PLUGINS.lock().unwrap_or_else(|e| e.into_inner());
    use rustgis_ui_egui::rustgis_engine::plugins as rp;
    let mut m: serde_json::Value = serde_json::from_str(&rp::testing::manifest("ui-tab", &[])).unwrap();
    // Its own names, so the other tests (which share the loaded plugins) don't see them.
    m["commands"][0]["label"] = json!("Tab Hello");
    m["tools"][0]["name"] = json!("Tab Points");
    m["tools"][1]["name"] = json!("Tab Select");
    m["tab"] = json!({"label": "Hello Tab", "groups": [
        {"label": "Things", "items": ["h", "tool:m", {"menu": "More", "items": ["e", "tool:x"]}]}
    ]});
    let pkg = rp::testing::zip_of(&[("plugin.json", m.to_string().as_bytes()), ("plugin.wasm", &rp::testing::wasm())]);
    rp::load(rp::Package::from_zip(&pkg).unwrap(), "test").unwrap();
    let mut h = harness();
    for _ in 0..4 {
        h.step();
    }
    // Its tab is in the tab strip; its buttons are there, not on the Plugins tab.
    h.state_mut().ui.tab = "Plugins".into();
    h.step();
    h.step();
    assert_eq!(h.query_all_by_label("Tab Points").count(), 0);
    h.state_mut().ui.tab = "plugin:ui-tab".into();
    h.step();
    h.step();
    h.get_by_label("Tab Hello").click();
    h.step();
    h.get_by_label("Tab Points").click();
    h.step();
    h.step();
    assert_eq!(h.state().gp.tool, Some("plugin:ui-tab/m"));
    h.get_by_label("More").click();
    for _ in 0..4 {
        h.step();
    }
    h.get_by_label("Tab Select").click();
    h.step();
    assert_eq!(h.state().gp.tool, Some("plugin:ui-tab/x"));
    // Hidden, then shown again, from Settings › Customize › Ribbon.
    h.state_mut().run("ui.customizeRibbon", json!({"tab": "plugin:ui-tab"})).unwrap();
    h.step();
    assert!(h.state().ui.hidden_tabs.contains(&"plugin:ui-tab".to_string()));
    h.state_mut().run("ui.customizeRibbon", json!({"tab": "plugin:ui-tab"})).unwrap();
    // A disabled plugin's tab goes, and the ribbon falls back to the Plugins tab.
    h.state_mut().ui.tab = "plugin:ui-tab".into();
    rp::set_enabled("ui-tab", false).unwrap();
    h.step();
    h.step();
    assert_eq!(h.state().ui.tab, "Plugins");
    rp::unload("ui-tab");
}

#[test]
fn browse_lists_thousands_of_plugins_quickly() {
    use rustgis_ui_egui::rustgis_engine::plugins::registry;
    let _turn = PLUGINS.lock().unwrap_or_else(|e| e.into_inner());
    let sha = "a".repeat(64);
    let list: Vec<serde_json::Value> = (0..5000)
        .map(|i| {
            let cat = ["Vector", "Raster", "Terrain"][i % 3];
            json!({"id": format!("fake-{i:04}"), "name": format!("Plugin {i}"), "version": "1.0.0",
                "description": format!("Does many things with data for plugin number {i}. ").repeat(3), "author": format!("Author {}", i % 97),
                "categories": [cat, "Utilities"], "package": format!("https://packages.example.com/fake-{i}.rgplugin"),
                "sha256": sha, "size": 123_456, "permissions": ["edit"]})
        })
        .collect();
    let entries = registry::parse(&json!({"plugins": list}).to_string(), "https://plugins.rustgis.app/registry.json").unwrap();
    assert_eq!(entries.len(), 5000);
    let mut h = harness();
    for _ in 0..4 {
        h.step();
    }
    h.state_mut().plugins.set_registry(Some(Ok(entries)));
    h.state_mut().run("ui.plugins", json!({"open": true, "tab": "browse"})).unwrap();
    h.step();
    let _ = h.get_by_label("5000 plugins");
    // Only the visible rows are laid out, so a frame stays cheap with thousands of plugins
    // (about 1 ms in a release build; it was over 100 ms when every card was drawn). The bound
    // is loose for debug builds on busy machines.
    let t0 = std::time::Instant::now();
    for _ in 0..10 {
        h.step();
    }
    let per_frame = t0.elapsed() / 10;
    assert!(per_frame < std::time::Duration::from_millis(100), "{per_frame:?} per frame");
    // Search and the category filter narrow the list.
    h.state_mut().plugins.filter = "plugin 4999".into();
    h.step();
    h.step();
    let _ = h.get_by_label("1 of 5000 plugins");
    h.state_mut().plugins.filter = String::new();
    h.state_mut().plugins.category = "Terrain".into();
    h.step();
    h.step();
    let _ = h.get_by_label("1666 of 5000 plugins");
    // A new registry of the same size replaces what is listed (the cache follows the registry,
    // not its allocation).
    h.state_mut().plugins.category = String::new();
    let two = |a: &str, b: &str| {
        let list: Vec<serde_json::Value> = [a, b]
            .iter()
            .map(|n| json!({"id": n.to_lowercase(), "name": n, "version": "1.0.0", "package": "https://packages.example.com/x.rgplugin", "sha256": sha}))
            .collect();
        registry::parse(&json!({"plugins": list}).to_string(), "https://plugins.rustgis.app/registry.json").unwrap()
    };
    h.state_mut().plugins.set_registry(Some(Ok(two("Alpha", "Beta"))));
    h.step();
    h.step();
    let _ = h.get_by_label("Alpha");
    h.state_mut().plugins.set_registry(Some(Ok(two("Gamma", "Delta"))));
    h.step();
    h.step();
    let _ = h.get_by_label("Gamma");
    assert!(h.query_by_label("Alpha").is_none());
    h.state_mut().plugins.set_registry(None);
    h.state_mut().plugins.open = false;
}

/// Switching to a CJK language keeps drawing the previous one for the frame its fonts are
/// installed in (`set_fonts` takes effect a frame late), so no frame shows missing-glyph boxes.
#[test]
fn language_switch_waits_for_its_font() {
    let mut h = harness();
    h.state_mut().run("ui.language", json!({"code": "en"})).unwrap();
    for _ in 0..4 {
        h.step();
    }
    assert_eq!(h.state().lang().code(), "en");
    h.state_mut().run("ui.language", json!({"code": "zh"})).unwrap();
    assert_eq!(h.state().chosen_lang().code(), "zh");
    h.step();
    assert_eq!(h.state().lang().code(), "en", "fonts installed this frame aren't active yet");
    h.step();
    assert_eq!(h.state().lang().code(), "zh");
    // Back to an alphabetic language: nothing to wait for.
    h.state_mut().run("ui.language", json!({"code": "de"})).unwrap();
    h.step();
    assert_eq!(h.state().lang().code(), "de");
}

/// Settings › Start-up (discussion #81): the preferences are saved, a later start follows them,
/// a shell's overrides win, and the start is neither an edit to undo nor unsaved work.
#[test]
fn start_up_preferences() {
    use std::collections::HashMap;
    use std::sync::{Arc, Mutex};
    let store: Arc<Mutex<HashMap<String, String>>> = Arc::default();
    let services = || {
        let (load, save) = (store.clone(), store.clone());
        rustgis_ui_egui::Services {
            load_pref: Some(Box::new(move |k| load.lock().unwrap().get(k).cloned())),
            save_pref: Some(Box::new(move |k, v| {
                save.lock().unwrap().insert(k.to_string(), v.to_string());
            })),
            ..Default::default()
        }
    };
    let mut app = RustGisApp::with_sample().with_services(services());
    assert!(app.run("ui.startup", json!({"projection": "nope"})).is_err());
    let r = app.run("ui.startup", json!({"blank": true, "projection": "epsg:3010"})).unwrap();
    assert_eq!(r, json!({"blank": true, "projection": "EPSG:3010", "extent": null}));
    assert_eq!(store.lock().unwrap().get("start_project").map(String::as_str), Some("blank"));

    let mut app = RustGisApp::with_sample().with_services(services());
    app.start(&Default::default()).unwrap();
    assert!(app.session.map().layers.is_empty());
    assert_eq!(app.session.map().projection.code(), "EPSG:3010");
    assert!(!app.session.dirty && !app.session.can_undo());

    // The web app's `?projection=robinson` without `?blank`: the sample still goes when the
    // preference says blank; an explicit `Some(false)` keeps it.
    let mut app = RustGisApp::with_sample().with_services(services());
    let start = |blank, projection: &str, extent: Option<&str>| rustgis_ui_egui::Start {
        blank,
        projection: Some(projection.into()),
        extent: extent.map(String::from),
    };
    app.start(&start(Some(false), "robinson", None)).unwrap();
    assert!(!app.session.map().layers.is_empty());
    assert_eq!(app.session.map().projection.code(), "robinson");
    assert!(app.start(&start(Some(false), "EPSG:4326", None)).is_err(), "degrees are refused");

    // A start extent: saved from the view, or given by the shell (`?extent=`).
    let mut app = RustGisApp::with_sample().with_services(services());
    app.start(&start(Some(true), "EPSG:3010", Some("10.5,55,24.5,69.5"))).unwrap();
    let view = app.session.run("map.extent", &json!(null)).unwrap();
    let e = rustgis_ui_egui::parse_extent(&view["extent"].as_array().unwrap().iter().map(|x| x.to_string()).collect::<Vec<_>>().join(","))
        .unwrap();
    assert!(e[0] < 11.0 && e[2] > 24.0 && e[1] < 55.5 && e[3] > 69.0, "Sweden is in view: {e:?}");
    assert!(e[2] - e[0] < 90.0 && e[3] - e[1] < 25.0, "and not the world: {e:?}");
    assert!(app.start(&start(None, "EPSG:3010", Some("10,60,5,50"))).is_err(), "west after east");
    app.run("ui.startup", json!({"extent": "view"})).unwrap();
    assert!(store.lock().unwrap().get("start_extent").is_some_and(|v| v.split(',').count() == 4));
    let mut again = RustGisApp::with_sample().with_services(services());
    assert!(again.ui.start_extent.is_some());
    again.start(&Default::default()).unwrap();
    assert!(again.run("ui.startup", json!({"extent": [1, 2]})).is_err());
    again.run("ui.startup", json!({"extent": null})).unwrap();
    assert_eq!(store.lock().unwrap().get("start_extent").map(String::as_str), Some(""));

    // Web Mercator is the default, stored as nothing.
    app.run("ui.startup", json!({"blank": false, "projection": "mercator"})).unwrap();
    assert_eq!(store.lock().unwrap().get("start_projection").map(String::as_str), Some(""));
}

#[test]
fn old_and_unknown_tab_names_open_a_current_tab() {
    let mut h = harness();
    for (asked, opened) in [("Imagery", "Analyze"), ("Feature Layer", "Style"), ("Share", "Publish"), ("Map", "Home"), ("nope", "Home")] {
        let _ = h.state_mut().run("ui.tab", json!({"tab": asked}));
        assert_eq!(h.state().ui.tab, opened, "{asked}");
        h.step();
    }
}

/// Settings holds Customize › Ribbon, so it (like Help) can't be hidden.
#[test]
fn settings_and_help_tabs_always_show() {
    let mut h = harness();
    for tab in ["Settings", "Help"] {
        assert!(h.state_mut().run("ui.customizeRibbon", json!({"tab": tab})).is_err(), "{tab}");
    }
    h.state_mut().run("ui.customizeRibbon", json!({"tab": "Edit"})).unwrap();
    assert_eq!(h.state().ui.hidden_tabs, vec!["Edit".to_string()]);
    h.state_mut().ui.tab = "Settings".into();
    h.step();
    h.step();
}

/// Add Delimited Text in the web app with a script's options: they go with the file picked next
/// (and only that one), so a projected CSV lands in place.
#[test]
fn delimited_text_options_go_with_the_picked_file() {
    let asked = std::sync::Arc::new(std::sync::Mutex::new(Vec::<String>::new()));
    let log = asked.clone();
    let services = rustgis_ui_egui::Services {
        open_async: Some(Box::new(move |purpose: &str| log.lock().unwrap().push(purpose.to_string()))),
        ..Default::default()
    };
    let mut app = RustGisApp::with_sample().with_services(services);
    app.run("ui.addDelimited", json!(null)).unwrap();
    app.run("ui.addDelimited", json!({"x": "east", "y": "north", "crs": "EPSG:2263", "delimiter": ";"})).unwrap();
    assert_eq!(asked.lock().unwrap().as_slice(), ["text", "text"]);
    // A drop (or another picker's file) doesn't take the options.
    let r = app.open_picked("dropped.csv", b"lon,lat\n3,4\n").unwrap();
    let e = app.session.map().layer(r["layer"].as_u64().unwrap() as u32).unwrap().extent();
    assert_eq!((e.min_x, e.min_y), (3.0, 4.0));
    let picked = format!("{}parks.csv", rustgis_ui_egui::DELIMITED_PICK);
    let r = app.open_picked(&picked, b"name;east;north\nCentral Park;992000;220000\n").unwrap();
    let id = r["layer"].as_u64().unwrap() as u32;
    let e = app.session.map().layer(id).unwrap().extent();
    assert!((e.min_x + 73.96).abs() < 0.05 && (e.min_y - 40.78).abs() < 0.05, "{e:?}");
    // The options were used up: the next pick is read as it is.
    let picked = format!("{}plain.csv", rustgis_ui_egui::DELIMITED_PICK);
    let r = app.open_picked(&picked, b"lon,lat\n1,2\n").unwrap();
    let e = app.session.map().layer(r["layer"].as_u64().unwrap() as u32).unwrap().extent();
    assert_eq!((e.min_x, e.min_y), (1.0, 2.0));
}

/// Add Delimited Text in the web app: the file first, then the dialog lists its columns (the ones
/// found by name picked), and the ones picked there place it.
#[test]
fn delimited_text_dialog_lists_the_picked_files_columns() {
    let services = rustgis_ui_egui::Services { open_async: Some(Box::new(|_: &str| {})), ..Default::default() };
    let app = RustGisApp::with_sample().with_services(services);
    let mut h = egui_kittest::Harness::builder().with_size(egui::vec2(1400.0, 860.0)).with_max_steps(64).build_ui_state(
        |ui, app: &mut RustGisApp| {
            let ctx = ui.ctx().clone();
            app.logic(&ctx);
            app.ui(ui);
        },
        app,
    );
    h.state_mut().run("ui.addDelimited", json!(null)).unwrap();
    let picked = format!("{}parks.csv", rustgis_ui_egui::DELIMITED_PICK);
    h.state_mut().open_picked(&picked, b"name;east;north\nCentral Park;992000;220000\n").unwrap();
    assert_eq!(h.state().dialogs.open, Some("addCsv"));
    assert_eq!(h.state().dialogs.list, ["name", "east", "north"]);
    assert_eq!(h.state().dialogs.mode, ";", "the separator found");
    assert!(h.state().dialogs.a.is_empty(), "east isn't a name RustGIS finds by itself");
    {
        let d = &mut h.state_mut().dialogs;
        (d.a, d.b, d.text) = ("east".into(), "north".into(), "EPSG:2263".into());
    }
    h.step();
    h.step();
    h.get_by_role_and_label(egui::accesskit::Role::Button, "Add").click();
    h.step();
    h.step();
    assert_eq!(h.state().dialogs.open, None);
    let e = h.state().session.map().layer_by_name("parks").unwrap().extent();
    assert!((e.min_x + 73.96).abs() < 0.05 && (e.min_y - 40.78).abs() < 0.05, "{e:?}");
    // A file whose columns are found: picked for you.
    h.state_mut().run("ui.addDelimited", json!(null)).unwrap();
    let picked = format!("{}stops.csv", rustgis_ui_egui::DELIMITED_PICK);
    h.state_mut().open_picked(&picked, b"stop,Lat,Lon\nA,1,2\n").unwrap();
    assert_eq!((h.state().dialogs.a.as_str(), h.state().dialogs.b.as_str()), ("Lon", "Lat"));
}

/// Add Data From URL downloads off the frame and adds the file when it lands; a failed download
/// is said.
#[test]
fn data_from_a_web_address_lands_later() {
    use std::io::{Read, Write};
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    std::thread::spawn(move || {
        let body = br#"{"type": "Feature", "properties": {}, "geometry": {"type": "Point", "coordinates": [1, 2]}}"#;
        for stream in listener.incoming().take(1) {
            let Ok(mut s) = stream else { continue };
            let mut buf = [0u8; 2048];
            let _ = s.read(&mut buf);
            let _ = write!(s, "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", body.len());
            let _ = s.write_all(body);
        }
    });
    let mut h = harness();
    h.step();
    let before = h.state().session.map().layers.len();
    h.state_mut().run("ui.addUrl", json!(null)).unwrap();
    h.step();
    h.state_mut().run("ui.addUrl", json!({"url": format!("http://{addr}/pin"), "name": "Pin"})).unwrap();
    h.state_mut().run("ui.addUrl", json!({"url": "http://169.254.169.254/x.geojson"})).unwrap();
    for _ in 0..100 {
        h.step();
        if h.state().session.map().layer_by_name("Pin").is_some() {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    assert_eq!(h.state().session.map().layers.len(), before + 1);
    assert!(h.state().session.map().layer_by_name("Pin").is_some());
    assert!(h.state().session.notices.iter().any(|n| n.error && n.text.contains("never fetched")));
    assert!(h.state().ui.downloads.is_empty());
}

/// A project by its web address (a `?project=` link) downloads off the frame and replaces the
/// open project when it lands, its layers and view with it; a failed download is said and keeps
/// the open project.
#[test]
fn a_project_from_a_web_address_opens_when_it_lands() {
    use std::io::{Read, Write};
    let mut h = harness();
    h.step();
    let mut shared = h.state().session.project.clone();
    shared.name = "Shared map".into();
    let layers = shared.map().layers.len();
    let body = serde_json::to_string(&shared).unwrap().into_bytes();
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    std::thread::spawn(move || {
        for stream in listener.incoming().take(1) {
            let Ok(mut s) = stream else { continue };
            let mut buf = [0u8; 2048];
            let _ = s.read(&mut buf);
            let _ = write!(s, "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", body.len());
            let _ = s.write_all(&body);
        }
    });
    h.state_mut().run("project.new", json!({})).unwrap();
    assert!(h.state_mut().run("ui.openUrl", json!({})).is_err());
    // An API's address, without the extension, is still read as a project.
    h.state_mut().run("ui.openUrl", json!({"url": format!("http://{addr}/maps/7/project?v=2")})).unwrap();
    for _ in 0..100 {
        h.step();
        if h.state().session.project.name == "Shared map" {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    assert_eq!(h.state().session.project.name, "Shared map");
    assert_eq!(h.state().session.map().layers.len(), layers);
    assert!(h.state().ui.project_download.is_none());
    h.state_mut().run("ui.openUrl", json!({"url": "http://169.254.169.254/a.rgis"})).unwrap();
    h.step();
    assert!(h.state().session.notices.iter().any(|n| n.error && n.text.contains("never fetched")));
    assert_eq!(h.state().session.project.name, "Shared map");
}

/// A project still downloading when another project is opened doesn't replace it; a pending item
/// picker isn't taken by the download either.
#[test]
fn a_late_project_download_leaves_the_newer_project_and_the_picker_alone() {
    use std::io::{Read, Write};
    let mut h = harness();
    h.step();
    let mut shared = h.state().session.project.clone();
    shared.name = "Late map".into();
    let body = serde_json::to_string(&shared).unwrap().into_bytes();
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    let (go, wait) = std::sync::mpsc::channel::<()>();
    std::thread::spawn(move || {
        for stream in listener.incoming().take(2) {
            let Ok(mut s) = stream else { continue };
            let mut buf = [0u8; 2048];
            let _ = s.read(&mut buf);
            // the first answer waits until the test has opened another project
            let _ = wait.recv_timeout(std::time::Duration::from_secs(5));
            let _ = write!(s, "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", body.len());
            let _ = s.write_all(&body);
        }
    });
    h.state_mut().run("ui.openUrl", json!({"url": format!("http://{addr}/late.rgis")})).unwrap();
    h.state_mut().run("project.new", json!({})).unwrap();
    let newer = h.state().session.project.name.clone();
    go.send(()).unwrap();
    for _ in 0..100 {
        h.step();
        if h.state().ui.project_download.is_none() {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    assert!(h.state().ui.project_download.is_none());
    assert_eq!(h.state().session.project.name, newer);
    assert!(h.state().session.notices.iter().any(|n| n.text.contains("after another project was opened")));
    // With an item picker waiting, a downloaded project opens as a project and the picker keeps waiting.
    h.state_mut().ui.pending_open = Some("style.add");
    let _ = go.send(());
    h.state_mut().run("ui.openUrl", json!({"url": format!("http://{addr}/late.rgis")})).unwrap();
    for _ in 0..100 {
        h.step();
        if h.state().session.project.name == "late" {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    assert_eq!(h.state().session.project.name, "late");
    assert_eq!(h.state().ui.pending_open, Some("style.add"));
}

/// Add From Service connects in the background and lists the service's layers in the dialog.
#[test]
fn a_service_is_listed_when_its_capabilities_land() {
    use std::io::{Read, Write};
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    std::thread::spawn(move || {
        let body = br#"<WMS_Capabilities version="1.3.0" xmlns="http://www.opengis.net/wms" xmlns:xlink="http://www.w3.org/1999/xlink"><Service><Title>Local</Title></Service><Capability><Request><GetMap><Format>image/png</Format><DCPType><HTTP><Get><OnlineResource xlink:href="http://127.0.0.1/wms?"/></Get></HTTP></DCPType></GetMap></Request><Layer><CRS>EPSG:3857</CRS><Layer><Name>a</Name><Title>A</Title></Layer></Layer></Capability></WMS_Capabilities>"#;
        for stream in listener.incoming().take(1) {
            let Ok(mut s) = stream else { continue };
            let mut buf = [0u8; 2048];
            let _ = s.read(&mut buf);
            let _ = write!(s, "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", body.len());
            let _ = s.write_all(body);
        }
    });
    let mut h = harness();
    h.state_mut().run("ui.addService", json!(null)).unwrap();
    h.step();
    h.state_mut().run("ui.addService", json!({"url": format!("http://{addr}/wms"), "type": "wms"})).unwrap();
    for _ in 0..100 {
        h.step();
        if h.state().ui.service.as_ref().is_some_and(|c| c.listing.is_some() || c.error.is_some()) {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    let conn = h.state().ui.service.as_ref().unwrap();
    assert_eq!(conn.error, None);
    assert_eq!(conn.listing.as_ref().unwrap()["layers"][0]["name"], "a");
    h.step();
}

/// Save in the web app (issue #197): the browser has no save dialog, so the first Save and every
/// Save As ask for the file name, and Save then downloads under it; an opened file keeps its name.
#[test]
fn web_save_asks_for_the_file_name() {
    let got = std::sync::Arc::new(std::sync::Mutex::new(Vec::<(String, Vec<u8>)>::new()));
    let log = got.clone();
    let services = rustgis_ui_egui::Services {
        download: Some(Box::new(move |name: &str, bytes: Vec<u8>| log.lock().unwrap().push((name.to_string(), bytes)))),
        ..Default::default()
    };
    let app = RustGisApp::with_sample().with_services(services);
    let mut h = egui_kittest::Harness::builder().with_size(egui::vec2(1400.0, 860.0)).with_max_steps(64).build_ui_state(
        |ui, app: &mut RustGisApp| {
            let ctx = ui.ctx().clone();
            app.logic(&ctx);
            app.ui(ui);
        },
        app,
    );
    let names = |got: &std::sync::Mutex<Vec<(String, Vec<u8>)>>| got.lock().unwrap().iter().map(|(n, _)| n.clone()).collect::<Vec<_>>();
    // A project never saved: Save asks first, with the project's name.
    h.state_mut().run("layer.color", json!({"layer": "Countries", "color": "#336699"})).unwrap();
    h.state_mut().run("ui.save", json!(null)).unwrap();
    assert_eq!(h.state().dialogs.open, Some("saveProject"));
    assert_eq!(h.state().dialogs.text, "World Atlas.rgis");
    assert!(names(&got).is_empty());
    h.step();
    h.state_mut().dialogs.text = "  ".into();
    h.step();
    h.get_by_role_and_label(egui::accesskit::Role::Button, "Save").click();
    h.step();
    assert_eq!(h.state().dialogs.open, Some("saveProject"), "an empty name is refused");
    assert!(h.state().dialogs.error.is_some() && names(&got).is_empty());
    h.state_mut().dialogs.text = "Trip plan".into();
    h.step();
    h.get_by_role_and_label(egui::accesskit::Role::Button, "Save").click();
    h.step();
    h.step();
    assert_eq!(h.state().dialogs.open, None);
    assert_eq!(names(&got), ["Trip plan.rgis"]);
    assert_eq!(h.state().session.project.name, "Trip plan");
    assert!(!h.state().session.dirty);
    // Saved once: Save downloads under that name again without asking; Save As asks.
    h.state_mut().run("ui.save", json!(null)).unwrap();
    assert_eq!(h.state().dialogs.open, None);
    assert_eq!(names(&got), ["Trip plan.rgis", "Trip plan.rgis"]);
    h.state_mut().run("ui.saveAs", json!(null)).unwrap();
    assert_eq!(h.state().dialogs.open, Some("saveProject"));
    assert_eq!(h.state().dialogs.text, "Trip plan.rgis");
    h.state_mut().dialogs.open = None;
    // An opened file names the project, and Save keeps its name.
    let bytes = got.lock().unwrap()[0].1.clone();
    h.state_mut().open_picked("test.rgis", &bytes).unwrap();
    assert_eq!(h.state().session.project.name, "test");
    h.state_mut().run("ui.save", json!(null)).unwrap();
    assert_eq!(h.state().dialogs.open, None);
    assert_eq!(names(&got).last().map(String::as_str), Some("test.rgis"));
    // Another project in its place hasn't been saved: Save asks again, even under the same name.
    let n = names(&got).len();
    h.state_mut().run("project.new", json!({"name": "test"})).unwrap();
    h.state_mut().run("ui.save", json!(null)).unwrap();
    assert_eq!(h.state().dialogs.open, Some("saveProject"));
    assert_eq!(names(&got).len(), n);
    // A name that can't be a file name is offered as one that can.
    h.state_mut().run("project.new", json!({"name": "Roads 2024/25"})).unwrap();
    h.state_mut().run("ui.saveAs", json!(null)).unwrap();
    assert_eq!(h.state().dialogs.text, "Roads 2024-25.rgis");
}

/// Without a browser's downloads, Save Project As (opened by `ui.dialog`) saves nothing and
/// leaves the project as it was.
#[test]
fn save_project_dialog_needs_downloads() {
    let mut h = harness();
    h.state_mut().run("layer.color", json!({"layer": "Countries", "color": "#336699"})).unwrap();
    h.state_mut().run("ui.dialog", json!({"name": "saveProject"})).unwrap();
    h.step();
    h.state_mut().dialogs.text = "Elsewhere".into();
    h.step();
    h.get_by_role_and_label(egui::accesskit::Role::Button, "Save").click();
    h.step();
    assert!(h.state().dialogs.error.is_some());
    assert_eq!(h.state().session.project.name, "World Atlas");
    assert!(h.state().session.dirty);
}

/// A tower line with a thermal photo, findings on it (one suggested, one confirmed), and the
/// Inspection pane and Findings window drawing them, in both themes.
#[test]
fn findings_window_and_summary_render() {
    let dir = std::env::temp_dir().join(format!("geogis-findings-ui-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let photo = dir.join("DJI_0001_T.JPG");
    image::RgbImage::from_fn(64, 48, |x, y| image::Rgb([(x * 4) as u8, (y * 5) as u8, 90])).save(&photo).unwrap();
    let path = photo.display().to_string();
    let kml = r#"<kml><Document><name>L</name>
      <Placemark><name>34/1</name><Point><coordinates>75.100,27.500</coordinates></Point></Placemark>
      <Placemark><name>34/2</name><Point><coordinates>75.104,27.500</coordinates></Point></Placemark>
    </Document></kml>"#;

    let mut h = harness();
    h.state_mut().run("project.new", json!(null)).unwrap();
    let layer = h.state_mut().run("geoflux.importLineKml", json!({"text": kml})).unwrap()["layer"].as_u64().unwrap();
    h.state_mut().run("geoflux.addPhotos", json!({"layer": layer, "row": 0, "slot": "THERMAL", "paths": [path]})).unwrap();
    // No tower is open yet: the window asks for one, and opens the pane to pick it.
    assert!(h.state_mut().run("ui.geofluxFindings", json!(null)).is_err());
    for _ in 0..4 {
        h.step();
    }
    h.state_mut().run("ui.geofluxFindings", json!(null)).unwrap();
    for _ in 0..4 {
        h.step();
    }
    let boxed = json!({"kind": "box", "a": {"x": 0.2, "y": 0.2}, "b": {"x": 0.5, "y": 0.6}});
    h.state_mut()
        .run("geoflux.addFinding", json!({"layer": layer, "row": 0, "path": path, "defect": "hotspot.joint", "region": boxed, "by": "eng"}))
        .unwrap();
    h.state_mut().run("geoflux.thermal.scan", json!({"layer": layer, "results": [{"row": 0, "maxC": 70.0, "referenceC": 30.0}]})).unwrap();
    for dark in [false, true] {
        h.state_mut().run("ui.theme", json!({"dark": dark})).unwrap();
        for _ in 0..6 {
            h.step();
        }
    }
    let _ = h.get_by_label("Next pending ›");
    let _ = h.get_by_label("Suggestions to review: 1");
    // Worst first: the 40 °C rise suggestion, then the engineer's box.
    let _ = h.get_by_label("1. Other hot spot");
    let _ = h.get_by_label("2. Hot joint or connector");
    // Confirming the suggestion makes it count: the tower turns critical.
    h.get_by_label("Confirm").click();
    for _ in 0..3 {
        h.step();
    }
    let set = h.state().session.project.geoflux.findings_of(layer as u32).unwrap();
    assert!(set.findings().iter().all(|f| f.status.counts()), "confirmed");
    assert_eq!(set.for_asset("34/1").severity(), rustgis_engine::geoflux_domain::Severity::Critical);
    h.get_by_label("New finding for the whole photo").click();
    for _ in 0..3 {
        h.step();
    }
    let _ = h.get_by_label("New finding");
    h.get_by_label("Cancel").click();
    for _ in 0..3 {
        h.step();
    }
    assert!(h.query_by_label("New finding").is_none(), "the draft is gone");
    // A suggestion on the other tower: Next pending goes there.
    let other = dir.join("DJI_0002_T.JPG");
    image::RgbImage::from_fn(64, 48, |x, _| image::Rgb([(x * 3) as u8, 20, 200])).save(&other).unwrap();
    let other = other.display().to_string();
    h.state_mut().run("geoflux.addPhotos", json!({"layer": layer, "row": 1, "slot": "THERMAL", "paths": [other]})).unwrap();
    h.state_mut().run("geoflux.thermal.scan", json!({"layer": layer, "results": [{"row": 1, "maxC": 50.0, "referenceC": 30.0}]})).unwrap();
    h.step();
    h.get_by_label("Next pending ›").click();
    for _ in 0..4 {
        h.step();
    }
    assert_eq!(h.state().inspection.row, Some(1));
    let _ = h.get_by_label("1. Other hot spot");
    // The photo zooms in and back to the whole photo.
    for _ in 0..20 {
        if h.query_by_label("Zoom in").is_some() {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(20));
        h.step();
    }
    let _ = h.get_by_label("Zoom 100%");
    h.get_by_label("Zoom in").click();
    h.step();
    h.step();
    let _ = h.get_by_label("Zoom 150%");
    h.get_by_label("Zoom in").click();
    h.step();
    h.step();
    let _ = h.get_by_label("Zoom 225%");
    h.get_by_label("Fit photo").click();
    h.step();
    h.step();
    let _ = h.get_by_label("Zoom 100%");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn unsaved_changes_are_asked_about_before_they_are_lost() {
    let mut h = harness();
    let dir = std::env::temp_dir().join(format!("geogis-unsaved-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let file = dir.join("line.rgis");
    h.state_mut().run("project.saveAs", json!({"path": file.display().to_string()})).unwrap();
    let maps = |h: &mut egui_kittest::Harness<'static, RustGisApp>| h.state_mut().session.project.maps.len();
    // An edit, then New Project: held until answered.
    h.state_mut().run("map.new", json!({"name": "Second"})).unwrap();
    assert!(h.state_mut().has_unsaved_changes());
    h.state_mut().run("project.new", json!(null)).unwrap();
    for _ in 0..3 {
        h.step();
    }
    assert_eq!(maps(&mut h), 2, "nothing is replaced before the answer");
    let _ = h.get_by_label_contains("Save changes to");
    h.get_by_label("Cancel").click();
    for _ in 0..3 {
        h.step();
    }
    assert_eq!(maps(&mut h), 2);
    assert!(h.query_by_label_contains("Save changes to").is_none());
    // Save: written to the project's file, then the new project opens.
    h.state_mut().run("project.new", json!(null)).unwrap();
    for _ in 0..3 {
        h.step();
    }
    h.get_by_label("Save").click();
    for _ in 0..3 {
        h.step();
    }
    assert_eq!(maps(&mut h), 1);
    assert!(std::fs::read_to_string(&file).unwrap().contains("Second"), "saved before replacing");
    // Closing the window with changes: asked, and Don't Save lets it close.
    h.state_mut().run("map.new", json!({"name": "Third"})).unwrap();
    assert!(!h.state_mut().may_close());
    for _ in 0..3 {
        h.step();
    }
    let _ = h.get_by_label_contains("close GeoGIS");
    h.get_by_label("Don't Save").click();
    for _ in 0..3 {
        h.step();
    }
    assert!(h.state_mut().quit_requested && h.state_mut().may_close());
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn map_tabs_close_and_undo_brings_them_back() {
    let mut h = harness();
    h.state_mut().run("map.new", json!({"name": "Second"})).unwrap();
    for _ in 0..4 {
        h.step();
    }
    let maps = |h: &mut egui_kittest::Harness<'static, RustGisApp>| {
        h.state_mut().session.project.maps.iter().map(|m| m.name.clone()).collect::<Vec<_>>()
    };
    let before = maps(&mut h);
    assert_eq!(before.len(), 2, "{before:?}");
    // The open tab shows its close button; closing removes the map, and Ctrl+Z restores it.
    h.get_by_label("Close map Second").click();
    for _ in 0..4 {
        h.step();
    }
    assert_eq!(maps(&mut h).len(), 1);
    assert!(h.state_mut().session.notices_since(0).iter().any(|n| n.text.contains("Closed map Second")));
    // The last map has no close button.
    assert!(h.query_by_label_contains("Close map").is_none());
    h.state_mut().run("edit.undo", json!(null)).unwrap();
    assert_eq!(maps(&mut h), before);
}

/// Opening the Inspection pane shows its first tower: selected and centred on the map, without
/// marking the project changed.
#[test]
fn inspection_pane_opens_on_its_first_tower() {
    let kml = r#"<kml><Document><name>L</name>
      <Placemark><name>34/1</name><Point><coordinates>75.100,27.500</coordinates></Point></Placemark>
      <Placemark><name>34/2</name><Point><coordinates>75.104,27.500</coordinates></Point></Placemark>
    </Document></kml>"#;
    let dir = std::env::temp_dir().join(format!("geogis-first-tower-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let mut h = harness();
    h.state_mut().run("project.new", json!(null)).unwrap();
    let layer = h.state_mut().run("geoflux.importLineKml", json!({"text": kml})).unwrap()["layer"].as_u64().unwrap() as u32;
    h.state_mut().run("map.zoomWorld", json!(null)).unwrap();
    h.state_mut().run("project.saveAs", json!({"path": dir.join("first.rgis").display().to_string()})).unwrap();
    assert!(!h.state().session.dirty);
    h.state_mut().ui.right = None;
    h.step();
    h.state_mut().run("ui.pane.inspection", json!(null)).unwrap();
    for _ in 0..4 {
        h.step();
    }
    let sel = h.state().session.map().layer(layer).unwrap().selection.clone();
    assert_eq!(sel.len(), 1, "the first tower is selected");
    let x = h.state().session.map().camera.center.x;
    let merc = |lon: f64| lon * 20_037_508.342_789_244 / 180.0;
    assert!((x - merc(75.100)).abs() < 1.0 || (x - merc(75.104)).abs() < 1.0, "centred on a tower, at x {x}");
    assert!(!h.state().session.dirty, "showing a tower changes nothing");
    let _ = std::fs::remove_dir_all(&dir);
}

/// Projects saved or opened show on the Project page, newest first; a click reopens one, and one
/// whose file is gone leaves the list with a note. The list is kept with the preferences.
#[test]
fn recent_projects_reopen_from_the_project_page() {
    use std::{cell::RefCell, rc::Rc};
    let dir = std::env::temp_dir().join(format!("geogis-recent-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let (a, b) = (dir.join("Line A.rgis"), dir.join("Line B.rgis"));
    let prefs: Rc<RefCell<std::collections::HashMap<String, String>>> = Rc::default();

    let mut h = harness();
    let store = prefs.clone();
    h.state_mut().services.save_pref = Some(Box::new(move |k: &str, v: &str| {
        store.borrow_mut().insert(k.to_string(), v.to_string());
    }));
    h.state_mut().run("project.saveAs", json!({"path": a.display().to_string()})).unwrap();
    h.state_mut().run("project.new", json!(null)).unwrap();
    h.state_mut().run("project.saveAs", json!({"path": b.display().to_string()})).unwrap();
    let kept: Vec<String> = serde_json::from_str(&prefs.borrow()["recent_projects"]).unwrap();
    assert_eq!(kept, [b.display().to_string(), a.display().to_string()], "newest first");

    h.state_mut().ui.backstage = true;
    for _ in 0..3 {
        h.step();
    }
    h.get_by_label("Open Line A").click();
    for _ in 0..3 {
        h.step();
    }
    assert_eq!(h.state().session.path.as_deref(), Some(a.as_path()));
    assert!(!h.state().ui.backstage, "back on the map");
    let kept: Vec<String> = serde_json::from_str(&prefs.borrow()["recent_projects"]).unwrap();
    assert_eq!(kept[0], a.display().to_string(), "reopened goes first");

    // B's file is gone: clicking it takes it off the list.
    std::fs::remove_file(&b).unwrap();
    h.state_mut().ui.backstage = true;
    for _ in 0..3 {
        h.step();
    }
    h.get_by_label("Open Line B").click();
    for _ in 0..3 {
        h.step();
    }
    assert!(h.state().dialogs.error.as_deref().is_some_and(|e| e.contains("no longer there")));
    assert_eq!(h.state().ui.recent.paths, [a.display().to_string()]);
    // × removes without opening.
    h.state_mut().dialogs.error = None;
    h.state_mut().ui.backstage = true;
    h.step();
    h.get_by_label("Remove Line A from the list").click();
    h.step();
    assert!(h.state().ui.recent.paths.is_empty());
    let _ = std::fs::remove_dir_all(&dir);
}

/// Esc closes the frontmost window (or the one under the pointer) one at a time, and with the
/// pointer over the map it stays the map's (back to Explore) and closes nothing.
#[test]
fn escape_closes_the_window_in_use() {
    let mut h = harness();
    for _ in 0..4 {
        h.step();
    }
    h.state_mut().run("ui.about", json!(null)).unwrap();
    h.state_mut().run("ui.shortcuts", json!(null)).unwrap();
    for _ in 0..3 {
        h.step();
    }
    let open = |h: &egui_kittest::Harness<'static, RustGisApp>| (h.state().ui.about, h.state().ui.shortcuts);
    assert_eq!(open(&h), (true, true));
    // Over the map: Esc is the map's.
    h.state_mut().run("tool.set", json!({"tool": "measure"})).unwrap();
    h.step();
    h.hover_at(egui::pos2(420.0, 760.0));
    h.step();
    h.key_press(egui::Key::Escape);
    h.step();
    h.step();
    assert_eq!(open(&h), (true, true), "nothing closed from the map");
    assert_eq!(h.state().session.tool.code(), "explore", "Esc on the map still goes back to Explore");
    // Command Search (a modal) takes Esc itself: the windows behind it stay.
    h.state_mut().run("ui.commandSearch", json!(null)).unwrap();
    h.event(egui::Event::PointerGone);
    h.step();
    h.step();
    h.key_press(egui::Key::Escape);
    h.step();
    h.step();
    assert!(h.state().ui.command_search.is_none(), "Command Search closed");
    assert_eq!(open(&h), (true, true), "only Command Search closed");
    // Pointer over the shortcuts window: it closes, About stays.
    let at = h.get_by_label("Close the window under the pointer").rect().center();
    h.hover_at(at);
    h.step();
    h.key_press(egui::Key::Escape);
    h.step();
    h.step();
    assert_eq!(open(&h), (true, false));
    let at = h.query_by_label_contains("A fast, free and open-source GIS").unwrap().rect().center();
    h.hover_at(at);
    h.step();
    h.key_press(egui::Key::Escape);
    h.step();
    h.step();
    assert_eq!(open(&h), (false, false));
}
