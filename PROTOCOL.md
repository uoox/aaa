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
| `~/project` | 项目根默认值（config 可改；示例里写作 `/Volumes/SSD/project`） |
| `<项目根>/.aaa-agents` | **沿用 aaa CLI 的注册表**：每行 `<目录>\t<agent>[\t<对话id>]`，原子整体重写。第三列是该目录最近一次 resume 用的 agent 对话 id：目录迁移后 agent 存储按旧 cwd 查不到会话，靠它兜底（`POST /sessions` resume 命中时回写；`PUT /config` 迁移前全量采集） |
| `~/.cache/aaa-cwds.json` | **沿用 aaa CLI 的缓存**：键 `claude:<path>`（jsonl → cwd）、`cname2:<path>` / `ainame:<path>`（命名缓存）；文件里旧的 `codex:` / `pi:` 键保留不读，格式兼容 |

config.toml 结构：

```toml
port = 2730
token = "aaa_tk_<32hex>"     # 首次运行生成
project_root = "/Volumes/SSD/project"
namer = true                  # haiku 会话命名开关（对应 AAA_NAMER）
```

历史上的 `[ntfy]` / `[watchdog]`（2026-09-02）与 `[checkpoint]`（2026-09-05）段已废弃，旧文件里留着也能解析，只是被忽略。

## Agent 表（2026-09-03 起只支持 Claude Code）

| id | label | 新会话命令 | resume 命令（%ID% 替换） |
|---|---|---|---|
| claude | Claude | `claude --dangerously-skip-permissions` | `claude --resume %ID% --dangerously-skip-permissions` |
| shell | 终端 | `exec zsh -l` | —（shell 无 resume）。**不是 agent**：`/agents` 里带 `terminal:true`，新建项目 / 换 agent 的选择里没有它，见「终端」 |

启动方式：`zsh -lc 'cd <dir> && <cmd>'`，并**由 daemon 显式设置 `PATH`**。
不能指望 login shell：非交互的 `zsh -l` 只读 `.zprofile`、不读 `.zshrc`，而 PATH 通常维护在后者——
launchd 起的 daemon 因此会让每个 agent 都 `command not found`，`shell` 却照常工作。
daemon 在启动时用 `zsh -lic` 问一次「终端里应有的 PATH」（带超时），再把 `~/.local/bin`、
`~/.npm-global/bin` 等常见安装位置并进去兜底；`/agents` 的 `available` 用**同一份** PATH 判断，
所以「显示可用」与「真能启动」不会打架。
会话查找（resume 用）、cwd 探测、purge 只针对 Claude Code 的存储（`~/.claude/projects/<cwd 编码>/<id>.jsonl`，cwd 取自 jsonl 头部记录；`~/.cache/aaa-cwds.json` 里 `claude:<path>` 键与旧 CLI 兼容）。codex / pi / reasonix / agy / grok 的存储读取、解析、purge 已于 2026-09-03 全部移除——本应用是 Claude Code 的指挥台，不再是多 agent 启动器。`Project.agent` / `Session.agent` 字段保留（值只会是 `claude`，终端是 `shell`）。

## 终端（2026-09-02：常驻工具，不与 agent 平齐）

- 终端 = `agent:"shell"` 的会话，但**不是项目的会话**：不出现在项目/会话列表的激活栏里，不参与项目主会话判定；daemon 开 shell 会话**不登记项目**（在项目根开也不会把根目录变成项目）。
- 客户端把它做成**常驻多标签面板**：Android 在首页右上角、设置齿轮左边一个入口；mac 在左侧栏底部、`daemon vX` 状态行上方一个入口。标签 = 存活的 shell 会话按 `created_at` 排序，标为「终端 1…n」（不在项目根开的追加 ` · <目录名>`）；「+」= `POST /sessions {project_path: <health.project_root>, agent:"shell", resume:false, fresh:true}`；关标签 = kill 后 DELETE。
- 终端没有回放价值：客户端看到 shell 会话 `exited` 就 `DELETE /sessions/:id`（daemon 侧仍按普通会话持久化，200 条上限兜底）。
- 「在此目录开终端」保留：在该项目目录开一个 shell 会话，同样归终端面板管。

## 消息流渲染约定

