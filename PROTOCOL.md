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
│ (bash 脚本) │  同一套 API，同一批会话
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
| `/Volumes/SSD/project/.aaa-agents` | **沿用 aaa CLI 的注册表**：每行 `<目录>\t<agent>[\t<对话id>]`，原子整体重写。第三列是该目录最近一次 resume 用的 agent 对话 id：目录迁移后 agent 存储按旧 cwd 查不到会话，靠它兜底（`POST /sessions` resume 命中时回写；`PUT /config` 迁移前全量采集） |
| `~/.cache/aaa-cwds.json` | **沿用 aaa CLI 的缓存**：键 `claude:<path>` `codex:<path>` `pi:<path>` `cname2:<path>` `ainame:<path>`，读写保持兼容 |

config.toml 结构：

```toml
port = 2730
token = "aaa_tk_<32hex>"     # 首次运行生成
project_root = "/Volumes/SSD/project"
namer = true                  # haiku 会话命名开关（对应 AAA_NAMER）
remote_control_name = true    # claude 会话给 Remote Control 起项目名（手机官方 App 的会话列表更可读；仅 claude 认此旗标）
[checkpoint]                  # 见「git checkpoint」
```

历史上的 `[ntfy]` / `[watchdog]` 段已废弃（2026-09-02），旧文件里留着也能解析，只是被忽略。

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
会话查找（resume 用）、cwd 探测、purge 的具体逻辑**逐条移植** 旧版 zsh 脚本 `~/.local/bin/aaal` 内嵌 Python（AAA_PY 的 find/detect/collect/purge），存储布局见该脚本注释。

## 会话模型

```jsonc
{
  "id": "s_9f2c81ab",            // daemon 生成
  "title": "aaa-ui 交互原型设计",  // AI 命名或用户重命名
  "project_path": "/Volumes/SSD/project/aaa-ui",
  "project_name": "aaa-ui",
  "agent": "claude",             // agent id 或 "shell"
  "state": "waiting",            // running | waiting | exited
  "asking": false,                // claude：transcript 里有一条 AskUserQuestion 还没被回答（结构化事实，不是猜的）
  "preview": "…最近 4 行纯文本…",
  "rows": 40, "cols": 120,
  "pid": 12345, "exit_code": null,
  "resume_id": "9f2c81…",        // 本次启动实际 resume 的会话 id（无则 null）
  "created_at": "…", "last_output_at": "…"
}
```

状态机（2026-09-02 简化）：屏幕内容在变 → `running`；进程存活 + **可见屏幕 6s 没变** → `waiting`（这轮干完了，轮到你）；进程退出 → `exited`（保留屏幕 + 回滚缓冲，daemon 重启后仍可查看回放）。
daemon **不再读屏猜「它在问什么」**：没有 `idle`，没有 `question`，没有提示模式匹配。agent 在等一个具体回答这件事只认一个来源——claude transcript 里的 `AskUserQuestion` 工具调用（结构化，见「消息流」），`asking` 就是它的镜像；其它 agent 没有这种结构化信号，`asking` 恒为 false。

三态口径（客户端分组，三端一致）：**待回复** = `asking`；**执行中** = `running`；**已完成** = 其余（`waiting`、`exited`）。

## REST（前缀 `/api/v1`）

