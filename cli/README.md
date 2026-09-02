# `aaa` — AAA daemon 的命令行前端

一个可移植的 bash 脚本，是 daemon 的第三个客户端（另两个是 Mac App 和 Android App）。
它不拥有任何会话：列表、新建、结束、回答全部走 daemon 的 REST/WS API，所以这里开的
会话在 Mac App 和手机上同样可见、可接管；关掉终端窗口，会话仍在 daemon 里跑。

## 依赖

- bash ≥ 3.2（macOS 自带的 `/bin/bash` 即可）
- curl
- python3（仅标准库：JSON 渲染、目标解析、`attach` 的 WebSocket 客户端）

不需要 jq，不需要编译。macOS / Linux / Termux 用同一个文件。

## 安装

本地 checkout：

```bash
install -m755 cli/aaa ~/.local/bin/aaa
```

任意机器（无需 clone）：

```bash
curl -fsSL https://raw.githubusercontent.com/uoox/aaa/main/cli/aaa -o ~/.local/bin/aaa && chmod +x ~/.local/bin/aaa
```

## 连接

- 默认读 `~/.config/aaa-daemon/config.toml` 的 `port` / `token`，连本机 `127.0.0.1`。
- `AAA_HOST=主机[:端口]`（`[v6]:端口` 也行）+ `AAA_TOKEN=…` 指向另一台机器的 daemon
  （tailscale / easytier 直接过去，不用 SSH）。
- `AAA_HOME` 可改配置文件所在的 home；`NO_COLOR` 关闭 ANSI 颜色。
- 本机 daemon 连不上时先试一次 `launchctl kickstart gui/$UID/com.aaa.daemon`，再报错。

## 动词

```
aaa                      交互菜单（执行中 / 待回复 / 已完成 分组；数字接入会话，p 项目 m 权限 n 新建）
aaa ls [-a] [--json]     会话列表（-a 含已退出）
aaa ps [--json]          项目列表
aaa new [名字] [-a AGENT]  新建项目目录 + 开一个会话并接入
aaa open <目标> [--fresh]   进入项目（有活会话就接回，否则 resume；--fresh 强制再开一个并行会话）
aaa attach <目标>         接入会话（Ctrl-] 脱离，会话继续跑）
aaa say <目标> <文本…>     把一句话写进会话并回车（回答提问用）
aaa kill <目标>           结束会话
aaa rm <目标>             删除会话记录
aaa rename <目标> <标题>   重命名会话
aaa wait [--json]         只列等待输入（且带提问）的会话（脚本/通知用）
aaa status [--json]       daemon 状态
aaa perms [--json]        macOS 权限状态
aaa perms all | <id…>     申请权限（弹窗出现在 Mac 上）
aaa help
```

目标可以是：会话 id 或其唯一前缀、`aaa ls` 里的序号、项目名（精确名，或唯一的子串）、
或 `.` 表示当前目录所属的项目。`kill` / `rm` 对序号或项目名这类会变动的目标，在终端里会先问一句
`[y/N]`；给 id 直接执行；非交互调用（脚本、手机）不问。

`ls` 与菜单按「执行中 / 待回复 / 已完成」三组排列（待回复 = 等待输入且带提问；只是停在输入框
的会话算已完成），组内按最近输出排序，已退出永远排最后且默认隐藏——序号贯通三组，`aaa ls` 里
看到几号，`aaa attach 几号` 就是它。
