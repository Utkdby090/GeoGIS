#!/usr/bin/env bash
# Packages release builds of GeoGIS (geogis) and rustgis-cli for one target into dist/.
#
#   packaging/package.sh <version> [rust-target]
#
# Linux: a .tar.gz, a .deb, an .rpm (when rpmbuild is installed) and an AppImage.
# macOS: a .dmg holding GeoGIS.app, plus a .tar.gz of the CLI
# (signed and notarized when the MACOS_SIGN_IDENTITY / APPLE_API_* variables are set).
# Windows (Git Bash): a portable .zip, an NSIS setup.exe (per user), a WiX .msi (all users), the
# DJI pack (GeoGIS-DJI-Pack-*.zip: the geoflux-dirp helper and an empty dji-sdk folder, since
# DJI's licence keeps their libraries out of what is published) and, when MSIX_PUBLISHER names
# a Microsoft Store identity, an .msix. The installers need makensis and the wix .NET tool with
# the WixToolset.UI.wixext extension (`wix extension add -g WixToolset.UI.wixext/<wix version>`).
# Build first with
# `cargo build --release --locked -p rustgis -p rustgis-cli -p geoflux-dirp [--target T]`.
#
# PUBLISHER (default GeoFlux) is the company shown by the installers and in Apps & features.
set -euo pipefail

version="${1:?usage: package.sh <version> [rust-target]}"
version="${version#v}"
target="${2:-}"
root="$(cd "$(dirname "$0")/.." && pwd)"
bin="$root/target/${target:+$target/}release"
dist="$root/dist"
mkdir -p "$dist"
work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT

case "$(uname -s)" in
  Linux*) os=linux ;;
  Darwin*) os=macos ;;
  MINGW* | MSYS* | CYGWIN*) os=windows ;;
  *) echo "unsupported OS: $(uname -s)" >&2; exit 1 ;;
esac
arch="${target%%-*}"
[ -n "$arch" ] || arch="$(uname -m)"
case "$arch" in
  x86_64 | amd64) arch=x86_64 ;;
  aarch64 | arm64) arch=aarch64 ;;
esac
exe=""
[ "$os" = windows ] && exe=".exe"
for b in geogis rustgis-cli; do
  [ -f "$bin/$b$exe" ] || { echo "missing $bin/$b$exe: build it first" >&2; exit 1; }
done
logo() { "$bin/rustgis-cli$exe" logo "$1" --size "$2"; }
name="GeoGIS-$version-$os-$arch"
publisher="${PUBLISHER:-GeoFlux}"

case "$os" in
  linux)
    # Portable tarball.
    d="$work/$name"
    mkdir -p "$d"
    cp "$bin/geogis" "$bin/rustgis-cli" "$root/LICENSE" "$root/NOTICE" "$root/README.md" \
      "$root/packaging/linux/geogis.desktop" "$root/packaging/linux/geogis-mime.xml" "$d/"
    logo "$d/geogis.png" 512
    tar -C "$work" -czf "$dist/$name.tar.gz" "$name"

    # Debian package.
    case "$arch" in x86_64) debarch=amd64 ;; aarch64) debarch=arm64 ;; *) debarch="$arch" ;; esac
    p="$work/deb"
    install -Dm755 "$bin/geogis" "$p/usr/bin/geogis"
    install -Dm755 "$bin/rustgis-cli" "$p/usr/bin/rustgis-cli"
    install -Dm644 "$root/packaging/linux/geogis.desktop" "$p/usr/share/applications/geogis.desktop"
    install -Dm644 "$root/packaging/linux/geogis-mime.xml" "$p/usr/share/mime/packages/geogis.xml"
    for s in 32 64 128 256 512; do
      mkdir -p "$p/usr/share/icons/hicolor/${s}x${s}/apps"
      logo "$p/usr/share/icons/hicolor/${s}x${s}/apps/geogis.png" "$s"
    done
    install -Dm644 "$root/LICENSE" "$p/usr/share/doc/geogis/copyright"
    install -Dm644 "$root/NOTICE" "$p/usr/share/doc/geogis/NOTICE"
    mkdir -p "$p/DEBIAN"
    # Libraries the binaries link against, worked out by dpkg-shlibdeps (it wants a
    # debian/control in the working directory). The windowing and GPU libraries are loaded at
    # run time, so they are listed by hand below.
    mkdir -p "$work/shlibs/debian"
    printf 'Source: geogis\n\nPackage: geogis\nArchitecture: any\n' > "$work/shlibs/debian/control"
    shlibs="$(cd "$work/shlibs" && dpkg-shlibdeps -O "$p/usr/bin/geogis" "$p/usr/bin/rustgis-cli" | sed -n 's/^shlibs:Depends=//p')" || shlibs=""
    if [ -z "$shlibs" ]; then
      # Off Debian/Ubuntu there is no dpkg database to consult. In CI that is an error: the
      # published package must carry the real list.
      if [ -n "${CI:-}" ]; then
        echo "dpkg-shlibdeps found no dependencies" >&2
        exit 1
      fi
      echo "dpkg-shlibdeps failed; using a minimal Depends list" >&2
    fi
    # Debian and RPM sort a pre-release before its release only with ~ (0.2.0~rc.1). Worked out
    # here: older bash keeps the backslash of ${version/-/\~} inside a here-document.
    pkgver="$(printf '%s' "$version" | tr - '~')"
    cat > "$p/DEBIAN/control" <<CONTROL