| 方法 | 路径 | 说明 |
|---|---|---|
| GET | `/health` | `{version, ssd_mounted, root_state, project_root, uptime_s}`；`root_state ∈ ok\|unmounted\|denied`，`ssd_mounted = (root_state==ok)`（向后兼容：对客户端它一直就是「能不能用」） |
| GET | `/agents` | agent 表 + `available`（which 检查） |
| GET | `/projects` | collect 移植：`[{path,name,mtime,dir_size,ctx_size,agent,session_title}]`，按 mtime 降序。**注册表就是项目名册**：根目录下未登记的目录（顺手 clone 的仓库、杂物）不出现在列表里；经 daemon 建项目/开会话的目录都会自动登记 |
| POST | `/projects` | `{name?, agent?}`；name 经 slugify，空则 `YYYY-MM-DD-HHMM`；已存在 → 409；agent 给了就写注册表。**响应 = 完整项目对象（至少 `{path,name,agent}`）**，客户端依赖 `path` 直接开会话 |
| POST | `/projects/delete` | `{paths:[…]}` → `{results:[{path, ok, purged:[{agent_label,count}]}]}`；目录删除 + 全 agent purge（含 grok 遗留） |
| POST | `/projects/agent` | `{path, agent}` 写注册表 |
| GET | `/sessions` | 全部会话（含 exited） |
| POST | `/sessions` | `{project_path, agent, resume}`；resume=true 时按 aaa 逻辑找最近会话套 resume 模板；目录不存在则创建（但见 SSD 守卫） |
| POST | `/sessions/:id/input` | `{text, enter}`：写入 PTY（enter 补 `\r`）。composer 用 |
| POST | `/sessions/:id/answer` | `{answers:[{selected:[0,2], other:"自填文本"|null}, …]}`：回答当前待答的 AskUserQuestion 表单，一项对应一个问题（顺序同 `question.questions`），`selected` 是 0 起的选项下标，`other` 是「其它」自填。daemon 负责把选择翻译成 Claude Code 对话框的按键并确认对话框已关闭（见「回答表单」）。无待答问题 / 非 claude 会话 / 对话框没吃下 → 409；答案形状不对 → 400 |
| POST | `/sessions/:id/kill` | TERM，2s 后 KILL；记录保留为 exited |
| DELETE | `/sessions/:id` | 删除记录与回放（活着先 kill） |
| POST | `/sessions/:id/rename` | `{title}` |
| GET | `/sessions/:id/ports` | 进程树监听端口 `[{port,cmd}]`（Web 预览入口用） |
| GET | `/mac/permissions` | 见「macOS 权限」 |
| POST | `/mac/permissions/request` | 见「macOS 权限」 |
| GET | `/pair` | `{payload}`，二维码内容（见「配对」） |
| GET | `/config` | `{port, token, project_root}`（以磁盘 config.toml 为准） |
| PUT | `/config` | `{port?, token?, project_root?, migrate?}`。**写盘 + daemon 自我重启**（`exec` 自身，PID 不变，launchd 托管不受影响）；有非 exited 会话 → 409。`project_root` 变更且 `migrate=true`：先把各项目对话 id 采进注册表，再整根 `rename`（同卷限定，跨卷报错让人手动拷），最后重写注册表路径前缀；`migrate=false` 时要求新目录已存在，只改指向 |

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

通知策略（客户端行为，2026-09-02 用户拍板）：**只有一种通知——「完成」**。`running→waiting` 与 `running→exited`（非本机用户手动 kill）各弹一条，标题带项目名，正文是会话标题。不识别「里面要回什么」、不按问题去重、没有高低优先级、没有空转告警；daemon 侧不推送（ntfy 已移除）。按项目静音仍是客户端本地配置。用户正盯着的会话（窗口前台且当前页就是它）不弹。

## macOS 权限（一键授权）

目的：agent 进程全部是 daemon 的子进程，TCC 弹窗归责到 daemon 可执行文件；人在 Mac 前**一次性**把弹窗全点掉，此后手机远程操作不再被权限弹窗卡死。

`GET /mac/permissions` →

```jsonc
[{"id":"accessibility","label":"辅助功能","status":"granted","hint":""},   // granted|needs_settings
 {"id":"screen_recording","label":"屏幕录制","status":"needs_settings","hint":"弹窗只有「打开系统设置」——在列表里把 aaa-daemon 勾上，然后重启 daemon 才读得到"},
 {"id":"input_monitoring","label":"输入监控","status":"denied","hint":"系统设置 → 隐私与安全性 → 输入监控 勾上 aaa-daemon，然后重启 daemon"},
 {"id":"full_disk_access","label":"完全磁盘访问","status":"needs_settings","hint":"系统设置 → 隐私与安全性 → 完全磁盘访问权限 → + 加入 ~/.local/bin/aaa-daemon（⌘⇧G 输路径），然后重启 daemon"},
 {"id":"automation_system_events","label":"自动化 · System Events","status":"undetermined","hint":"aaa perms <id> 弹窗后点允许（daemon 会先把目标 App 拉起来）"},
 {"id":"automation_finder","label":"自动化 · Finder","status":"undetermined","hint":"aaa perms <id> 弹窗后点允许（daemon 会先把目标 App 拉起来）"},
 {"id":"camera","label":"摄像头","status":"unknown","hint":"daemon 没有 Info.plist，系统不给弹窗；一般用不到"},
 {"id":"microphone","label":"麦克风","status":"unknown","hint":"daemon 没有 Info.plist，系统不给弹窗；一般用不到"}]
```

