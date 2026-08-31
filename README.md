# AAA

一个常驻 daemon + 三个原生客户端，把 `aaa` 那个交互脚本变成随时随地能用的东西：
Mac 上会话常驻不掉线，手机上两步答一句，终端里 `aaa` 依然一敲就开。

```
┌─────────────┐   tailscale / easytier    ┌──────────────────┐   spawn PTY    ┌─────────────┐
│  AAA.app    │◄────── REST + WS ────────►│    aaa-daemon    │◄──────────────►│ agent CLIs  │
│ (gpui 原生) │        Bearer token       │  Mac mini :2730  │    读会话存储   │ + zsh 终端  │
├─────────────┤                           │  launchd 常驻    │                └─────────────┘
│ AAA android │◄──────────────────────────│  PTY 池 + VT     │
│(Kotlin 原生)│                           └──────────────────┘
├─────────────┤                                    ▲
│  aaa (CLI)  │◄───────────────────────────────────┘
│ (Rust 终端) │       同一套 API，同一批会话
└─────────────┘
```

三个客户端没有主次：**同一个会话可以在终端里开、在 Mac App 里接着看、在手机上回答一句**，
因为 PTY 活在 daemon 里，谁都只是接上去而已。

| 目录 | 内容 |
|---|---|
| `daemon/` | Rust 常驻服务：PTY 池 + 服务端 VT + 全套 API + aaa 逻辑移植 + TCC 权限 + checkpoint/消息流/收件箱/watchdog。**新的 `aaa` CLI 也在这里**（`src/cli/`），与 daemon 共用配置类型 |
| `mac/` | gpui 原生客户端（tty7 路线，alacritty_terminal + 自绘渲染） |
| `android/` | Kotlin/Compose 原生客户端（vendor Termux 终端引擎，无 WebView） |
| `brand/` | 品牌标志：`logo.py` 一次运行导出 macOS `.icns` 与 Android 各密度自适应图标 |
| `PROTOCOL.md` | 四方唯一契约（API/状态机/CLI/设计令牌） |
| `prototype.html` | 双端交互原型（已确认） |

## 部署（Mac，一次性）

```bash
# 1. daemon + CLI 一起编出来（daemon crate 有两个 bin）
cd daemon && cargo build --release
cp target/release/aaa-daemon ~/.local/bin/aaa-daemon
cp target/release/aaa        ~/.local/bin/aaa

# 2. 首次运行生成 ~/.config/aaa-daemon/config.toml（含随机 token），确认能起来后 Ctrl-C
aaa-daemon run

# 3. launchd 常驻（KeepAlive；日志在 ~/.local/state/aaa-daemon/）
aaa-daemon service install        # 对应 uninstall / status

# 3.5 项目根在外置卷上时，这一步是必须的（见下）
open "x-apple.systempreferences:com.apple.preference.security?Privacy_AllFiles"

# 4. 一键申请全部 macOS 权限（弹窗在 Mac 上逐个允许；授权归到 daemon，所有 agent 子进程共享，
#    此后手机远程操作不再被权限弹窗卡死）。也可以在 `aaa` 菜单里的「macOS 权限」进
aaa perms all                     # 或 aaa perms 只看状态

# 5.（可选）Claude 精确「等待输入」信号：原子/幂等合并 Notification/Stop hooks
aaa-daemon install-claude-hooks
```

config.toml 要点：`port=2730`（=0xAAA）、`project_root=/Volumes/SSD/project`、`namer`（haiku 会话命名）、
`[checkpoint] enabled/auto_init_git/interval_minutes/auto_init_max_mb`、`[watchdog] stall_minutes/auto_kill`、
`[ntfy] url/topic`（离线推送兜底）。

> **`auto_init_git` 默认 false**：项目常常只是一个任务目录（笔记、抓取、一堆 yml），
> 替你 `git init` 不是 daemon 该做的事。已经是 git 仓库的项目照常有检查点/diff/回滚；
> 想让非仓库目录也享受后悔药，再显式打开（届时 `auto_init_max_mb` 护栏仍生效）。

> **项目根在外置卷（`/Volumes/…`）上时，daemon 需要「完全磁盘访问权限」。**
> macOS 把可移动卷挡在 TCC 后面，而 launchd 起的进程没有任何「负责 App」持有这个授权——
> 现象不是报错而是**卡死**：`stat` 能过，`opendir` 一直等一个没人去点的授权弹窗。
> （拿 `/bin/ls` 做成 launchd job 一样会 `Operation not permitted`，这是系统策略不是 daemon 的锅。）
> 到「系统设置 → 隐私与安全性 → 完全磁盘访问权限」把 `~/.local/bin/aaa-daemon` 加进去打开，
> 然后 `aaa-daemon service uninstall && aaa-daemon service install`。
> daemon 自身不会因此卡住：读不到项目根时它照常监听、`/health.root_state=denied`，并在日志和 API
> 错误里直接给出这段修法。
> 注意 ad-hoc 签名每次重新构建都会让这个授权失效，需要重新勾一次。

