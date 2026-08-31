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
  读不到时在设置页手动填 host/port/token。
- 终端 VT 模型：`alacritty_terminal`（Apache-2.0）；渲染层为本仓库自研
  （`src/ui/terminal_view.rs`），不含 Zed GPL 代码。

## 模块

| 路径 | 职责 |
|---|---|
| `src/theme.rs` | PROTOCOL.md 设计令牌硬编码 + ANSI 调色板 |
| `src/model.rs` | 协议类型（serde，宽容解析，含 v1.1 session_stalled）+ config.toml 读取 |
| `src/net.rs` | tokio 后台运行时：REST、/events 与 attach WS（指数退避重连） |
| `src/notify.rs` | macOS 系统通知（osascript display notification） |
| `src/term.rs` | alacritty Term 包装 + 按键→控制序列编码 + bracketed paste |
| `src/ui/` | gpui 视图：根视图/侧栏/tab/终端/总览/项目表格/设置/模态框/单行输入 |

终端选区：基于 `alacritty_terminal::selection`（Simple/双击词选/三击行选，shift 扩展），
cmd-C 复制；实现为自研（与 tty7 同用 alacritty 原语，未移植其代码）。
