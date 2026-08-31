# aaa-ui

给 `~/.local/bin/aaa` 长出的常驻 daemon + 双端原生客户端：Mac 上会话常驻不掉线，手机上两步答一句。

```
┌─────────────┐   tailscale / easytier    ┌──────────────────┐   spawn PTY    ┌─────────────┐
│ aaa-ui-mac  │◄────── REST + WS ────────►│    aaa-daemon    │◄──────────────►│ agent CLIs  │
│ (gpui 原生) │       Bearer token        │  Mac mini :2730  │    读会话存储  │ + zsh 终端  │
├─────────────┤                           │  launchd 常驻    │                └─────────────┘
│ aaa-android │◄──────────────────────────│  PTY 池 + VT     │
│ (Kotlin 原生)│                          └──────────────────┘
└─────────────┘
```

| 目录 | 内容 | 验证状态 |
|---|---|---|
| `daemon/` | Rust 常驻服务：PTY 池 + 服务端 VT + 全套 API + aaa 逻辑移植 + TCC 权限 + checkpoint/消息流/收件箱/watchdog | 66 测试全绿，E2E 冒烟 + 独立复核通过 |
| `mac/` | gpui 原生客户端（tty7 路线，alacritty_terminal + 自绘渲染） | 31 测试全绿，build 0 warning |
| `android/` | Kotlin/Compose 原生客户端（vendor Termux 终端引擎，无 WebView） | 166 测试全绿（含 2 项真 daemon 集成） |
| `PROTOCOL.md` | 三端唯一契约（API/状态机/设计令牌/v1.1 扩展） | — |
| `prototype.html` | 双端交互原型（已确认），claude.ai artifact 同步发布 | — |

## 部署（Mac，一次性）

```bash
# 1. 安装 daemon 到固定路径（launchd plist 记录绝对路径，别用 target/ 里的）
cp daemon/target/release/aaa-daemon ~/.local/bin/aaa-daemon

# 2. 首次运行生成 ~/.config/aaa-daemon/config.toml（含随机 token），确认能起来后 Ctrl-C
aaa-daemon run

# 3. launchd 常驻（KeepAlive；日志在 ~/.local/state/aaa-daemon/）
aaa-daemon service install        # 对应 uninstall / status

# 4. 一键申请全部 macOS 权限（弹窗在 Mac 上逐个允许；授权归到 daemon，所有 agent 子进程共享，
#    此后手机远程操作不再被权限弹窗卡死；amcu 在 agent 链下运行时同样被覆盖）
aaa-daemon perms request-all      # perms status 查看八项状态

# 5.（可选）Claude 精确「等待输入」信号：原子/幂等合并 Notification/Stop hooks
aaa-daemon install-claude-hooks
```

config.toml 要点：`port=2730`（=0xAAA）、`project_root=/Volumes/SSD/project`、`namer`（haiku 会话命名）、
`[checkpoint] enabled/auto_init_git/interval_minutes/auto_init_max_mb`（超 512MB 的无 .git 目录跳过自动 init，护栏防 `.git/objects` 暴涨）、`[watchdog] stall_minutes/auto_kill`、`[ntfy] url/topic`（离线推送兜底）。
> 部署提醒：`service install` 的 plist 记录二进制的当前绝对路径（`current_exe`），移动/覆盖二进制后需 `service uninstall && service install` 重新登记。
与 aaa CLI 磁盘格式双向兼容（`.aaa-agents` 注册表、`~/.cache/aaa-cwds.json`），两者可混用。
SSD 未挂载时 daemon 只读降级、绝不 mkdir 项目根。

## macOS 客户端

```bash
cd mac
export PATH="$HOME/.rustup/toolchains/stable-aarch64-apple-darwin/bin:$PATH"
cargo run          # 同机自动读 ~/.config/aaa-daemon/config.toml 连 127.0.0.1:2730
```

会话侧栏（状态点：绿=运行 黄=等待输入 灰=空闲 红=退出）、多标签终端（选区/复制/CJK）、
waiting 选项胶囊直接点、项目表格（列与 aaa CLI 一致）、系统通知、设置页出配对二维码。

## Android 客户端

```bash
cd android
export JAVA_HOME=/opt/homebrew/opt/openjdk@21/libexec/openjdk.jdk/Contents/Home  # lint 需 21；assemble/test 26 也可
export ANDROID_HOME=$HOME/Library/Android/sdk
./gradlew assembleDebug            # → app/build/outputs/apk/debug/app-debug.apk
adb install -r app/build/outputs/apk/debug/app-debug.apk
```

配对：扫 Mac 设置页二维码，或手输 `host:2730` + token（多 host 依次试连，tailscale 优先）。
主视图为**消息流**（claude 完整解析；不支持的 agent 自动回落终端），右上可切终端，设置里改默认 UI。
通知：等待输入 = 高优先级 + **通知栏内联回复** + 选项按钮；完成/空转分级；按项目静音。
其余：diff 卡片 + 一键回滚（后悔药）、任务收件箱（agent 等待时自动喂入）、系统分享 → 上传进项目 `_inbox/`、快捷短语 chips、前台服务保活。

## 日常动线

1. **手机答一句**：推送到达 → 通知栏直接点选项/内联回复（零步进 App）。
2. **无缝接力**：Mac 开的会话 = 手机同一 PTY，断线重连整屏重绘。
3. **开新活**：＋ → 选 agent（Claude/Codex/Pi/Reasonix/Antigravity/普通终端）→ 秒进；名称留空用时间戳。
4. **项目治理**：换 agent / 批量删除（purge 语义与 CLI 的 d 键一致，删前列明各 agent 会话条数）。

## 已知限制

- 活会话不跨 daemon 重启（exited 回放会恢复）；重启 daemon 前先收尾要紧会话。
- checkpoint 默认开启：无 .gitignore 的大目录首个检查点会使 `.git/objects` 一次性膨胀。
- codex/pi 消息流解析基于 2026-08 存储格式样本，上游变更需跟进；reasonix/agy 回落终端。
- Android 深度 Doze 下 events WS 可能被限流，离线兜底走 ntfy；attach 断线重连后回滚缓冲有一段重复（当前屏正确）。
- mac 端菜单栏 NSStatusItem、消息流视图为后续项。

## 开发

```bash
# daemon（cargo 不在 PATH：rustup run stable）
cd daemon && rustup run stable cargo test          # 66 tests
# mac（gpui 锁定 zed v1.17.2；本机无 Xcode → runtime_shaders 特性）
cd mac && rustup run stable cargo test             # 31 tests
# android（Termux 引擎 vendor 自 termux-app，见 android/NOTICE.md）
cd android && ./gradlew test                       # 166 tests
```

契约变更流程：先改 `PROTOCOL.md`，再改三端。事件帧向前兼容（未知帧忽略）。