> **升级二进制时先 `rm` 再 `cp`**：直接 `cp` 覆盖正在运行的二进制会写坏它的 ad-hoc 签名，
> 之后每次执行都被 macOS 直接 `SIGKILL`（现象是命令无输出、退出码 137）。
> ```bash
> rm -f ~/.local/bin/aaa-daemon && cp daemon/target/release/aaa-daemon ~/.local/bin/aaa-daemon
> codesign --force --sign - ~/.local/bin/aaa-daemon
> aaa-daemon service uninstall && aaa-daemon service install   # plist 记的是绝对路径，重新登记
> ```

与旧 CLI 磁盘格式双向兼容（`.aaa-agents` 注册表、`~/.cache/aaa-cwds.json`）。
SSD 未挂载时 daemon 只读降级、绝不 mkdir 项目根。

## aaa（终端）

```
aaa                      交互菜单（活会话在上，等待输入的排最前）
aaa ls [-a] [--json]     会话列表          aaa ps [--json]     项目列表
aaa new [名字] [-a AGENT]  新建项目目录 + 开会话并接入
aaa open <目标>           进入项目（有活会话就接回，否则 resume）
aaa attach <目标>         接入会话（Ctrl-] 脱离，会话继续跑）
aaa say <目标> <文本…>     写一句进会话并回车（回答提问用）
aaa kill / rm / rename <目标>
aaa wait [--json]        只列等待输入的会话（脚本/通知用）
aaa status [--json]      daemon 状态       aaa perms [all|<id…>]  macOS 权限
```

目标可写：会话 id 或其前缀、`aaa ls` 里的序号、项目名、或 `.`（当前目录所属项目）。

连接默认读 `~/.config/aaa-daemon/config.toml`；`AAA_HOST=主机:2730 AAA_TOKEN=…` 可指向另一台机器的 daemon
（tailscale 直接过去，不用 SSH）。本机 daemon 掉了会先试一次 `launchctl kickstart` 再报错。

> 旧的 zsh 菜单脚本改名 **`aaal`** 留在 `~/.local/bin/`：它不经过 daemon，把 agent 直接跑在当前终端里，
> 是 daemon 不可用时的兜底。

## macOS 客户端

```bash
cd mac
export PATH="$HOME/.rustup/toolchains/stable-aarch64-apple-darwin/bin:$PATH"
packaging/bundle.sh build      # → mac/target/AAA.app
packaging/bundle.sh install    # → /Applications，之后 Spotlight 直接启动
```

同机启动会自动读 `~/.config/aaa-daemon/config.toml` 连 `127.0.0.1:2730`，无需配置。
> 签名是 ad-hoc（本机无开发者证书），**TCC 授权绑定签名、每次重新构建即失效**——这正是权限由 daemon
> 而非客户端持有的原因。

左侧会话栏（状态点：绿=运行 黄=等待输入 灰=空闲 红=退出，行内 × 关闭，栏宽可拖）、
终端（选区/选中复制/右键直接粘贴/Ctrl-V·Cmd-V/链接点击/CJK）、waiting 选项胶囊直接点、项目表格、系统通知、设置页出配对二维码。

## Android 客户端

```bash
cd android
export JAVA_HOME=/opt/homebrew/opt/openjdk@21/libexec/openjdk.jdk/Contents/Home
export ANDROID_HOME=$HOME/Library/Android/sdk
./gradlew assembleDebug            # → app/build/outputs/apk/debug/app-debug.apk
adb install -r app/build/outputs/apk/debug/app-debug.apk
```

配对：扫 Mac 设置页二维码，或手输 `host:2730` + token（多 host 依次试连，tailscale 优先）。
主视图为**消息流**（claude 完整解析；不支持的 agent 自动回落终端），右上可切终端，设置里改默认 UI。
通知：等待输入 = 高优先级 + **通知栏内联回复** + 选项按钮；完成/空转分级；按项目静音。
其余：diff 卡片 + 一键回滚、任务收件箱、系统分享 → 上传进项目 `_inbox/`、快捷短语 chips、前台服务保活、
折叠屏/大屏展开后底栏收成左侧导航 rail（内容仍单栏）。

## 日常动线

1. **手机答一句**：推送到达 → 通知栏直接点选项/内联回复（零步进 App）。
2. **无缝接力**：终端 `aaa` 开的会话 = Mac App 里的同一个 = 手机上的同一个 PTY。
3. **开新活**：`aaa new 任务名` 或菜单里 New \<agent\>；只建目录 + 开会话，不碰 git。
4. **项目治理**：换 agent / 批量删除（purge 语义与旧 CLI 的 d 键一致）。

## 已知限制

- 活会话不跨 daemon 重启（exited 回放会恢复）；重启 daemon 前先收尾要紧会话。
- codex/pi 消息流解析基于 2026-08 存储格式样本，上游变更需跟进；reasonix/agy 回落终端。
- Android 深度 Doze 下 events WS 可能被限流，离线兜底走 ntfy。
- `refs/aaa-ckpt` 只增不减，尚无 GC。

## 开发

```bash
cd daemon  && cargo test           # 109 tests（含 CLI）
cd mac     && cargo test           # 47 tests
cd android && ./gradlew test        # 178 tests
```

契约变更流程：先改 `PROTOCOL.md`，再改三端。事件帧向前兼容（未知帧忽略）。
