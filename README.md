# AAA

一个 Claude Code daemon，加三个客户端。

daemon 常驻在一台机器上，持有 Claude Code 会话的 PTY，通过 Claude Code hooks 精确知道每个会话在跑、在等、在问什么，
用量与 plan 配额也由它收集；Mac App、Android App 和终端里的 `aaa` 接上去看进度、答问题、开新活。客户端断开，会话照样跑。

```
┌─────────────┐   内网 REST + WS   ┌──────────────────┐  PTY + hooks  ┌─────────────┐
│  Mac App    │◄─────────────────►│    aaa-daemon    │◄─────────────►│ Claude Code │
│  Android    │◄─────────────────►│  :2730  常驻服务  │               │ + shell     │
│  aaa (CLI)  │◄─────────────────►│  会话池 · 消息流   │               └─────────────┘
└─────────────┘                   └──────────────────┘
```

| 目录 | 内容 |
|---|---|
| `daemon/` | Rust 常驻服务：PTY 池、服务端终端、hooks 事件源、REST/WS API、消息流解析、表单作答、checkpoint |
| `mac/` | macOS 原生客户端（gpui） |
| `android/` | Android 原生客户端（Kotlin/Compose，终端用 ConnectBot termlib） |
| `cli/` | `aaa` 终端客户端，一个 bash 脚本 |
| `install.sh` | daemon + CLI 一键安装（macOS / Linux） |
| `PROTOCOL.md` | 三端与 daemon 的契约 |

## 安装

daemon 与 CLI，macOS 或 Linux：

```bash
curl -fsSL https://raw.githubusercontent.com/uoox/aaa/main/install.sh | bash
```

macOS arm64 下载预编译包；其它平台用本机 Rust 从源码编。脚本把 `aaa-daemon` 和 `aaa` 装进 `~/.local/bin`，
登记为常驻服务（launchd 或 systemd --user），首次启动生成 `~/.config/aaa-daemon/config.toml` 和随机 token。

客户端从 [Releases](https://github.com/uoox/aaa/releases) 下载：`AAA-*-macos-arm64.zip` 解压到 `/Applications`，
`AAA-*-android.apk` 装到手机。手机扫 Mac App 设置页的二维码配对，或手输 `主机:2730` 加 token。
手机与 daemon 之间要能直连，比如 tailscale 一类的内网。

> daemon 需要本机能运行 `claude`。项目根放在外置卷（`/Volumes/…`）时要给 `aaa-daemon` 开完全磁盘访问权限并用固定签名，见 `daemon/README.md`。

## 用法

- **Mac App**：左栏是会话，顶部输入框输文件夹名回车即新建项目，左下角是 plan 配额。⌘N 新建、⌃Tab 切会话、⌘E 消息流与终端互切、⌘I 右侧详情栏（会话用量、发布过的产物链接、改动与回滚、收件箱、静音）、⌘W 关闭。消息流里过程步骤折叠成一行，📎 上传文件后以 `@路径` 引用。
- **Android**：Claude 跑完一轮收到「完成」通知，点开直达会话；提问以原生表单作答；系统分享或 📎 把文件传进项目；首页顶部是 plan 配额，会话菜单里有「产物」。
- **终端里不再需要状态栏插件**：daemon 接管了 Claude Code 的 statusLine，模型、上下文、费用、配额都进了客户端，shell 里一行不占。
- **终端**：

```
aaa                    交互菜单
aaa ls / ps            会话 / 项目列表
aaa new [名字]          新建项目并开会话
aaa open <目标>         进入项目（接回活会话或 resume）
aaa attach <目标>       接入会话，Ctrl-] 脱离
aaa say <目标> <文本>    写一句进会话并回车
aaa kill / rm / rename <目标>
```

`AAA_HOST=主机:2730 AAA_TOKEN=…` 可指向另一台机器的 daemon。详见 `cli/README.md`。

## 从源码构建

```bash
cd daemon  && cargo build --release && cargo test
cd mac     && packaging/bundle.sh build        # → mac/target/AAA.app
cd android && ./gradlew assembleDebug          # 需要 JDK 21 与 Android SDK
```

契约变更先改 `PROTOCOL.md`，再改三端。
