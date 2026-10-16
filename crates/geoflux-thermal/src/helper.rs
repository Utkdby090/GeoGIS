//! The desktop [`DjiBackend`]: the `geoflux-dirp` helper program, started on first use and kept
//! while photos are read.
//!
//! - **Time limit.** Each photo has [`HelperConfig::photo_timeout`]. A helper that overruns it is
//!   killed and the photo fails with [`ThermalError::HelperTimeout`].
//! - **Crash isolation.** A helper that dies mid-photo (a crash inside DJI's library) costs that
//!   photo only ([`ThermalError::HelperCrashed`]); the next photo starts a fresh helper.
//! - **No stuck threads.** The pipes are served by two threads per helper, which end when it does,
//!   so a helper that stops reading or writing can never block the caller past the time limit.
//! - **One photo at a time** per helper; callers on other threads wait their turn.

use std::io::{BufReader, BufWriter};
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, SyncSender};
use std::sync::{Mutex, TryLockError};
use std::time::Duration;

use crate::dji::{DjiBackend, DjiMeasurement};
use crate::error::ThermalError;
use crate::model::SceneOverrides;
use crate::protocol::{self, MAX_TEMPS, PROTOCOL_VERSION, Request, Response};

/// Where the helper and DJI's libraries are, and how long a photo may take.
#[derive(Debug, Clone)]
pub struct HelperConfig {
    pub program: PathBuf,
    /// The folder with `libdirp` and the rest of DJI's library set.
    pub sdk_dir: PathBuf,
    pub photo_timeout: Duration,
    pub start_timeout: Duration,
}

impl HelperConfig {
    /// The helper and `dji-sdk` folder installed next to the running program, unless the
    /// `GEOFLUX_DIRP` and `GEOFLUX_DJI_SDK` environment variables name others (for development).
    pub fn beside_app() -> Self {
        let exe_dir = std::env::current_exe().ok().and_then(|p| p.parent().map(|d| d.to_path_buf())).unwrap_or_default();
        let name = if cfg!(windows) { "geoflux-dirp.exe" } else { "geoflux-dirp" };
        Self {
            program: std::env::var_os("GEOFLUX_DIRP").map_or_else(|| exe_dir.join(name), PathBuf::from),
            sdk_dir: std::env::var_os("GEOFLUX_DJI_SDK").map_or_else(|| exe_dir.join("dji-sdk"), PathBuf::from),
            photo_timeout: Duration::from_secs(20),
            start_timeout: Duration::from_secs(10),
        }
    }
}

type Message = (Response, Vec<u8>);

/// A running helper: the child, a writer thread fed through `to_helper`, and a reader thread
/// that forwards each answer to `from_helper`.
struct Running {
    child: Child,
    to_helper: SyncSender<(Request, Vec<u8>)>,
    from_helper: Receiver<Result<Message, ThermalError>>,
    next_id: u64,
}

impl Running {
    fn start(cfg: &HelperConfig) -> Result<Self, ThermalError> {
        let unavailable = |why: String| ThermalError::HelperUnavailable(why);
        if !cfg.program.is_file() {
            return Err(unavailable(format!("the thermal helper isn't installed ({})", cfg.program.display())));
        }
        let lib = if cfg!(windows) { "libdirp.dll" } else { "libdirp.so" };
        if !cfg.sdk_dir.join(lib).is_file() {
            return Err(unavailable(format!("DJI's thermal library isn't installed ({})", cfg.sdk_dir.display())));
        }
        let mut cmd = Command::new(&cfg.program);
        cmd.arg("--sdk").arg(&cfg.sdk_dir).current_dir(&cfg.sdk_dir);
        cmd.stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::null());
        // DJI's library finds its helper libraries on the search path, which only a new process
        // can be given; this is one more reason the SDK lives in its own program.
        let (var, sep) = if cfg!(windows) { ("PATH", ";") } else { ("LD_LIBRARY_PATH", ":") };
        let mut search = cfg.sdk_dir.as_os_str().to_os_string();
        if let Some(old) = std::env::var_os(var).filter(|v| !v.is_empty()) {
            search.push(sep);
            search.push(old);
        }
        cmd.env(var, search);
        hide_window(&mut cmd);
        let mut child = cmd.spawn().map_err(|e| unavailable(format!("the thermal helper could not start ({e})")))?;
        let (Some(stdin), Some(stdout)) = (child.stdin.take(), child.stdout.take()) else {
            let _ = child.kill();
            let _ = child.wait();
            return Err(unavailable("the thermal helper's pipes could not be opened".into()));
        };