- assistant 的 `text`（回复与过程中的中途文本）按 **CommonMark** 渲染：标题、粗斜体、行内代码、围栏代码块（等宽 + 横向滚动 + 语言标签）、有序/无序/嵌套列表、引用、分隔线、链接（可点）、GFM 表格与删除线尽力支持。用户消息、工具输出、thinking 保持纯文本。两端解析器：mac `pulldown-cmark`，Android `org.commonmark`。**表格（v1.13.1）是真正的网格**：列宽 = 该列最宽单元格按实际排版测出的像素（mac `WindowTextSystem::layout_line`，Android `TextMeasurer`），单元格里的粗体 / 行内代码 / 链接照常渲染，表头加粗下划线，比消息宽时横向滚动。此前拼成等宽文本靠空格对齐，中文落到备用字体时不是等宽字体的两倍宽，列会漂。
- mac 快捷键：⌘N 新建、⌃Tab 切换会话、⌘E 消息流⇄终端、**⌘W 关闭当前会话**（存活 → 终止确认；已退出 → 删除确认；终端面板里 = 关闭当前标签）。

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
  "hooked": true,                 // v1.3：状态由 Claude Code hooks 驱动（见「Claude Code hooks」）
  "error": null,                  // v1.3：上一轮 StopFailure 的错误类型，下一次提交清空
  "background": false,            // v1.13：waiting 且后台还有任务（run_in_background 的 Bash / 异步子代理 / Monitor）没回来 → 客户端标「后台」
  "background_tasks": 0,          // v1.13：上面那个数；都从 transcript 数出来（tool_use 发起 → tool_result 确认 → <task-notification> 回来销掉），只算本进程发起的
  "compacting": false,            // v1.3：PreCompact → PostCompact 之间
  "user_killed": false,           // v1.3：用户主动结束的，客户端不弹「退出」通知
  "usage": {                      // v1.4：statusLine 转来的本会话用量；没收到过为 null
    "model": "Fable 5.1", "model_id": "claude-fable-5-1", "effort": "medium",
    "context_pct": 4.0, "context_window_size": 1000000, "input_tokens": 39759, "output_tokens": 4,
    "cost_usd": 0.28, "duration_ms": 15749, "lines_added": 0, "lines_removed": 0,
    "cache_read_tokens": 30000, "cache_creation_tokens": 10000, "fresh_input_tokens": 200, "cache_hit_pct": 99  // v1.8：statusLine `context_window.current_usage`（最近一次调用的输入构成）；命中率 = 缓存读 ÷ 三项之和
  }
  "preview": "…最近 4 行纯文本…",
  "rows": 40, "cols": 120,
  "pid": 12345, "exit_code": null,
  "resume_id": "9f2c81…",        // 本次启动实际 resume 的会话 id（无则 null）
  "created_at": "…", "last_output_at": "…",
  "summary": "- [x] 修好登录页\n- [ ] 补测试",   // v1.7：整个对话的进度清单（markdown 任务列表），每轮结束后 haiku 重写
  "updated_at": "…"               // v1.6：最近一次状态翻转（running ⇄ waiting、exited）或改名的时刻；
                                  // 不随 PTY 字节跳（last_output_at 会），项目列表按它排序
}
```

状态机（2026-09-02 简化）：屏幕内容在变 → `running`；进程存活 + **可见屏幕 6s 没变** → `waiting`（这轮干完了，轮到你）；进程退出 → `exited`（保留屏幕 + 回滚缓冲，daemon 重启后仍可查看回放）。claude 会话（2026-09-06）：**起始就是 `waiting`、`hooked=true`**——TUI 起来停在输入框就是轮到你；`SessionStart`（startup / resume / clear，不含 compact）也置 `waiting` 并触发一次收件箱喂入；第一次 `UserPromptSubmit` 才是 `running`。以前起始 `running` 没人翻回来，resume 出来的会话会一直显示执行中。
**v1.13 被叫醒**：后台任务 / 异步子代理完成后 Claude Code 把 `<task-notification>` 塞回去、模型重新开跑——这不是用户发言，`UserPromptSubmit` 不触发，以前状态会一直停在 `waiting`（列表上标着激活其实在干活）。现在 daemon 每秒看 transcript：waiting 的 hooked 会话在最近一次 `Stop` 之后（+2s，避免 Stop 前落盘、之后才 tail 到的那条最后回答误判）又出现 assistant 内容 / 工具调用 → 翻回 `running`，直到下一个 `Stop`；这样判出来的 running 若 Stop 一直不来，transcript 60s 没动就压回 `waiting` 兜底。

daemon **不再读屏猜「它在问什么」**：没有 `idle`，没有 `question`，没有提示模式匹配。agent 在等一个具体回答这件事只认一个来源——claude transcript 里的 `AskUserQuestion` 工具调用（结构化，见「消息流」），`asking` 就是它的镜像；其它 agent 没有这种结构化信号，`asking` 恒为 false。

GUI 列表口径（2026-09-06 用户拍板，mac 侧栏 / Android 首页一致；2026-09-07 改成五态、统一两个字）——**单列，一项目一行，不分栏**：
- **Android 没有独立首页了（v1.13.1，2026-09-07 用户拍板）**：会话页 ☰ 抽屉画的就是完整的项目面板（连接状态 / SSD / 终端 / 看板 / 设置、套餐用量、新建项目框、项目列表与长按操作、下拉刷新），当前项目高亮；只有「一个会话都没打开」时（首次进入、按返回退出会话）同一块面板铺满屏当落地页。app 起来直接回最近打开的会话（本机记 `last_session`）。
- 行首是**状态字，不是色点**，五态：`待回复` = `asking`（弹着选项等你选，不选就卡住；哪怕屏幕还在变）；`运行` = 会话 `running` 且不 `asking`；`后台` = `waiting` 且 `background`（停在输入框，但后台 Bash / 异步子代理 / Monitor 还没回来，会自己被叫醒）；`激活` = 会话活着、停在输入框轮到你（`waiting`）；`暂停` = 没有存活项目会话（已 `exited`、只有旧对话、从没跑过）。终端（shell）不算。CLI 的 `ls` / 交互菜单同一套词。Android 一行到底（状态字 + 标题 + 更新时间），没有第二行摘要。
- **`exited` 会话不代表项目**——进程没了它就只是历史，项目标未激活，点一下即 resume（`POST /sessions` `resume:true`）。
- **顺序 = 置顶 → 状态 → 时间**（2026-09-07 用户拍板）：置顶的永远在最前（自己按的顶，状态不该把它挤下去）；组内按状态 `待回复` > `运行` > `后台` > `激活` > `暂停`——卡在等你回答的排最上，它不动，别的都还能自己往前跑；后台仅低于运行；**同状态**里才比时间，键是该项目最新一条会话（含已退出）的 `updated_at`（老 daemon 没有 → `created_at`），没有会话的用目录 mtime；同刻按路径 / 标题稳住。`updated_at` 只在状态翻转 / 改名时变，所以几个会话同时在跑时行不互相换位。
- **关闭确认只在还在执行时弹**：`running` 且不 `asking` → 确认后 `kill`；`waiting` / `asking` → 直接 `kill`；`exited` → 只收起页面，不删记录。

CLI 的 `ls` / 交互菜单按同一五组打印（v1.13 起；以前是「执行中 / 待回复 / 已完成」三组），那是一次性文本输出，不存在跳行问题。

## REST（前缀 `/api/v1`）

| 方法 | 路径 | 说明 |
|---|---|---|
| GET | `/health` | `{version, ssd_mounted, root_state, project_root, uptime_s}`；`root_state ∈ ok\|unmounted\|denied`，`ssd_mounted = (root_state==ok)`（向后兼容：对客户端它一直就是「能不能用」） |
| GET | `/agents` | agent 表 + `available`（which 检查） |
| GET | `/projects` | collect 移植：`[{path,name,mtime,dir_size,ctx_size,agent,session_title}]`，按 mtime 降序。**注册表就是项目名册**：根目录下未登记的目录（顺手 clone 的仓库、杂物）不出现在列表里；经 daemon 建项目/开会话的目录都会自动登记 |
| POST | `/projects` | `{name?, agent?}`；name 经 slugify，空则 `YYYY-MM-DD-HHMM`；已存在 → 409；agent 给了就写注册表。**响应 = 完整项目对象（至少 `{path,name,agent}`）**，客户端依赖 `path` 直接开会话 |
| GET | `/history?limit=<n=200>` | v1.9 会话日志 `{entries:[{id, project_path, project_name, agent, title, created_at, ended_at, exit_code, deleted_at, summary, last_state}]}`：**所有出现过的会话，含已退出、已删除**，最新在前，最多 500 条（`~/.local/state/aaa-daemon/history.json`）。daemon 每秒把池子里的会话同步进去（标题 / 状态 / 清单变了就更新）；`DELETE /sessions/:id`、删项目盖 `deleted_at`。v1.11 起客户端改用 `/history/dashboard`；本接口仍是原始日志（调试 / 兼容旧客户端） |
| GET | `/history/days` | **已移除（v1.13）**：日历 / haiku 日摘要没人看，daemon 不再每 5 分钟跑 haiku 写日摘要 |
| GET | `/history/dashboard` | v1.13 **看板 = 所有会话的进度，没有时间维度**（2026-09-07 用户拍板第二版；聚合只在 daemon 算一次，两端只画）：`{counts:{asking, running, background, active, paused, open_items}, sessions:[{id, title, project_name, project_path, status, alive, deleted, done, open, items:[{done,text}], updated_at}]}`。`status` ∈ asking/running/background/active/paused 与列表五态同口径（池子里 exited 的 = paused；不在池子里的 = paused 且 `alive:false` 不能点开）；`sessions` 已按 待回复 > 运行 > 后台 > 激活 > 暂停 排好，同状态里已删除的沉到组尾、其余最近更新在前；终端不进；已删除的进（`deleted:true`）但不计数。客户端：顶上计数条（点一个只看那一组）+ 未完成条目数 + 搜索；一会话一张卡：状态字 + 标题 + 项目、进度条 `done/total`、没勾的项直接列、做完的折成「已做 N」；「已完成」（paused 且 open=0）默认收起，已删除默认不显示。v1.11 的 today/week/days/spark/date 全部删除 |
| POST | `/projects/pin` | `{path, pinned}`：置顶 / 取消置顶，daemon 侧存（`~/.local/state/aaa-daemon/pins.json`），随后广播 `projects_changed`；`GET /projects` 行多一个 `pinned`。列表口径：置顶的在最前，组内仍按状态 → 时间 |
| POST | `/projects/delete` | `{paths:[…]}` → `{results:[{path, ok, purged:[{agent_label,count}]}], killed:[标题…]}`；目录删除 + Claude Code 会话存储 purge（`purged` 里只会有 `Claude` 一项）。**v1.11.2 起先收会话**：这些目录下还活着的会话（含终端）一起终止（并行，与 `/restart` 同一套）、摘出池子、在会话日志里盖 `deleted_at`，`killed` 报出它们的标题。此前只删目录不动进程——手机上的「删除项目…」对活着的项目也能按，删完 PTY 还在，cwd 成幽灵，`/sessions` 里赖着，mac 侧栏还会为「有会话但没登记」的目录补一行 |
| GET | `/sessions` | 全部会话（含 exited） |
| POST | `/sessions` | `{project_path, agent, resume}`；resume=true 时按 aaa 逻辑找最近会话套 resume 模板；目录不存在则创建（但见 SSD 守卫） |
| POST | `/sessions/:id/input`（信任对话框在屏上时整句消息改进收件箱，返回 `{"ok":true,"queued":true}`）| `{text, enter}`：写入 PTY（enter 补 `\r`）。composer 用。**多行文本**：TUI 开着 bracketed paste（DECSET 2004）时包成一次粘贴、换行归一为 CR，否则每个换行都是一次提交；没开 2004 的 shell 只做 CR 归一 |
| POST | `/sessions/:id/answer` | `{answers:[{selected:[0,2], other:"自填文本"|null}, …]}`：回答当前待答的 AskUserQuestion 表单，一项对应一个问题（顺序同 `question.questions`），`selected` 是 0 起的选项下标，`other` 是「其它」自填。daemon 负责把选择翻译成 Claude Code 对话框的按键并确认对话框已关闭（见「回答表单」）。无待答问题 / 非 claude 会话 / 对话框没吃下 → 409；答案形状不对 → 400 |
| POST | `/sessions/:id/kill` | TERM，2s 后 KILL；记录保留为 exited |
| DELETE | `/sessions/:id` | 删除记录与回放（活着先 kill） |
| POST | `/sessions/:id/rename` | `{title}` |
| GET | `/sessions/:id/ports` | 进程树监听端口 `[{port,cmd}]`（mac「Web 预览」入口用；其它客户端未接） |
| POST | `/hooks/:event` | Claude Code hooks 回调（见「Claude Code hooks」）；头 `X-AAA-Session`；永远 200 `{}`。`:event=statusline` 是 statusLine 命令转来的状态 JSON |
| GET | `/usage` | `{plan}`：账号 plan 配额：`{five_hour:{used_percentage,resets_at}, seven_day:{…}, model_scoped:[{display_name,utilization,resets_at}]|null, updated_at}`。5h / 7d 来自最近一次 statusLine 的 `rate_limits`，也来自 daemon 每分钟对 claude.ai usage 接口的轮询；`model_scoped`（按模型的周窗口，如 Fable）**只**来自轮询——statusLine 从不带它。轮询用 Claude Code 自己登录的 OAuth 令牌（macOS 钥匙串 `Claude Code-credentials` / `~/.claude/.credentials.json`），只读不刷新；没登录或令牌过期时沿用旧值。两个来源都还没给过时 `plan=null` |
| GET | `/sessions/:id/artifacts` | `{artifacts:[{url,title,description,file_path,ts}]}`：会话里用 Artifact 工具发布过的链接（报告 / 原型 / 图），按 url 去重，来自 transcript |
| GET | `/sessions/:id/screen` | daemon 侧 vt100 的屏幕文本 `{text, alternate_screen}`。非备用屏时 text 前带最近 500 行回滚；备用屏（Claude Code）只有可见画面。客户端「复制屏幕内容」「打开链接」用它。 |
| GET | `/mac/permissions` | 见「macOS 权限」 |
| POST | `/mac/permissions/request` | 见「macOS 权限」 |
| GET | `/pair` | `{payload}`，二维码内容（见「配对」） |
| GET | `/config` | `{port, token, project_root}`（以磁盘 config.toml 为准） |
| POST | `/restart` | `{force?}`：daemon `exec` 自身（PID 不变，launchd 不受影响）。PTY 都是子进程，重启 = 全部终止，所以有非 exited 会话且不 `force` → 409（消息里列出它们）；`force` 时先 kill 全部（回放保留）再重启——v1.10.2 起一起发 SIGTERM、并行等退出，总耗时 = 最慢一个（约 2–3s），不再一个个等。**v1.10：重启不丢会话**——kill 前把活着的项目会话（终端除外）记进 `resume_after_restart.json`，起来 1.5s 后逐个 `POST /sessions {resume:true}` 自动 resume。resume 出来的新会话把同一对话上一份进度清单带过来（先找池子里已退出的同 resume_id 记录，再找会话日志）。`/health` 多两个字段：`update_pending`（磁盘上的二进制比运行中的新）、`restarting` |
| PUT | `/config` | `{port?, token?, project_root?, migrate?, force?}`。**写盘 + daemon 自我重启**（托管下 exit 让 launchd 拉起，游离下 spawn 新进程）；有非 exited 会话且不带 `force` → 409。**v1.12 `force`**：像 `/restart {force}` 一样先正经结束存活会话，重启后自动 resume——迁根时按**新**路径 resume。**`project_root` 变更且 `migrate=true`（v1.12 起是整套迁移，不只注册表）**：① 各项目对话 id 采进注册表 ② 注册表键改新前缀（原子写，失败整体中止）③ 整根 `rename`（同卷限定，跨卷报错让人手动拷；失败回滚注册表）④ 之后 best-effort 逐项改指向并把失败记进 `report.warnings`：daemon 会话元数据（`sessions/*.json` 的 project_path）、会话日志、置顶、`~/.cache/aaa-cwds.json`（键里的 Claude 目录名与值里的路径，haiku 起的项目名不丢）、**Claude Code 的存储**——`~/.claude/projects/<slug>/` 目录按新路径改名（slug 规则：每个非 ASCII 字母数字换 `-`；撞名合并，同名文件放成 `.migrated`）、目录里 jsonl 的 `"cwd":"…"` 字段换新根（只认紧凑格式，正文里的副本不碰）、`~/.claude.json` 的 `projects` 键换新（allowedTools / 信任不丢）。`claude --resume <id>` 本身全局按 id 找（实测换目录也能 resume），改 Claude 存储是为了 daemon 按 cwd 找「这个目录最近的对话」、新目录下 `/resume` 能列出老对话、memory 跟着走。响应 `{ok, migrated, report:{session_metas, history, pins, cwd_cache, claude_dirs, claude_files, claude_json, warnings[]}, killed[], restarting, port}`。`migrate=false` 时要求新目录已存在，只改指向 |

## WS

### `/api/v1/sessions/:id/attach`

- 连接后 server 先发一个文本帧 `{"t":"hello","session":{…},"rows":R,"cols":C}`。
- 随后 server 发二进制帧：先是**整屏重绘**（回滚缓冲尾部若干行 + 若在备用屏则 `?1049h` + ANSI 清屏 + vt100 `state_formatted()`：当前屏幕**加终端状态**——鼠标上报 1000/1006、括号粘贴、应用光标键、光标显隐），之后持续转发 PTY 原始输出字节。状态必须随重绘下发：claude code 一启动就开鼠标上报，晚于它 attach 的客户端若不知道，点击/滚轮就会被当成本地选区（2026-09-03 修）。
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
{"t":"usage","plan":{…}}                        // v1.4：plan 配额变化，形状同 GET /usage 的 plan
```

通知策略（客户端行为，2026-09-02 用户拍板）：**只有一种通知——「完成」**。`running→waiting` 与 `running→exited`（非本机用户手动 kill）各弹一条，标题带项目名，正文是会话标题。不识别「里面要回什么」、不按问题去重、没有高低优先级、没有空转告警；daemon 侧不推送（ntfy 已移除）。按项目静音是客户端本地配置（两端各存各的）。用户正盯着的会话（窗口前台且当前页就是它）不弹。mac（2026-09-06）：装成 .app 时走 UserNotifications，以 AAA 自己的名义发、点一下回到 App 打开那条会话（`userInfo.session`）；首次会弹系统的通知授权；`cargo run` 没有 bundle 时退回 `osascript`（发件人是脚本编辑器，点了不跳）。

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

权限状态列表与「一键申请全部」目前只在 CLI（`aaa perms`）里；mac / Android 设置页尚未接。

## 配对

`GET /pair` → `{"payload":"aaa://pair?v=1&name=<hostname>&hosts=<h1:p>,<h2:p>&token=<token>"}`
hosts = daemon 探测到的 tailscale MagicDNS 名 / tailscale IP / easytier IP（带端口，按优先级排列）。
Mac 客户端把 payload 渲染成二维码；Android 扫码解析后逐个 host 试连，成功即保存。手输兜底。

## Claude Code hooks（v1.3，事件源）

daemon 起 claude 会话时追加 `--settings ~/.local/state/aaa-daemon/claude-hooks.json`（自动生成、0600、含 token），
片段里全是 `type: http` 的 hooks 指向 `POST /api/v1/hooks/<event>`，`async: true`，请求头
`X-AAA-Session` 取自 PTY 环境变量 `AAA_SESSION`（= daemon 会话 id）。与用户自己的 settings 合并，不改用户文件。

订阅的事件与用途：

| 事件 | 作用 |
|---|---|
| `SessionStart` | `source≠compact` → `state=waiting`（TUI 就绪停在输入框），触发收件箱投喂 |
| `UserPromptSubmit` | `state=running`，清 `error` |
| `Stop` / `Notification(idle_prompt)` | `state=waiting`（精确的「这轮跑完」；触发收件箱投喂）。`Stop` 还触发**进度清单**：daemon 让 haiku（`claude -p --model haiku`，与 namer 同一开关 `namer`）拿「上一版清单 + 这一轮（要求、用过的工具、最后回复）」重写整个对话的 todo list——`- [x] 已做` / `- [ ] 未做`，已做在前，最多 16 项——写进会话的 `summary`；第一份（新会话或 resume 进来还没有清单）看整段对话。mac 详情面板「进度」、Android 会话菜单「进度」渲染成 ☑ / ☐ |
| `StopFailure` | `state=waiting`，`error`=错误类型（rate_limit / overloaded / authentication_failed…） |
| `PreToolUse`（matcher `AskUserQuestion`） | 立刻 `asking=true`，不等 transcript 落盘 |
| `PreCompact` / `PostCompact` | `compacting` 开/关（「整理上下文中」） |
| 任一事件 | `session_id` → `resume_id` 并写回注册表第三列；`transcript_path` → 消息流直接尾随这个文件，不再扫目录 |

收到过事件的会话 `hooked=true`：其 running/waiting 只由事件决定，PTY 输出不再把它翻成 running，6 秒静默启发式也不再作用于它。
没有 hooks 的会话（shell、旧版 Claude Code、写不出 settings 文件时）沿用屏幕启发式。

会话对象新增字段（老客户端忽略）：`hooked`、`error`（string|null）、`compacting`、`user_killed`。

Claude 以 `--dangerously-skip-permissions` 运行，`PermissionRequest` 不会发生，所以没有权限卡片。
片段里同时写 `remoteControlAtStartup=false`：Claude Code 自带的 Remote Control 与本应用功能重叠，daemon 起的会话一律不开（原 `remote_control_name` 配置作废）。

## SSD 守卫（硬性约束）

`project_root` 不存在时：**绝不 mkdir**（防止在系统盘建占位目录挤掉真 SSD 挂载点）。`/health.ssd_mounted=false`；一切创建/启动/删除类 API 返回 503 `ssd_unmounted`；已有会话不受影响。挂载恢复后自动解除。

**可读性与挂载是两件事**：外置卷被 macOS 的 TCC 挡住时 `stat` 能过而 `opendir` 会**无限阻塞**（等一个没人去点的授权弹窗）。因此：

- 探测一律 `read_dir` 而非 `metadata`，且跑在带超时（3s）的独立线程上——**daemon 永不因权限弹窗卡在启动**，超时即判为 `denied`。
- `root_state` 区分 `unmounted`（挂上就好）与 `denied`（要给 daemon 完全磁盘访问权限），两者修法完全不同，503 的 `message` 直接带上修法（手机端看不到 Mac 的日志）。
- 状态由 5s 健康轮询维护；请求只读缓存值，不逐次探测。

## v1.1 扩展（2026-08-30 用户拍板：消息流 / 收件箱 / 上传 / 通知细化；checkpoint+diff 已于 2026-09-05 移除）

### 消息流（手机主视图，终端保留可切换）

- `GET /sessions/:id/messages?after=<seq>&limit=<n=200>` → `{"supported":bool,"source":"claude|none","last_seq":N,"messages":[…]}`
- 消息结构：`{"seq":N,"ts":"…","role":"user|assistant|tool|system","kind":"text|thinking|tool_use|tool_result|question|answer","text":"…","tool":{"name":"Bash","summary":"cargo build","status":"ok|err|running"}|null,"question":{…}?}`
- **表单（claude）**：`AskUserQuestion` 工具调用不当普通 tool_use 显示，而是 `kind:"question"`（role assistant）：`text` = 第一题题面，`tool.summary` = 各题 header 用 ` · ` 连接，并附 `"question":{"questions":[{"header":"Color","question":"Pick a color","options":[{"label":"Red","description":"A warm color"}],"multi_select":false}]}`（原样来自工具入参，`multiSelect` 已转 snake_case）。它的 tool_result 变成 `kind:"answer"`（role **user**）：`text` 为用户的回答（单题就是答案本身；多题每行 `题面 → 答案`），`tool.status` 沿用 ok/err。**待答** = 最新一条 question 后面没有 answer，且它不早于本进程 `created_at`（resume 进来的旧 transcript 里悬着的问题，新进程不会再弹框，不算）。这就是会话 `asking` 的定义。
- 客户端渲染约定：`question` 一律**原生对话框**——单选画单选、`multi_select` 画复选、末尾固定一条「其它…」自填；已有 answer 的表单折成已答态；待答且会话存活时才可交互，提交走 `POST /sessions/:id/answer`。不折叠进过程；`answer` 画在用户一侧。
- daemon 在会话 spawn/resume 后定位该会话的 Claude transcript（resume 已知文件；新会话按 cwd 匹配 + mtime ≥ 启动时刻轮询发现）并增量 tail 解析（jsonl：user/assistant/tool_use/tool_result/thinking，过滤 isSidechain 与注入块）。shell（终端）返回 `supported:false`。resume 场景：旧 id 的 transcript 只是延迟兜底（~30s），发现会话自己写的新文件后自动升级；同目录并发会话不共享同一存储文件（已被认领的候选跳过）。
- **折叠约定（v1.11 修，2026-09-07）**：assistant 的**每一条** `text` 都是回答，一律露出——Claude 的回答天生分段（说一句 → 干活 → 再说一句）。折叠里只放 `thinking` / `tool_use` / `tool_result` / system；一轮内连续的过程消息并成一个折叠段，夹在各段回答之间，展开状态按段内第一条 `seq` 记。只有会话在跑、且**贴在最后**的那一段画成「进行中 · N 步 · 最近：…」。此前只把一轮的最后一条 text 当回答、其余折进「过程」，中途的真回答看起来就成了思考过程。
- `/events` 新帧：`{"t":"messages_changed","id":"s_…","last_seq":N}`（≥500ms 节流）。客户端收到后增量拉取。
- `/events` 心跳：服务端每 20s 发一个 WS Ping；客户端应以「45s 无任何帧」为读超时并重连（overlay 网络半开连接检测）。

### 回答表单（daemon 驾驭 Claude Code 对话框）

按键协议实测于 Claude Code 2.1.258（2026-09-02，pyte 采屏）：表单**有 Review/Submit 页** iff 多于一题或任一题多选；单题单选按下即提交。单选：按选项数字键（自动跳下一页/提交）；单选自填：按「Type something」的数字（= 选项数 + 1）、输入文本、回车。多选：逐个数字键切换（高亮停在第 1 行），自填要 **↓×选项数** 落到「Type something」行再输入（自动打勾；此时**回车会把勾取消**，绝不能按），随后 Tab（文本态下 Tab 移到 Submit/Next 行，再回车前进；非文本态 Tab 直接翻页）。最后 daemon 看屏：出现 `Ready to submit your answers?` 就回车确认；`Enter to select` 提示行消失才算成功，3s 内没消失返回 409，让用户去终端收尾。按键之间留 70–160ms 节拍（Ink 一次 read 当一个事件）。

### 任务收件箱

- `GET /inbox?path=<proj>` → `[{"id","text","created_at"}]`；`POST /inbox` `{path,text}`；`DELETE /inbox/:id`。
- 自动喂入（2026-09-06 起**每秒重试**，不再每会话一次）：项目会话处于 `waiting` 且收件箱非空、门槛放行，daemon 就把条目写入 PTY（+ `\r`）并删除条目——状态翻转、`POST /inbox`、信任对话框刚被接受、表单刚答完，都在下一个 tick 内送达。一条就是那句话本身；多条拼成 `任务清单：\n1. …\n2. …`。`POST /sessions` 可带 `"feed_inbox":false` 禁用。**不喂的两种情形（都是结构化判断，不读屏）**：claude 会话 `asking`（对话框开着，自由文本会替用户按下高亮项）；claude 会话的目录在 `~/.claude.json` 里尚无 `hasTrustDialogAccepted` **且屏幕上正显示信任对话框**（新项目第一屏；父目录已信任时 claude 不问也不写记录，只看文件会永远挡住）。这两种情形条目留在箱里，下一次 waiting 再试。daemon 只读 `~/.claude.json`，永不写它（claude 自己频繁改写，读改写会撞）。
- **自动信任（2026-09-03，2026-09-07 改为一键一 tick）**：config `auto_trust=true`（默认）时，daemon 在每秒 tick 里看 claude 会话的可见屏幕。新版对话框认「Yes, I trust this folder」+「No, exit」两行（提示行滚出屏幕也行）：高亮在 No → 只按 ↓；**高亮到了 Yes 才按 Enter**，绝不 ↓+Enter 连发——连发时 ↓ 偶尔丢（Ink 还没进 raw mode），Enter 落在「No, exit」上 Claude 就退出了，会话卡成 exited、对话框还画在屏上，只能重进项目再来一次。旧版对话框（Yes, proceed 高亮）直接 Enter。每键至少隔 1s，最多 8 键。用户在 AAA 里已经选定了目录，再问一遍纯属摩擦。信任记录仍由 claude 自己写进 `~/.claude.json`，daemon 不碰。这是 daemon 唯一保留的「读屏行动」，条件刻意收窄（两串同现、仅 claude、有上限）。
- `POST /sessions` **幂等**：同项目 + 同 agent 已有存活会话时直接返回该会话（不孵第二个进程）；显式并行开第二个用 `"fresh":true`。事件 `{"t":"inbox_changed","path"}`。
- **客户端呈现（2026-09-06 用户拍板）**：Android 没有单独的收件箱页——收件箱就画在消息流末尾，标「待发送」，✕ 撤回（`DELETE /inbox/:id`）。消息流的「发送」按会话状态分流：会话 `running` 或 `asking` → `POST /inbox`（排成待发送，和终端里先敲好等它一样）；空着 → `POST /sessions/:id/input`。输入框的草稿按会话保存在客户端本地，切出去再回来字还在。会话页左上角是 ☰（不是返回）：拉出与首页同一份项目列表，点一行切会话（回退栈始终 home → 当前会话）。mac 的收件箱仍在详情面板（⌘I）。

### 手机→项目文件通道

- `POST /projects/upload?path=<proj>&name=<fname>`，body = 原始字节（`application/octet-stream`，≤50MB）→ `{"saved_path":"<proj>/_inbox/<ts>-<name>"}`。文件名 slugify、防覆盖。客户端上传后自行把路径发进 composer 告知 agent。

### 已移除

**2026-09-05：git checkpoint + diff + 回滚。** `[checkpoint]` 配置、`refs/aaa-ckpt/*` 检查点、`GET /sessions/:id/diff`、`POST /sessions/:id/rollback`、会话记录里的 `ckpt_start_ref`、Mac 详情栏「改动」块与 Android「本次改动」屏全部拆掉。理由：改动审阅在 IDE / `git diff` 里做得更好，手机上看 patch 不实用，而自动打检查点在无 .git 的任务目录里根本不生效。旧 daemon 留在磁盘上的 `refs/aaa-ckpt/` 引用无害，想清理：`git for-each-ref --format="%(refname)" refs/aaa-ckpt | xargs -n1 git update-ref -d`。

**2026-09-02：** Watchdog（`session_stalled` 事件 + 空转告警）、ntfy 推送、waiting 推送去重与冷却、通知渠道分级、快捷短语 chips、Claude hooks——这一整层「监测 + 推送」都拆掉了。理由：读屏猜问题误报不断，去重/冷却掩盖不了根因；用户真正要的只是「跑完了告诉我一声」，而问题本身由消息流按结构化数据原生呈现。

## aaa CLI（第三个客户端）

`aaa` 是 daemon 的终端前端，**不复制任何业务逻辑**：列表、新建、结束全部走上面的 API（`say` 走 `/input`；表单作答请 `attach` 进 TUI），因此
CLI 开的会话在 Mac App 和手机上同样可见、可接管。

`aaa` 现在是 `cli/aaa` bash 脚本，依赖 bash ≥3.2、curl、python3，无需构建。CLI 动词保持不变；
`ls` 与交互菜单按「执行中 / 待回复 / 已完成」顺序分组会话（待回复 = `asking`）；`wait` 只列 `asking` 的会话；`perms` 会在每个权限的状态旁打印 `hint` 提示文本。

- 连接：默认读 `~/.config/aaa-daemon/config.toml` 取 port + token 连本机；`AAA_HOST=主机:2730`
  + `AAA_TOKEN=…` 指向另一台机器的 daemon。本机连不上时尝试 `launchctl kickstart` 唤醒一次。
- 无参数 = 交互菜单：会话按「执行中 / 待回复 / 已完成」三组列出（序号贯通三组，与 `aaa ls` 一致），其次「项目管理 / macOS 权限 / 新建项目」。
- 动词：`ls` `ps` `wait` `status` `perms` `new` `open` `attach` `say` `kill` `rm` `rename`，
  列表类均有 `--json`。目标可写会话 id / id 前缀 / `ls` 序号 / 项目名 / `.`（当前目录所属项目）。
- `attach` = 直接连 `/sessions/:id/attach`：本地终端进 raw 模式，stdin 原样转发为二进制帧，
  窗口大小变化发 `{"t":"resize"}`。**Ctrl-]** 脱离，会话继续留在 daemon 里。

> **主题（2026-09-03）**：两端各有三套主题——黑暗（下表的原始令牌）、明亮（#F6F7F9 底 / #1B2229 墨 / 强调 #0F8A9E）、Claude 橙（Anthropic 象牙 #FAF9F5 底 / #141413 墨 / 强调 #D97757，终端暖白 #FFFDF7 底 / 墨字，ANSI 走 gruvbox-light）。三套里只有黑暗是暗底终端；两套浅色主题的终端 ANSI 16 色各自带一套亮底可读的（明亮 one-light、Claude gruvbox-light），两端逐色相同。令牌是**角色**（bg / surface / ink / dim / faint / edge / accent / term_bg …），下表数值是黑暗主题的取值；原「CYAN」角色改叫 accent。设置里切换，mac 存 `~/.config/aaa-ui/ui.toml`，Android 存 DataStore `theme`。消息流里用户消息是右对齐的强调色气泡，Claude 的回复是整宽正文 + 「✻ Claude」小字标题。

## 设计令牌（两端 UI 必须一致）

| 令牌 | 值 | 用途 |
|---|---|---|
| bg | `#0e1216` | 页面底 |
| surface | `#1a222b` / raised `#212b36` | 卡片/面板 |
| edge | `#28323e` / `#36434f` | 描边 |
| ink / dim / faint | `#e3ebf3` / `#8b99a8` / `#5f6d7c` | 文字三级 |
| term-bg | `#0a0e12` | 终端底 |
| cyan | `#53c6dd` | 主操作/选中 |
| magenta | `#c583e0` | 品牌（banner） |
| green / amber / red | `#5ecb8f` / `#e3b45c` / `#e57373` | 执行中 / 已激活（轮到你）/ 出错、已退出 |
| agent 色 | 已取消（只有 Claude 一个 agent，不再按 agent 着色） | — |

项目列表不用色点（2026-09-06）：状态写成带边框的小标签——`执行中`（绿）/ `待回复`（黄）/ `已激活`（强调色）/ `未激活`（faint）。连接状态行、工具步骤等处的小色点照旧。
终端字体：等宽（mac 端 SF Mono/Menlo 族，Android 端打包 JetBrains Mono 或系统 monospace）。
