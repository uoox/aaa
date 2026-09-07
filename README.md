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
| `daemon/` | Rust 常驻服务：PTY 池、服务端终端、hooks 事件源、REST/WS API、消息流解析、表单作答、plan 配额 |
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

- **Mac App**：左栏是项目列表（一项目一行，行首写 待回复 / 运行 / 后台 / 激活 / 暂停；置顶的在最前，其余按这个顺序排，同状态里最近更新的在前），顶部输入框输文件夹名回车即新建项目，左下角是 plan 配额。⌘N 新建、⌃Tab 切会话、⌘E 消息流与终端互切、⌘I 右侧详情栏（会话用量与提示缓存命中、对话进度清单、发布过的产物链接、收件箱、静音）、侧栏底部「看板」（所有会话的进度：状态、进度条、没勾的项）、⌘W 关闭。消息流里 Claude 的每段回答都露出来，中间的思考与工具步骤折叠成一行，📎 上传文件后以 `@路径` 引用。
- **Android**：Claude 跑完一轮收到「完成」通知，点开直达会话；提问以原生表单作答；系统分享或 📎 把文件传进项目；首页是项目列表（一行一个，状态字 + 标题；按 待回复 > 运行 > 后台 > 激活 > 暂停 排，同状态里最近更新的在前），顶部是 plan 配额（5h、7d 与 Fable 周窗口），会话菜单里有「进度」（整个对话的 todo list：做了什么、还没做什么，每轮结束后更新）和「产物」。会话页左上角 ☰ 拉出的就是完整项目面板（首页整块：连接状态、终端、看板、设置、用量、新建、项目列表与长按操作），app 起来直接回上次的会话，没有单独的首页。首页 ▦ 是看板：所有会话的进度——五态计数、每个会话一张卡（进度条 + 没勾的项）。消息流里 Claude 还在跑时点「发送」，那句话排成「待发送」挂在末尾，跑完自动发出（✕ 撤回）；输入框的字切出去再回来还在。终端视图的快捷键条在画面上方，回车在最前；终端字号固定，不再提供字号设置。
- **终端里不再需要状态栏插件**：daemon 接管了 Claude Code 的 statusLine，模型、上下文、费用、配额都进了客户端，shell 里一行不占。按模型的周配额（Fable 还剩多少）statusLine 给不了，daemon 每分钟用 Claude Code 的登录态问一次 claude.ai 的 usage 接口，三端一起显示。
- **终端（`aaa`，工具箱）**：项目与会话管理是 App 的事，CLI 只做工具性的事：

```
aaa [status]                 daemon 状态与会话五态计数
aaa ls [-a] / aaa wait       会话列表（按五态分组）/ 只看等你回答的
aaa perms [all|<id…>]        macOS 权限状态 / 申请
aaa restart [--force]        重启 daemon（活会话先结束、起来后自动 resume）
aaa config [port|token|root …]   看 / 改配置
aaa migrate-root <新根>       一键迁根（检查 → 换新 daemon → 迁移 → 验证）
aaa service install|uninstall|status
```

`AAA_HOST=主机:2730 AAA_TOKEN=…` 可指向另一台机器的 daemon。详见 `cli/README.md`。

## 从源码构建

```bash
cd daemon  && cargo build --release && cargo test
cd mac     && packaging/bundle.sh build        # → mac/target/AAA.app
cd android && ./gradlew assembleDebug          # 需要 JDK 21 与 Android SDK
```

契约变更先改 `PROTOCOL.md`，再改三端。