每个权限条目都会携带 `hint` 字符串；没有可操作提示时为空字符串。accessibility / screen_recording 的状态只有 `granted` 或 `needs_settings`；full_disk_access 的状态只有 `granted`、`needs_settings` 或 `unknown`。

`POST /mac/permissions/request` body `{"ids":["all"]}` → 202 `{"triggered":[…],"opened_settings":[…]}`。
实现（daemon 进程内直接调用，弹窗出现在 Mac 屏幕上）：

- accessibility: `AXIsProcessTrustedWithOptions(kAXTrustedCheckOptionPrompt=true)`
- screen_recording: `CGPreflightScreenCaptureAccess` / `CGRequestScreenCaptureAccess`
- input_monitoring: `IOHIDCheckAccess` / `IOHIDRequestAccess(kIOHIDRequestTypeListenEvent)`
- automation_*: daemon 先通过 `open -g -j -b` 拉起目标 App，再轮询其状态后调用 `AEDeterminePermissionToAutomateTarget`（askUserIfNeeded=true），确保弹窗实际出现
- camera/microphone: `AVCaptureDevice.authorizationStatus(for:)` 经 objc runtime 直调（无 Info.plist 用途声明，不能弹窗，只读状态）
- full_disk_access: 无 API 可弹，依次尝试读取 legacy `TCC.db` 与 `~/Library/Safari`、`Mail`、`Messages`、`Cookies`、`HomeKit` 等 FDA 保护目录，再打开 `x-apple.systempreferences:com.apple.preference.security?Privacy_AllFiles`

accessibility、screen_recording、full_disk_access、input_monitoring 的变更需要重启 daemon 才能被观测到；探针结果按运行中的进程缓存 TCC 授权。AAA.app 本体不申请任何 TCC 权限——归责全在 aaa-daemon，agent 子进程继承。

客户端设置页展示状态列表 + 「一键申请全部」按钮；从手机点按钮时提示「弹窗将出现在 Mac 上，请在 Mac 前完成一次」。

## 配对

`GET /pair` → `{"payload":"aaa://pair?v=1&name=<hostname>&hosts=<h1:p>,<h2:p>&token=<token>"}`
hosts = daemon 探测到的 tailscale MagicDNS 名 / tailscale IP / easytier IP（带端口，按优先级排列）。
Mac 客户端把 payload 渲染成二维码；Android 扫码解析后逐个 host 试连，成功即保存。手输兜底。

## Claude Code hook（可选精确信号）

（Claude hooks 通道已于 2026-09-02 移除：`install-claude-hooks` 与 `/hooks/claude` 不再存在，状态只看屏幕是否在变、问题只看 transcript。）

## SSD 守卫（硬性约束）

`project_root` 不存在时：**绝不 mkdir**（防止在系统盘建占位目录挤掉真 SSD 挂载点）。`/health.ssd_mounted=false`；一切创建/启动/删除类 API 返回 503 `ssd_unmounted`；已有会话不受影响。挂载恢复后自动解除。

**可读性与挂载是两件事**：外置卷被 macOS 的 TCC 挡住时 `stat` 能过而 `opendir` 会**无限阻塞**（等一个没人去点的授权弹窗）。因此：

- 探测一律 `read_dir` 而非 `metadata`，且跑在带超时（3s）的独立线程上——**daemon 永不因权限弹窗卡在启动**，超时即判为 `denied`。
- `root_state` 区分 `unmounted`（挂上就好）与 `denied`（要给 daemon 完全磁盘访问权限），两者修法完全不同，503 的 `message` 直接带上修法（手机端看不到 Mac 的日志）。
- 状态由 5s 健康轮询维护；请求只读缓存值，不逐次探测。

## v1.1 扩展（2026-08-30 用户拍板：消息流 / checkpoint+diff / 收件箱 / 上传 / watchdog / 通知细化）

### 消息流（手机主视图，终端保留可切换）