        // Capacity 0: a request is handed over only when the writer is ready for it.
        let (to_helper, requests) = mpsc::sync_channel::<(Request, Vec<u8>)>(0);
        let spawned_writer = std::thread::Builder::new().name("geoflux-dirp-write".into()).spawn(move || {
            let mut w = BufWriter::new(stdin);
            for (req, payload) in requests {
                if protocol::write_message(&mut w, &req, &payload).is_err() {
                    break; // the helper is gone; the reader reports it
                }
            }
            // Dropping `w` closes stdin, which tells the helper to exit.
        });
        let (answers, from_helper) = mpsc::sync_channel::<Result<Message, ThermalError>>(1);
        let spawned_reader = std::thread::Builder::new().name("geoflux-dirp-read".into()).spawn(move || {
            let mut r = BufReader::new(stdout);
            loop {
                let msg = protocol::read_message::<_, Response>(&mut r, MAX_TEMPS);
                let end = !matches!(msg, Ok(Some(_)));
                let sent = match msg {
                    Ok(Some(m)) => answers.send(Ok(m)),
                    Ok(None) => answers.send(Err(ThermalError::HelperCrashed)),
                    Err(e) => answers.send(Err(e)),
                };
                if end || sent.is_err() {
                    break;
                }
            }
        });
        let mut running = Self { child, to_helper, from_helper, next_id: 1 };
        if spawned_writer.is_err() || spawned_reader.is_err() {
            running.stop();
            return Err(unavailable("the thermal helper's threads could not start".into()));
        }
        match running.exchange(Request::Hello { protocol: PROTOCOL_VERSION }, Vec::new(), cfg.start_timeout) {
            Ok((Response::Hello { protocol, .. }, _)) if protocol == PROTOCOL_VERSION => Ok(running),
            Ok((Response::Hello { protocol, .. }, _)) => {
                running.stop();
                Err(unavailable(format!("the thermal helper speaks version {protocol}, not {PROTOCOL_VERSION}; reinstall GeoGIS")))
            }
            Ok(_) => {
                running.stop();
                Err(unavailable("the thermal helper did not introduce itself".into()))
            }
            Err(e) => {
                running.stop();
                // It started but died at once: almost always DJI's libraries failing to load.
                Err(match e {
                    ThermalError::HelperCrashed | ThermalError::HelperTimeout => {
                        unavailable("DJI's thermal library could not be loaded (a file may be missing)".into())
                    }
                    other => other,
                })
            }
        }
    }

    /// Sends one request and waits at most `limit` for its answer.
    fn exchange(&mut self, req: Request, payload: Vec<u8>, limit: Duration) -> Result<Message, ThermalError> {
        let deadline = std::time::Instant::now() + limit;
        // The writer takes the request only when free; if it is stuck on a full pipe, this waits
        // in short steps until the deadline instead of blocking forever.
        let mut pending = Some((req, payload));
        while let Some(item) = pending.take() {
            match self.to_helper.try_send(item) {
                Ok(()) => {}
                Err(mpsc::TrySendError::Full(item)) => {
                    if std::time::Instant::now() >= deadline {
                        return Err(ThermalError::HelperTimeout);
                    }
                    std::thread::sleep(Duration::from_millis(2));
                    pending = Some(item);
                }
                Err(mpsc::TrySendError::Disconnected(_)) => return Err(ThermalError::HelperCrashed),
            }
        }
        let left = deadline.saturating_duration_since(std::time::Instant::now());
        match self.from_helper.recv_timeout(left) {
            Ok(r) => r,
            Err(RecvTimeoutError::Timeout) => Err(ThermalError::HelperTimeout),
            Err(RecvTimeoutError::Disconnected) => Err(ThermalError::HelperCrashed),
        }
    }

    /// Kills the helper and reaps it; its pipe threads end as the pipes close.
    fn stop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

#[cfg(windows)]
fn hide_window(cmd: &mut Command) {
    use std::os::windows::process::CommandExt;
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    cmd.creation_flags(CREATE_NO_WINDOW);
}

#[cfg(not(windows))]
fn hide_window(_: &mut Command) {}

/// The DJI backend over the helper program.
pub struct HelperBackend {
    cfg: HelperConfig,
    state: Mutex<State>,
    /// A [`HelperBackend::stop`] that came while a photo was being measured: the photo's thread
    /// stops the helper when it finishes, so the caller (the UI thread) never waits for it.
    stop_after: AtomicBool,
}

#[derive(Default)]
struct State {
    running: Option<Running>,
    /// Why the helper can't run here, once found out, so every DJI photo doesn't retry a missing
    /// install. Cleared by [`HelperBackend::stop`].
    unavailable: Option<ThermalError>,
}