Package: geogis
Version: $pkgver
Section: science
Priority: optional
Architecture: $debarch
Maintainer: ${MAINTAINER:-$publisher <https://github.com/Utkdby090/GeoGIS/issues>}
Homepage: https://github.com/Utkdby090/GeoGIS
Installed-Size: $(du -sk "$p/usr" | cut -f1)
Depends: ${shlibs:-libc6, libgcc-s1}, libxkbcommon0, libwayland-client0 | libx11-6
Recommends: libxkbcommon-x11-0, libxcursor1, libxi6, libxrandr2, libvulkan1 | libegl1
Description: Desktop GIS for power-line inspection
 Links each tower to its inspection photos, reads FLIR and DJI thermal images
 and writes inspection reports, with maps and layers, symbology, editing,
 geoprocessing and export. Includes rustgis-cli for scripting every command.
CONTROL
    dpkg-deb --root-owner-group --build "$p" "$dist/geogis_${version}_${debarch}.deb"

    # RPM package (Fedora, RHEL, openSUSE) from the same tree. Library dependencies are found
    # by rpmbuild; the run-time loaded windowing and GPU libraries are listed by soname, which
    # every RPM distribution provides under the same name.
    if command -v rpmbuild > /dev/null; then
      r="$work/rpm"
      mkdir -p "$r/SPECS"
      cat > "$r/SPECS/geogis.spec" <<SPEC
%global debug_package %{nil}
%global __os_install_post %{nil}
Name: geogis
Version: $pkgver
Release: 1
Summary: Desktop GIS for power-line inspection
License: Apache-2.0
URL: https://github.com/Utkdby090/GeoGIS
Requires: libxkbcommon.so.0()(64bit)
Requires: (libwayland-client.so.0()(64bit) or libX11.so.6()(64bit))
Recommends: libxkbcommon-x11.so.0()(64bit)
Recommends: libXcursor.so.1()(64bit)
Recommends: libXi.so.6()(64bit)
Recommends: libXrandr.so.2()(64bit)
Recommends: (libvulkan.so.1()(64bit) or libEGL.so.1()(64bit))

%description
Links each tower to its inspection photos, reads FLIR and DJI thermal images
and writes inspection reports, with maps and layers, symbology, editing,
geoprocessing and export. Includes rustgis-cli for scripting every command.

%install
mkdir -p %{buildroot}
cp -a "$p/usr" %{buildroot}/

%files
/usr/bin/geogis
/usr/bin/rustgis-cli
/usr/share/applications/geogis.desktop
/usr/share/mime/packages/geogis.xml
/usr/share/icons/hicolor/*/apps/geogis.png
%license /usr/share/doc/geogis/copyright
%license /usr/share/doc/geogis/NOTICE
SPEC
      rpmbuild --quiet -bb --target "$arch" --define "_topdir $r" "$r/SPECS/geogis.spec"
      # The file is named after $version (0.2.0-rc.1), like the other assets: GitHub rewrites
      # the ~ of the package version in release asset names.
      cp "$r"/RPMS/*/geogis-*.rpm "$dist/geogis-$version-1.$arch.rpm"
    elif [ -n "${CI:-}" ]; then
      echo "rpmbuild not found" >&2
      exit 1
    else
      echo "rpmbuild not found: skipping the .rpm" >&2
    fi

    # AppImage: a single file that runs on most distributions without installing. It uses the
    # static type-2 runtime, so it needs no libfuse2. The tools are pinned by checksum.
    case "$arch" in
      x86_64)
        tool_sha=ed4ce84f0d9caff66f50bcca6ff6f35aae54ce8135408b3fa33abfc3cb384eb0
        runtime_sha=2fca8b443c92510f1483a883f60061ad09b46b978b2631c807cd873a47ec260d ;;
      aarch64)
        tool_sha=f0837e7448a0c1e4e650a93bb3e85802546e60654ef287576f46c71c126a9158
        runtime_sha=00cbdfcf917cc6c0ff6d3347d59e0ca1f7f45a6df1a428a0d6d8a78664d87444 ;;
      *) tool_sha="" ;;
    esac
    if [ -n "$tool_sha" ]; then
      fetch() { # url sha out
        curl -fsSL --retry 3 -o "$3" "$1"
        echo "$2  $3" | sha256sum -c --quiet -
      }
      t="$work/appimage-tools"
      mkdir -p "$t"
      fetch "https://github.com/AppImage/appimagetool/releases/download/1.9.1/appimagetool-$arch.AppImage" \
        "$tool_sha" "$t/appimagetool"
      fetch "https://github.com/AppImage/type2-runtime/releases/download/20251108/runtime-$arch" \
        "$runtime_sha" "$t/runtime"
      chmod +x "$t/appimagetool"
      a="$work/GeoGIS.AppDir"
      mkdir -p "$a"
      cp -a "$p/usr" "$a/"
      cp "$root/packaging/linux/geogis.desktop" "$a/"
      logo "$a/geogis.png" 256
      ln -s usr/bin/geogis "$a/AppRun"
      ARCH="$arch" APPIMAGE_EXTRACT_AND_RUN=1 "$t/appimagetool" --no-appstream --runtime-file "$t/runtime" \
        "$a" "$dist/$name.AppImage"
    fi
    ;;

  macos)
    app="$work/GeoGIS.app"
    mkdir -p "$app/Contents/MacOS" "$app/Contents/Resources"
    cp "$bin/geogis" "$app/Contents/MacOS/geogis"
    cp "$root/LICENSE" "$root/NOTICE" "$app/Contents/Resources/"
    # Bundle versions must be dot-separated numbers: drop a pre-release suffix (0.2.0-rc.1).
    sed "s/@VERSION@/${version%%-*}/g" "$root/packaging/macos/Info.plist" > "$app/Contents/Info.plist"
    iconset="$work/geogis.iconset"
    mkdir -p "$iconset"
    for s in 16 32 128 256 512; do
      logo "$iconset/icon_${s}x${s}.png" "$s"
      logo "$iconset/icon_${s}x${s}@2x.png" "$((s * 2))"
    done
    iconutil -c icns "$iconset" -o "$app/Contents/Resources/geogis.icns"
    # Signing: with MACOS_SIGN_IDENTITY (a Developer ID Application identity in the keychain)
    # the app and CLI get a hardened-runtime signature; without it, an ad-hoc one, which
    # Apple Silicon needs to launch anything. With APPLE_API_KEY_PATH, APPLE_API_KEY_ID and
    # APPLE_API_ISSUER as well, the app, the DMG and the CLI are notarized and the app and the
    # DMG are stapled.
    identity="${MACOS_SIGN_IDENTITY:-}"
    sign() {
      if [ -n "$identity" ]; then
        codesign --force --options runtime --timestamp --sign "$identity" "$@"
      else
        codesign --force --sign - "$@"
      fi
    }
    notarize() {
      [ -n "$identity" ] && [ -n "${APPLE_API_KEY_PATH:-}" ] || return 0
      xcrun notarytool submit "$1" --key "$APPLE_API_KEY_PATH" --key-id "$APPLE_API_KEY_ID" \
        --issuer "$APPLE_API_ISSUER" --wait --timeout 30m
    }
    sign "$app"
    # Staple the app itself, not only the DMG: Homebrew and people dragging it to Applications
    # copy the app out of the DMG, and an app without its own ticket opens only if Gatekeeper
    # reaches Apple's servers at first launch (#89). Stapling adds a file outside the signed
    # contents, so the signature stays valid.
    if [ -n "$identity" ] && [ -n "${APPLE_API_KEY_PATH:-}" ]; then
      ditto -c -k --keepParent "$app" "$work/GeoGIS.app.zip"
      notarize "$work/GeoGIS.app.zip"
      xcrun stapler staple "$app"
    fi

    stage="$work/dmg"
    mkdir -p "$stage"
    cp -R "$app" "$stage/"
    ln -s /Applications "$stage/Applications"
    hdiutil create -volname "GeoGIS $version" -srcfolder "$stage" -ov -format UDZO "$dist/$name.dmg"
    if [ -n "$identity" ]; then
      sign "$dist/$name.dmg"
      if notarize "$dist/$name.dmg" && [ -n "${APPLE_API_KEY_PATH:-}" ]; then
        xcrun stapler staple "$dist/$name.dmg"
      fi
    fi

    d="$work/rustgis-cli-$version-$os-$arch"
    mkdir -p "$d"
    cp "$bin/rustgis-cli" "$root/LICENSE" "$root/NOTICE" "$d/"
    sign "$d/rustgis-cli"
    # A bare executable cannot be stapled: notarizing it registers the ticket that Gatekeeper
    # looks up online.
    if [ -n "$identity" ]; then
      ditto -c -k "$d/rustgis-cli" "$work/rustgis-cli.zip"
      notarize "$work/rustgis-cli.zip"
    fi
    tar -C "$work" -czf "$dist/rustgis-cli-$version-$os-$arch.tar.gz" "$(basename "$d")"
    ;;

  windows)
    # Portable zip: unzip anywhere and run, no installer or admin rights.
    d="$work/$name"
    mkdir -p "$d"
    cp "$bin/geogis.exe" "$bin/rustgis-cli.exe" "$root/LICENSE" "$root/NOTICE" "$root/README.md" "$d/"
    sed "s/@VERSION@/$version/g" "$root/packaging/windows/README-portable.txt" > "$d/README.txt"
    (cd "$work" && 7z a -tzip -bso0 "$dist/$name-portable.zip" "$name")

    # DJI pack: the geoflux-dirp helper and an empty dji-sdk folder, unzipped straight into the
    # GeoGIS folder (no top folder in the zip). DJI's licence keeps their libraries out of it; the
    # customer adds them (README). File names don't clash with the app's own README.txt, and
    # LICENSE.txt and NOTICE.txt match the installed ones.
    if [ -f "$bin/geoflux-dirp.exe" ]; then
      k="$work/dji-pack"
      mkdir -p "$k/dji-sdk"
      cp "$bin/geoflux-dirp.exe" "$k/"
      cp "$root/LICENSE" "$k/LICENSE.txt"
      cp "$root/NOTICE" "$k/NOTICE.txt"
      sed "s/@VERSION@/$version/g" "$root/packaging/windows/README-dji-pack.txt" > "$k/README-DJI-pack.txt"
      sed "s/@VERSION@/$version/g" "$root/packaging/windows/README-dji-sdk.txt" > "$k/dji-sdk/README.txt"
      (cd "$k" && 7z a -tzip -bso0 "$dist/GeoGIS-DJI-Pack-$version-$os-$arch.zip" \
        geoflux-dirp.exe LICENSE.txt NOTICE.txt README-DJI-pack.txt dji-sdk)
    elif [ -n "${CI:-}" ]; then
      echo "missing $bin/geoflux-dirp.exe: build it with -p geoflux-dirp" >&2
      exit 1
    else
      echo "geoflux-dirp.exe not built: skipping the DJI pack" >&2
    fi

    # Installers: setup.exe (NSIS) installs for the current user without admin rights, the .msi
    # (WiX) for all users. Both add a Start menu shortcut, put rustgis-cli on the PATH and open
    # .rgis projects. Wizard images are the logo on white, drawn by rustgis-cli.
    case "$arch" in x86_64) instarch=x64 ;; aarch64) instarch=arm64 ;; *) instarch="$arch" ;; esac
    s="$work/installer"
    mkdir -p "$s"
    cp "$bin/geogis.exe" "$bin/rustgis-cli.exe" "$root/LICENSE" "$root/NOTICE" "$root/packaging/windows/path.ps1" "$s/"
    "$bin/rustgis-cli.exe" logo "$s/geogis.ico"
    "$bin/rustgis-cli.exe" logo "$s/sidebar.bmp" --size 128 --canvas 164x314
    "$bin/rustgis-cli.exe" logo "$s/banner.bmp" --size 44 --canvas 493x58 --at 440,7
    "$bin/rustgis-cli.exe" logo "$s/dialog.bmp" --size 120 --canvas 493x312 --at 22,96
    w() { cygpath -w "$1"; }
    # A missing tool fails a CI build (the release must carry every package) and is skipped
    # locally.
    missing() {
      if [ -n "${CI:-}" ]; then echo "$1 not found" >&2; exit 1; fi
      echo "$1 not found: skipping the $2" >&2
    }

    makensis="$(command -v makensis.exe || command -v makensis || true)"
    [ -n "$makensis" ] || [ ! -x "/c/Program Files (x86)/NSIS/makensis.exe" ] ||
      makensis="/c/Program Files (x86)/NSIS/makensis.exe"
    if [ -n "$makensis" ]; then
      # VIProductVersion wants four numbers: drop a pre-release suffix (0.2.0-rc.1 -> 0.2.0.0).
      MSYS2_ARG_CONV_EXCL='*' "$makensis" -V2 -DVERSION="$version" -DVERSION4="${version%%-*}.0" \
        -DPUBLISHER="$publisher" -DSTAGE="$(w "$s")" -DICON="$(w "$s/geogis.ico")" \
        -DSIDEBAR="$(w "$s/sidebar.bmp")" -DOUTFILE="$(w "$dist/$name-setup.exe")" \
        "$(w "$root/packaging/windows/geogis.nsi")"
    else
      missing makensis setup.exe
    fi

    wix="$(command -v wix.exe || command -v wix || true)"
    if [ -n "$wix" ]; then
      # The license dialog shows RTF: LICENSE with RTF's special characters escaped.
      {
        printf '{\\rtf1\\ansi\\deff0{\\fonttbl{\\f0 Segoe UI;}}\\f0\\fs16\n'
        tr -d '\r' < "$root/LICENSE" | sed -e 's/[\\{}]/\\&/g' -e 's/$/\\par/'
        printf '}\n'
      } > "$s/LICENSE.rtf"
      # MSI versions are three numbers: drop a pre-release suffix.
      MSYS2_ARG_CONV_EXCL='*' "$wix" build -arch "$instarch" -ext WixToolset.UI.wixext \
        -d Version="${version%%-*}" -d Publisher="$publisher" -d Stage="$(w "$s")" -d Icon="$(w "$s/geogis.ico")" \
        -d License="$(w "$s/LICENSE.rtf")" -d Banner="$(w "$s/banner.bmp")" -d Dialog="$(w "$s/dialog.bmp")" \
        -o "$(w "$dist/$name.msi")" "$(w "$root/packaging/windows/geogis.wxs")"
      rm -f "$dist/$name.wixpdb"
    else
      missing wix .msi
    fi

    # MSIX package for the Microsoft Store, which signs it on ingestion. Built only when
    # MSIX_PUBLISHER is set: the identity (MSIX_NAME, MSIX_PUBLISHER) must match the product
    # reserved in Partner Center, and an unsigned package with any other identity cannot be
    # installed.
    if [ -n "${MSIX_PUBLISHER:-}" ]; then
      m="$work/msix"
      mkdir -p "$m/Assets"
      cp "$bin/geogis.exe" "$bin/rustgis-cli.exe" "$root/LICENSE" "$root/NOTICE" "$m/"
      logo "$m/Assets/Square44x44Logo.png" 44
      logo "$m/Assets/Square150x150Logo.png" 150
      logo "$m/Assets/StoreLogo.png" 50
      # A value for the manifest: XML-escaped, then escaped for a sed replacement.
      xmlval() { printf '%s' "$1" | sed -e 's/&/\&amp;/g; s/</\&lt;/g; s/>/\&gt;/g; s/"/\&quot;/g' -e 's/[\\|&]/\\&/g'; }
      # MSIX versions are four numbers: drop a pre-release suffix (0.2.0-rc.1 -> 0.2.0.0).
      sed -e "s|@VERSION@|${version%%-*}.0|g" -e "s|@ARCH@|$instarch|g" \
        -e "s|@NAME@|$(xmlval "${MSIX_NAME:-GeoFlux.GeoGIS}")|g" \
        -e "s|@PUBLISHER@|$(xmlval "$MSIX_PUBLISHER")|g" \
        -e "s|@PUBLISHER_DISPLAY_NAME@|$(xmlval "${MSIX_PUBLISHER_DISPLAY_NAME:-$publisher}")|g" \
        "$root/packaging/windows/AppxManifest.xml" > "$m/AppxManifest.xml"
      makeappx="$(command -v makeappx.exe || true)"
      if [ -z "$makeappx" ]; then
        # The host's own build of the SDK tools, x64 or ARM64.
        case "$(uname -m)" in aarch64 | arm64) sdkarch=arm64 ;; *) sdkarch=x64 ;; esac
        makeappx="$(ls -d "/c/Program Files (x86)/Windows Kits/10/bin/"*/"$sdkarch"/makeappx.exe 2>/dev/null | sort -V | tail -1)"
      fi
      [ -n "$makeappx" ] || { echo "makeappx.exe not found: install the Windows SDK" >&2; exit 1; }
      MSYS2_ARG_CONV_EXCL='*' "$makeappx" pack /d "$(cygpath -w "$m")" /p "$(cygpath -w "$dist/$name.msix")" /o
    fi
    ;;
esac

ls -l "$dist"
