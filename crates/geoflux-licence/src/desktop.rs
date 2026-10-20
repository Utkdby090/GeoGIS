//! The desktop's own parts for the [`Gate`](crate::Gate): reading this computer's ID, and keeping
//! the pass between runs.
//!
//! | | Computer ID | Refresh token (the secret) | Pass and last run |
//! |---|---|---|---|
//! | Windows | `Win32_ComputerSystemProduct` UUID, as GeoFlux read it | Credential Manager | `pass.json` |
//! | macOS | `IOPlatformUUID` | Keychain | `pass.json` |
//! | Linux | `/etc/machine-id` | `pass.json`, readable by the user only | `pass.json` |
//!
//! `pass.json` lives in GeoGIS's settings folder. The pass itself is signed, so it needs no hiding.

use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

use crate::gate::{Clock, PassStore, StoredPass};
use crate::{LicenceError, MachineId, MachineIdSource};

/// How long an ID command may take (PowerShell can take a few seconds to start on a cold machine).
const ID_TIMEOUT: Duration = Duration::from_secs(20);
/// The longest output read from an ID command.
const MAX_ID_OUTPUT: u64 = 64 * 1024;

/// Reads this computer's ID with the system's own tools, hidden, with a time limit.
#[derive(Clone, Copy, Debug, Default)]
pub struct SystemMachineId;

impl MachineIdSource for SystemMachineId {
    fn machine_id(&self) -> Result<MachineId, LicenceError> {
        #[cfg(windows)]
        {
            let mut last = LicenceError::MachineId("no command worked".into());
            for args in crate::machine::WINDOWS_ID_COMMANDS {
                match run(args).and_then(|out| {
                    if args.first() == Some(&"wmic") { MachineId::from_wmic_output(&out) } else { MachineId::from_output(&out) }
                }) {
                    Ok(id) => return Ok(id),
                    Err(e) => last = e,
                }
            }
            Err(last)
        }
        #[cfg(target_os = "macos")]
        {
            let out = run(&["ioreg", "-rd1", "-c", "IOPlatformExpertDevice"])?;
            let line =
                out.lines().find(|l| l.contains("IOPlatformUUID")).ok_or_else(|| LicenceError::MachineId("no IOPlatformUUID".into()))?;
            let uuid = line.rsplit('=').next().unwrap_or_default().trim().trim_matches('"');
            MachineId::from_output(uuid)
        }
        #[cfg(not(any(windows, target_os = "macos")))]
        {
            let text = std::fs::read_to_string("/etc/machine-id")
                .or_else(|_| std::fs::read_to_string("/var/lib/dbus/machine-id"))
                .map_err(|e| LicenceError::MachineId(e.to_string()))?;
            MachineId::from_output(&text)
        }
    }
}

/// The computer's clock.
#[derive(Clone, Copy, Debug, Default)]
pub struct SystemClock;

impl Clock for SystemClock {
    fn now(&self) -> i64 {
        std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| i64::try_from(d.as_secs()).unwrap_or(i64::MAX))
    }
}

/// Runs `args` hidden (no console window flashes on Windows) and returns its output, killing it
/// after [`ID_TIMEOUT`].
#[cfg_attr(not(any(windows, target_os = "macos")), allow(dead_code))]
fn run(args: &[&str]) -> Result<String, LicenceError> {
    let fail = |e: String| LicenceError::MachineId(e);
    let (program, rest) = args.split_first().ok_or_else(|| fail("no command".into()))?;
    let mut cmd = Command::new(program);
    cmd.args(rest).stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::null());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
    let mut child = cmd.spawn().map_err(|e| fail(format!("{program}: {e}")))?;
    let mut out = child.stdout.take().ok_or_else(|| fail("no output".into()))?;
    // Read on a thread so a command that never ends can be killed after the time limit.
    let reader = std::thread::spawn(move || {
        let mut text = String::new();
        let _ = (&mut out).take(MAX_ID_OUTPUT).read_to_string(&mut text);
        text
    });
    let started = Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(_)) => break,
            Ok(None) if started.elapsed() < ID_TIMEOUT => std::thread::sleep(Duration::from_millis(50)),
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(fail(format!("{program} took too long")));
            }
        }
    }
    reader.join().map_err(|_| fail("reading the output failed".into()))
}

/// What `pass.json` holds.
#[derive(Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct PassFile {
    pass: String,
    /// Only where the system has no secret store (Linux); the file is then the user's alone.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    refresh_token: Option<String>,
    /// Whether a refresh token is kept in the system's secret store.
    #[serde(default)]
    token_in_keychain: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    last_seen: Option<i64>,
}

/// The pass in `<settings>/pass.json`, the refresh token in the system's secret store.
#[derive(Clone, Debug)]
pub struct FilePassStore {
    path: PathBuf,
    /// The secret store entry's service name (tests use their own).
    #[cfg_attr(not(any(windows, target_os = "macos")), allow(dead_code))]
    service: String,
}

impl FilePassStore {
    /// Keeps the pass in `dir` (GeoGIS's settings folder).
    pub fn new(dir: &Path) -> Self {
        Self::with_service(dir, "GeoGIS")
    }

    pub fn with_service(dir: &Path, service: &str) -> Self {
        FilePassStore { path: dir.join("pass.json"), service: service.to_string() }
    }

