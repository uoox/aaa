#!/bin/sh
# 把刚编译出来的 daemon 装到 ~/.local/bin 并重启。
#
# 用法：daemon/packaging/install.sh [--no-restart]
#
# **不要用 `cp` 代替它。** macOS 的隐私授权（辅助功能 / 屏幕录制 / 输入监控 / 自动化）
# 记的是二进制的**代码身份**，不是路径，所以升级这一步有两件事一件都不能少：
#
#   ① 先签好再 rename，绝不就地覆盖。`cp` 是把新内容写进同一个 inode，正在跑的那个
#      daemon 的代码页当场失效，它**立刻**就通不过 TCC 校验——不用等重启，手里的
#      授权当场没。rename 只换目录项，跑着的进程还拿着老 inode，一直有效到它退出。
#   ② 用固定的自签证书 + 固定 identifier 签。cargo 产出的是 ad-hoc 签名，身份随每次
#      编译变，装上去就等于换了一个 App，之前给的授权全部作废。
#
# 细节见 daemon/src/service.rs 顶部那段注释。
set -eu

REPO="$(cd "$(dirname "$0")/../.." && pwd)"
BIN="${AAA_BIN_DIR:-$HOME/.local/bin}"
SRC="$REPO/daemon/target/release/aaa-daemon"
DST="$BIN/aaa-daemon"

[ -f "$SRC" ] || { echo "先编译：cd daemon && cargo build --release" >&2; exit 1; }
mkdir -p "$BIN"

TMP="$DST.incoming"
rm -f "$TMP"
cp "$SRC" "$TMP"
chmod 755 "$TMP"

if [ "$(uname -s)" = "Darwin" ]; then
  if security find-identity -v -p codesigning 2>/dev/null | grep -q "AAA Local Signing"; then
    codesign --force --sign "AAA Local Signing" -i aaa-daemon "$TMP"
    codesign --verify --strict "$TMP"
    echo "==> 已签：AAA Local Signing / aaa-daemon（隐私授权跨升级保留）"
  else
    codesign --force --sign - -i aaa-daemon "$TMP"
    echo "==> 没有「AAA Local Signing」自签证书，退回 ad-hoc：这次升级之后要重新授权一次"
    echo "    建一张（一次就够，之后所有升级都免授权）：见 daemon/README.md"
  fi
fi

mv -f "$TMP" "$DST"
echo "==> 装好：$DST"
codesign -d -r- "$DST" 2>&1 | sed -n 's/^designated/    designated/p'

[ "${1:-}" = "--no-restart" ] && exit 0
echo "==> 重启"
"$DST" service restart
