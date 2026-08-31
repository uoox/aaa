# AAA 协议契约 v1

daemon 与三个客户端的唯一协调契约。实现与本文冲突时，以本文为准；发现本文缺陷，先改本文再改代码。

```
┌─────────────┐   tailscale / easytier    ┌──────────────────┐   spawn PTY    ┌─────────────┐
│  AAA.app    │◄────── REST + WS ────────►│    aaa-daemon    │◄──────────────►│ agent CLIs  │
│ (gpui 原生) │        Bearer token       │  Mac mini :2730  │   读会话存储    │ + zsh 终端  │
├─────────────┤                           │  launchd 常驻    │                └─────────────┘
│ AAA android │◄──────────────────────────│  PTY 池 + VT     │
│ (Kotlin 原生)│                          └──────────────────┘
├─────────────┤                                    ▲
│  aaa (CLI)  │◄───────────────────────────────────┘
│ (Rust 终端) │  同一套 API，同一批会话
└─────────────┘
```

- 端口固定 **2730**（= 0xAAA），监听 `0.0.0.0`（个人内网，安全性由 overlay 网络 + token 提供）。
- 所有 HTTP/WS 均为明文（链路已有 WireGuard 加密），认证 `Authorization: Bearer <token>`；WS 可用 `?token=` 查询参数。
- 时间一律 ISO-8601 UTC 字符串；大小一律原始字节数（人性化格式由客户端渲染）。
- 错误统一 `{"error":{"code":"...","message":"..."}}`，code ∈ `unauthorized | not_found | conflict | agent_unknown | ssd_unmounted | internal`。

## 目录与文件约定

| 路径 | 用途 |
|---|---|
| `~/.config/aaa-daemon/config.toml` | daemon 配置（首次运行自动生成，含随机 token） |
| `~/.local/state/aaa-daemon/` | 会话元数据、已退出会话的回放、日志 |
| `/Volumes/SSD/project` | 项目根（config 可改） |
| `/Volumes/SSD/project/.aaa-agents` | **沿用 aaa CLI 的注册表**：每行 `<目录>\t<agent>`，原子整体重写 |
| `~/.cache/aaa-cwds.json` | **沿用 aaa CLI 的缓存**：键 `claude:<path>` `codex:<path>` `pi:<path>` `cname2:<path>` `ainame:<path>`，读写保持兼容 |

config.toml 结构：

```toml
port = 2730
token = "aaa_tk_<32hex>"     # 首次运行生成
project_root = "/Volumes/SSD/project"
namer = true                  # haiku 会话命名开关（对应 AAA_NAMER）
[ntfy]                        # 可选，离线推送兜底
url = "https://ntfy.example.com"
topic = "aaa"
```

## Agent 表（与 aaa CLI 完全一致）

| id | label | 新会话命令 | resume 命令（%ID% 替换） |
|---|---|---|---|
| claude | Claude | `claude --dangerously-skip-permissions` | `claude --resume %ID% --dangerously-skip-permissions` |
| codex | Codex | `codex --dangerously-bypass-approvals-and-sandbox` | `codex resume %ID% --dangerously-bypass-approvals-and-sandbox` |
| pi | Pi | `pi` | `pi --continue` |
| reasonix | Reasonix | `reasonix --permission-mode bypassPermissions` | `reasonix --continue --permission-mode bypassPermissions` |
| agy | Antigravity | `agy --dangerously-skip-permissions` | `agy --conversation %ID% --dangerously-skip-permissions` |
| shell | 终端 | `exec zsh -l` | —（shell 无 resume） |

启动方式：`zsh -lc 'cd <dir> && <cmd>'`，并**由 daemon 显式设置 `PATH`**。
不能指望 login shell：非交互的 `zsh -l` 只读 `.zprofile`、不读 `.zshrc`，而 PATH 通常维护在后者——
launchd 起的 daemon 因此会让每个 agent 都 `command not found`，`shell` 却照常工作。
daemon 在启动时用 `zsh -lic` 问一次「终端里应有的 PATH」（带超时），再把 `~/.local/bin`、
`~/.npm-global/bin` 等常见安装位置并进去兜底；`/agents` 的 `available` 用**同一份** PATH 判断，
所以「显示可用」与「真能启动」不会打架。
会话查找（resume 用）、cwd 探测、purge 的具体逻辑**逐条移植** `~/.local/bin/aaa` 内嵌 Python（AAA_PY 的 find/detect/collect/purge），存储布局见该脚本注释。

