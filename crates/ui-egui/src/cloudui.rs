//! GeoFlux › Account and GeoFlux › Sync to Cloud: signing this computer in to a GeoFlux account,
//! and sending the inspection layer to it.
//!
//! Signing in uses a code the person approves in the browser (as TVs do), so GeoGIS never sees a
//! password. The pass, kept in the settings folder, says which paid features are on; the refresh
//! token that renews it is kept in Windows Credential Manager or the macOS Keychain.
//!
//! Every network call and every read of the pass runs on a worker thread; the window only reads
//! results. The refresh token is used under one lock: renewing replaces it, and the server signs
//! a computer out when an old one is used again, so two renewals must never race.
//!
//! Paid features aren't locked yet ([`ENFORCE`]): the check is in place, switched off until the
//! account server is live.

use egui::{Align2, RichText, vec2};
use geoflux_licence::{Access, Feature, Plan};

use crate::RustGisApp;

/// Whether paid features are locked for the free tier. Off until the account server is live.
pub const ENFORCE: bool = false;

/// The preference holding the account server's address.
const SERVER_PREF: &str = "cloudServer";

/// The paid feature a command needs, if any. Free use (the map, importing a survey sheet, linking
/// photos, the Inspection pane read-only) needs none.
pub fn feature_of(id: &str) -> Option<Feature> {
    Some(match id {
        "geoflux.setValues" | "geoflux.addPhotos" | "geoflux.removePhoto" | "geoflux.classify" => Feature::EditForms,
        "geoflux.annotate" => Feature::Markup,
        "geoflux.report" | "ui.geofluxReport" => Feature::Reports,
        "geoflux.reportBulk" | "ui.geofluxReportBulk" => Feature::BulkReports,
        "geoflux.importGeox" | "ui.geofluxImportGeox" => Feature::GeoxImport,
        "ui.cloudSync" => Feature::CloudSync,
        _ if id.starts_with("geoflux.thermal.") || id.starts_with("ui.thermal.") => Feature::Thermal,
        _ => return None,
    })
}

/// Refuses a command whose paid feature the current pass doesn't include (only when [`ENFORCE`]).
pub fn check(app: &RustGisApp, id: &str) -> Result<(), String> {
    match feature_of(id) {
        Some(f) if ENFORCE && !app.cloud.access().is_some_and(|a| a.allows(f)) => {
            Err(tl!("This needs a GeoFlux Pro or Organisation plan. Open GeoFlux › Account to sign in.").into())
        }
        _ => Ok(()),
    }
}

/// A finished sync.
#[cfg(not(target_arch = "wasm32"))]
#[derive(Clone, Debug)]
struct SyncDone {
    survey: String,
    revision: u32,
    towers: usize,
    sent: usize,
    photos: u32,
    skipped: Vec<String>,
}

/// The Account window, the account itself, and a sync in progress.
#[derive(Default)]
pub struct CloudState {
    /// The Account window is open.
    pub open: bool,
    /// The account server's address, as typed.
    server: String,
    server_loaded: bool,
    note: Option<(String, bool)>,
    #[cfg(not(target_arch = "wasm32"))]
    native: native::State,
}

impl CloudState {
    /// What the pass allows now; `None` while it's still being read.
    pub fn access(&self) -> Option<Access> {
        #[cfg(not(target_arch = "wasm32"))]
        {
            self.native.account.as_ref().and_then(|a| a.access())
        }
        #[cfg(target_arch = "wasm32")]
        {
            None
        }
    }
}

/// One line on the account: plan and days left, or why the free tier.
#[cfg_attr(target_arch = "wasm32", allow(dead_code))]
fn status_text(access: &Access) -> String {
    match access {
        Access::Subscribed { pass, days_left } => {
            let plan = match pass.plan {
                Plan::Organisation => tl!("Organisation plan"),
                Plan::Pro => tl!("Pro plan"),
                Plan::Free | Plan::Unknown => tl!("Free plan"),
            };
            let who = pass.org.as_deref().map_or_else(|| pass.name.clone(), |org| format!("{} ({org})", pass.name));
            crate::i18n::fmt(
                "Signed in as {who}, {plan}. GeoGIS works offline for {days} more days.",
                &[("who", &who), ("plan", plan), ("days", &days_left.to_string())],
            )
        }
        Access::Free(reason) => tl!(reason.message()).to_string(),
    }
}

