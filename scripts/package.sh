#!/usr/bin/env bash
# 在当前平台构建 release 并打包到 dist/。
#   macOS:   kwscope-<ver>-macos-universal.zip（ad-hoc 签名的 Kwscope.app；只有 arm64，名字是 appkit 更新器认的）
#   Windows: kwscope-<ver>-windows-<arch>.zip（Git Bash 下运行，需要 7z）
#   Linux:   kwscope-<ver>-linux-<arch>.tar.gz
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
    # 预编译的 ONNX Runtime 最低就是 13.4
    export MACOSX_DEPLOYMENT_TARGET=13.4
    # ONNX Runtime 已不出 x86_64 的 macOS 库（ort 的预编译与官方发布都没有），只构建 Apple Silicon
    cargo build --release --locked --target aarch64-apple-darwin
    stage=$(mktemp -d)
    app="$stage/Kwscope.app"
    mkdir -p "$app/Contents/MacOS" "$app/Contents/Resources"
    cp "$target_dir/aarch64-apple-darwin/release/kwscope" "$app/Contents/MacOS/kwscope"
    sed "s/@VERSION@/$version/g" packaging/macos/Info.plist >"$app/Contents/Info.plist"
    cp packaging/icon/kwscope.icns "$app/Contents/Resources/"
    # 没有开发者证书时只能 ad-hoc；Apple Silicon 不接受未签名的 bundle
    codesign --force --sign - "$app"
    cp "${extras[@]}" "$stage/"
    (cd "$stage" && ditto -c -k --sequesterRsrc . "$OLDPWD/dist/kwscope-$version-macos-universal.zip")
    rm -rf "$stage"
    ;;
Linux)
    cargo build --release --locked
    name="kwscope-$version-linux-$arch"
    mkdir -p "dist/$name"
    cp "$target_dir/release/kwscope" "${extras[@]}" "dist/$name/"
    tar -C dist -czf "dist/$name.tar.gz" "$name"
    rm -rf "dist/${name:?}"
    ;;
MINGW* | MSYS* | CYGWIN*)
    cargo build --release --locked
    name="kwscope-$version-windows-$arch"
    mkdir -p "dist/$name"
    cp "$target_dir/release/kwscope.exe" "${extras[@]}" "dist/$name/"
    (cd dist && 7z a -bso0 "$name.zip" "$name")
    rm -rf "dist/${name:?}"
    ;;
*)
    echo "unsupported platform: $(uname -s)" >&2
    exit 1
    ;;
esac

ls -l dist
