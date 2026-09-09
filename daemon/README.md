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
`auto_trust`（默认 true，新项目的 Claude Code 信任对话框自动回车）。

**升级用 `daemon/packaging/install.sh`，别用 `cp`。** 它做的三件事一件都不能少：写临时文件 →
用固定身份签 → `rename` 顶上去 → `service restart`。就地覆盖会让**正在跑的**那个 daemon 的
代码页当场失效，它立刻通不过 TCC 校验（辅助功能 / 屏幕录制那些授权当场就没了，不用等重启），
严重时还会被 `SIGKILL`；而不签或用 ad-hoc 签，新进程就是「另一个 App」，授权也得重给。
换了二进制路径要重新 `service uninstall && service install`，plist 记的是绝对路径。

## 项目根在外置卷上

macOS 把 `/Volumes/…` 挡在 TCC 后面，launchd 起的进程没有「负责 App」持有授权，
现象是 `opendir` 卡死而不是报错。到「系统设置 → 隐私与安全性 → 完全磁盘访问权限」
把 `~/.local/bin/aaa-daemon` 加进去，再重装 service。读不到项目根时 daemon 照常监听，
`/health.root_state=denied`，错误里给出这段修法。

**TCC 授权认的是代码身份，不是路径。** 授权时系统记下这个二进制的 designated requirement，
之后每次请求都拿它比对；本机自签证书签出来的是

```
designated => identifier "aaa-daemon" and certificate leaf = H"<证书指纹>"
```

——**与文件内容无关**，所以只要每次都用同一张证书、同一个 identifier 签，重新编译多少次授权都还在。
ad-hoc 签名（`--sign -`）的身份里带着随构建变的 identifier 和 cdhash，换一次就等于换一个 App。

一次性：在「钥匙串访问 → 证书助理 → 创建证书」里做一个**代码签名**类型的自签名证书，
名字叫 `AAA Local Signing`。之后 `install.sh`、`daemon/packaging/install.sh`、
`aaa-daemon service install` 都会自动用它。

已经装歪了（比如有人用 `cp` 覆盖过）就补一刀，**它会自己走临时文件 + rename，不动正在跑的进程**：

```bash
aaa-daemon service sign      # 只补签
aaa-daemon service restart   # 补签 + 重启（daemon 关着也能用）
```

## 开发

```bash
cargo test
```

活会话不跨 daemon 重启（已退出会话的回放会恢复）。

## plan 配额

5h / 7d 窗口来自 Claude Code statusLine 转来的 `rate_limits`；按模型的周窗口（Fable）statusLine 没有，
`quota.rs` 每分钟 `GET https://api.anthropic.com/api/oauth/usage` 取 `limits[]` 里的 `weekly_scoped`。
令牌读 Claude Code 自己的：macOS 钥匙串 `Claude Code-credentials`（`security find-generic-password`），
其它平台 `~/.claude/.credentials.json`（`CLAUDE_CONFIG_DIR` 优先）。只读不刷新，过期就等 Claude Code 下次自己续；
拉不到只在日志里报一次，客户端沿用旧值。