#[cfg_attr(target_arch = "wasm32", allow(dead_code))]
fn signed_in(access: Option<&Access>) -> bool {
    matches!(access, Some(Access::Subscribed { .. }))
}

/// Reads the saved server address once.
fn load_server(app: &mut RustGisApp) {
    if !app.cloud.server_loaded {
        app.cloud.server_loaded = true;
        app.cloud.server = app.services.load_pref.as_ref().and_then(|f| f(SERVER_PREF)).unwrap_or_default();
    }
}

/// `ui.account` and `ui.cloudSync`.
pub fn command(app: &mut RustGisApp, id: &str) -> Result<(), String> {
    load_server(app);
    match id {
        "ui.account" => {
            app.cloud.open = !app.cloud.open;
            Ok(())
        }
        _ => start_sync(app),
    }
}

/// Starts reading the stored pass once the settings folder is known (the first frame).
pub fn poll(app: &mut RustGisApp, ctx: &egui::Context) {
    #[cfg(not(target_arch = "wasm32"))]
    native::poll(app, ctx);
    #[cfg(target_arch = "wasm32")]
    let _ = (app, ctx);
}

#[cfg(target_arch = "wasm32")]
fn start_sync(_: &mut RustGisApp) -> Result<(), String> {
    Err(tl!("Sync to Cloud needs the desktop app.").into())
}

#[cfg(not(target_arch = "wasm32"))]
fn start_sync(app: &mut RustGisApp) -> Result<(), String> {
    native::start_sync(app)
}

/// Draws the Account window and a sync's progress.
pub fn show(app: &mut RustGisApp, ctx: &egui::Context) {
    if app.cloud.open {
        load_server(app);
        account_window(app, ctx);
    }
    #[cfg(not(target_arch = "wasm32"))]
    native::sync_window(app, ctx);
}

fn account_window(app: &mut RustGisApp, ctx: &egui::Context) {
    let mut open = true;
    egui::Window::new(tl!("GeoFlux Account"))
        .open(&mut open)
        .collapsible(false)
        .resizable(false)
        .anchor(Align2::CENTER_CENTER, vec2(0.0, 0.0))
        .show(ctx, |ui| {
            ui.set_width(420.0);
            if cfg!(target_arch = "wasm32") {
                ui.label(tl!("Signing in to a GeoFlux account needs the desktop app."));
                return;
            }
            #[cfg(not(target_arch = "wasm32"))]
            native::account_ui(app, ui);
            if let Some((m, err)) = &app.cloud.note {
                ui.add_space(4.0);
                let t = crate::theme::Tokens::get(ui.ctx());
                ui.label(RichText::new(m.as_str()).small().color(if *err { t.red } else { t.text_dim }));
            }
            if !ENFORCE {
                ui.add_space(6.0);
                ui.label(RichText::new(tl!("Paid features are not locked yet: everything works without signing in.")).small().weak());
            }
        });
    if !open {
        app.cloud.open = false;
    }
}

#[cfg(not(target_arch = "wasm32"))]
mod native {
    use std::path::{Path, PathBuf};
    use std::sync::atomic::{AtomicBool, AtomicU8, Ordering};
    use std::sync::{Arc, Mutex, MutexGuard};
    use std::time::{Duration, Instant};

    use egui::{Align2, RichText, vec2};
    use geoflux_client::{AccessTokens, CloudClient, DevicePoll, Progress as UploadProgress, SyncError};
    use geoflux_cloud::api::DeviceGrant;
    use geoflux_licence::desktop::{FilePassStore, SystemClock, SystemMachineId};
    use geoflux_licence::{Access, AccountError, Ed25519Keys, FreeReason, Gate, Licensing};
    use rustgis_engine::cmd::cloud_sync;
    use rustgis_engine::geoflux_report::Progress;

