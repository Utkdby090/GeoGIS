# Installation

## Download

Each [GitHub release](https://github.com/Utkdby090/GeoGIS/releases/latest) carries the Windows
downloads. macOS and Linux builds will follow.

| File | What it is |
| --- | --- |
| `GeoGIS-<version>-windows-x86_64-setup.exe` | Installer for your account, no admin rights |
| `GeoGIS-<version>-windows-x86_64.msi` | Installer for all users, for IT departments |
| `GeoGIS-<version>-windows-x86_64-portable.zip` | No install: unzip and run `geogis.exe` |
| `GeoGIS-DJI-Pack-<version>-windows-x86_64.zip` | Adds DJI thermal photos (see below) |

`setup.exe` installs in `%LOCALAPPDATA%\Programs\GeoGIS` and needs no admin rights; the `.msi`
installs for every user in `C:\Program Files\GeoGIS` and suits managed deployment
(`msiexec /i GeoGIS-<version>-windows-x86_64.msi /qn`). Both add a Start menu entry, put
`rustgis-cli` on the PATH, open `.rgis` projects and uninstall from **Settings › Apps**. GeoGIS
installs beside RustGIS without affecting it. `SHA256SUMS` lists the checksum of every file.

The Windows builds are not signed yet: choose **More info › Run anyway** if SmartScreen warns
about an unknown publisher.

### DJI thermal photos

FLIR thermal photos work straight away. DJI thermal photos need the DJI pack and DJI's own
Thermal SDK, which DJI's licence keeps us from including:

1. Unzip the DJI pack into the GeoGIS folder, so that `geoflux-dirp.exe` sits next to
   `geogis.exe`.
2. Download the DJI Thermal SDK for Windows from DJI and copy every file from its
   `tsdk-core\lib\windows\release_x64` folder directly into the pack's `dji-sdk` folder.
3. Restart GeoGIS.

## From source

GeoGIS builds from source with a recent stable Rust toolchain (1.90 or later).

```sh
git clone https://github.com/Utkdby090/GeoGIS.git
cd GeoGIS
cargo run --release -p rustgis            # the desktop app (geogis)
cargo build --release -p rustgis-cli      # the command line tool
cargo build --release -p geoflux-dirp     # the DJI thermal helper
```

The Natural Earth sample data is in the repository, so the build works offline.

## Platforms

| Platform | Status |
| --- | --- |
| Linux (X11 and Wayland) | Supported |
| macOS | Supported |
| Windows | Supported |
| Web browser | Supported: [web.rustgis.app](https://web.rustgis.app) (WebGPU or WebGL2), installable as an app on iOS, Android and desktops ([how](#install-the-web-app)) |

## Web app

RustGIS also runs in the browser, compiled to WebAssembly: open
**[web.rustgis.app](https://web.rustgis.app)**. It is the same app with three differences:

- **Open** and **Add Data** use the browser's file picker, and you can drop files on the page.
  GeoJSON, CSV, GeoTIFF, ASCII grids and `.rgis` projects work; Shapefiles need the desktop
  app.
- **Save** and **Export** download the file. **Save As**, and the first **Save** of a project,
  ask for the file name first; the browser decides which folder downloads go to (Firefox, for
  one, saves to its Downloads folder unless told to ask). A project opened from a file is named
  after it, and **Save** downloads it under that name again.
- The interface uses the built-in font, because a web page can't read installed fonts.

The web app opens the way **Settings › Start-up** says (remembered in the browser). The URL can
override that for one visit: add `?blank` to start with an empty project,
`?projection=EPSG:3010` (a supported projected EPSG code, a preset id such as `robinson`, or `globe`) to
start in another projection, `?extent=10.5,55,24.5,69.5` (west, south, east, north in degrees) to
open on an area, `?lang=de` (or any language code) to pick the interface language, or `?webgl` to
force WebGL2. Combine them with `&`, so a self-hosted build can link to
`?blank&projection=EPSG:3010&extent=10.5,55,24.5,69.5`.

Links can carry data too: `?data=<address>` adds a file by its web address (any format Add Data
reads, downloaded in the background; repeat it for up to 16, and the map zooms to the first that
lands with something to show), `?style=<address>` adds a MapLibre style, `?tiles=<template>`
a tile layer, and `?zarr=<address>` opens Add Zarr Variable on a Zarr store. Percent-encode an address holding `&`, `#` or spaces (`encodeURIComponent`); a `+`
stays a `+`. The server must allow the browser to fetch the file (CORS); a file that can't be
added is said in a notice. For example,
`?blank&data=https://earthquake.usgs.gov/earthquakes/feed/v1.0/summary/2.5_week.geojson` opens
the past week's earthquakes.

`?project=<address>` opens a whole project by its web address: a `.rgis` file (or a
`.rgispkg` package), or any address that answers with a project's JSON, such as an API that
builds the project on the fly. Its maps, layers, symbology and view open as saved, so one link
shares a styled map. The project replaces the start-up one, so `?data=`, `?stac=`, `?style=`,
`?tiles=` and `?zarr=` are left out beside it (a notice says so). The data the project points to
is read as it would be from a saved file, and its servers must allow the browser to fetch it
(CORS), as must the project's.

`?stac=<address>` opens STAC Search on a [STAC](https://stacspec.org) item or collection: an
item's footprint is drawn and its default asset (its true-colour `visual` image, else its first
GeoTIFF) added, signed first when it is a Microsoft Planetary Computer one; a collection is chosen
in the dialog, with its API, ready to search. For example,
`?stac=https://planetarycomputer.microsoft.com/api/stac/v1/collections/sentinel-2-l2a/items/S2B_MSIL2A_20240727T102559_R108_T32TLS_20240727T131405`
opens a Sentinel-2 scene of Lake Geneva.

`?embed` runs the app inside another page, as the [Python package's notebook widget](python.md)
does: it starts blank, keeps its settings in memory rather than in the browser, and exchanges
the project with the page that frames it through `postMessage` (the messages are described in
`apps/rustgis-web/src/embed.rs`).

### Install the web app

The web app installs like an app on phones, tablets and computers, with its own icon and a window
without the browser's toolbars, and starts without a network connection once it has been opened
online. Nothing is downloaded from an app store, and it updates itself from web.rustgis.app.

| Platform | How |
| --- | --- |
| iPhone and iPad | Open web.rustgis.app in Safari, tap **Share**, then **Add to Home Screen** |
| Android | Open it in Chrome, tap the **⋮** menu, then **Install app** (or **Add to Home screen**) |
| Windows, macOS, Linux, ChromeOS | In Chrome or Edge, click the install icon at the right of the address bar (or **⋮ › Cast, save and share › Install page as app**) |
| Mac (Safari 17 or later) | **File › Add to Dock** |

To remove it, delete it like any other app: long-press the icon on a phone or tablet, or use
**Uninstall** in the app window's menu on a computer.

Offline, the installed app opens the World Atlas sample and the files you open, but not basemap
tiles, web data, plugins or the AI Assistant, which need the network. When a new version has been
published while the app is open, a notice offers to reload into it; otherwise the next launch
picks it up. Two things to know on iPhone and iPad: iOS gives a web page less memory than a
desktop browser, so very large rasters may not open, and it can clear the offline copy when the
device runs short of storage, so the next launch then needs a connection.

To build it yourself, install [Trunk](https://trunkrs.dev), the `wasm32-unknown-unknown` target
and clang (it compiles the Zstandard decoder used for GeoTIFFs to WebAssembly). Linux
distributions' clang can; on macOS, Apple's can't, so install `llvm` with Homebrew and put its
`bin` first on your `PATH`. Then:

```sh
cd apps/rustgis-web
trunk serve          # http://127.0.0.1:8772
trunk build --release   # output in dist/web
cargo xtask web-pwa ../../dist/web   # add the service worker (offline start), as the deploy does
```

The service worker is left out of `trunk serve`, so a development build is never served from a
stale cache.

## Fonts

RustGIS uses an installed sans-serif when it finds one (Segoe UI, San Francisco, Inter, Noto
Sans, Open Sans or DejaVu Sans) and falls back to a built-in face. Set
`RUSTGIS_BUILTIN_FONTS=1` to always use the built-in face.

## If RustGIS closes unexpectedly

The desktop app writes its internal errors to `crash.log` in its settings folder: one that closed
the app, and also one it recovered from (an error in the AI Assistant, for example, stops the
request and says so in the chat instead of closing the app). The folder is `%APPDATA%\RustGIS` on
Windows, `~/Library/Application Support/RustGIS` on macOS, and `$XDG_CONFIG_HOME/rustgis` on
Linux (`~/.config/rustgis` when that isn't set). The next launch moves the log to
`crash-last.log` and names the file in a notice.

Attach it to a [bug report](https://github.com/opengeos/RustGIS/issues/new?template=bug_report.yml).
It holds the version, the operating system, the error message and where in RustGIS it happened.
An error message can quote what RustGIS was working on, such as a file path or a layer name, so
read the log first and remove anything you don't want to share.
