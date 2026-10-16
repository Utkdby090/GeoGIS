//! The machine ID a licence is locked to: the Windows SMBIOS UUID, read the way GeoFlux read it,
//! so keys already issued keep matching.

use crate::LicenceError;

/// A normalised machine ID: whitespace removed and upper case, as GeoFlux compared it.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct MachineId(String);

/// What GeoFlux returned when it could not read an ID. A key issued for one of these would unlock
/// every computer where the lookup fails, so none of them is ever accepted as an ID.
pub const PLACEHOLDER_IDS: &[&str] = &["WINDOWS-UUID-NOT-FOUND", "UNKNOWN-HARDWARE-ID", "NON-WINDOWS-ID-GENERIC"];

/// Longer than any SMBIOS UUID; output past this is an error message, not an ID.
pub const MAX_ID_LEN: usize = 64;

/// The commands GeoFlux ran, in order: PowerShell first, then `wmic` for older Windows.
/// `-NoProfile` keeps a person's PowerShell profile from printing into the ID; the command itself
/// is GeoFlux's, so the ID comes out the same.
pub const WINDOWS_ID_COMMANDS: &[&[&str]] = &[
    &["powershell.exe", "-NoProfile", "-NonInteractive", "-Command", "(Get-CimInstance -Class Win32_ComputerSystemProduct).UUID"],
    &["wmic", "csproduct", "get", "uuid"],
];

impl MachineId {
    /// Normalises a command's output as GeoFlux did (every whitespace character removed, upper
    /// case). Refuses output that is empty, mentions an `Exception` or `Error` (GeoFlux's own test),
    /// is a [`PLACEHOLDER_IDS`] value, is longer than [`MAX_ID_LEN`], or is the all-zero or all-F
    /// UUID that boards without a real ID report.
    pub fn from_output(raw: &str) -> Result<MachineId, LicenceError> {
        if raw.contains("Exception") || raw.contains("Error") {
            return Err(LicenceError::MachineId("the system returned an error".into()));
        }
        let id: String = raw.chars().filter(|c| !c.is_whitespace()).flat_map(char::to_uppercase).collect();
        if id.is_empty() {
            return Err(LicenceError::MachineId("the system returned no ID".into()));
        }
        if id.chars().count() > MAX_ID_LEN {
            return Err(LicenceError::MachineId("the system returned something other than an ID".into()));
        }
        if PLACEHOLDER_IDS.contains(&id.as_str()) {
            return Err(LicenceError::MachineId(format!("{id} is not a real ID")));
        }
        let hex: String = id.chars().filter(|c| *c != '-').collect();
        if !hex.is_empty() && (hex.chars().all(|c| c == '0') || hex.chars().all(|c| c == 'F')) {
            return Err(LicenceError::MachineId("this computer reports a blank hardware ID".into()));
        }
        Ok(MachineId(id))
    }

    /// The ID from `wmic csproduct get uuid` output: the first non-blank line after the header.
    pub fn from_wmic_output(raw: &str) -> Result<MachineId, LicenceError> {
        let line = raw.lines().skip(1).map(str::trim).find(|l| !l.is_empty()).unwrap_or("");
        MachineId::from_output(line)
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for MachineId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// Where the machine ID comes from. The desktop app runs [`WINDOWS_ID_COMMANDS`] hidden, with a
/// time limit, on a worker thread (PowerShell can take a second or two to start); tests pass a
/// fixed ID.
pub trait MachineIdSource: Send + Sync {
    fn machine_id(&self) -> Result<MachineId, LicenceError>;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn output_is_normalised_like_geoflux() {
        let id = MachineId::from_output(" 4c4c4544-0042-3510-8051-b7c04f4e3332\r\n").map(|m| m.to_string());
        assert_eq!(id.as_deref(), Ok("4C4C4544-0042-3510-8051-B7C04F4E3332"));
        let wmic = "UUID                                  \r\n4C4C4544-0042-3510-8051-B7C04F4E3332  \r\n\r\n";
        assert_eq!(MachineId::from_wmic_output(wmic).map(|m| m.to_string()).as_deref(), Ok("4C4C4544-0042-3510-8051-B7C04F4E3332"));
    }

    #[test]
    fn errors_and_placeholders_are_never_ids() {
        for bad in [
            "",
            "  \r\n",
            "Get-CimInstance : Access denied\r\nCategoryInfo : PermissionDenied ... Exception",
            "Error: no instance",
            "WINDOWS-UUID-NOT-FOUND",
            "unknown-hardware-id",
            "NON-WINDOWS-ID-GENERIC",
            "00000000-0000-0000-0000-000000000000",
            "FFFFFFFF-FFFF-FFFF-FFFF-FFFFFFFFFFFF",
            "ffffffff-ffff-ffff-ffff-ffffffffffff",
            &"A".repeat(MAX_ID_LEN + 1),
        ] {
            assert!(MachineId::from_output(bad).is_err(), "{bad:?}");
        }
        assert!(MachineId::from_wmic_output("UUID\r\n\r\n").is_err());
    }
}