    use super::{SERVER_PREF, SyncDone, signed_in, status_text};
    use crate::{RustGisApp, Slot};

    /// A fresh revision is tried this many times when the server already has the one sent (an
    /// earlier run sent it and the project wasn't saved afterwards).
    const MAX_REVISION_TRIES: u32 = 5;

    fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
        m.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// The pass on this computer, shared with the worker threads.
    pub struct Account {
        gate: Gate,
        /// `None` until the stored pass has been read.
        access: Mutex<Option<Access>>,
        /// The access token from the last sign-in or renewal (kept in memory only). Held while
        /// the refresh token is used, so only one renewal runs at a time.
        token: Mutex<Option<String>>,
    }

    impl Account {
        fn new(dir: &Path) -> Result<Self, String> {
            Ok(Account {
                gate: Gate {
                    machine: Box::new(SystemMachineId),
                    verifier: Box::new(keys()?),
                    store: Box::new(FilePassStore::new(dir)),
                    clock: Box::new(SystemClock),
                },
                access: Mutex::new(None),
                token: Mutex::new(None),
            })
        }

        pub fn access(&self) -> Option<Access> {
            lock(&self.access).clone()
        }

        fn set(&self, access: Access) {
            *lock(&self.access) = Some(access);
        }

        /// Keeps a grant from signing in or renewing. Called with the token lock held.
        fn accept(&self, token: &mut Option<String>, grant: DeviceGrant) -> Result<(), String> {
            let access_token = grant.access_token.clone();
            match self.gate.accept_grant(&grant.into()) {
                Ok(a) => {
                    self.set(a);
                    *token = Some(access_token);
                    Ok(())
                }
                Err(reason) => {
                    self.set(Access::Free(reason));
                    *token = None;
                    Err(tl!(reason.message()).to_string())
                }
            }
        }

        /// Renews the pass with the stored refresh token and returns a new access token. A refresh
        /// token the server no longer accepts signs this computer out here too.
        fn renew(&self, client: &CloudClient) -> Result<String, AccountError> {
            let mut token = lock(&self.token);
            let refresh = self.gate.store.load().ok().flatten().and_then(|s| s.refresh_token).ok_or(AccountError::Revoked)?;
            let machine = self.gate.machine_id().map_err(|e| AccountError::Server(e.to_string()))?;
            match client.renew_device(&refresh, &machine) {
                Ok(grant) => {
                    self.accept(&mut token, grant).map_err(AccountError::Server)?;
                    token.clone().ok_or(AccountError::Revoked)
                }
                Err(AccountError::Revoked) => {
                    let _ = self.gate.forget();
                    self.set(Access::Free(FreeReason::NotSignedIn));
                    *token = None;
                    Err(AccountError::Revoked)
                }
                Err(e) => Err(e),
            }
        }

        /// Frees this computer's seat (when the server can be reached) and forgets the pass.
        fn sign_out(&self, client: Option<&CloudClient>) -> Result<(), String> {
            let mut token = lock(&self.token);
            let refresh = self.gate.store.load().ok().flatten().and_then(|s| s.refresh_token);
            let reached = match (client, &refresh) {
                (Some(c), Some(r)) => c.sign_out_device(r).map_err(|e| e.to_string()),
                _ => Ok(()),
            };
            *token = None;
            self.gate.forget().map_err(|e| e.to_string())?;
            self.set(Access::Free(FreeReason::NotSignedIn));
            reached.map_err(|e| {
                crate::i18n::fmt(
                    "Signed out here, but the account server couldn't be told ({error}). Sign this computer out on the account page to free its seat.",
                    &[("error", &e)],
                )
            })
        }
    }