    fn storage(e: impl std::fmt::Display) -> LicenceError {
        LicenceError::Storage(e.to_string())
    }
}

#[cfg(any(windows, target_os = "macos"))]
mod secret {
    use super::{FilePassStore, LicenceError};

    const USER: &str = "GeoFlux Cloud refresh token";

    fn entry(store: &FilePassStore) -> Result<keyring::Entry, LicenceError> {
        keyring::Entry::new(&store.service, USER).map_err(FilePassStore::storage)
    }

    pub fn save(store: &FilePassStore, token: &str) -> Result<bool, LicenceError> {
        entry(store)?.set_password(token).map_err(FilePassStore::storage)?;
        Ok(true)
    }

    pub fn load(store: &FilePassStore) -> Result<Option<String>, LicenceError> {
        match entry(store)?.get_password() {
            Ok(t) => Ok(Some(t)),
            Err(keyring::Error::NoEntry) => Ok(None),
            Err(e) => Err(FilePassStore::storage(e)),
        }
    }

    pub fn clear(store: &FilePassStore) -> Result<(), LicenceError> {
        match entry(store)?.delete_credential() {
            Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
            Err(e) => Err(FilePassStore::storage(e)),
        }
    }
}

#[cfg(not(any(windows, target_os = "macos")))]
mod secret {
    use super::{FilePassStore, LicenceError};

    /// No secret store: the token goes in the file, which only the user can read.
    pub fn save(_: &FilePassStore, _: &str) -> Result<bool, LicenceError> {
        Ok(false)
    }
    pub fn load(_: &FilePassStore) -> Result<Option<String>, LicenceError> {
        Ok(None)
    }
    pub fn clear(_: &FilePassStore) -> Result<(), LicenceError> {
        Ok(())
    }
}

/// Writes `bytes` to `path` through a temporary file renamed over it, so a crash never leaves a
/// half-written pass; on Unix the file is the user's alone from its creation.
fn write_atomic(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    use std::io::Write;
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let tmp = path.with_extension("json.tmp");
    let mut opts = std::fs::OpenOptions::new();
    opts.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        opts.mode(0o600);
    }
    let result = (|| {
        let mut f = opts.open(&tmp)?;
        f.write_all(bytes)?;
        f.sync_all()?;
        drop(f);
        std::fs::rename(&tmp, path)
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    result
}

impl PassStore for FilePassStore {
    fn load(&self) -> Result<Option<StoredPass>, LicenceError> {
        let text = match std::fs::read_to_string(&self.path) {
            Ok(t) => t,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(e) => return Err(Self::storage(e)),
        };
        let file: PassFile = serde_json::from_str(&text).map_err(Self::storage)?;
        let refresh_token = if file.token_in_keychain { secret::load(self)? } else { file.refresh_token };
        Ok(Some(StoredPass { pass: file.pass, refresh_token, last_seen: file.last_seen }))
    }

    fn save(&self, stored: &StoredPass) -> Result<(), LicenceError> {
        let in_keychain = match &stored.refresh_token {
            Some(t) => secret::save(self, t)?,
            None => {
                secret::clear(self)?;
                false
            }
        };
        let file = PassFile {
            pass: stored.pass.clone(),
            refresh_token: if in_keychain { None } else { stored.refresh_token.clone() },
            token_in_keychain: in_keychain,
            last_seen: stored.last_seen,
        };
        let json = serde_json::to_vec_pretty(&file).map_err(Self::storage)?;
        write_atomic(&self.path, &json).map_err(Self::storage)
    }

    fn clear(&self) -> Result<(), LicenceError> {
        secret::clear(self)?;
        match std::fs::remove_file(&self.path) {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(Self::storage(e)),
        }
    }
}

#[cfg(all(test, not(any(windows, target_os = "macos"))))]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    #[test]
    fn the_pass_round_trips_and_only_the_user_can_read_it() {
        let dir = std::env::temp_dir().join(format!("geoflux-pass-test-{}", std::process::id()));
        let store = FilePassStore::new(&dir);
        assert_eq!(store.load().unwrap(), None);
        let stored = StoredPass { pass: "gfp1.a.b".into(), refresh_token: Some("gfr_x".into()), last_seen: Some(5) };
        store.save(&stored).unwrap();
        assert_eq!(store.load().unwrap(), Some(stored));
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(dir.join("pass.json")).unwrap().permissions().mode();
            assert_eq!(mode & 0o077, 0, "{mode:o}");
        }
        store.clear().unwrap();
        assert_eq!(store.load().unwrap(), None);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn this_computer_has_an_id() {
        // CI runners and desktops have /etc/machine-id; containers may not.
        if std::path::Path::new("/etc/machine-id").exists() {
            assert!(SystemMachineId.machine_id().is_ok());
        }
    }

    #[test]
    fn commands_run_and_missing_ones_fail() {
        let started = Instant::now();
        let out = run(&["sh", "-c", "echo 4C4C4544-0042-3510-8051-B4C04F4E3732"]).unwrap();
        assert_eq!(MachineId::from_output(&out).unwrap().as_str(), "4C4C4544-0042-3510-8051-B4C04F4E3732");
        assert!(started.elapsed() < ID_TIMEOUT);
        assert!(run(&["/nonexistent/program"]).is_err());
    }
}
