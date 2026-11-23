#!/bin/sh
set -eu
cd "$(dirname "$0")/.."
cargo build --release --locked
binary=$(realpath "${CARGO_TARGET_DIR:-target}/release/vader5")
version=$(awk -F'"' '/^version = / {print $2; exit}' Cargo.toml)
arch=$(dpkg --print-architecture)
out="$(pwd)/target/linux"
mkdir -p "$out"
tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT
install -Dm755 "$binary" "$tmp/pkg/usr/bin/vader5"
strip "$tmp/pkg/usr/bin/vader5"
install -Dm644 linux/vader5.desktop "$tmp/pkg/usr/share/applications/vader5.desktop"
install -Dm644 linux/70-vader5.rules "$tmp/pkg/usr/lib/udev/rules.d/70-vader5.rules"
mkdir -p "$tmp/debian" "$tmp/pkg/DEBIAN"
printf 'Source: vader5\n' > "$tmp/debian/control"
deps=$(cd "$tmp" && dpkg-shlibdeps -O -e"pkg/usr/bin/vader5" | sed 's/^shlibs:Depends=//')
cat > "$tmp/pkg/DEBIAN/control" <<EOF
Package: vader5
Version: $version
Architecture: $arch
Maintainer: Vader 5 Pro project <maintainer@localhost>
Section: utils
Priority: optional
Depends: $deps, libxkbcommon0, libxkbcommon-x11-0, libegl1, libgl1, libx11-6, libxcursor1, libxi6, libxrandr2, libwayland-client0
Description: Desktop profile editor for the Flydigi Vader 5 Pro controller
EOF
dpkg-deb --root-owner-group --build "$tmp/pkg" "$out/vader5_${version}_${arch}.deb"