    /// The account server's public keys. A debug build also trusts `GEOFLUX_TRUSTED_KEY`
    /// (`kid:base64`), the key a local test server prints at start-up.
    fn keys() -> Result<Ed25519Keys, String> {
        if cfg!(debug_assertions)
            && let Ok(v) = std::env::var("GEOFLUX_TRUSTED_KEY")
        {
            let (kid, b64) = v.split_once(':').ok_or("GEOFLUX_TRUSTED_KEY must be kid:base64")?;
            return Ed25519Keys::from_base64(&[(kid.trim(), b64.trim())]).map_err(|e| e.to_string());
        }
        Ed25519Keys::production().map_err(|e| e.to_string())
    }

    /// The name the account page shows for this computer.
    fn device_name() -> String {
        ["COMPUTERNAME", "HOSTNAME"]
            .iter()
            .find_map(|k| std::env::var(k).ok().filter(|v| !v.trim().is_empty()))
            .or_else(|| std::fs::read_to_string("/etc/hostname").ok().map(|h| h.trim().to_string()).filter(|h| !h.is_empty()))
            .unwrap_or_else(|| "GeoGIS computer".into())
    }

    fn client(server: &str) -> Result<CloudClient, String> {
        if server.trim().is_empty() {
            return Err(tl!("Enter the account server's address first.").into());
        }
        CloudClient::new(server.trim(), &device_name()).map_err(|e| e.to_string())
    }