## 会话模型

```jsonc
{
  "id": "s_9f2c81ab",            // daemon 生成
  "title": "aaa-ui 交互原型设计",  // AI 命名或用户重命名
  "project_path": "/Volumes/SSD/project/aaa-ui",
  "project_name": "aaa-ui",
  "agent": "claude",             // agent id 或 "shell"
  "state": "waiting",            // running | waiting | idle | exited
  "question": {                   // 仅 waiting 且解析成功时非 null
    "text": "原型是否需要包含 iPad 布局？",
    "options": [{"key":"1","label":"需要"},{"key":"2","label":"不需要"}]
  },
  "preview": "…最近 4 行纯文本…",
  "rows": 40, "cols": 120,
  "pid": 12345, "exit_code": null,
  "resume_id": "9f2c81…",        // 本次启动实际 resume 的会话 id（无则 null）
  "created_at": "…", "last_output_at": "…"
}
```

状态机：有输出 → `running`；进程存活 + 静默 ≥ 6s + 屏幕末行匹配提示模式（`? `、`❯`、`(y/n)`、`[Y/n]`、编号选项、输入框 `│ >` 等）→ `waiting`（尽力解析出 question/options）；静默且无提示 → `idle`；进程退出 → `exited`（保留屏幕 + 回滚缓冲，daemon 重启后仍可查看回放）。
Claude 的 hook 事件（见下）可精确覆盖启发式。

## REST（前缀 `/api/v1`）

| 方法 | 路径 | 说明 |
|---|---|---|
| GET | `/health` | `{version, ssd_mounted, root_state, project_root, uptime_s}`；`root_state ∈ ok\|unmounted\|denied`，`ssd_mounted = (root_state==ok)`（向后兼容：对客户端它一直就是「能不能用」） |
| GET | `/agents` | agent 表 + `available`（which 检查） |
| GET | `/projects` | collect 移植：`[{path,name,mtime,dir_size,ctx_size,agent,session_title}]`，按 mtime 降序 |
| POST | `/projects` | `{name?, agent?}`；name 经 slugify，空则 `YYYY-MM-DD-HHMM`；已存在 → 409；agent 给了就写注册表。**响应 = 完整项目对象（至少 `{path,name,agent}`）**，客户端依赖 `path` 直接开会话 |
| POST | `/projects/delete` | `{paths:[…]}` → `{results:[{path, ok, purged:[{agent_label,count}]}]}`；目录删除 + 全 agent purge（含 grok 遗留） |
| POST | `/projects/agent` | `{path, agent}` 写注册表 |
| GET | `/sessions` | 全部会话（含 exited） |
| POST | `/sessions` | `{project_path, agent, resume}`；resume=true 时按 aaa 逻辑找最近会话套 resume 模板；目录不存在则创建（但见 SSD 守卫） |
| POST | `/sessions/:id/input` | `{text, enter}`：写入 PTY（enter 补 `\r`）。composer / 快捷作答用 |
| POST | `/sessions/:id/kill` | TERM，2s 后 KILL；记录保留为 exited |
| DELETE | `/sessions/:id` | 删除记录与回放（活着先 kill） |
| POST | `/sessions/:id/rename` | `{title}` |
| GET | `/sessions/:id/ports` | 进程树监听端口 `[{port,cmd}]`（Web 预览入口用） |
| GET | `/mac/permissions` | 见「macOS 权限」 |
| POST | `/mac/permissions/request` | 见「macOS 权限」 |
| GET | `/pair` | `{payload}`，二维码内容（见「配对」） |
| POST | `/hooks/claude` | **仅接受 localhost 来源，免 token**；Claude Code hook 转发 |

## WS

### `/api/v1/sessions/:id/attach`

