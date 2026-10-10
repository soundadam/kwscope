#!/usr/bin/env bash
# 在当前平台构建 release 并打包到 dist/。
#   macOS:   tonescope-<ver>-macos-universal.zip（ad-hoc 签名的 Tonescope.app）
#   Windows: tonescope-<ver>-windows-<arch>.zip（Git Bash 下运行，需要 7z）
#   Linux:   tonescope-<ver>-linux-<arch>.tar.gz
set -euo pipefail
cd "$(dirname "$0")/.."

pkgid=$(cargo pkgid)
version=${pkgid##*[#@]}
target_dir=${CARGO_TARGET_DIR:-target}
arch=$(uname -m)
extras=(README.md LICENSE-MIT LICENSE-APACHE)

rm -rf dist
mkdir -p dist

case "$(uname -s)" in
Darwin)
    for t in aarch64-apple-darwin x86_64-apple-darwin; do
        cargo build --release --locked --target "$t"
    done
    stage=$(mktemp -d)
    app="$stage/Tonescope.app"
    mkdir -p "$app/Contents/MacOS"
    lipo -create -output "$app/Contents/MacOS/tonescope" \
        "$target_dir"/{aarch64,x86_64}-apple-darwin/release/tonescope
    sed "s/@VERSION@/$version/g" packaging/macos/Info.plist >"$app/Contents/Info.plist"
    # 没有开发者证书时只能 ad-hoc；Apple Silicon 不接受未签名的 bundle
    codesign --force --sign - "$app"
    cp "${extras[@]}" "$stage/"
    (cd "$stage" && ditto -c -k --sequesterRsrc . "$OLDPWD/dist/tonescope-$version-macos-universal.zip")
    rm -rf "$stage"
    ;;
Linux)
    cargo build --release --locked
    name="tonescope-$version-linux-$arch"
    mkdir -p "dist/$name"
    cp "$target_dir/release/tonescope" "${extras[@]}" "dist/$name/"
    tar -C dist -czf "dist/$name.tar.gz" "$name"
    rm -rf "dist/${name:?}"
    ;;
MINGW* | MSYS* | CYGWIN*)
    cargo build --release --locked
    name="tonescope-$version-windows-$arch"
    mkdir -p "dist/$name"
    cp "$target_dir/release/tonescope.exe" "${extras[@]}" "dist/$name/"
    (cd dist && 7z a -bso0 "$name.zip" "$name")
    rm -rf "dist/${name:?}"
    ;;
*)
    echo "unsupported platform: $(uname -s)" >&2
    exit 1
    ;;
esac

ls -l dist