    /// Runs `work` on a named thread; a panic lands as an error.
    fn spawn<T: Send + 'static>(name: &str, slot: Slot<Result<T, String>>, work: impl FnOnce() -> Result<T, String> + Send + 'static) {
        let land = slot.clone();
        let started = std::thread::Builder::new().name(name.into()).spawn(move || {
            let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(work))
                .unwrap_or_else(|_| Err(tl!("That failed unexpectedly.").to_string()));
            *lock(&land) = Some(r);
        });
        if let Err(e) = started {
            *lock(&slot) = Some(Err(e.to_string()));
        }
    }

    fn take<T>(slot: &Option<Slot<T>>) -> Option<T> {
        slot.as_ref().and_then(|s| lock(s).take())
    }

    /// A sign-in waiting for the person to approve its code.
    struct SignIn {
        /// The code and page, once the server gave them.
        code: Arc<Mutex<Option<(String, String)>>>,
        opened: bool,
        cancel: Arc<AtomicBool>,
        result: Slot<Result<(), String>>,
    }

    const HASHING: u8 = 0;
    const SENDING: u8 = 1;

    /// A sync on its worker thread.
    pub struct SyncJob {
        layer_name: String,
        /// Reading the photos; its Cancel stops the upload too.
        hashing: Arc<Progress>,
        upload: Arc<Mutex<UploadProgress>>,
        stage: Arc<AtomicU8>,
        result: Slot<Result<SyncDone, String>>,
    }

    #[derive(Default)]
    pub struct State {
        pub account: Option<Arc<Account>>,
        started: bool,
        sign_in: Option<SignIn>,
        /// Signing out, or checking the stored pass.
        busy: Option<Slot<Result<String, String>>>,
        sync: Option<SyncJob>,
        sync_note: Option<(String, bool)>,
    }

    pub fn poll(app: &mut RustGisApp, ctx: &egui::Context) {
        let st = &mut app.cloud.native;
        if !st.started {
            st.started = true;
            let Some(dir) = app.services.settings_dir.clone() else { return };
            match Account::new(&dir) {
                Ok(account) => {
                    let account = Arc::new(account);
                    st.account = Some(account.clone());
                    let server = app.services.load_pref.as_ref().and_then(|f| f(SERVER_PREF)).unwrap_or_default();
                    let slot = Slot::default();
                    st.busy = Some(slot.clone());
                    let repaint = ctx.clone();
                    spawn("geoflux-account", slot, move || {
                        let r = startup(&account, &server);
                        repaint.request_repaint();
                        r
                    });
                }
                Err(e) => {
                    log::warn!("account: {e}");
                    app.cloud.note = Some((e, true));
                }
            }
        }
        if let Some(r) = take(&st.busy) {
            st.busy = None;
            match r {
                Ok(m) if m.is_empty() => {}
                Ok(m) => app.cloud.note = Some((m, false)),
                Err(e) => app.cloud.note = Some((e, true)),
            }
        }
        if let Some(r) = take(&st.sign_in.as_ref().map(|s| s.result.clone())) {
            st.sign_in = None;
            app.cloud.note = Some(match r {
                Ok(()) => (tl!("Signed in.").to_string(), false),
                Err(e) => (e, true),
            });
        }
        if let Some(s) = &mut st.sign_in
            && !s.opened
            && let Some((_, uri)) = lock(&s.code).clone()
        {
            s.opened = true;
            app.ui.open_url = Some(uri);
        }
        poll_sync(app);
    }

    /// The stored pass, renewed when it's a day old and the server can be reached.
    fn startup(account: &Account, server: &str) -> Result<String, String> {
        account.set(account.gate.startup());
        if account.gate.due_for_renewal().is_none() || server.trim().is_empty() {
            return Ok(String::new());
        }
        match client(server).and_then(|c| account.renew(&c).map_err(|e| e.to_string())) {
            Ok(_) => Ok(String::new()),
            // Offline is normal in the field: the pass works for 30 days.
            Err(e) => {
                log::info!("pass renewal: {e}");
                Ok(String::new())
            }
        }
    }

    pub fn account_ui(app: &mut RustGisApp, ui: &mut egui::Ui) {
        let Some(account) = app.cloud.native.account.clone() else {
            ui.label(tl!("Signing in isn't available in this copy of GeoGIS."));
            return;
        };
        ui.horizontal(|ui| {
            ui.label(tl!("Account server"));
            let r = ui.add_enabled(
                app.cloud.native.sign_in.is_none(),
                egui::TextEdit::singleline(&mut app.cloud.server).hint_text("https://cloud.example.com").desired_width(260.0),
            );
            if r.lost_focus()
                && let Some(save) = &app.services.save_pref
            {
                save(SERVER_PREF, app.cloud.server.trim());
            }
        });
        ui.add_space(6.0);
        let access = account.access();
        match &access {
            None => {
                ui.horizontal(|ui| {
                    ui.spinner();
                    ui.label(tl!("Checking your sign-in…"));
                });
            }
            Some(a) => {
                ui.label(status_text(a));
            }
        }
        ui.add_space(6.0);
        let busy = app.cloud.native.busy.is_some();
        if let Some(s) = &app.cloud.native.sign_in {
            match lock(&s.code).clone() {
                Some((code, uri)) => {
                    ui.label(tl!("Approve this code on the account page that opened in your browser:"));
                    ui.label(RichText::new(code).monospace().size(22.0).strong());
                    ui.horizontal(|ui| {
                        if ui.button(tl!("Open the Page Again")).clicked() {
                            app.ui.open_url = Some(uri);
                        }
                        if ui.button(tl!("Cancel")).clicked() {
                            s.cancel.store(true, Ordering::Relaxed);
                        }
                    });
                }
                None => {
                    ui.horizontal(|ui| {
                        ui.spinner();
                        ui.label(tl!("Asking the account server for a code…"));
                    });
                }
            }
            ui.ctx().request_repaint_after(Duration::from_millis(250));
            return;
        }
        ui.horizontal(|ui| {
            if signed_in(access.as_ref()) {
                if ui.add_enabled(!busy, egui::Button::new(tl!("Sign Out"))).clicked() {
                    let server = app.cloud.server.clone();
                    let slot = Slot::default();
                    app.cloud.native.busy = Some(slot.clone());
                    let repaint = ui.ctx().clone();
                    spawn("geoflux-sign-out", slot, move || {
                        let r = account.sign_out(client(&server).ok().as_ref()).map(|()| tl!("Signed out.").to_string());
                        repaint.request_repaint();
                        r
                    });
                }
            } else if ui.add_enabled(!busy && access.is_some(), egui::Button::new(tl!("Sign In…"))).clicked() {
                if let Some(save) = &app.services.save_pref {
                    save(SERVER_PREF, app.cloud.server.trim());
                }
                match client(&app.cloud.server) {
                    Ok(c) => {
                        app.cloud.note = None;
                        app.cloud.native.sign_in = Some(start_sign_in(account, c, ui.ctx().clone()));
                    }
                    Err(e) => app.cloud.note = Some((e, true)),
                }
            }
        });
        if busy {
            ui.ctx().request_repaint_after(Duration::from_millis(250));
        }
    }

    /// Asks for a code, then waits for the person to approve it.
    fn start_sign_in(account: Arc<Account>, client: CloudClient, repaint: egui::Context) -> SignIn {
        let s = SignIn { code: Arc::default(), opened: false, cancel: Arc::default(), result: Slot::default() };
        let (code_out, cancel) = (s.code.clone(), s.cancel.clone());
        spawn("geoflux-sign-in", s.result.clone(), move || {
            let r = sign_in(&account, &client, &code_out, &cancel);
            repaint.request_repaint();
            r
        });
        s
    }

    fn sign_in(
        account: &Account,
        client: &CloudClient,
        code_out: &Mutex<Option<(String, String)>>,
        cancel: &AtomicBool,
    ) -> Result<(), String> {
        let machine = account.gate.machine_id().map_err(|e| e.to_string())?;
        let code = client.device_code(&machine).map_err(|e| e.to_string())?;
        *lock(code_out) = Some((code.user_code.clone(), code.verification_uri_complete.clone()));
        let deadline = Instant::now() + Duration::from_secs(code.expires_in.clamp(60, 3600));
        let mut interval = code.interval.clamp(1, 60);
        loop {
            // Waits in short steps so Cancel answers at once.
            let until = Instant::now() + Duration::from_secs(interval);
            while Instant::now() < until {
                if cancel.load(Ordering::Relaxed) {
                    return Err(tl!("Sign-in cancelled.").into());
                }
                std::thread::sleep(Duration::from_millis(100));
            }
            if Instant::now() >= deadline {
                return Err(tl!("The code expired before it was approved. Sign in again.").into());
            }
            match client.poll_device(&code.device_code) {
                Ok(DevicePoll::Pending) => {}
                Ok(DevicePoll::SlowDown(s)) => interval = s.clamp(interval + 5, 60),
                Ok(DevicePoll::Granted(grant)) => {
                    let mut token = lock(&account.token);
                    return account.accept(&mut token, grant);
                }
                Ok(DevicePoll::Denied) => return Err(tl!("The sign-in was declined on the account page.").into()),
                Ok(DevicePoll::Expired) => return Err(tl!("The code expired before it was approved. Sign in again.").into()),
                // A dropped connection mid-wait: keep asking until the code expires.
                Err(AccountError::Offline(e)) => log::info!("sign-in poll: {e}"),
                Err(e) => return Err(e.to_string()),
            }
        }
    }

    /// Hands uploads the access token, renewing when the server asks for a fresh one.
    struct Tokens<'a> {
        account: &'a Account,
        client: &'a CloudClient,
    }

    impl AccessTokens for Tokens<'_> {
        fn access_token(&mut self, fresh: bool) -> Result<String, AccountError> {
            if !fresh && let Some(t) = lock(&self.account.token).clone() {
                return Ok(t);
            }
            self.account.renew(self.client)
        }
    }

    /// Plans the active inspection layer's survey (on this thread: text only) and sends it on a
    /// worker thread.
    pub fn start_sync(app: &mut RustGisApp) -> Result<(), String> {
        if app.cloud.native.sync.is_some() {
            return Err(tl!("A sync is already running.").into());
        }
        let account = app.cloud.native.account.clone().ok_or_else(|| tl!("GeoGIS has no settings folder to keep a sign-in in."))?;
        if !signed_in(account.access().as_ref()) {
            app.cloud.open = true;
            return Err(tl!("Sign in to your GeoFlux account first.").into());
        }
        let client = client(&app.cloud.server).inspect_err(|_| app.cloud.open = true)?;
        let layer = app
            .inspection
            .layer
            .or(app.session.active_layer)
            .ok_or_else(|| tl!("Open the Inspection pane on a tower layer first.").to_string())?;
        let layer_name = app.session.map().layer(layer).map(|l| l.name.clone()).unwrap_or_default();
        let link = cloud_sync::ensure_link(&mut app.session);
        let plan = cloud_sync::plan(&app.session, layer)?;
        cloud_sync::attempted(&mut app.session, &link.survey, plan.revision);
        let job = SyncJob {
            layer_name,
            hashing: Progress::new(),
            upload: Arc::default(),
            stage: Arc::new(AtomicU8::new(HASHING)),
            result: Slot::default(),
        };
        let (hashing, upload, stage) = (job.hashing.clone(), job.upload.clone(), job.stage.clone());
        spawn("geoflux-sync", job.result.clone(), move || sync(&account, &client, plan, &hashing, &upload, &stage));
        app.cloud.native.sync = Some(job);
        app.cloud.native.sync_note = None;
        Ok(())
    }

    fn sync(
        account: &Account,
        client: &CloudClient,
        plan: cloud_sync::SyncPlan,
        hashing: &Progress,
        upload: &Mutex<UploadProgress>,
        stage: &AtomicU8,
    ) -> Result<SyncDone, String> {
        let survey = plan.survey.clone();
        let towers = plan.assets.len();
        let cloud_sync::Prepared { mut manifest, files, skipped } = cloud_sync::prepare(plan, hashing)?;
        stage.store(SENDING, Ordering::Relaxed);
        let find = |sha: &geoflux_cloud::Sha256| files.get(sha).map(PathBuf::from);
        let mut tokens = Tokens { account, client };
        for _ in 0..MAX_REVISION_TRIES {
            let r = client.upload(&mut tokens, &manifest, &find, &mut |p| {
                *lock(upload) = p;
                !hashing.is_cancelled()
            });
            match r {
                Ok(done) if !done.existing => {
                    return Ok(SyncDone {
                        survey,
                        revision: manifest.revision,
                        towers,
                        sent: done.sent,
                        photos: done.survey.photos,
                        skipped,
                    });
                }
                // The server already has this revision from a run whose project wasn't saved: send
                // the next one (photos it has aren't sent again).
                Ok(_) => {}
                Err(SyncError::Photo { sha, .. }) if !files.contains_key(&sha) => {}
                Err(SyncError::Cancelled) => return Err("cancelled".into()),
                Err(e) => return Err(e.to_string()),
            }
            manifest.revision = manifest.revision.saturating_add(1);
        }
        Err(tl!("The account server keeps refusing new revisions of this project's survey.").into())
    }

    fn poll_sync(app: &mut RustGisApp) {
        let Some(r) = take(&app.cloud.native.sync.as_ref().map(|j| j.result.clone())) else { return };
        let Some(job) = app.cloud.native.sync.take() else { return };
        let note = match r {
            Ok(done) => {
                cloud_sync::committed(&mut app.session, &done.survey, done.revision);
                let mut text = crate::i18n::fmt(
                    "Sent {layer}: {towers} towers and {photos} photos ({sent} new). Save the project to keep its link to the cloud survey.",
                    &[
                        ("layer", &job.layer_name),
                        ("towers", &done.towers.to_string()),
                        ("photos", &done.photos.to_string()),
                        ("sent", &done.sent.to_string()),
                    ],
                );
                if !done.skipped.is_empty() {
                    text.push_str(&crate::i18n::fmt(" {n} left out:", &[("n", &done.skipped.len().to_string())]));
                    for s in done.skipped.iter().take(5) {
                        text.push_str("\n• ");
                        text.push_str(s);
                    }
                }
                (text, false)
            }
            Err(e) if e == "cancelled" => {
                (tl!("Sync cancelled. Sync again to send the rest; photos already sent aren't sent twice.").to_string(), false)
            }
            Err(e) => (e, true),
        };
        app.session.notify(note.0.lines().next().unwrap_or_default().to_string(), note.1);
        app.cloud.native.sync_note = Some(note);
    }

    /// The Sync to Cloud window: progress while a sync runs, then how it went.
    pub fn sync_window(app: &mut RustGisApp, ctx: &egui::Context) {
        let st = &mut app.cloud.native;
        if st.sync.is_none() && st.sync_note.is_none() {
            return;
        }
        let mut open = true;
        egui::Window::new(tl!("Sync to Cloud"))
            .open(&mut open)
            .collapsible(false)
            .resizable(false)
            .anchor(Align2::RIGHT_BOTTOM, vec2(-16.0, -40.0))
            .show(ctx, |ui| {
                ui.set_width(360.0);
                if let Some(job) = &st.sync {
                    ui.label(RichText::new(&job.layer_name).strong());
                    let (frac, text) = if job.stage.load(Ordering::Relaxed) == HASHING {
                        let (done, total) = job.hashing.get();
                        let f = if total == 0 { 0.0 } else { done as f32 / total as f32 };
                        (
                            f,
                            crate::i18n::fmt(
                                "Reading photos: {done} of {total}",
                                &[("done", &done.to_string()), ("total", &total.to_string())],
                            ),
                        )
                    } else {
                        let p = *lock(&job.upload);
                        let f = if p.bytes == 0 { 1.0 } else { p.bytes_sent as f32 / p.bytes as f32 };
                        let mb = |b: u64| format!("{:.1}", b as f64 / 1_048_576.0);
                        (
                            f,
                            crate::i18n::fmt(
                                "Sending photos: {sent} of {photos} ({done} of {total} MB)",
                                &[
                                    ("sent", &p.sent.to_string()),
                                    ("photos", &p.photos.to_string()),
                                    ("done", &mb(p.bytes_sent)),
                                    ("total", &mb(p.bytes)),
                                ],
                            ),
                        )
                    };
                    ui.add(egui::ProgressBar::new(frac).text(text));
                    if job.hashing.is_cancelled() {
                        ui.label(RichText::new(tl!("Stopping…")).small().weak());
                    } else if ui.small_button(tl!("Cancel")).clicked() {
                        job.hashing.cancel();
                    }
                    ui.ctx().request_repaint_after(Duration::from_millis(200));
                } else if let Some((m, err)) = &st.sync_note {
                    let t = crate::theme::Tokens::get(ui.ctx());
                    ui.label(RichText::new(m.as_str()).color(if *err { t.red } else { t.text }));
                }
            });
        // Closing the window doesn't stop a running sync; it closes the result.
        if !open && st.sync.is_none() {
            st.sync_note = None;
        }
    }
}