impl HelperBackend {
    pub fn new(cfg: HelperConfig) -> Self {
        Self { cfg, state: Mutex::new(State::default()), stop_after: AtomicBool::new(false) }
    }

    /// Stops the helper (when the Inspection pane closes); the next DJI photo starts it again.
    /// Never waits: while a photo is being measured, the helper is stopped as soon as it's done.
    pub fn stop(&self) {
        match self.state.try_lock() {
            Ok(mut st) => Self::stop_locked(&mut st),
            Err(TryLockError::Poisoned(e)) => Self::stop_locked(&mut e.into_inner()),
            Err(TryLockError::WouldBlock) => self.stop_after.store(true, Ordering::Release),
        }
    }

    fn stop_locked(st: &mut State) {
        if let Some(mut r) = st.running.take() {
            r.stop();
        }
        st.unavailable = None;
    }

    /// Whether a photo is being measured now.
    #[cfg(test)]
    fn is_busy(&self) -> bool {
        matches!(self.state.try_lock(), Err(TryLockError::WouldBlock))
    }

    /// Whether a helper is running now (for the status bar and tests).
    pub fn is_running(&self) -> bool {
        self.state.lock().unwrap_or_else(|e| e.into_inner()).running.is_some()
    }
}

impl Drop for HelperBackend {
    fn drop(&mut self) {
        Self::stop_locked(self.state.get_mut().unwrap_or_else(|e| e.into_inner()));
    }
}

impl DjiBackend for HelperBackend {
    fn measure(&self, jpeg: &[u8], overrides: &SceneOverrides) -> Result<DjiMeasurement, ThermalError> {
        if jpeg.len() as u64 > u64::from(protocol::MAX_JPEG) {
            return Err(ThermalError::Damaged("the file is too large for a thermal photo".into()));
        }
        // A poisoned lock only means another photo's thread panicked; the state is still sound.
        let mut st = self.state.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(e) = &st.unavailable {
            return Err(e.clone());
        }
        if st.running.is_none() {
            match Running::start(&self.cfg) {
                Ok(r) => st.running = Some(r),
                Err(e) => {
                    st.unavailable = Some(e.clone());
                    return Err(e);
                }
            }
        }
        let Some(run) = st.running.as_mut() else { return Err(ThermalError::HelperCrashed) };
        let id = run.next_id;
        run.next_id += 1;
        let result = run.exchange(Request::Measure { id, overrides: *overrides }, jpeg.to_vec(), self.cfg.photo_timeout);
        let answer = match result {
            Ok((Response::Measured { id: got, width, height, applied, refused, camera }, payload)) if got == id => {
                let count = u64::from(width) * u64::from(height);
                match protocol::temps_from_bytes(&payload, count) {
                    Some(temps) => Ok(DjiMeasurement { width, height, temps, applied, refused, camera }),
                    None => Err(ThermalError::Protocol("the temperatures don't match the image size".into())),
                }
            }
            Ok((Response::Failed { id: got, code, message }, _)) if got == id => Err(Response::failure(code, &message)),
            Ok(_) => Err(ThermalError::Protocol("an answer for a different photo".into())),
            Err(e) => Err(e),
        };
        if let Err(e) = &answer
            && e.restarts_helper()
            && let Some(mut r) = st.running.take()
        {
            r.stop();
        }
        if self.stop_after.swap(false, Ordering::AcqRel) {
            Self::stop_locked(&mut st);
        }
        answer
    }
}