- 连接后 server 先发一个文本帧 `{"t":"hello","session":{…},"rows":R,"cols":C}`。
- 随后 server 发二进制帧：先是**整屏重绘**（回滚缓冲尾部若干行 + ANSI 清屏 + vt100 `contents_formatted()` 生成的当前屏幕），之后持续转发 PTY 原始输出字节。
- client → server：二进制帧 = 原样写入 PTY 的输入字节；文本帧 = 控制消息 `{"t":"resize","cols":C,"rows":R}`。
- 多客户端可同时 attach 同一会话（尺寸取最后 resize 者）。

### `/api/v1/events`

server → client JSON 文本帧：

```jsonc
{"t":"snapshot","sessions":[…全部会话…]}        // 连接即发
{"t":"session","session":{…}}                   // 状态/preview/title 变化，≥250ms 节流
{"t":"session_removed","id":"s_…"}
{"t":"projects_changed"}
{"t":"health","ssd_mounted":true}
```

通知策略（客户端行为）：`waiting`（带 question 优先）与 `running→exited` 触发系统通知；daemon 侧在配置了 ntfy 时同步推送一份（手机 App 进程不在时的兜底）。

## macOS 权限（一键授权）

目的：agent 进程全部是 daemon 的子进程，TCC 弹窗归责到 daemon 可执行文件；人在 Mac 前**一次性**把弹窗全点掉，此后手机远程操作不再被权限弹窗卡死。

`GET /mac/permissions` →

```jsonc
[{"id":"accessibility","label":"辅助功能","status":"granted"},   // granted|denied|undetermined|unknown|needs_settings
 {"id":"screen_recording","label":"屏幕录制","status":"undetermined"},
 {"id":"input_monitoring","label":"输入监控","status":"denied"},
 {"id":"full_disk_access","label":"完全磁盘访问","status":"needs_settings"},
 {"id":"automation_system_events","label":"自动化 · System Events","status":"undetermined"},
 {"id":"automation_finder","label":"自动化 · Finder","status":"undetermined"},
 {"id":"camera","label":"摄像头","status":"unknown"},
 {"id":"microphone","label":"麦克风","status":"unknown"}]
```

`POST /mac/permissions/request` body `{"ids":["all"]}` → 202 `{"triggered":[…],"opened_settings":[…]}`。
实现（daemon 进程内直接调用，弹窗出现在 Mac 屏幕上）：

- accessibility: `AXIsProcessTrustedWithOptions(kAXTrustedCheckOptionPrompt=true)`
- screen_recording: `CGPreflightScreenCaptureAccess` / `CGRequestScreenCaptureAccess`
- input_monitoring: `IOHIDCheckAccess` / `IOHIDRequestAccess(kIOHIDRequestTypeListenEvent)`
- automation_*: `AEDeterminePermissionToAutomateTarget`（askUserIfNeeded=true）
- camera/microphone: `AVCaptureDevice`（objc2；实现困难可报 unknown）
- full_disk_access: 无 API 可弹，探测（尝试读 `~/Library/Application Support/com.apple.TCC/TCC.db`）+ `open "x-apple.systempreferences:com.apple.preference.security?Privacy_AllFiles"`

客户端设置页展示状态列表 + 「一键申请全部」按钮；从手机点按钮时提示「弹窗将出现在 Mac 上，请在 Mac 前完成一次」。

## 配对

`GET /pair` → `{"payload":"aaa://pair?v=1&name=<hostname>&hosts=<h1:p>,<h2:p>&token=<token>"}`
hosts = daemon 探测到的 tailscale MagicDNS 名 / tailscale IP / easytier IP（带端口，按优先级排列）。
Mac 客户端把 payload 渲染成二维码；Android 扫码解析后逐个 host 试连，成功即保存。手输兜底。

## Claude Code hook（可选精确信号）

`aaa-daemon install-claude-hooks` 子命令（**仅显式执行，daemon 不自动改用户配置**）向 `~/.claude/settings.json` 合并 Notification / Stop hook，向 `http://127.0.0.1:2730/api/v1/hooks/claude` POST hook 原始 JSON。daemon 按 `cwd` 匹配会话：Notification → `waiting`，Stop → 静默计时重置。写入需原子、幂等、保留原有配置。

