# aaa — AAA daemon 的工具箱

纯 python3 标准库脚本（`cli/aaa`），无需构建。2026-09-07 起 CLI 只做工具性的事——授权、重启、配置、迁根、看状态；
项目与会话的新建 / 打开 / 结束 / 改名请用 Mac App 或手机。

```
aaa [status] [--json]          daemon 状态（版本、项目根、会话五态计数）
aaa ls [-a] [--json]           会话列表，按 待回复 / 运行 / 后台 / 激活 / 暂停 分组
aaa wait [--json]              只列有表单等你回答的会话
aaa perms [--json]             macOS 权限状态
aaa perms all | <id…>          申请权限（弹窗出现在 Mac 上）
aaa restart [--force]          重启 daemon
aaa config                     看配置（token 默认打码，--show-token 看全）
aaa config port <n> | token <t> | root <path> [--migrate] [--force]
aaa migrate-root <新根> [--yes] 一键迁根
aaa service install|uninstall|status
```

连接：默认读 `~/.config/aaa-daemon/config.toml`；`AAA_HOST=主机:2730 AAA_TOKEN=…` 指向远端。token 只走请求头，永远不进 argv。
不要用 sudo。
