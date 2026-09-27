#!/usr/bin/env bash
# Packages a release build of the desktop app into
# dist/mkv-track-editor-<version>-<platform>.tar.gz (plus a .sha256 file).
#
#   packaging/package-desktop.sh <version> <platform> [binary]
#
# <platform> is macos-arm64, linux-x86_64 or linux-arm64. [binary] defaults
# to target/release/mkv-track-editor. On Linux the archive holds the binary;
# on macOS it holds "MKV Track Editor.app" with the binary inside.
set -euo pipefail

version=${1:?usage: package-desktop.sh <version> <platform> [binary]}
platform=${2:?usage: package-desktop.sh <version> <platform> [binary]}
binary=${3:-target/release/mkv-track-editor}

root=$(cd "$(dirname "$0")/.." && pwd)
name="mkv-track-editor-${version}-${platform}"
stage="${root}/dist/${name}"

rm -rf "$stage" "${stage}.tar.gz" "${stage}.tar.gz.sha256"
mkdir -p "$stage"

cp "$root/README.md" "$stage/"

case "$platform" in
  macos-*)
    # The binary ships once, inside the bundle; the formula links it into bin.
    app="$stage/MKV Track Editor.app/Contents"
    mkdir -p "$app/MacOS"
    install -m 0755 "$binary" "$app/MacOS/mkv-track-editor"
    # 11.0 is Rust's default deployment target for aarch64-apple-darwin.
    # Apps started from Finder/Dock get a bare PATH (/usr/bin:/bin:...), so
    # without LSEnvironment they couldn't find the Homebrew-installed
    # mkvmerge/mkvpropedit/mediainfo.
    cat > "$app/Info.plist" <<EOF
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>CFBundleDevelopmentRegion</key><string>en</string>
  <key>CFBundleDisplayName</key><string>MKV Track Editor</string>
  <key>CFBundleExecutable</key><string>mkv-track-editor</string>
  <key>CFBundleIdentifier</key><string>io.github.bugsbunny-25.mkv-track-editor</string>
  <key>CFBundleInfoDictionaryVersion</key><string>6.0</string>
  <key>CFBundleName</key><string>MKV Track Editor</string>
  <key>CFBundlePackageType</key><string>APPL</string>
  <key>CFBundleShortVersionString</key><string>${version}</string>
  <key>CFBundleVersion</key><string>${version}</string>
  <key>LSApplicationCategoryType</key><string>public.app-category.video</string>
  <key>LSMinimumSystemVersion</key><string>11.0</string>
  <key>NSHighResolutionCapable</key><true/>
  <key>LSEnvironment</key>
  <dict>
    <key>PATH</key><string>/opt/homebrew/bin:/usr/local/bin:/usr/bin:/bin:/usr/sbin:/sbin</string>
  </dict>
</dict>
</plist>
EOF
    ;;
  linux-*)
    install -m 0755 "$binary" "$stage/mkv-track-editor"
    ;;
  *)
    echo "unknown platform: $platform" >&2
    exit 1
    ;;
esac

# No extended attributes or AppleDouble files, so GNU tar extracts cleanly.
COPYFILE_DISABLE=1 tar --no-xattrs -C "${root}/dist" -czf "${stage}.tar.gz" "$name"
rm -rf "$stage"

cd "${root}/dist"
if command -v sha256sum >/dev/null; then
  sha256sum "${name}.tar.gz" > "${name}.tar.gz.sha256"
else
  shasum -a 256 "${name}.tar.gz" > "${name}.tar.gz.sha256"
fi
cat "${name}.tar.gz.sha256"