- `GET /sessions/:id/messages?after=<seq>&limit=<n=200>` → `{"supported":bool,"source":"claude|codex|pi|none","last_seq":N,"messages":[…]}`
- 消息结构：`{"seq":N,"ts":"…","role":"user|assistant|tool|system","kind":"text|thinking|tool_use|tool_result|question|answer","text":"…","tool":{"name":"Bash","summary":"cargo build","status":"ok|err|running"}|null,"question":{…}?}`
- **表单（claude）**：`AskUserQuestion` 工具调用不当普通 tool_use 显示，而是 `kind:"question"`（role assistant）：`text` = 第一题题面，`tool.summary` = 各题 header 用 ` · ` 连接，并附 `"question":{"questions":[{"header":"Color","question":"Pick a color","options":[{"label":"Red","description":"A warm color"}],"multi_select":false}]}`（原样来自工具入参，`multiSelect` 已转 snake_case）。它的 tool_result 变成 `kind:"answer"`（role **user**）：`text` 为用户的回答（单题就是答案本身；多题每行 `题面 → 答案`），`tool.status` 沿用 ok/err。**待答** = 最新一条 question 后面没有 answer，且它不早于本进程 `created_at`（resume 进来的旧 transcript 里悬着的问题，新进程不会再弹框，不算）。这就是会话 `asking` 的定义。
- 客户端渲染约定：`question` 一律**原生对话框**——单选画单选、`multi_select` 画复选、末尾固定一条「其它…」自填；已有 answer 的表单折成已答态；待答且会话存活时才可交互，提交走 `POST /sessions/:id/answer`。不折叠进过程；`answer` 画在用户一侧。
- daemon 在会话 spawn/resume 后定位该会话的 agent 存储文件（resume 已知文件；新会话按 cwd 匹配 + mtime ≥ 启动时刻轮询发现）并增量 tail 解析。**claude 必须支持**（jsonl：user/assistant/tool_use/tool_result/thinking，过滤 isSidechain 与注入块），codex/pi/reasonix 尽力而为（reasonix：chat-jsonl，raw_content 为用户原文，tool_execution.state=failed → err），agy/shell 返回 `supported:false`（agy 存储为 SQLite，客户端回落终端视图）。resume 场景：旧 id 的 transcript 只是延迟兜底（~30s），发现会话自己写的新文件后自动升级；同目录并发会话不共享同一存储文件（已被认领的候选跳过）。
- `/events` 新帧：`{"t":"messages_changed","id":"s_…","last_seq":N}`（≥500ms 节流）。客户端收到后增量拉取。
- `/events` 心跳：服务端每 20s 发一个 WS Ping；客户端应以「45s 无任何帧」为读超时并重连（overlay 网络半开连接检测）。

### git checkpoint + diff + 回滚（后悔药）

- config：`[checkpoint] enabled=true, auto_init_git=false, interval_minutes=10, auto_init_max_mb=512`。agent 会话（≠shell）创建时打 `start` 检查点、退出时打 `end`、运行中每 interval 分钟有变更则打 `auto`。**已有 .git 的项目才有检查点**：项目常常只是一个任务目录（笔记、抓取、一堆 yml），替用户 `git init` 不是 daemon 该做的事，所以 `auto_init_git` 默认 **false**。显式开成 true 时，仍受 `auto_init_max_mb`（默认 512MB）护栏限制——超预算的无 .git 目录跳过 init 与检查点（避免 `.git/objects` 暴涨；0 = 关闭护栏）。已有 .git 的项目不受体积护栏限制。
- 实现硬约束:**绝不触碰项目的 HEAD/index/工作区**：临时 `GIT_INDEX_FILE` + `git add -A` + `write-tree` + `commit-tree`，ref 收在 `refs/aaa-ckpt/<session_id>/<n>-<label>` 下（普通 git 界面不可见，`git log --all` 不污染分支）。
- `GET /sessions/:id/diff` → `{"supported":bool,"base":"<ref>","files":[{"path","status":"added|modified|deleted","additions":N,"deletions":N,"patch":"…≤64KB","truncated":bool}]}`：start 检查点树 vs 当前工作区（含未跟踪文件，同样经临时 index）。
- `POST /sessions/:id/rollback` body `{"confirm":true,"force":false}`：恢复工作区到 start 检查点（checkout 树 + 删除 start 后新增文件；`.git` 与忽略文件不动）。会话仍存活时必须 `force:true`（daemon 先 kill）。客户端必须二次确认。

