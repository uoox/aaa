# aaa-daemon

常驻在 Mac 上的服务：持有 Claude Code 与 shell 的 PTY，向三个客户端提供 REST/WS API。
接口契约见 `../PROTOCOL.md`。

## 安装

```bash
cargo build --release
rm -f ~/.local/bin/aaa-daemon && cp target/release/aaa-daemon ~/.local/bin/aaa-daemon
aaa-daemon run                 # 首次运行生成 ~/.config/aaa-daemon/config.toml（含随机 token），能起来后 Ctrl-C
aaa-daemon service install     # launchd 常驻（KeepAlive），对应 uninstall / status；日志在 ~/.local/state/aaa-daemon/
aaa perms all                  # 一次申请全部 macOS 权限，授权归到 daemon，所有子进程共享
```

config.toml：`port`（默认 2730）、`token`、`project_root`（默认 `~/project`）、`namer`（haiku 会话命名）、
`auto_trust`（默认 true，新项目的 Claude Code 信任对话框自动回车）、`[checkpoint]`。

升级二进制先 `rm` 再 `cp`：直接覆盖运行中的二进制会写坏签名，之后每次执行都被 macOS `SIGKILL`。
换了二进制路径要重新 `service uninstall && service install`，plist 记的是绝对路径。

## 项目根在外置卷上

macOS 把 `/Volumes/…` 挡在 TCC 后面，launchd 起的进程没有「负责 App」持有授权，
现象是 `opendir` 卡死而不是报错。到「系统设置 → 隐私与安全性 → 完全磁盘访问权限」
把 `~/.local/bin/aaa-daemon` 加进去，再重装 service。读不到项目根时 daemon 照常监听，
`/health.root_state=denied`，错误里给出这段修法。

TCC 授权绑定代码签名，ad-hoc 签名（`--sign -`）每次构建都不同，授权随之失效。
在钥匙串访问里做一个「代码签名」类型的自签名证书（下面叫 `AAA Local Signing`），每次装 daemon 都用它签：

```bash
if security find-identity -v -p codesigning | grep -q "AAA Local Signing"; then
  codesign --force --sign "AAA Local Signing" ~/.local/bin/aaa-daemon
else
  codesign --force --sign - ~/.local/bin/aaa-daemon
fi
```

## 开发

```bash
cargo test
```

活会话不跨 daemon 重启（已退出会话的回放会恢复）。