## SSD 守卫（硬性约束）

`project_root` 不存在时：**绝不 mkdir**（防止在系统盘建占位目录挤掉真 SSD 挂载点）。`/health.ssd_mounted=false`；一切创建/启动/删除类 API 返回 503 `ssd_unmounted`；已有会话不受影响。挂载恢复后自动解除。

**可读性与挂载是两件事**：外置卷被 macOS 的 TCC 挡住时 `stat` 能过而 `opendir` 会**无限阻塞**（等一个没人去点的授权弹窗）。因此：

- 探测一律 `read_dir` 而非 `metadata`，且跑在带超时（3s）的独立线程上——**daemon 永不因权限弹窗卡在启动**，超时即判为 `denied`。
- `root_state` 区分 `unmounted`（挂上就好）与 `denied`（要给 daemon 完全磁盘访问权限），两者修法完全不同，503 的 `message` 直接带上修法（手机端看不到 Mac 的日志）。
- 状态由 5s 健康轮询维护；请求只读缓存值，不逐次探测。

## v1.1 扩展（2026-08-30 用户拍板：消息流 / checkpoint+diff / 收件箱 / 上传 / watchdog / 通知细化）

### 消息流（手机主视图，终端保留可切换）

- `GET /sessions/:id/messages?after=<seq>&limit=<n=200>` → `{"supported":bool,"source":"claude|codex|pi|none","last_seq":N,"messages":[…]}`
- 消息结构：`{"seq":N,"ts":"…","role":"user|assistant|tool|system","kind":"text|thinking|tool_use|tool_result|question","text":"…","tool":{"name":"Bash","summary":"cargo build","status":"ok|err|running"}|null}`
- daemon 在会话 spawn/resume 后定位该会话的 agent 存储文件（resume 已知文件；新会话按 cwd 匹配 + mtime ≥ 启动时刻轮询发现）并增量 tail 解析。**claude 必须支持**（jsonl：user/assistant/tool_use/tool_result/thinking，过滤 isSidechain 与注入块），codex/pi 尽力而为，reasonix/agy/shell 返回 `supported:false`（客户端回落终端视图）。
- `/events` 新帧：`{"t":"messages_changed","id":"s_…","last_seq":N}`（≥500ms 节流）。客户端收到后增量拉取。

### git checkpoint + diff + 回滚（后悔药）

- config：`[checkpoint] enabled=true, auto_init_git=false, interval_minutes=10, auto_init_max_mb=512`。agent 会话（≠shell）创建时打 `start` 检查点、退出时打 `end`、运行中每 interval 分钟有变更则打 `auto`。**已有 .git 的项目才有检查点**：项目常常只是一个任务目录（笔记、抓取、一堆 yml），替用户 `git init` 不是 daemon 该做的事，所以 `auto_init_git` 默认 **false**。显式开成 true 时，仍受 `auto_init_max_mb`（默认 512MB）护栏限制——超预算的无 .git 目录跳过 init 与检查点（避免 `.git/objects` 暴涨；0 = 关闭护栏）。已有 .git 的项目不受体积护栏限制。
- 实现硬约束:**绝不触碰项目的 HEAD/index/工作区**：临时 `GIT_INDEX_FILE` + `git add -A` + `write-tree` + `commit-tree`，ref 收在 `refs/aaa-ckpt/<session_id>/<n>-<label>` 下（普通 git 界面不可见，`git log --all` 不污染分支）。
- `GET /sessions/:id/diff` → `{"supported":bool,"base":"<ref>","files":[{"path","status":"added|modified|deleted","additions":N,"deletions":N,"patch":"…≤64KB","truncated":bool}]}`：start 检查点树 vs 当前工作区（含未跟踪文件，同样经临时 index）。
- `POST /sessions/:id/rollback` body `{"confirm":true,"force":false}`：恢复工作区到 start 检查点（checkout 树 + 删除 start 后新增文件；`.git` 与忽略文件不动）。会话仍存活时必须 `force:true`（daemon 先 kill）。客户端必须二次确认。

### 任务收件箱

