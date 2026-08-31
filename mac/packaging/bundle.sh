#!/bin/bash
# AAA 打包脚本：cargo release 构建 → 组装 .app → ad-hoc 签名 →（可选）安装到 /Applications
#
# 用法：
#   packaging/bundle.sh              # = build
#   packaging/bundle.sh build        # 产出 mac/target/AAA.app
#   packaging/bundle.sh install      # build + 覆盖安装到 /Applications（先退出正在运行的实例）
#
# 已知约束（本机无 Apple 开发者证书，仅 CommandLineTools）：
# - 使用 ad-hoc 签名（codesign --sign -）。ad-hoc 签名没有稳定身份，
#   每次重新构建后 TCC（通知/自动化等）授权会失效需重新授予——这正是把权限
#   归责到 aaa-daemon 的原因（见 PROTOCOL.md），UI 本体不申请任何 TCC 权限。
# - 本机构建的 app 无 quarantine 属性，可直接打开；若拷贝到其他机器，
#   首次打开需「系统设置 → 隐私与安全性」放行，或 `xattr -dr com.apple.quarantine`。
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/../.." && pwd)"   # 仓库根（…/aaa-ui）
MAC="$ROOT/mac"
ICNS="$ROOT/brand/out/aaa-ui.icns"
APP_NAME="AAA"
APP="$MAC/target/$APP_NAME.app"
BIN="aaa-ui-mac"
BUNDLE_ID="cc.uoox.aaaui"

# cargo 不在 PATH 时补 rustup stable 工具链
command -v cargo >/dev/null 2>&1 || \
    export PATH="$HOME/.rustup/toolchains/stable-aarch64-apple-darwin/bin:$PATH"

VERSION="$(sed -n 's/^version *= *"\(.*\)"/\1/p' "$MAC/Cargo.toml" | head -1)"
BUILD_NUM="$(git -C "$ROOT" rev-parse --short HEAD 2>/dev/null || date +%Y%m%d%H%M)"

build() {
    echo "==> cargo build --release (v$VERSION, build $BUILD_NUM)"
    (cd "$MAC" && cargo build --release)

    echo "==> 组装 $APP"
    rm -rf "$APP"
    mkdir -p "$APP/Contents/MacOS" "$APP/Contents/Resources"
    cp "$MAC/target/release/$BIN" "$APP/Contents/MacOS/$BIN"
    cp "$ICNS" "$APP/Contents/Resources/aaa-ui.icns"

    cat > "$APP/Contents/Info.plist" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
	<key>CFBundleDevelopmentRegion</key>
	<string>zh-Hans</string>
	<key>CFBundlePackageType</key>
	<string>APPL</string>
	<key>CFBundleInfoDictionaryVersion</key>
	<string>6.0</string>
	<key>CFBundleIdentifier</key>
	<string>$BUNDLE_ID</string>
	<key>CFBundleName</key>
	<string>$APP_NAME</string>
	<key>CFBundleDisplayName</key>
	<string>$APP_NAME</string>
	<key>CFBundleExecutable</key>
	<string>$BIN</string>
	<key>CFBundleIconFile</key>
	<string>aaa-ui</string>
	<key>CFBundleShortVersionString</key>
	<string>$VERSION</string>
	<key>CFBundleVersion</key>
	<string>$BUILD_NUM</string>
	<key>LSMinimumSystemVersion</key>
	<string>12.0</string>
	<key>LSApplicationCategoryType</key>
	<string>public.app-category.developer-tools</string>
	<key>NSHighResolutionCapable</key>
	<true/>
	<key>NSSupportsAutomaticGraphicsSwitching</key>
	<true/>
	<key>NSHumanReadableCopyright</key>
	<string>Apache-2.0</string>
</dict>
</plist>
PLIST

    plutil -lint "$APP/Contents/Info.plist"

    echo "==> ad-hoc 签名"
    codesign --force --deep --sign - "$APP"
    codesign --verify --strict "$APP"
    codesign -dv "$APP" 2>&1 | sed -n '1,4p'
    echo "==> OK: $APP"
}

install_app() {
    build
    local dest="/Applications/$APP_NAME.app"
    if pgrep -xq "$BIN"; then
        echo "==> 退出正在运行的实例"
        osascript -e "tell application id \"$BUNDLE_ID\" to quit" >/dev/null 2>&1 || true
        for _ in $(seq 1 20); do pgrep -xq "$BIN" || break; sleep 0.25; done
        pkill -x "$BIN" 2>/dev/null || true
    fi
    echo "==> 安装到 $dest"
    # 2026-08-31 前叫 "AAA UI.app"；改名后清掉旧的，免得 Spotlight 里两个都在
    rm -rf "/Applications/AAA UI.app"
    rm -rf "$dest"
    ditto "$APP" "$dest"
    codesign --verify --strict "$dest"
    echo "==> 已安装。提示：重新构建安装后 ad-hoc 签名变化，此前授予的 TCC 权限会失效。"
    echo "    启动：open -a \"$APP_NAME\""
}

case "${1:-build}" in
    build)   build ;;
    install) install_app ;;
    *) echo "用法: $0 [build|install]" >&2; exit 2 ;;
esac