#[cfg(test)]
mod tests {
    use geoflux_licence::FreeReason;

    use super::*;

    #[test]
    fn paid_commands_name_their_feature_and_free_ones_none() {
        assert_eq!(feature_of("geoflux.report"), Some(Feature::Reports));
        assert_eq!(feature_of("ui.geofluxReportBulk"), Some(Feature::BulkReports));
        assert_eq!(feature_of("geoflux.thermal.measure"), Some(Feature::Thermal));
        assert_eq!(feature_of("ui.thermal.view"), Some(Feature::Thermal));
        assert_eq!(feature_of("ui.cloudSync"), Some(Feature::CloudSync));
        assert_eq!(feature_of("geoflux.annotate"), Some(Feature::Markup));
        for free in ["geoflux.importCsv", "geoflux.linkPhotos", "geoflux.photos", "ui.pane.inspection", "ui.account", "map.zoomIn"] {
            assert_eq!(feature_of(free), None, "{free}");
        }
    }

    #[test]
    fn nothing_is_locked_until_enforced() {
        let app = RustGisApp::new(rustgis_engine::Session::new(rustgis_engine::Project::default()));
        assert_eq!(check(&app, "geoflux.report").is_ok(), !ENFORCE);
        assert!(check(&app, "map.zoomIn").is_ok());
    }

    #[test]
    fn the_free_tier_says_why() {
        assert!(status_text(&Access::Free(FreeReason::Expired)).contains("30 days"));
    }
}