- `GET /inbox?path=<proj>` → `[{"id","text","created_at"}]`；`POST /inbox` `{path,text}`；`DELETE /inbox/:id`。
- 自动喂入：项目会话**首次进入 waiting** 且收件箱非空时，daemon 把条目拼成一条消息（`任务清单：\n1. …\n2. …` + `\r`）写入 PTY 并删除条目。`POST /sessions` 可带 `"feed_inbox":false` 禁用。事件 `{"t":"inbox_changed","path"}`。

### 手机→项目文件通道

- `POST /projects/upload?path=<proj>&name=<fname>`，body = 原始字节（`application/octet-stream`，≤50MB）→ `{"saved_path":"<proj>/_inbox/<ts>-<name>"}`。文件名 slugify、防覆盖。客户端上传后自行把路径发进 composer 告知 agent。

### Watchdog

- config：`[watchdog] stall_minutes=10, auto_kill=false`。`running` 且静默 ≥ stall_minutes（waiting 不算）→ 事件 `{"t":"session_stalled","id","quiet_s":N}` + ntfy（high）；auto_kill=true 则随后 kill。

### 通知细化

- ntfy 消息带 priority：waiting / stalled = high，exited = default。
- waiting 推送去重：同会话同 question 只推一次，且同会话冷却 5 分钟（避免 Claude 每回合结束常驻输入框导致刷屏）。
- 客户端侧：通知渠道分级（等待输入=high、完成=default）、按项目静音列表、快捷短语 chips、**默认 UI 设置（消息流 / 终端）**——均为客户端本地配置，不进 daemon。

## aaa CLI（第三个客户端）

`aaa` 是 daemon 的终端前端，**不复制任何业务逻辑**：列表、新建、结束、回答全部走上面的 API，因此
CLI 开的会话在 Mac App 和手机上同样可见、可接管。旧的 zsh 菜单脚本改名 `aaal` 保留，作为 daemon
不可用时的兜底（它把 agent 直接跑在当前终端里，不常驻）。

- 连接：默认读 `~/.config/aaa-daemon/config.toml` 取 port + token 连本机；`AAA_HOST=主机:2730`
  + `AAA_TOKEN=…` 指向另一台机器的 daemon。本机连不上时尝试 `launchctl kickstart` 唤醒一次。
- 无参数 = 交互菜单：**活会话在上**（等待输入的排最前）、其次「项目管理 / macOS 权限 / New \<agent\>」。
- 动词：`ls` `ps` `wait` `status` `perms` `new` `open` `attach` `say` `kill` `rm` `rename`，
  列表类均有 `--json`。目标可写会话 id / id 前缀 / `ls` 序号 / 项目名 / `.`（当前目录所属项目）。
- `attach` = 直接连 `/sessions/:id/attach`：本地终端进 raw 模式，stdin 原样转发为二进制帧，
  窗口大小变化发 `{"t":"resize"}`。**Ctrl-]** 脱离，会话继续留在 daemon 里。

## 设计令牌（两端 UI 必须一致，来源 prototype.html）

| 令牌 | 值 | 用途 |
|---|---|---|
| bg | `#0e1216` | 页面底 |
| surface | `#1a222b` / raised `#212b36` | 卡片/面板 |
| edge | `#28323e` / `#36434f` | 描边 |
| ink / dim / faint | `#e3ebf3` / `#8b99a8` / `#5f6d7c` | 文字三级 |
| term-bg | `#0a0e12` | 终端底 |
| cyan | `#53c6dd` | 主操作/选中 |
| magenta | `#c583e0` | 品牌（banner） |
| green / amber / red | `#5ecb8f` / `#e3b45c` / `#e57373` | running / waiting / exited |
| agent 色 | claude `#e8b46a` · codex `#8fd0ff` · pi `#b5e08f` · reasonix `#e08fb5` · agy `#c8a8f0` | 标签 |

状态点语义：绿=运行中、黄=等待输入（一等状态：置顶、高亮、推送）、灰=空闲、红=已退出。
终端字体：等宽（mac 端 SF Mono/Menlo 族，Android 端打包 JetBrains Mono 或系统 monospace）。