#[cfg(all(test, unix))]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use crate::model::SceneOverrides;

    /// A stand-in helper: a shell script, in a folder that also holds an (empty) `libdirp.so` so
    /// the install check passes.
    fn fake(name: &str, script: &str) -> HelperConfig {
        use std::os::unix::fs::PermissionsExt;
        let dir = std::env::temp_dir().join(format!("geoflux-helper-test-{name}-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("libdirp.so"), b"").unwrap();
        let program = dir.join("helper.sh");
        std::fs::write(&program, format!("#!/bin/sh\n{script}\n")).unwrap();
        std::fs::set_permissions(&program, std::fs::Permissions::from_mode(0o755)).unwrap();
        HelperConfig { program, sdk_dir: dir, photo_timeout: Duration::from_millis(500), start_timeout: Duration::from_millis(500) }
    }

    const JPEG: &[u8] = b"\xFF\xD8\xFF\xD9";

    #[test]
    fn a_missing_install_is_reported_once_and_not_retried_per_photo() {
        let cfg = HelperConfig {
            program: "/nonexistent/geoflux-dirp".into(),
            sdk_dir: "/nonexistent".into(),
            photo_timeout: Duration::from_secs(1),
            start_timeout: Duration::from_secs(1),
        };
        let b = HelperBackend::new(cfg);
        assert!(matches!(b.measure(JPEG, &SceneOverrides::default()), Err(ThermalError::HelperUnavailable(_))));
        assert!(matches!(b.measure(JPEG, &SceneOverrides::default()), Err(ThermalError::HelperUnavailable(_))));
        assert!(!b.is_running());
    }

    #[test]
    fn a_helper_that_dies_or_hangs_at_start_is_unavailable_not_stuck() {
        for (name, script) in [("dies", "exit 3"), ("hangs", "exec sleep 30"), ("babbles", "exec cat")] {
            let b = HelperBackend::new(fake(name, script));
            let started = std::time::Instant::now();
            let r = b.measure(JPEG, &SceneOverrides::default());
            assert!(matches!(r, Err(ThermalError::HelperUnavailable(_)) | Err(ThermalError::Protocol(_))), "{name}: {r:?}");
            assert!(started.elapsed() < Duration::from_secs(5), "{name} took {:?}", started.elapsed());
            assert!(!b.is_running(), "{name}");
        }
    }

    /// A script that introduces itself properly, swallows GeoGIS's hello, then runs `then`.
    fn greets_then(name: &str, then: &str) -> HelperConfig {
        let hello = r#"{"type":"hello","protocol":1,"helper":"t","sdk":"t"}"#;
        let head = format!("GFD1\\{:03o}\\000\\000\\000", hello.len());
        fake(name, &format!("printf '{head}{hello}\\000\\000\\000\\000'\nhead -c 64 >/dev/null\n{then}"))
    }

    #[test]
    fn a_crash_or_hang_mid_photo_costs_that_photo_only() {
        for (name, then, want) in
            [("crash", "head -c 8 >/dev/null; exit 1", ThermalError::HelperCrashed), ("hang", "exec sleep 30", ThermalError::HelperTimeout)]
        {
            let b = HelperBackend::new(greets_then(name, then));
            let started = std::time::Instant::now();
            assert_eq!(b.measure(JPEG, &SceneOverrides::default()), Err(want.clone()), "{name}");
            assert!(started.elapsed() < Duration::from_secs(5), "{name} took {:?}", started.elapsed());
            assert!(!b.is_running(), "{name}: the helper is replaced");
            // The next photo gets a fresh helper (which fails the same way here, but runs).
            assert_eq!(b.measure(JPEG, &SceneOverrides::default()), Err(want), "{name} again");
        }
    }

    #[test]
    fn stop_never_waits_for_a_photo_in_flight() {
        let mut cfg = greets_then("stop", "exec sleep 30");
        cfg.photo_timeout = Duration::from_secs(3);
        let b = std::sync::Arc::new(HelperBackend::new(cfg));
        let worker = b.clone();
        let photo = std::thread::spawn(move || worker.measure(JPEG, &SceneOverrides::default()));
        // Let the photo take the lock, then stop from "the UI thread".
        let until = std::time::Instant::now() + Duration::from_secs(2);
        while !b.is_busy() && std::time::Instant::now() < until {
            std::thread::sleep(Duration::from_millis(10));
        }
        let started = std::time::Instant::now();
        b.stop();
        assert!(started.elapsed() < Duration::from_millis(200), "stop waited {:?}", started.elapsed());
        let _ = photo.join().unwrap();
        assert!(!b.is_running(), "the deferred stop happened when the photo ended");
    }

    /// The real helper on DJI's Linux libraries: set `GEOFLUX_DIRP` and `GEOFLUX_DJI_SDK` to run it.
    #[test]
    fn the_real_helper_refuses_a_non_dji_file_and_keeps_running() {
        let (Some(program), Some(sdk)) = (std::env::var_os("GEOFLUX_DIRP"), std::env::var_os("GEOFLUX_DJI_SDK")) else { return };
        let b = HelperBackend::new(HelperConfig {
            program: program.into(),
            sdk_dir: sdk.into(),
            photo_timeout: Duration::from_secs(20),
            start_timeout: Duration::from_secs(10),
        });
        let mut junk = b"\xFF\xD8".to_vec();
        junk.extend((0..20_000u32).map(|i| (i.wrapping_mul(2_654_435_761) >> 24) as u8));
        for _ in 0..3 {
            assert_eq!(b.measure(&junk, &SceneOverrides::default()), Err(ThermalError::NotRadiometric));
        }
        assert!(b.is_running(), "a refused photo must not cost the helper");
        b.stop();
        assert!(!b.is_running());
    }
}
