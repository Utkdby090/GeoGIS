# Packaging

`package.sh` builds the release files for one platform (see its header). The Release workflow
(`.github/workflows/release.yml`) runs it on Windows when a version tag is pushed and attaches
the files to a draft GitHub Release. macOS and Linux builds come later; `package.sh` already
handles both.

## Making a release

1. Set `version` in `[workspace.package]` in the root `Cargo.toml` (for example `0.3.0`) and
   merge that into `main`.
2. Tag that commit and push the tag: `git tag v0.3.0 && git push origin v0.3.0`. The tag must
   match the version, or the workflow stops. A tag with a hyphen (`v0.3.0-rc.1`) makes a
   pre-release.
3. When the workflow finishes, open the draft on the Releases page, check it and publish it.
   Only a published release reaches the in-app update check.

To try a build without a release, run the Release workflow by hand (Actions › Release › Run
workflow): the files are kept as a workflow artifact.

## What a release holds (Windows)

| File | What it is |
| --- | --- |
| `GeoGIS-<version>-windows-x86_64-setup.exe` | Installs for the current user, no admin rights, in `%LOCALAPPDATA%\Programs\GeoGIS` |
| `GeoGIS-<version>-windows-x86_64.msi` | Installs for all users in `C:\Program Files\GeoGIS` (for IT departments) |
| `GeoGIS-<version>-windows-x86_64-portable.zip` | Unzip and run, nothing installed |
| `GeoGIS-DJI-Pack-<version>-windows-x86_64.zip` | The `geoflux-dirp` helper and an empty `dji-sdk` folder, unzipped next to `geogis.exe` |
| `SHA256SUMS` | Checksums of the files above |

Both installers add a Start menu entry, put `rustgis-cli` on the PATH, open `.rgis`/`.rgisx`
projects in GeoGIS and list GeoGIS under Open with for `.geojson`. Their Windows identities
(the WiX `UpgradeCode`, the `Software\GeoGIS` registry key and the `GeoGIS.Project` file type)
are GeoGIS's own, so installing or removing GeoGIS never touches an installed RustGIS. Never
change the `UpgradeCode` in `windows/geogis.wxs`: Windows uses it to replace an older GeoGIS
with a newer one.

The DJI pack holds no DJI library. DJI's licence keeps their Thermal SDK out of what we publish,
so each customer downloads it from DJI and copies its `tsdk-core\lib\windows\release_x64` files
into `dji-sdk` (the pack's README says how).

Every package carries `LICENSE` and `NOTICE`, which Apache-2.0 requires for the RustGIS code.

## Settings

| Variable | Default | Used for |
| --- | --- | --- |
| `PUBLISHER` (repository variable `GEOGIS_PUBLISHER` in the workflow) | `GeoFlux` | The company shown by the installers and in Apps & features |
| `MAINTAINER` | `<publisher> <issues page>` | The Debian package maintainer |
| `MSIX_PUBLISHER`, `MSIX_NAME`, `MSIX_PUBLISHER_DISPLAY_NAME` | none, `GeoFlux.GeoGIS`, the publisher | An `.msix` for the Microsoft Store, built only when `MSIX_PUBLISHER` is set to the identity reserved in Partner Center |
| `MACOS_SIGN_IDENTITY`, `APPLE_API_KEY_PATH`, `APPLE_API_KEY_ID`, `APPLE_API_ISSUER` | none | Signing and notarizing the macOS app |

## Code signing

The Windows files are not signed yet, so Windows SmartScreen warns the first time someone runs
the installer ("Windows protected your PC", then More info › Run anyway). A code-signing
certificate removes that warning; signing is added to the workflow once there is one.
