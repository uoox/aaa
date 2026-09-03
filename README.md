# AAA

Claude Code 的远程工具：会话跑在一台常驻的 Mac 上，Mac App、手机、终端随时接上去，
看进度、答问题、开新活。

```
┌─────────────┐   内网 REST + WS   ┌──────────────┐   PTY   ┌─────────────┐
│  Mac App    │◄─────────────────►│  aaa-daemon  │◄───────►│ Claude Code │
│  Android    │◄─────────────────►│  :2730       │         │ + shell     │
│  aaa (CLI)  │◄─────────────────►│  launchd 常驻 │         └─────────────┘
└─────────────┘                   └──────────────┘
```

三个客户端接的是同一批会话：终端里开的，Mac 上接着看，手机上答一句。
会话活在 daemon 里，客户端断开它照样跑。

| 目录 | 内容 |
|---|---|
| `daemon/` | Rust 常驻服务：PTY 池、服务端终端、REST/WS API、消息流解析、表单作答、checkpoint |
| `mac/` | macOS 原生客户端（gpui） |
| `android/` | Android 原生客户端（Kotlin/Compose，终端用 ConnectBot termlib） |
| `cli/` | `aaa` 终端客户端，一个 bash 脚本 |
| `PROTOCOL.md` | 三端与 daemon 的契约 |

## 安装

从 [Releases](https://github.com/uoox/aaa/releases) 下载：

- `aaa-cli-*.tar.gz`：`aaa-daemon` 和 `aaa`，放进 `~/.local/bin`
- `AAA-*-macos-arm64.zip`：Mac App，解压到 `/Applications`
- `AAA-*-android.apk`：手机端

daemon 装好后常驻：

```bash
aaa-daemon service install     # 写 launchd plist 并启动，配置在 ~/.config/aaa-daemon/config.toml
```

首次运行会生成随机 token。手机扫 Mac App 设置页的二维码配对，或手输 `主机:2730` 加 token。
手机与 Mac 之间需要能直连，比如 tailscale 一类的内网。

> 项目根放在外置卷（`/Volumes/…`）时，要给 `~/.local/bin/aaa-daemon` 开「完全磁盘访问权限」，
> 并用一个固定的自签名证书签名，否则重新构建后授权会失效。细节见 `daemon/README.md`。

## 用法

- **Mac App**：左栏是会话，顶部输入框输文件夹名回车即新建项目。⌘N 新建、⌃Tab 切会话、⌘E 消息流与终端互切、⌘W 关闭。
- **Android**：Claude 跑完一轮会收到「完成」通知，点开直达会话；提问以原生表单作答；右上切终端。
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