### 回答表单（daemon 驾驭 Claude Code 对话框）

按键协议实测于 Claude Code 2.1.258（2026-09-02，pyte 采屏）：表单**有 Review/Submit 页** iff 多于一题或任一题多选；单题单选按下即提交。单选：按选项数字键（自动跳下一页/提交）；单选自填：按「Type something」的数字（= 选项数 + 1）、输入文本、回车。多选：逐个数字键切换（高亮停在第 1 行），自填要 **↓×选项数** 落到「Type something」行再输入（自动打勾；此时**回车会把勾取消**，绝不能按），随后 Tab（文本态下 Tab 移到 Submit/Next 行，再回车前进；非文本态 Tab 直接翻页）。最后 daemon 看屏：出现 `Ready to submit your answers?` 就回车确认；`Enter to select` 提示行消失才算成功，3s 内没消失返回 409，让用户去终端收尾。按键之间留 70–160ms 节拍（Ink 一次 read 当一个事件）。

### 任务收件箱

- `GET /inbox?path=<proj>` → `[{"id","text","created_at"}]`；`POST /inbox` `{path,text}`；`DELETE /inbox/:id`。
- 自动喂入：项目会话**进入 waiting**（每会话最多一次）且收件箱非空时，daemon 把条目拼成一条消息（`任务清单：\n1. …\n2. …` + `\r`）写入 PTY 并删除条目。`POST /sessions` 可带 `"feed_inbox":false` 禁用。**不喂的两种情形（都是结构化判断，不读屏）**：claude 会话 `asking`（对话框开着，自由文本会替用户按下高亮项）；claude 会话的目录在 `~/.claude.json` 里尚无 `hasTrustDialogAccepted`（新项目第一屏是信任对话框）。这两种情形条目留在箱里，下一次 waiting 再试。daemon 只读 `~/.claude.json`，永不写它（claude 自己频繁改写，读改写会撞）。
- `POST /sessions` **幂等**：同项目 + 同 agent 已有存活会话时直接返回该会话（不孵第二个进程）；显式并行开第二个用 `"fresh":true`。事件 `{"t":"inbox_changed","path"}`。

### 手机→项目文件通道

- `POST /projects/upload?path=<proj>&name=<fname>`，body = 原始字节（`application/octet-stream`，≤50MB）→ `{"saved_path":"<proj>/_inbox/<ts>-<name>"}`。文件名 slugify、防覆盖。客户端上传后自行把路径发进 composer 告知 agent。

### 已移除（2026-09-02）

Watchdog（`session_stalled` 事件 + 空转告警）、ntfy 推送、waiting 推送去重与冷却、通知渠道分级、快捷短语 chips、Claude hooks——这一整层「监测 + 推送」都拆掉了。理由：读屏猜问题误报不断，去重/冷却掩盖不了根因；用户真正要的只是「跑完了告诉我一声」，而问题本身由消息流按结构化数据原生呈现。

## aaa CLI（第三个客户端）

`aaa` 是 daemon 的终端前端，**不复制任何业务逻辑**：列表、新建、结束、回答全部走上面的 API，因此
CLI 开的会话在 Mac App 和手机上同样可见、可接管。旧的 zsh 菜单脚本改名 `aaal` 保留，作为 daemon
不可用时的兜底（它把 agent 直接跑在当前终端里，不常驻）。

`aaa` 现在是 `cli/aaa` bash 脚本，依赖 bash ≥3.2、curl、python3，无需构建。CLI 动词保持不变；
`ls` 与交互菜单按「执行中 / 待回复 / 已完成」顺序分组会话（待回复 = `asking`）；`wait` 只列 `asking` 的会话；`perms` 会在每个权限的状态旁打印 `hint` 提示文本。

- 连接：默认读 `~/.config/aaa-daemon/config.toml` 取 port + token 连本机；`AAA_HOST=主机:2730`
  + `AAA_TOKEN=…` 指向另一台机器的 daemon。本机连不上时尝试 `launchctl kickstart` 唤醒一次。
- 无参数 = 交互菜单：会话按「执行中 / 待回复 / 已完成」三组列出（序号贯通三组，与 `aaa ls` 一致），其次「项目管理 / macOS 权限 / New \<agent\>」。
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
