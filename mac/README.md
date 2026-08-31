# aaa-ui-mac

macOS 原生 GUI 客户端（gpui GPU 渲染，非 webview），连接本机 aaa-daemon（REST + WS，契约见 `../PROTOCOL.md`）。

## 构建与运行

```bash
# cargo 不在 PATH 时：
export PATH="$HOME/.rustup/toolchains/stable-aarch64-apple-darwin/bin:$PATH"

cargo build          # 首次编译 gpui 依赖树较久
cargo test           # 协议层 / 终端编码 / 主题单测
cargo run            # 拉起窗口；无 daemon 时显示「未连接」
```

- gpui 锁定 `zed-industries/zed` tag `v1.17.2`（commit `c8e44cf…`）。该 rev 上
  `runtime_shaders` feature 位于 `gpui_platform`（→ `gpui_macos` → `gpui_apple`），
  运行时拼接 Metal shader，无需完整 Xcode（CommandLineTools 即可）。
- 连接参数：启动时读 `~/.config/aaa-daemon/config.toml`（host 固定 127.0.0.1）；
  读不到时在设置页手动填 host/port/token。daemon 的 config.toml 只读不写，
  UI 自己的本机偏好（目前只有侧栏宽度）落在 `~/.config/aaa-ui/ui.toml`。
- 终端 VT 模型：`alacritty_terminal`（Apache-2.0）；渲染层为本仓库自研
  （`src/ui/terminal_view.rs`），不含 Zed GPL 代码。

## 打包与安装（.app）

```bash
packaging/bundle.sh            # 产出 mac/target/AAA UI.app（含图标 + Info.plist + ad-hoc 签名）
packaging/bundle.sh install    # 覆盖安装到 /Applications（先退出正在运行的实例）
open -a "AAA UI"
```

- Bundle ID `cc.uoox.aaaui`，图标来自 `../brand/out/aaa-ui.icns`，版本号取自 Cargo.toml，
  build 号取 git 短 hash。
- **ad-hoc 签名的已知坑**：本机无开发者证书，`codesign --sign -` 没有稳定身份，
  每次重新构建安装后系统会视为「新 app」，TCC 授权（通知/自动化等）随之失效。
  因此权限全部归责到 aaa-daemon（见 PROTOCOL.md），UI 本体不申请任何 TCC 权限；
  系统通知走 `osascript` 子进程，归属不受重签影响。
- 本机构建无 quarantine，可直接打开；拷到别的机器首次打开需在
  「系统设置 → 隐私与安全性」放行，或 `xattr -dr com.apple.quarantine "/Applications/AAA UI.app"`。
- 无 daemon / 无 `~/.config/aaa-daemon/config.toml` 时正常启动，显示「未连接」，
  可在设置页手动填 host/port/token。

## 模块

| 路径 | 职责 |
|---|---|
| `src/theme.rs` | PROTOCOL.md 设计令牌硬编码 + ANSI 调色板 |
| `src/model.rs` | 协议类型（serde，宽容解析，含 v1.1 session_stalled）+ config.toml 读取 |
| `src/net.rs` | tokio 后台运行时：REST、/events 与 attach WS（指数退避重连） |
| `src/notify.rs` | macOS 系统通知（osascript display notification） |
| `src/term.rs` | alacritty Term 包装 + 按键→控制序列编码 + bracketed paste |
| `src/ui/` | gpui 视图：根视图/侧栏/终端/总览/项目表格/设置/模态框/单行输入 |

会话切换只有左侧栏一处（没有横向 tab 条）：行右侧的 ✕ 关 tab（仅 detach，进程照跑），
拖侧栏右边缘改宽度（180–480，松手写 `ui.toml`）。

终端选区：基于 `alacritty_terminal::selection`（Simple/双击词选/三击行选，shift 扩展），
松手即复制、cmd-C 复制、右键菜单复制/粘贴；实现为自研（与 tty7 同用 alacritty 原语，
未移植其代码）。输出里的 http/https/ftp/file 地址与 OSC 8 超链接悬停加下划线、单击用
默认浏览器打开（识别规则与 Android 端 `Links.kt` 一致：只认带 scheme 的绝对地址，
避免把 main.rs、a.b.c 认成链接）。
