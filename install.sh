#!/usr/bin/env bash
# AAA 安装脚本：daemon + aaa CLI（macOS / Linux）
#
#   curl -fsSL https://raw.githubusercontent.com/uoox/aaa/main/install.sh | bash
#
# 做的事：
#   1. 取二进制：macOS arm64 从 GitHub Release 下载；其它平台用 cargo 从源码编（需要 Rust）
#   2. 装到 ~/.local/bin（AAA_BIN 可改）；macOS 上用固定签名身份签名（有 "AAA Local Signing" 证书时）
#   3. 首次运行生成 ~/.config/aaa-daemon/config.toml（随机 token），登记为常驻服务
#      （macOS launchd / Linux systemd --user）
# 环境变量：AAA_VERSION 指定版本（默认最新）；AAA_BIN 安装目录；AAA_NO_SERVICE=1 只装不登记。
set -euo pipefail

REPO="uoox/aaa"
BIN="${AAA_BIN:-$HOME/.local/bin}"
VERSION="${AAA_VERSION:-}"
OS="$(uname -s)"; ARCH="$(uname -m)"

say() { printf '\033[1m==> %s\033[0m\n' "$*"; }
die() { printf '\033[31m%s\033[0m\n' "$*" >&2; exit 1; }
need() { command -v "$1" >/dev/null 2>&1 || die "需要 $1"; }

need curl; need python3
mkdir -p "$BIN"

if [ -z "$VERSION" ]; then
  VERSION="$(curl -fsSL "https://api.github.com/repos/$REPO/releases/latest" | python3 -c 'import sys,json;print(json.load(sys.stdin)["tag_name"])')"
fi
VER="${VERSION#v}"
BASE="https://github.com/$REPO/releases/download/$VERSION"
TMP="$(mktemp -d)"; trap 'rm -rf "$TMP"' EXIT

install_prebuilt() {
  say "下载 aaa-cli-$VER-macos-arm64.tar.gz"
  curl -fsSL "$BASE/aaa-cli-$VER-macos-arm64.tar.gz" -o "$TMP/cli.tgz"
  tar -xzf "$TMP/cli.tgz" -C "$TMP"
  rm -f "$BIN/aaa-daemon"            # 覆盖运行中的二进制会写坏签名，先删再放
  install -m755 "$TMP/aaa-daemon" "$BIN/aaa-daemon"
  install -m755 "$TMP/aaa" "$BIN/aaa"
}

install_from_source() {
  command -v cargo >/dev/null 2>&1 || die "这个平台没有预编译包，需要 Rust（https://rustup.rs），装好后重跑"
  say "从源码编译 daemon（$OS $ARCH）"
  curl -fsSL "https://github.com/$REPO/archive/refs/tags/$VERSION.tar.gz" -o "$TMP/src.tgz"
  tar -xzf "$TMP/src.tgz" -C "$TMP"
  local src; src="$(find "$TMP" -maxdepth 1 -type d -name 'aaa-*' | head -1)"
  (cd "$src/daemon" && cargo build --release --quiet)
  rm -f "$BIN/aaa-daemon"
  install -m755 "$src/daemon/target/release/aaa-daemon" "$BIN/aaa-daemon"
  install -m755 "$src/cli/aaa" "$BIN/aaa"
}

case "$OS-$ARCH" in
  Darwin-arm64) install_prebuilt ;;
  *) install_from_source ;;
esac

if [ "$OS" = "Darwin" ]; then
  if security find-identity -v -p codesigning 2>/dev/null | grep -q "AAA Local Signing"; then
    # -i 把 identifier 钉死：授权认的是 `identifier "aaa-daemon" and certificate leaf = …`
    # 这一整句，不钉死就得靠「codesign 会拿文件名当 identifier」这个巧合
    codesign --force --sign "AAA Local Signing" -i aaa-daemon "$BIN/aaa-daemon"
    say "已用 AAA Local Signing 签名（TCC 授权跨升级保留）"
  else
    codesign --force --sign - -i aaa-daemon "$BIN/aaa-daemon"
    say "ad-hoc 签名。若项目根在外置卷上，建议做一个「AAA Local Signing」自签名证书，见 daemon/README.md"
  fi
fi

case ":$PATH:" in *":$BIN:"*) ;; *) say "提示：$BIN 不在 PATH 里，把它加进 shell 配置" ;; esac

if [ "${AAA_NO_SERVICE:-}" != "1" ]; then
  say "登记常驻服务"
  "$BIN/aaa-daemon" service uninstall >/dev/null 2>&1 || true
  "$BIN/aaa-daemon" service install
fi

say "完成：aaa-daemon $VER 与 aaa 已装到 $BIN"
echo "  配置：~/.config/aaa-daemon/config.toml（首次启动自动生成 token）"
echo "  试试：aaa status"
echo "  Mac App / Android APK 在 https://github.com/$REPO/releases/tag/$VERSION"
