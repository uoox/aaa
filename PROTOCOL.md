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
│ (py 脚本)   │  同一套 API，同一批会话
└─────────────┘
```

- 端口固定 **2730**（= 0xAAA），监听 `0.0.0.0`（个人内网，安全性由 overlay 网络 + token 提供）。
- 所有 HTTP/WS 均为明文（链路已有 WireGuard 加密），认证 `Authorization: Bearer <token>`；WS 可用 `?token=` 查询参数。
- 时间一律 ISO-8601 UTC 字符串；大小一律原始字节数（人性化格式由客户端渲染）。
- 错误统一 `{"error":{"code":"...","message":"..."}}`，code ∈ `unauthorized | not_found | conflict | bad_request | agent_unknown | ssd_unmounted | internal`。

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

## Agent 表（2026-09-10：Claude Code + Antigravity）

| id | label | 新会话命令 | resume 命令（%ID% 替换） |
|---|---|---|---|
| claude | Claude | `claude --dangerously-skip-permissions` | `claude --resume %ID% --dangerously-skip-permissions` |
| agy | Antigravity | `agy --dangerously-skip-permissions` | `agy --conversation %ID% --dangerously-skip-permissions` |
| shell | 终端 | `exec zsh -l` | —（shell 无 resume）。**不是 agent**：新建项目 / 换 agent 的选择里没有它，见「终端」 |

启动方式：`zsh -lc 'cd <dir> && <cmd>'`，并**由 daemon 显式设置 `PATH`**。
不能指望 login shell：非交互的 `zsh -l` 只读 `.zprofile`、不读 `.zshrc`，而 PATH 通常维护在后者——
launchd 起的 daemon 因此会让每个 agent 都 `command not found`，`shell` 却照常工作。
daemon 在启动时用 `zsh -lic` 问一次「终端里应有的 PATH」（带超时），再把 `~/.local/bin`、
`~/.npm-global/bin` 等常见安装位置并进去兜底；这份 PATH 也是查 agent 可执行文件用的，
所以「显示可用」与「真能启动」不会打架。
每个 agent 的会话存储各一套，daemon 按 agent 分派。**只有事实来源分派，判定不分派**：五态、代表会话、标题回退、排序、未读全程只有一份实现，客户端除了标签之外不需要知道跑的是谁。

| agent | 存储 | resume id | 消息流 |
|---|---|---|---|
| claude | `~/.claude/projects/<cwd 编码>/<id>.jsonl`，cwd 取自 jsonl 头部记录 | 扫该 cwd 最新的 jsonl，文件名即 id | 支持：逐行读 transcript |
| agy | `~/.gemini/antigravity-cli/`：`cache/last_conversations.json` 就是现成的 `cwd → 最近对话 id`，对话本体在 `conversations/<id>.db` | 直接查表，再确认 `.db`/`.pb` 还在（表里可能指向已被 GC 的对话） | 不支持：对话存在 SQLite 里，没有可跟读的 transcript。客户端回落到终端画面 |

**选 agent 的入口**（`GET /agents` 说有哪些、装没装；两端只画）：新建项目时是输入框边上一个字母小标，点一下在装了的之间轮换；换已有项目**下次**开谁，mac 是项目行尾一个字母小标（默认 agent 的行悬停才现身，非默认的常显——一列扫下来看得出哪几个不是 claude），Android 在长按操作单里。**这处两端故意不一样**：mac 有悬停、Android 没有，✕/删 早就是这么分的（mac 行尾悬停按钮，Android 长按单），agent 跟着同一条线走，不为了「一致」在手机每一行上多挂一个字母。装了的少于两个时两端都不画任何入口。

`~/.cache/aaa-cwds.json` 里 `claude:<path>` 键与旧 CLI 兼容。agy 没有 hooks 也没有 statusLine 接入，五态由屏幕差分给出（见「状态」），`asking` 恒为 false，用量/模型/上下文占比为空。pi / reasonix / codex / grok 的存储支持仍在 2026-09-03 删除的状态，没有恢复。

## 终端（2026-09-02：常驻工具，不与 agent 平齐）

- 终端 = `agent:"shell"` 的会话，但**不是项目的会话**：不出现在项目列表里、不参与项目主会话判定（两端都把它单列一节：mac 在侧栏底部的终端面板，Android 在项目面板的项目列表下方，底部一行「＋ 新增终端」）；daemon 开 shell 会话**不登记项目**（在项目根开也不会把根目录变成项目）。
- 客户端把它做成**常驻多标签面板**：Android 在首页右上角、设置齿轮左边一个入口；mac 在左侧栏底部、`daemon vX` 状态行上方一个入口。标签 = 存活的 shell 会话按 `created_at` 排序，标为「终端 1…n」（不在项目根开的追加 ` · <目录名>`）；「+」= `POST /sessions {project_path: <health.project_root>, agent:"shell", resume:false, fresh:true}`；关标签 = kill 后 DELETE。
- 终端没有回放价值：客户端看到 shell 会话 `exited` 就 `DELETE /sessions/:id`（daemon 侧仍按普通会话持久化，200 条上限兜底）。
- 「在此目录开终端」保留：在该项目目录开一个 shell 会话，同样归终端面板管。

## 消息流渲染约定

- assistant 的 `text`（回复与过程中的中途文本）按 **CommonMark** 渲染：标题、粗斜体、行内代码、围栏代码块（等宽 + 横向滚动 + 语言标签）、有序/无序/嵌套列表、引用、分隔线、链接（可点）、GFM 表格与删除线尽力支持。用户消息、工具输出、thinking 保持纯文本。两端解析器：mac `pulldown-cmark`，Android `org.commonmark`。**表格（v1.13.1）是真正的网格**：列宽 = 该列最宽单元格按实际排版测出的像素（mac `WindowTextSystem::layout_line`，Android `TextMeasurer`），单元格里的粗体 / 行内代码 / 链接照常渲染，表头加粗下划线，比消息宽时横向滚动。此前拼成等宽文本靠空格对齐，中文落到备用字体时不是等宽字体的两倍宽，列会漂。
- mac 快捷键：⌘N 新建、⌃Tab 切换会话、⌘E 消息流⇄终端、**⌘W 关闭当前会话**（存活 → 终止确认；已退出 → 删除确认；终端面板里 = 关闭当前标签）。
- mac 输入框（新建项目 / composer / 表单自填 / 设置）里 **⌘ 与 Ctrl 同义**（2026-09-10 用户要求「输入框最好也可以用 command/ctrl+A/V/C」）：A 全选、C 复制、X 剪切、V 粘贴；处理掉的键不再往上冒，免得 macOS 给一声「没人要」的提示音。终端不在此列——终端里 Ctrl-C 是中断，只有 Ctrl-V 与 ⌘V 同为粘贴。
- mac 终端（2026-09-10）：**⇧⏎ 与 ⌥⏎ 都发 `ESC CR`**——Claude Code 读作「换行不发送」，其 `/terminal-setup` 给 iTerm2 / VS Code 配的 Shift+Enter 发的就是这串；zsh / bash 对它没有绑定，误按不出事。**⇧PgUp / ⇧PgDn** 翻本地回滚一屏（备用屏没有回滚，照常送给应用）。视图尺寸变化时本地模型立刻重排、发给 daemon 的 resize 控制帧**去抖 80ms**——拖窗口边缘时一秒几十个尺寸，每个都发就是几十次 SIGWINCH 和 TUI 整屏重排。block 光标底下那个字**一定反色**（光标格单独成一段来画），以前光标压在词中间时是橙块顶着一个墨字。

## 会话模型

```jsonc
{
  "id": "s_9f2c81ab",            // daemon 生成
  "title": "aaa-ui 交互原型设计",  // AI 命名或用户重命名
  "project_path": "/Volumes/SSD/project/aaa-ui",
  "project_name": "aaa-ui",
  "agent": "claude",             // agent id 或 "shell"
  "state": "waiting",            // running | waiting | exited
  "status": "active",             // v1.22：五态里的哪一个（asking|running|background|active|paused）。
                                  // **只此一处**：看板、项目列表、CLI 的分组说的都是这一句；此前
                                  // daemon 一份、mac `RowStatus::of` 一份、Android `projectStateOf` 一份、
                                  // CLI `status_of` 一份，同一台机器同一时刻能给出不同的答案
  "asking": false,                // claude：transcript 里有一条 AskUserQuestion 还没被回答（结构化事实，不是猜的）
  "queued": [],                   // v1.22：此刻排着还没送进去的消息 `[{ts,text}]`——Claude Code 自己的队列
                                  // （transcript 的 queue-operation）加上信任对话框弹着时 daemon 替你收下的那几条。
                                  // 客户端只画不管：排队是 Claude Code 的行为，AAA 不另做一套
  "asking_seq": null,             // v1.22：**待答的就是这一条**（消息流里的 `seq`）；null = 没有待答表单。
                                  // 判定只在 daemon 做一次（`MsgStore::pending_question`），客户端不再各自从消息流倒着找——
                                  // 三端此前各写一遍，daemon 整串比 ts、两端取前 19 字符，同一秒内两边会说不同的话，
                                  // 提交答案就得 409。`asking` 为 true 而这里是 null 的情形是有的：权限对话框 / hook 抢跑
                                  // （transcript 还没落盘）——那两种本来就不画表单卡
  "hooked": true,                 // v1.3：状态由 Claude Code hooks 驱动（见「Claude Code hooks」）
  "error": null,                  // v1.3：上一轮 StopFailure 的错误类型，下一次提交清空
  "background": false,            // v1.13：waiting 且后台还有任务（run_in_background 的 Bash / 异步子代理 / Monitor）没回来 → 客户端标「后台」
  "background_tasks": 0,          // v1.13：上面那个数；都从 transcript 数出来（tool_use 发起 → tool_result 确认 → `<task-notification>` 回来销掉；通知在模型跑着时走 `queue-operation` / `attachment(queued_command)` 记录而不是 user 消息，两种都认），只算本进程发起的；回来时往消息流塞一行 `system`「后台任务完成：<summary>」，折叠在过程里，好看出「后台」→「运行」是谁触发的
  "permission": null,             // v1.16：正在等的权限对话框 {kind, tool_name, summary, tool_input, tool_use_id, since}；null = 没有。
                                  // kind = permission（PermissionRequest hook，或 Notification(permission_prompt) 兜底；能替答）
                                  //      | elicitation（Notification(elicitation_dialog)：MCP 表单，只能去终端；elicitation_response 清掉）
                                  // 来自 PermissionRequest hook（Bash 授权、ExitPlanMode 批准…）；有它就 asking=true（待回复）。
                                  // 客户端画成「允许 / 拒绝」卡片 → POST /sessions/:id/permission；PostToolUse / Stop / UserPromptSubmit 清掉
  "compacting": false,            // v1.3：PreCompact → PostCompact 之间
  "user_killed": false,           // v1.3：用户主动结束的，客户端不弹「退出」通知
  "usage": {                      // v1.4：statusLine 转来的本会话用量；没收到过为 null
    "model": "Fable 5.1", "model_id": "claude-fable-5-1", "effort": "medium",
    "context_pct": 4.0, "context_window_size": 1000000, "input_tokens": 39759, "output_tokens": 4,
    "cost_usd": 0.28, "duration_ms": 15749, "lines_added": 0, "lines_removed": 0,
    "cache_read_tokens": 30000, "cache_creation_tokens": 10000, "fresh_input_tokens": 200, "cache_hit_pct": 99.6  // v1.8：statusLine `context_window.current_usage`（最近一次调用的输入构成）；命中率 = 缓存读 ÷ 三项之和，v1.15 起一位小数（整数四舍五入下 99.6% 显示成 100%，让人以为「全命中了不用 compact」；它只描述上一次调用的输入构成，跟要不要 compact 无关，那看 `context_pct`）
  }
  "rows": 40, "cols": 120,
  "exit_code": null,
  "resume_id": "9f2c81…",        // 本次启动实际 resume 的会话 id（无则 null）
  "created_at": "…", "last_output_at": "…",
  "summary": "- [x] 修好登录页\n- [ ] 补测试",   // v1.7：整个对话的进度清单（markdown 任务列表），每轮结束后 haiku 重写
  "checklist": [                  // v1.22：上面那串 markdown 由 daemon 解析好的结果，客户端直接画，不再各自解析
    {"done": true,  "text": "修好登录页"},
    {"done": false, "text": "补测试"}
  ],                              // `summary` 仍是唯一的真相（haiku 重写它、`POST /checklist` 改它），这里只是它的镜像
  "updated_at": "…"               // v1.6：最近一次状态翻转（running ⇄ waiting、exited）或改名的时刻；
                                  // 不随 PTY 字节跳（last_output_at 会），项目列表按它排序
}
```

状态机（2026-09-02 简化）：屏幕内容在变 → `running`；进程存活 + **可见屏幕 6s 没变** → `waiting`（这轮干完了，轮到你）；进程退出 → `exited`（保留屏幕 + 回滚缓冲，daemon 重启后仍可查看回放）。claude 会话（2026-09-06）：**起始就是 `waiting`、`hooked=true`**——TUI 起来停在输入框就是轮到你；`SessionStart`（startup / resume / clear，不含 compact）也置 `waiting` 并触发一次收件箱喂入；第一次 `UserPromptSubmit` 才是 `running`。以前起始 `running` 没人翻回来，resume 出来的会话会一直显示执行中。
**v1.13 被叫醒**：后台任务 / 异步子代理完成后 Claude Code 把 `<task-notification>` 塞回去、模型重新开跑——这不是用户发言，`UserPromptSubmit` 不触发，以前状态会一直停在 `waiting`（列表上标着激活其实在干活）。现在 daemon 每秒看 transcript：waiting 的 hooked 会话在最近一次 `Stop` 之后（+2s，避免 Stop 前落盘、之后才 tail 到的那条最后回答误判）又出现 assistant 内容 / 工具调用 → 翻回 `running`，直到下一个 `Stop`；这样判出来的 running 若 Stop 一直不来，transcript 60s 没动就压回 `waiting` 兜底。

daemon **不再读屏猜「它在问什么」**：没有 `idle`，没有 `question`，没有提示模式匹配。agent 在等一个具体回答这件事只认一个来源——claude transcript 里的 `AskUserQuestion` 工具调用（结构化，见「消息流」），`asking` 就是它的镜像；其它 agent 没有这种结构化信号，`asking` 恒为 false。

GUI 列表口径（mac 侧栏 / Android 项目面板一致）——**单列，一项目一行，不分栏**：
- **项目列表顶上那个输入框只用来新建项目，两端都不做搜索（v1.22 用户拍板：「侧栏就不要做搜索框了，双端都不要，就是用来创建项目的」）**：输文件夹名、回车（或右边的 ＋）建项目，**不再边打字边过滤列表**（Android 此前顺手做了过滤，mac 没有——又是一处两端不一样）。要找一个项目就在列表里翻：一行只有标题 + 时间 + 一根线，本来就是给眼睛扫的；看板那边的搜索框留着，那里一屏装不下。
- **Android 没有独立首页了（v1.13.1，2026-09-07 用户拍板）**：会话页 ☰ 抽屉画的就是完整的项目面板（顶栏一行、新建项目框、项目列表与长按操作、下拉刷新），当前项目高亮；只有「一个会话都没打开」时（首次进入、按返回退出会话）同一块面板铺满屏当落地页。app 起来直接回最近打开的会话（本机记 `last_session`）。
- **Android 项目面板顶栏只有一行（2026-09-08 用户拍板）**：左边**额度**（套餐用量那串 5h / 7d / 按模型，点开看重置时间），右边 **▦ 看板**、**⚙ 设置**。`AAA` 标题和 `IP · 延迟` 去掉了——app 只有一个，标题是废话；IP 和毫秒数连着好的时候没人看。连接**不**正常时（连接中 / 已断开 / 未配对）左边那一格改写连接状态，断了得说一声，但这不值得常年占一整行；SSD 掉了仍在顶栏加一个 `SSD ✗`。以前额度是顶栏底下单独一行，现在并进这一行。
- **Android 的☰ 左侧栏：小屏铺满，大屏半屏（2026-09-08 用户拍板：「整个左侧栏的宽度：小屏情况下，直接铺满，大屏情况下，半屏」——此前先收对话界面、再给项目列表封顶 400dp，都改错了地方）**：门槛是 `screenWidthDp < 600`（Material 的 compact / medium 分界）——小于就 `fillMaxWidth(1f)` 铺满并去掉抽屉圆角（不然右缘两个角漏出底色），否则 `fillMaxWidth(0.5f)` 占半屏，右半屏留着正在看的那个对话，切之前先看得见要切去哪儿。手机竖屏（343dp）与折叠机外屏走铺满，折叠机内屏（≈674dp）/ 平板 / 横屏走半屏。**宽度只在抽屉那一层定**（会话屏与终端屏各一处 `ModalDrawerSheet`），`ProjectPanel` 自己不管宽度、铺满给它的地方——所以「一个会话都没打开」时它就是整屏。**对话界面、终端、看板都不收**——那几个宽了是有用的。
- **状态用整行的淡底色说，不再画任何记号（2026-09-10 用户拍板：「去掉竖线状态的设计，改为背景色，用浅色」；2026-09-08 那四版记号——转圈 → 蓝点 → 竖线 → 行尾竖线——全部作废）**：**淡蓝底**（令牌 `row_running`）= 它还在动（`running`，或 `waiting` 且 `background`——后台 Bash / 异步子代理 / Monitor 还没回来，会自己被叫醒）；**淡黄底**（令牌 `row_unread`）= 跑完了 / 在等你回话，而**这台设备**还没进去看过；**无底色** = 已读，没什么要你操心的。黄盖过蓝（黄的那个在等你）。两端同一对令牌、同一套判定；行高不随状态跳，也没有任何按帧重画的动画。**选中的项目（当前打开的会话所属的那一行）标题用强调色，整行套一圈 1px 强调色边框**（2026-09-10 用户拍板：下划线改边框）——底色现在归状态用，选中态不能再靠整行强调色底去抢它（mac 此前是那样画的；Android 此前是 `raised` 底）。行高不因选中而变：mac 每一行都留着这 1px 的边框位、平时透明，Android 的 `border` 本来就画在自己的边界内。终端（shell）不算。**一行到底：标题 + 更新时间**，没有第二行摘要——标题顶格起，时间在行尾，两端一致。**时间自己会走**：两端各有一个一分钟一次的空转重画——这一屏平时只在 daemon 推东西时重画，整套系统闲着时那行字会一直停在「刚刚」。
  - 「激活 / 未激活 / 暂停」这些字于 2026-09-08 去掉（用户：对着一列项目说不出任何有用的东西）。`asking` / `background` 等状态仍在协议里，它们是「底色是不是蓝的」和排序的依据，只是不再写成字。
  - **黄点是客户端本地状态，不进 daemon**（用户 2026-09-08 拍板）：打点的时机与三种系统通知完全一样（`asking` 翻 true / `running→waiting` / 出错），进这个项目的会话就清掉。mac 存 `~/.config/aaa-ui/ui.toml` 的 `unread_projects`，Android 存 DataStore 的 `unread_projects`，**两端各看各的**——黄点说的是「我这台还没看」，跨设备同步反而会替另一台把话说了。静音只关通知，不关黄点。
  - **终端行没有状态底色、没有记号**（2026-09-08 用户拍板拿掉三道横杠）：终端没有状态可言；当前打开的终端行与项目行同一种说法：整行一圈强调色边框。两端的终端行顶格起（Android 还比项目行更矮一档：上下各 6dp、字号小半档、行尾一个 28dp 的小 ×——它那边的行本来就比 mac 高得多；mac 侧栏项目行与终端行共用同一套 `sidebar_row`，本来就只有 5px 的上下内边距，不再另做一档），一列扫下来「项目在上、终端在下」靠的是分节和行高，不是图标。
  - CLI 的 `ls` / 交互菜单仍按五个词分组打印：那是一次性文本输出，词在那里是有用的。
- **`exited` 会话不代表项目**——进程没了它就只是历史，这一行没有底色（或淡黄底，若还没看过），点一下即 resume（`POST /sessions` `resume:true`）。
- **顺序 = 黄底 → 在跑 → 时间**（2026-09-08 用户拍板；2026-09-10 拿掉置顶——用户拍板「去掉置顶功能」，`POST /projects/pin`、`pins.json` 与行上的 `pinned` 一并删除）：先是有黄底的——蓝底的还在自己往前走，黄底的那个在等你；再按 `asking` > `running` > `background` > `active` > `paused` 的老次序分档——**这两样都读 `/projects` 行上的 `status` 与 `updated_at`，客户端只留一张 5 行的 rank 表，不再自己判状态**；**同档**里才比时间，同刻按路径 / 标题稳住。`updated_at` 只在状态翻转 / 改名时变，所以几个会话同时在跑时行不互相换位。
- **关闭确认只在还在执行时弹**：`running` 且不 `asking` → 确认后 `kill`；`waiting` / `asking` → 直接 `kill`；`exited` → 只收起页面，不删记录。

CLI 的 `ls` / 交互菜单按五组打印（v1.13 起；以前是「执行中 / 待回复 / 已完成」三组），那是一次性文本输出，不存在跳行问题。

## 版本兼容

**一件事只算一次**：凡是 daemon 能算的（五态、代表会话、标题回退、清单解析、待答是哪一条），v1.22 起一律由 daemon 算好下发，客户端只画。此前三端各写一遍，写出来的还不一样——不是「重复」，是**同一个问题三个答案**。

代价是新客户端配老 daemon 时那些字段不在。规矩只有一条：**`GET /health` 的 `schema` 是唯一的闸门，且降级必须说出来，不许悄悄换一套口径**。

- `schema >= 2`：直接用 `status` / `title` / `session_id` / `updated_at` / `asking_seq` / `checklist`。
- `schema` 缺失或 `< 2`（老 daemon）：客户端**不保留第二套算法**——留着就等于把刚删掉的分歧又养回来。项目行按「没有活会话」画（无底色、标题退到目录名），并在项目列表顶上挂一条**明说的**横幅「daemon 版本过旧（vX.Y.Z），项目状态不可用 —— 请更新 daemon」。这是个几分钟的窗口（daemon 和 mac App 同机同版发布，只有手机可能先更新），值不上养一套影子实现。
- 反过来（老客户端配新 daemon）照旧能用：新字段都是**增量**，老客户端忽略未知字段，仍走它自己那套。
- v1.26 加 agy 走的也是这条路：`GET /agents` / `POST /projects/agent` 是**新增路由**，老 daemon 404 → 客户端拿到空表 → 一个 agent 切换入口都不画，其余照常。加路由不是改语义，`schema` 不动。

**三端共享测试向量**（`fixtures/`，一端改口径就得先改这里，两端的测试都读同一份）：`dashboard.json` = 看板（daemon 的输入 → 必须产出的卡片 → 客户端的过滤与分节）；`projects.json` = 项目列表（daemon 算好的一行 + 本机黄点 → 侧栏的顺序 / 蓝底 / 黄底 / 标题 / 路径归一）；`tokens.json` = 设计令牌与终端 16 色（只有一套主题）。**「两端一致」这句话只有被一份共同的向量盯着才成立**——此前两端各测各的一套，所以 A1–A8 那八处口径分歧谁都没发现。

`schema` 只在**语义**变化时 +1（某个判定从客户端搬进 daemon、某个字段改口径），纯粹新增可选字段不动它。当前值：**2**。

## REST（前缀 `/api/v1`）

| 方法 | 路径 | 说明 |
|---|---|---|
| GET | `/health` | `{version, schema, ssd_mounted, root_state, project_root, uptime_s}`；`root_state ∈ ok\|unmounted\|denied`，`ssd_mounted = (root_state==ok)`（向后兼容：对客户端它一直就是「能不能用」）。**`schema` 是客户端唯一的兼容闸门**（v1.22 起 = `2`，见下方「版本兼容」） |
| GET | `/projects` | collect 移植：`[{path,name,mtime,dir_size,agent,session_title}]`，按 mtime 降序（`pinned` 于 v1.25 随置顶功能一起删除）。**注册表就是项目名册**：根目录下未登记的目录（顺手 clone 的仓库、杂物）不出现在列表里；经 daemon 建项目/开会话的目录都会自动登记。**v1.22 起每行还带这一项目此刻的样子，daemon 算一次、两端只画**：`session_id`（代表这个项目的那个会话，没有活会话时是最近退出的那个；一个都没有 → null）、`status` ∈ `asking\|running\|background\|active\|paused`（与看板、CLI 同一套五态；没有活会话 = `paused`）、`title`（活会话的标题 → agent 存储读出的 `session_title` → 目录名，这条回退链此前三端各写一遍）、`updated_at`（排序键：该项目最新一条**非终端**会话的 `updated_at`，含已退出的；一个会话都没有 → 目录 mtime）。还有 `registered`：注册表里的项目为 `true`；在别处 `aaa open` 开出来、注册表里没有但**此刻有活会话**的目录，daemon 也补一行（`registered:false`，不能 resume / 删项目），否则它无处可点——这一行 mac 此前自己补、Android 压根没补，同一台 daemon 在两端连行数都不一样。客户端**不得**再自己从 `/sessions` 推这几样——两端此前的推法就不一样（mac 取 `updated_at` 最大，Android 先按 agent 过滤再按 `待回复<执行中<其它` 排），同一个项目在两台设备上会显示不同的标题和状态 |
| GET | `/agents` | `[{id,label,available}]`——agent 表加上「这台机器装没装」（`which` 走的是 daemon 给 agent 用的那份 PATH，所以「显示可用」与「真能启动」不会打架）。**不含 `shell`**：终端是面板，不是 agent。客户端据此画「新项目开谁 / 换成谁」，可用的少于两个就一个入口都不画；老 daemon 没有这个路由 → 空表 → 同样不画 |
| POST | `/projects/agent` | `{path, agent}` → `{path, agent}`：换这个项目**下次**开哪个 agent。只改注册表那一行，活着的会话不碰；注册表第三列的对话 id 顺手清掉（那是上一个 agent 的 id，留着就会拿 claude 的 id 去喂 agy）。目录不在注册表里 → 404 |
| POST | `/projects` | `{name?, agent?}`；name 经 slugify，空则 `YYYY-MM-DD-HHMM`；已存在 → 409；agent 给了就写注册表。**响应 = 完整项目对象（至少 `{path,name,agent}`）**，客户端依赖 `path` 直接开会话 |
| GET | `/history?limit=<n=200>` | v1.9 会话日志 `{entries:[{id, project_path, project_name, agent, title, created_at, ended_at, exit_code, deleted_at, summary, last_state}]}`：**所有出现过的会话，含已退出、已删除**，最新在前，最多 500 条（`~/.local/state/aaa-daemon/history.json`）。daemon 每秒把池子里的会话同步进去（标题 / 状态 / 清单变了就更新）；`DELETE /sessions/:id`、删项目盖 `deleted_at`。v1.11 起客户端改用 `/history/dashboard`；本接口仍是原始日志（调试 / 兼容旧客户端） |
| GET | `/history/days` | **已移除（v1.13）**：日历 / haiku 日摘要没人看，daemon 不再每 5 分钟跑 haiku 写日摘要 |
| GET | `/history/dashboard` | v1.13 **看板 = 所有会话的进度，没有时间维度**（2026-09-07 用户拍板第二版；聚合只在 daemon 算一次，两端只画）。**v1.15 第三版：瀑布流、全展开**——一会话一张卡，全部清单项直接列出（没勾的在前、做完的灰掉），不折叠不分组，卡片按估算高度塞进最短的一列（mac 按窗口宽度算列数，Android `LazyVerticalStaggeredGrid` 自适应 300dp），一眼看全；已删除默认藏起来一个开关；**点清单项直接勾 / 取消勾**（`POST /sessions/:id/checklist`）：`{counts:{open_items}, sessions:[{id, title, project_name, project_path, status, alive, deleted, done, open, items:[{done,text}], updated_at}]}`。`status` ∈ asking/running/background/active/paused 与列表五态同口径（池子里 exited 的 = paused；不在池子里的 = paused 且 `alive:false` 不能点开）；`sessions` 已按 待回复 > 运行 > 后台 > 激活 > 暂停 排好，同状态里已删除的沉到组尾、其余最近更新在前；终端不进；已删除的进（`deleted:true`）但不计数。客户端（v1.17 起）：顶上只有未完成条目数 + 搜索；一会话一张卡：**蓝点**（status ∈ running/background）/ 什么都没有 + 标题 + 项目、进度条 `done/total`、没勾的项直接列、做完的折成「已做 N」；已删除默认不显示。**五态计数条与状态字于 2026-09-08 拿掉**（用户拍板：看板和项目列表要说同一套话），`counts` 因此只剩 `open_items`；`status` 仍在协议里——它是排序和「画不画蓝点」的依据。v1.11 的 today/week/days/spark/date 全部删除。**v1.21 两端分成两节**（2026-09-08 用户拍板「看板里东西太多了」）：「**在 AAA 里**」= `alive && status != "paused"`——此刻**还活着**的会话（在跑，或停在输入框等你说话），也就是项目列表上那几行；「**不在 AAA 里**」= 其余全部（进程退出了的，哪怕还在池子里点得开；以及只剩一条记录的），默认**折起来**只写一行「不在 AAA 里 N」，点开才铺。**`alive` 不是这条线**（用户拍板时的实测：318 张卡里 174 张 `alive`，其中 164 张是已退出只为回放留在池子里的，按 `alive` 切等于没切）；`alive` 仍然只管一件事——卡片上有没有「打开」。共享向量 `fixtures/dashboard.json` 里的 `poolpau`（`alive:true` 而 `status:paused`）就是这条线的分水岭，三端测试都盯着它。搜索框里有字时两节都展开——不然搜到的东西藏在折叠节里等于没搜到。分节只在客户端做，daemon 的 `sessions` 顺序不变（分节内部沿用它）。两端的看板都有**滚动条**（mac 右缘一条可拖的细条，Android 一条随内容长短的指示条）：瀑布流全展开之后一屏装不下，没有滚动条就不知道自己在哪儿 |
| POST | `/projects/pin` | **已移除（v1.25，2026-09-10 用户拍板「去掉置顶功能」）**：daemon 不再存 `pins.json`，`GET /projects` 行上也没有 `pinned` 了；旧客户端仍发这个请求会得到 404。列表口径见「会话模型」：黄底 → 在跑 → 时间 |
| POST | `/projects/delete` | `{paths:[…]}` → `{results:[{path, ok, purged:[{agent_label,count}]}], killed:[标题…]}`；目录删除 + 会话存储 purge（`purged` 每个 agent 一项，只报删掉了东西的那些）。agy 的 purge 还要摘掉 `cache/last_conversations.json` 里这个 cwd 的条目——那张表按 cwd 记，不摘的话同名目录重建之后会 resume 到上一个项目的对话。**v1.11.2 起先收会话**：这些目录下还活着的会话（含终端）一起终止（并行，与 `/restart` 同一套）、摘出池子、在会话日志里盖 `deleted_at`，`killed` 报出它们的标题。此前只删目录不动进程——手机上的「删除项目…」对活着的项目也能按，删完 PTY 还在，cwd 成幽灵，`/sessions` 里赖着，mac 侧栏还会为「有会话但没登记」的目录补一行 |
| GET | `/sessions` | 全部会话（含 exited） |
| POST | `/sessions` | `{project_path, agent, resume}`；resume=true 时按 aaa 逻辑找最近会话套 resume 模板；目录不存在则创建（但见 SSD 守卫） |
| POST | `/sessions/:id/input`（信任对话框在屏上时整句消息改进收件箱，返回 `{"ok":true,"queued":true}`）| `{text, enter}`：写入 PTY（enter 补 `\r`）。composer 用。**多行文本**：TUI 开着 bracketed paste（DECSET 2004）时包成一次粘贴、换行归一为 CR，否则每个换行都是一次提交；没开 2004 的 shell 只做 CR 归一 |
| POST | `/sessions/:id/answer` | `{answers:[{selected:[0,2], other:"自填文本"|null}, …]}`：回答当前待答的 AskUserQuestion 表单，一项对应一个问题（顺序同 `question.questions`），`selected` 是 0 起的选项下标，`other` 是「其它」自填。daemon 负责把选择翻译成 Claude Code 对话框的按键并确认对话框已关闭（见「回答表单」）。无待答问题 / 非 claude 会话 / 对话框没吃下 → 409；答案形状不对 → 400 |
| POST | `/sessions/:id/permission` | v1.16 `{behavior: "allow"|"deny"}`：替用户答权限对话框。allow = 按对话框第 1 项（Yes，数字键即选中）再补一个 Return 兜底；deny = Esc（No，回到输入框让用户说要怎么改）。答完等 `permission` 被清掉（PostToolUse / Stop），2 秒还在 → 409「去终端里看一眼」。没有对话框在等 → 409。**根因**：以前 daemon 不订阅 `PermissionRequest`，Bash 授权 / ExitPlanMode 批准弹在终端里，消息流一无所知，用户切到终端才发现在等（2026-09-07 反馈） |
| POST | `/sessions/:id/checklist` | v1.16 `{text, done}`，**v1.22 加 `index`**（这是清单里的第几项，与会话对象上的 `checklist` 同下标）：给了就只翻这一条、文字只用来核对；不给退回「只翻第一条匹配的」。以前按文字匹配且**每一条同文的都跟着翻**——haiku 重写出两条一样的话时，点一条会勾掉两条。改 `summary` 里那一行并记进 `checklist_overrides`——haiku 下一轮重写清单时按它盖回去，手工勾的不会被冲掉；会话日志同步 |
| POST | `/history/backfill` | v1.16.2：给池子里没有清单的 claude 会话补清单（后台跑，立刻返回 `{missing}`）。transcript 先按 `resume_id` 找，找不到（/clear 过、GC 了）就按项目目录 + 时间窗口（同 cwd 的 transcript 里落在这条会话 [created_at, last_output_at] 的记录最多的那个），按整段对话 haiku 生成一次。daemon 起来 90s 后自动补一轮、之后每小时补 20 条；`aaa backfill` 手动触发。用户 2026-09-07：看板里没显示进度的那些对话要显示进度 |
| POST | `/sessions/:id/kill` | TERM，2s 后 KILL；记录保留为 exited |
| DELETE | `/sessions/:id` | 删除记录与回放（活着先 kill） |
| POST | `/sessions/:id/rename` | `{title}` |
| GET | `/sessions/:id/ports` | 进程树监听端口 `[{port,cmd}]`（mac「Web 预览」入口用；其它客户端未接） |
| POST | `/hooks/:event` | Claude Code hooks 回调（见「Claude Code hooks」）；头 `X-AAA-Session`；永远 200 `{}`。`:event=statusline` 是 statusLine 命令转来的状态 JSON |
| GET | `/usage` | `{plan}`：账号 plan 配额：`{five_hour:{used_percentage,resets_at}, seven_day:{…}, model_scoped:[{display_name,utilization,resets_at}]|null, updated_at}`。5h / 7d 来自最近一次 statusLine 的 `rate_limits`，也来自 daemon 每分钟对 claude.ai usage 接口的轮询；`model_scoped`（按模型的周窗口，如 Fable）**只**来自轮询——statusLine 从不带它。轮询用 Claude Code 自己登录的 OAuth 令牌（macOS 钥匙串 `Claude Code-credentials` / `~/.claude/.credentials.json`），只读不刷新；没登录或令牌过期时沿用旧值。两个来源都还没给过时 `plan=null` |
| GET | `/sessions/:id/artifacts` | `{artifacts:[{url,title,description,file_path,ts}]}`：会话里用 Artifact 工具发布过的链接（报告 / 原型 / 图），按 url 去重，来自 transcript |
| GET | `/sessions/:id/detail` | v1.17 详情屏（mac 右侧详情栏 / Android 详情屏）一次要齐的四样「消息流里翻不出来」的东西：`{subagents:[{tool,kind,summary,status,ts}], background_tasks:[{tool,summary,ts}], uploads:[{name,path,size,ts}], skills:[{name,count,last_ts}]}`。`subagents` = transcript 里的 `Agent`（老 transcript 是 `Task`）调用，`status ∈ running|ok|err`，按发起顺序；`background_tasks` = 还没等到 `<task-notification>` 的后台任务（`run_in_background` 的 Bash / Agent，或 Monitor），带工具名和摘要而不只是一个计数（会话 `background` 字段仍是它的条数），早发起的在前；`uploads` = 项目 `_inbox/` 里的文件（`POST /projects/upload` 的落点），新的在前、最多 200 个，目录不存在就是空表；`skills` = 用过的 `Skill` 工具按名字合并计数。会话不在池子里 → 404 |
| GET | `/sessions/:id/screen` | daemon 侧 vt100 的屏幕文本 `{text, alternate_screen}`。非备用屏时 text 前带最近 500 行回滚；备用屏（Claude Code）只有可见画面。客户端「复制屏幕内容」「打开链接」用它。 |
| GET | `/mac/permissions` | 见「macOS 权限」 |
| POST | `/mac/permissions/request` | 见「macOS 权限」 |
| GET | `/pair` | `{payload}`，二维码内容（见「配对」） |
| GET | `/config` | `{port, token, project_root}`（以磁盘 config.toml 为准） |
| POST | `/restart` | `{force?}`：daemon `exec` 自身（PID 不变，launchd 不受影响）。PTY 都是子进程，重启 = 全部终止，所以有非 exited 会话且不 `force` → 409（消息里列出它们）；`force` 时先 kill 全部（回放保留）再重启——v1.10.2 起一起发 SIGTERM、并行等退出，总耗时 = 最慢一个（约 2–3s），不再一个个等。**v1.10：重启不丢会话**——kill 前把活着的项目会话（终端除外）记进 `resume_after_restart.json`，起来 1.5s 后逐个 `POST /sessions {resume:true}` 自动 resume。resume 出来的新会话把同一对话上一份进度清单带过来（先找池子里已退出的同 resume_id 记录，再找会话日志）。`/health` 多两个字段：`update_pending`（磁盘上的二进制比运行中的新）、`restarting`。**v1.16 `{when_idle:true}`**：不是现在——有会话在跑就记下约定（`/health` 的 `restart_scheduled`），tick 里等到没有会话 `running` 时按 force 路径重启（waiting 的收掉、起来后自动 resume）；此刻就没有在跑的直接重启 |
| PUT | `/config` | `{port?, token?, project_root?, migrate?, force?}`。**写盘 + daemon 自我重启**（托管下 exit 让 launchd 拉起，游离下 spawn 新进程）；有非 exited 会话且不带 `force` → 409。**v1.12 `force`**：像 `/restart {force}` 一样先正经结束存活会话，重启后自动 resume——迁根时按**新**路径 resume。**`project_root` 变更且 `migrate=true`（v1.12 起是整套迁移，不只注册表）**：① 各项目对话 id 采进注册表 ② 注册表键改新前缀（原子写，失败整体中止）③ 整根 `rename`（同卷限定，跨卷报错让人手动拷；失败回滚注册表）④ 之后 best-effort 逐项改指向并把失败记进 `report.warnings`：daemon 会话元数据（`sessions/*.json` 的 project_path）、会话日志、`~/.cache/aaa-cwds.json`（键里的 Claude 目录名与值里的路径，haiku 起的项目名不丢）、**Claude Code 的存储**——`~/.claude/projects/<slug>/` 目录按新路径改名（slug 规则：每个非 ASCII 字母数字换 `-`；撞名合并，同名文件放成 `.migrated`）、目录里 jsonl 的 `"cwd":"…"` 字段换新根（只认紧凑格式，正文里的副本不碰）、`~/.claude.json` 的 `projects` 键换新（allowedTools / 信任不丢）。`claude --resume <id>` 本身全局按 id 找（实测换目录也能 resume），改 Claude 存储是为了 daemon 按 cwd 找「这个目录最近的对话」、新目录下 `/resume` 能列出老对话、memory 跟着走。响应 `{ok, migrated, report:{session_metas, history, cwd_cache, claude_dirs, claude_files, claude_json, warnings[]}, killed[], restarting, port}`。`migrate=false` 时要求新目录已存在，只改指向 |

## WS

### `/api/v1/sessions/:id/attach`

- 连接后 server 先发一个文本帧 `{"t":"hello","session":{…},"rows":R,"cols":C}`。
- 随后 server 发二进制帧：先是**整屏重绘**（回滚缓冲尾部若干行 + 若在备用屏则 `?1049h` + ANSI 清屏 + vt100 `state_formatted()`：当前屏幕**加终端状态**——鼠标上报 1000/1006、括号粘贴、应用光标键、光标显隐），之后持续转发 PTY 原始输出字节。状态必须随重绘下发：claude code 一启动就开鼠标上报，晚于它 attach 的客户端若不知道，点击/滚轮就会被当成本地选区（2026-09-03 修）。
- client → server：二进制帧 = 原样写入 PTY 的输入字节；文本帧 = 控制消息 `{"t":"resize","cols":C,"rows":R}`。
- 多客户端可同时 attach 同一会话。**PTY 的尺寸每一维取所有连着的客户端里最小的那个**（v1.23，tmux 的老规矩；`pool::effective_size`）：大的那扇窗户右边 / 下边空一条，总好过小的那扇看不全——看不全的那个连滚动都救不回来，超出的列根本没画出来过。此前是「最后 resize 的说了算」，手机在后台连着时一帧 resize 就把桌面那扇窗户按成手机宽，桌面上正在看的输出当场重排。断开的那一位立刻从计算里去掉；一个都不剩就保持现状（没人看的时候把 PTY 抖一下没有意义，还让 TUI 白白重排一次）。

### `/api/v1/events`

server → client JSON 文本帧：

```jsonc
{"t":"snapshot","sessions":[…全部会话…]}        // 连接即发
{"t":"session","session":{…}}                   // 状态/title 变化，≥250ms 节流
{"t":"session_removed","id":"s_…"}
{"t":"projects_changed"}
{"t":"health","ssd_mounted":true}
{"t":"usage","plan":{…}}                        // v1.4：plan 配额变化，形状同 GET /usage 的 plan
```

**补齐机制只有一个：`snapshot`，而且它是权威的。** 客户端收到 snapshot 就**整表替换**自己那份会话列表（不是合并）。daemon 在两种情形下补发它：**连上来**时；以及这条连接**掉帧**时（broadcast 落后于 512 帧的缓冲区——手机被系统冻住、网络卡一阵都会）。所以事件流**不需要 revision / seq 号**（2026-09-08 两位外部评审都提过这条，核过代码后判定不需要）：WS 之上是 TCP，帧不会乱序也不会静默丢；能丢的只有「客户端跟不上」这一种，而那一种的出口就是一份新的全量 snapshot。`session_removed` 也不必补——整表替换本身就带着删除。`projects_changed` / `messages_changed` / `health` 是通知不是状态，客户端收到自己去拉。钉住这条的是 `smoke.rs::events_reconnect_snapshot_is_authoritative`。

通知策略（客户端行为，2026-09-07 用户拍板，**三种**）：**待回复**（`asking` 翻 true：表单 / 权限对话框等你）、**运行结束**（`running→waiting`）、**出错**（`error` 出现，或非 0 退出码）；正常退出、自己在 app 里 kill 的都不弹。此前（2026-09-02）只有一种「完成」：`running→waiting` 与 `running→exited`（非本机用户手动 kill）各弹一条，标题带项目名，正文是会话标题。不识别「里面要回什么」、不按问题去重、没有高低优先级、没有空转告警；daemon 侧不推送（ntfy 已移除）。按项目静音是客户端本地配置（两端各存各的）。用户正盯着的会话（窗口前台且当前页就是它）不弹。mac（2026-09-06）：装成 .app 时走 UserNotifications，以 AAA 自己的名义发、点一下回到 App 打开那条会话（`userInfo.session`）；首次会弹系统的通知授权；`cargo run` 没有 bundle 时退回 `osascript`（发件人是脚本编辑器，点了不跳）。

## macOS 权限（一键授权）

目的：agent 进程全部是 daemon 的子进程，TCC 弹窗归责到 daemon 可执行文件；人在 Mac 前**一次性**把弹窗全点掉，此后手机远程操作不再被权限弹窗卡死。

`GET /mac/permissions` →

```jsonc
[{"id":"accessibility","label":"辅助功能","status":"granted","hint":""},   // granted|needs_settings
 {"id":"screen_recording","label":"屏幕录制","status":"needs_settings","hint":"弹窗只有「打开系统设置」——在列表里把 aaa-daemon 勾上，然后重启 daemon 才读得到"},
 {"id":"input_monitoring","label":"输入监控","status":"denied","hint":"系统设置 → 隐私与安全性 → 输入监控 勾上 aaa-daemon，然后重启 daemon"},
 {"id":"full_disk_access","label":"完全磁盘访问","status":"needs_settings","hint":"系统设置 → 隐私与安全性 → 完全磁盘访问权限 → + 加入 ~/.local/bin/aaa-daemon（⌘⇧G 输路径），然后重启 daemon"},
 {"id":"automation_system_events","label":"自动化 · System Events","status":"undetermined","hint":"aaa perms <id> 弹窗后点允许（daemon 会先把目标 App 拉起来）"},
 {"id":"automation_finder","label":"自动化 · Finder","status":"undetermined","hint":"aaa perms <id> 弹窗后点允许（daemon 会先把目标 App 拉起来）"}]
```

每个权限条目都会携带 `hint` 字符串；没有可操作提示时为空字符串。accessibility / screen_recording 的状态只有 `granted` 或 `needs_settings`；full_disk_access 的状态只有 `granted`、`needs_settings` 或 `unknown`。

`POST /mac/permissions/request` body `{"ids":["all"]}` → 202 `{"triggered":[…],"opened_settings":[…]}`。
实现（daemon 进程内直接调用，弹窗出现在 Mac 屏幕上）：

- accessibility: `AXIsProcessTrustedWithOptions(kAXTrustedCheckOptionPrompt=true)`
- screen_recording: `CGPreflightScreenCaptureAccess` / `CGRequestScreenCaptureAccess`
- input_monitoring: `IOHIDCheckAccess` / `IOHIDRequestAccess(kIOHIDRequestTypeListenEvent)`
- automation_*: daemon 先通过 `open -g -j -b` 拉起目标 App，再轮询其状态后调用 `AEDeterminePermissionToAutomateTarget`（askUserIfNeeded=true），确保弹窗实际出现
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
| `Stop` / `Notification(idle_prompt)` | `state=waiting`（精确的「这轮跑完」；触发收件箱投喂）。`Stop` 还触发**进度清单**：daemon 让 haiku（`claude -p --model haiku`，与 namer 同一开关 `namer`）拿「上一版清单 + 这一轮（要求、用过的工具、最后回复）」重写整个对话的 todo list——`- [x] 已做` / `- [ ] 未做`，已做在前，最多 16 项——写进会话的 `summary`；第一份（新会话或 resume 进来还没有清单）看整段对话。mac 详情面板「进度」、Android 会话菜单「进度」渲染成 ☑ / ☐。**v1.22：所有 haiku 调用（命名 + 清单）走同一个全局闸门，最多两个同时在跑**——并行干活时一批会话会同时 `Stop`，以前一口气拉起十几个 `claude -p`（每个能占 60s），机器卡住、配额一起烧；排队比丢掉好，晚几秒无所谓。每会话仍是「上一次还没写完就跳过这一次」（下一轮覆盖得了） |
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
- **表单（claude）**：`AskUserQuestion` 工具调用不当普通 tool_use 显示，而是 `kind:"question"`（role assistant）：`text` = 第一题题面，`tool.summary` = 各题 header 用 ` · ` 连接，并附 `"question":{"questions":[{"header":"Color","question":"Pick a color","options":[{"label":"Red","description":"A warm color"}],"multi_select":false}]}`（原样来自工具入参，`multiSelect` 已转 snake_case）。它的 tool_result 变成 `kind:"answer"`（role **user**）：`text` 为用户的回答（单题就是答案本身；多题每行 `题面 → 答案`），`tool.status` 沿用 ok/err。**待答** = 最新一条 question 后面没有 answer，且它不早于本进程 `created_at`（resume 进来的旧 transcript 里悬着的问题，新进程不会再弹框，不算）。这就是会话 `asking` 的定义。**这条判定只在 daemon 做**（`MsgStore::pending_question`，整串比 `ts`），结果就是会话对象上的 `asking_seq`；客户端画哪张卡片一律看它，不再自己倒着找消息流。v1.22 前三端各判一次、比法还不同（daemon 整串、两端取前 19 字符），同一秒里 daemon 说「不是待答」而客户端说「是」，点提交就是 409。
- 客户端渲染约定：`question` 一律**原生对话框**——单选画单选、`multi_select` 画复选、末尾固定一条「其它…」自填；已有 answer 的表单折成已答态；待答且会话存活时才可交互，提交走 `POST /sessions/:id/answer`。不折叠进过程；`answer` 画在用户一侧。
- daemon 在会话 spawn/resume 后定位该会话的 Claude transcript（resume 已知文件；新会话按 cwd 匹配 + mtime ≥ 启动时刻轮询发现）并增量 tail 解析（jsonl：user/assistant/tool_use/tool_result/thinking，过滤 isSidechain 与注入块）。shell（终端）返回 `supported:false`。resume 场景：旧 id 的 transcript 只是延迟兜底（~30s），发现会话自己写的新文件后自动升级；同目录并发会话不共享同一存储文件（已被认领的候选跳过）。
- **折叠约定（v1.11 修，2026-09-07）**：assistant 的**每一条** `text` 都是回答，一律露出——Claude 的回答天生分段（说一句 → 干活 → 再说一句）。折叠里只放 `thinking` / `tool_use` / `tool_result` / system；一轮内连续的过程消息并成一个折叠段，夹在各段回答之间，展开状态按段内第一条 `seq` 记。只有会话在跑、且**贴在最后**的那一段画成「进行中 · N 步 · 最近：…」。此前只把一轮的最后一条 text 当回答、其余折进「过程」，中途的真回答看起来就成了思考过程。
- `/events` 新帧：`{"t":"messages_changed","id":"s_…","last_seq":N}`（≥500ms 节流）。客户端收到后增量拉取。
  尾随节拍 **250ms**（v1.18 从 1s 改下来；活会话每拍都看，已退出的每 4 拍一次——它们的 transcript 不会再长）。以前节拍本身就是那个 ≥500ms 节流，现在是显式的：窗口内攒着、下一拍补发，一帧都不丢。**这条链路的延迟天花板不在轮询上**——Claude Code 每条**写完**的消息才落一行 JSONL，没有半截的流式条目，所以消息永远是整块到达的，逐字流只存在于 PTY 屏幕上（终端视图看得到）。
- `/events` 心跳：服务端每 20s 发一个 WS Ping；客户端应以「45s 无任何帧」为读超时并重连（overlay 网络半开连接检测）。

### 回答表单（daemon 驾驭 Claude Code 对话框）

按键协议实测于 Claude Code 2.1.258（2026-09-02，pyte 采屏）：表单**有 Review/Submit 页** iff 多于一题或任一题多选；单题单选按下即提交。单选：按选项数字键（自动跳下一页/提交）；单选自填：按「Type something」的数字（= 选项数 + 1）、输入文本、回车。多选：逐个数字键切换（高亮停在第 1 行），自填要 **↓×选项数** 落到「Type something」行再输入（自动打勾；此时**回车会把勾取消**，绝不能按），随后 Tab（文本态下 Tab 移到 Submit/Next 行，再回车前进；非文本态 Tab 直接翻页）。最后 daemon 看屏：出现 `Ready to submit your answers?` 就回车确认；`Enter to select` 提示行消失才算成功，3s 内没消失返回 409，让用户去终端收尾。按键之间留 70–160ms 节拍（Ink 一次 read 当一个事件）。

### 待发送 / 任务收件箱

**v1.22 用户拍板：「排队发送按照 claude code 逻辑，不需要另外实现这个功能，只需要在消息流适配 claude code 逻辑」。** 模型正在跑时你照样能往 TUI 里敲字——Claude Code 自己就会把它排进队列、这一轮结束再送进去。AAA 因此**不再自己做一套排队发送**：

- **客户端的「发送」只有一条路**：`POST /sessions/:id/input`，不再按会话状态分流。排不排队是 Claude Code 的事。
- **消息流末尾画的「待发送」，画的是 Claude Code 自己的队列**：daemon 从 transcript 的 `queue-operation` 读出来（`enqueue` 带 content 进队；`dequeue`（不带 content）弹队头；`remove` / `popAll` 带原文的删那一条、不带的整队清空；排着的那句被送进这一轮时以 `attachment` 的 `prompt` 露面，同样划掉）。`<task-notification>` 与斜杠命令不算——那不是「你打的字在等着发出去」。结果镜在会话对象的 `queued` 上，客户端**只画不管**（没有撤回：那是 TUI 里的事）。
- **`_inbox` 队列降级成 daemon 的安全兜底**，不再是一个功能：`session_input` 只在**信任对话框弹着**时把整句改收进箱里（那时写进去的字会被对话框吞掉，甚至替用户按下「No, exit」），对话框一被接受，下一个 tick 就送达。它和 Claude Code 的队列合成同一份 `queued` 下发，用户看到的是一件事。

- `GET /inbox?path=<proj>` → `[{"id","text","created_at"}]`；`POST /inbox` `{path,text}`；`DELETE /inbox/:id`。
- 自动喂入（2026-09-06 起**每秒重试**，不再每会话一次）：项目会话处于 `waiting` 且收件箱非空、门槛放行，daemon 就把条目写入 PTY（+ `\r`）并删除条目——状态翻转、`POST /inbox`、信任对话框刚被接受、表单刚答完，都在下一个 tick 内送达。一条就是那句话本身；多条拼成 `任务清单：\n1. …\n2. …`。`POST /sessions` 可带 `"feed_inbox":false` 禁用。**不喂的两种情形（都是结构化判断，不读屏）**：claude 会话 `asking`（对话框开着，自由文本会替用户按下高亮项）；claude 会话的目录在 `~/.claude.json` 里尚无 `hasTrustDialogAccepted` **且屏幕上正显示信任对话框**（新项目第一屏；父目录已信任时 claude 不问也不写记录，只看文件会永远挡住）。这两种情形条目留在箱里，下一次 waiting 再试。daemon 只读 `~/.claude.json`，永不写它（claude 自己频繁改写，读改写会撞）。
- **自动信任（2026-09-03，2026-09-07 改为一键一 tick）**：config `auto_trust=true`（默认）时，daemon 在每秒 tick 里看 claude 会话的可见屏幕。新版对话框认「Yes, I trust this folder」+「No, exit」两行（提示行滚出屏幕也行）：高亮在 No → 只按 ↓；**高亮到了 Yes 才按 Enter**，绝不 ↓+Enter 连发——连发时 ↓ 偶尔丢（Ink 还没进 raw mode），Enter 落在「No, exit」上 Claude 就退出了，会话卡成 exited、对话框还画在屏上，只能重进项目再来一次。旧版对话框（Yes, proceed 高亮）直接 Enter。每键至少隔 1s，最多 8 键。用户在 AAA 里已经选定了目录，再问一遍纯属摩擦。信任记录仍由 claude 自己写进 `~/.claude.json`，daemon 不碰。这是 daemon 唯一保留的「读屏行动」，条件刻意收窄（两串同现、仅 claude、有上限）。
- `POST /sessions` **幂等**：同项目 + 同 agent 已有存活会话时直接返回该会话（不孵第二个进程）；显式并行开第二个用 `"fresh":true`。事件 `{"t":"inbox_changed","path"}`。
- **客户端呈现**：两端都在消息流末尾画 `queued`（标「待发送」，只读）。**发送一律 `POST /sessions/:id/input`**（v1.22；此前 Android 按 `running` / `asking` 分流去 `POST /inbox`，那是 AAA 自己那套队列）。输入框的草稿按会话保存在客户端本地，切出去再回来字还在。会话页左上角是 ☰（不是返回）：拉出与首页同一份项目列表，点一行切会话（回退栈始终 home → 当前会话）。mac 的收件箱仍在详情面板（⌘I）。

### 手机→项目文件通道

- `POST /projects/upload?path=<proj>&name=<fname>`，body = 原始字节（`application/octet-stream`，≤50MB）→ `{"saved_path":"<proj>/_inbox/<ts>-<name>"}`。文件名 slugify、防覆盖。客户端上传后自行把路径发进 composer 告知 agent。

### 已移除

**2026-09-08（v1.18）：一批「声明了但没人读」的东西。** 跑了一遍全仓的过度设计审计，删掉的都是功能被砍之后留在原地的残留：
- **`preview`**（`/sessions` 与 `session` 帧里那段「最近 4 行纯文本」）。v1.17 拿掉状态字之后两端都不读它，而 `Session::to_json` 每次都要拿终端解析器的锁、把整屏逐行剥框线。`screen.rs` 因此只剩 `screen_contains`（answer 驱动确认对话框关掉用）。
- ~~**`GET /agents`** 与 `Agent` DTO~~：2026-09-08 删掉的理由是「agent 表冻结成一个常量，给常量做服务发现」。v1.26 加回 agy 之后表不再是常量，接口**已恢复**（见「接口」），这次还带上「这台机器装没装」。
- **`ctx_size`**（`/projects` 行上的字段）：算到了线上，两端都只声明不读。store 层为「多 agent」准备的形状当时一并拆掉（`stores::detect`、`id_exists` / `find` 的 `agent` 参数、`purge` 那个只装一项的 `Vec`、`push` 的 8 参数签名）——**除 `detect` 与 `push` 外都在 v1.26 随 agy 回来了**。删项目返回的 `purged: [{agent_label, count}]` 形状始终没变。
- **摄像头 / 麦克风权限探测**：`/mac/permissions` 少了这两项。`perms.rs` 自己的提示就写着「daemon 没有 Info.plist，系统不给弹窗」——状态永远读不出来，request 也只是打开系统设置。那段 `objc_msgSend` transmute 的裸 FFI 跟着走了。
- **明亮主题 / 黑暗主题**（2026-09-08 / 2026-09-10）：见「设计令牌」——现在只有一套。
- **置顶**（2026-09-10）：`POST /projects/pin`、`pins.json`、行上的 `pinned`、两端的「顶 / 置顶」按钮与排序档一并删除。
- 两端一批只声明不读的 DTO 字段（`pid` / `hooked` / `compacting` / `model_id` / `input_tokens` …）。线上照旧带着它们，serde 与 `ignoreUnknownKeys` 都吃得下，删的只是客户端的解析。

**没删**、审计点名但顶回来的：`GET /history`（看板不收终端，历史账本收——删项目后「记录还在吗」只有这里看得见）、`aaa restart --when-idle` 的本地守望（它存在的前提就是「旧 daemon 还在跑」）、清单解析三合一与 `/artifacts` 并进 `/detail`（都会让新客户端配旧 daemon 时丢数据）、`daemon/examples/` 里的 `migrate_debug` 与 `dashboard_debug`（零运行时成本，是唯一的离线演练 / 聚合入口）。

**2026-09-08：Web 预览（`GET /sessions/:id/ports`）。** 端口扫描（`ports.rs`：`ps` 找进程树 + `lsof` 找监听）、mac 会话头上的 `▶ 预览 :3000` 胶囊、Android ⋮ 里的「打开 Web 预览」全部拆掉。理由（用户 2026-09-08）：从没用过。

**2026-09-08：Android 的 ⋮ 会话菜单。** 九项里大半一年用一次，却占着顶栏。换成右上角一个**详情**按钮（`SessionDetailScreen`）：进度、用量、子代理、后台任务、已上传、产物、已使用技能，重命名 / 重启 agent 收在最后的「更多」一节（结束进程与删除记录**不在**里面：会话的生杀归项目列表长按）。会话顶栏同时从三行并成一行（标题 · 模型 · 上下文占比）。

**2026-09-08：归档。** `POST /projects/archive`、`archived.json`、`auto_archive_days` 自动归档、`GET /projects` 与看板卡片上的 `archived` 字段、三端的「归档 / 取消归档」入口与「归档 N」折叠节全部拆掉。理由（用户 2026-09-08）：归档这套设定和 Claude Code 的用法不搭——项目不是邮件，放着不动就是放着不动，多一层「藏起来」只是多一个要维护的状态。旧 daemon 留在磁盘上的 `~/.local/state/aaa-daemon/archived.json` 无害，可以直接删。

**2026-09-05：git checkpoint + diff + 回滚。** `[checkpoint]` 配置、`refs/aaa-ckpt/*` 检查点、`GET /sessions/:id/diff`、`POST /sessions/:id/rollback`、会话记录里的 `ckpt_start_ref`、Mac 详情栏「改动」块与 Android「本次改动」屏全部拆掉。理由：改动审阅在 IDE / `git diff` 里做得更好，手机上看 patch 不实用，而自动打检查点在无 .git 的任务目录里根本不生效。旧 daemon 留在磁盘上的 `refs/aaa-ckpt/` 引用无害，想清理：`git for-each-ref --format="%(refname)" refs/aaa-ckpt | xargs -n1 git update-ref -d`。

**2026-09-02：** Watchdog（`session_stalled` 事件 + 空转告警）、ntfy 推送、waiting 推送去重与冷却、通知渠道分级、快捷短语 chips、Claude hooks——这一整层「监测 + 推送」都拆掉了。理由：读屏猜问题误报不断，去重/冷却掩盖不了根因；用户真正要的只是「跑完了告诉我一声」，而问题本身由消息流按结构化数据原生呈现。

## aaa CLI（工具箱，v2.0 / 2026-09-07 用户拍板）

`aaa` 不再是「第三个客户端」：项目与会话管理是 Mac App 和手机的事，CLI 只做**工具性**的事——授权、重启、配置、迁根、看状态。
`cli/aaa` 是纯 python3 标准库脚本（无需构建，不再依赖 bash——macOS 自带 bash 3.2 会把紧跟中文的变量名吞掉、`set -e` 下命令替换失败静默退出，都踩过）。token 只走请求头，永远不进 argv。

- 连接：默认读 `~/.config/aaa-daemon/config.toml` 取 port + token 连本机；`AAA_HOST=主机:2730` + `AAA_TOKEN=…` 指向另一台机器的 daemon。本机连不上时 `launchctl kickstart` 唤醒一次。
- `aaa` / `aaa status [--json]`：版本、项目根、运行时长、会话五态计数（终端不算）；新二进制装好没重启会提示。
- `aaa ls [-a] [--json]`：会话按 待回复 / 运行 / 后台 / 激活 / 暂停 分组（与 App 同口径，`-a` 含暂停）；`aaa wait [--json]` 只列 `asking` 的。
- `aaa perms [--json]` / `aaa perms all | <id…>`：`GET /mac/permissions` / `POST /mac/permissions/request`。
- `aaa restart [--force]`：`POST /restart`；有活会话必须 `--force`（先结束、起来后自动 resume），然后等 `/health` 回来并报版本。
- `aaa config`（token 默认打码，`--show-token` 看全）/ `aaa config port <n> | token <t> | root <path> [--migrate] [--force]`：`PUT /config`。
- `aaa migrate-root <新根> [--yes]`：一键迁根——检查旧根 / 目标空 → 发现不在服务的游离 aaa-daemon 进程先问再清 → 列出 cwd 在旧根里、**不是 daemon 起的**进程（环境里没有 `AAA_SESSION` 且祖先链里没有 aaa-daemon）让你确认 → daemon 版本 < 1.12.1 或 `update_pending` 就 `POST /restart {force}` 并等自动 resume 收尾 → `PUT /config {project_root, migrate:true, force:true}` 打印 `report` 与 warnings → 等 daemon 回来验证项目根 → 顺手把 `~/.config/aaa-ui/ui.toml` 里的静音路径改成新根。禁止 sudo。
- `aaa service install|uninstall|status`：转调 `aaa-daemon service …`。
- 已移除（v2.0）：交互菜单、`ps` `new` `open` `attach` `say` `kill` `rm` `rename`。

> **主题（2026-09-03 起多套；2026-09-08 砍到两套；2026-09-10 砍到一套）**：两端只有**一套**主题——Anthropic 的象牙白 `#FAF9F5` 底 / `#141413` 墨 / 强调 `#D97757`，终端暖白 `#FFFDF7` 底 / 墨字，ANSI 走 gruvbox-light。黑暗主题于 2026-09-10 拿掉（用户：「不需要黑暗模式，仅保留一个主题即可，精简代码」）：没有主题开关、没有进程级调色板、没有 `is_dark` 分支——令牌就是常量（mac `theme.rs` 的 `pub const`，Android `Tok` 的 `val`）。存量配置里 mac `ui.toml` 的 `theme` 字段和 Android DataStore 的 `theme` 键被忽略，不需要迁移。令牌是**角色**（bg / surface / ink / dim / faint / edge / accent / term_bg …），下表就是它们的值；原「CYAN」角色叫 accent。消息流里用户消息是右对齐的强调色气泡，Claude 的回复是整宽正文 + 「✻ Claude」小字标题。

## 设计令牌（两端 UI 必须一致）

| 令牌 | 值 | 用途 |
|---|---|---|
| bg | `#faf9f5` | 页面底（象牙白） |
| surface | `#f0eee6` / raised `#e8e6dc` | 卡片/面板 / 浮起一层 |
| edge | `#dad8ce` / light `#c8c6bc` | 描边 |
| ink / dim / faint | `#141413` / `#5e5d59` / `#91908a` | 文字三级 |
| term-bg / term-fg | `#fffdf7` / `#141413` | 终端底（暖白纸面，和界面一体）/ 终端字（= ink） |
| accent / on_accent | `#d97757` / `#141413` | 主操作 / 选中 / 链接 / 选中那一行的边框（陶土橙）；实心主按钮上的字 = ink（v1.25 对齐：mac 此前按亮度算出 `#21120d`，Android 直接用 ink，两端差一丁点也是差） |
| magenta | `#9b6b9e` | 品牌辅色（哑紫；不取橙的邻色，否则和 accent 分不开） |
| green / amber / red | `#2f855a` / `#b8860b` / `#c0392b` | 执行中 / 已激活（轮到你）/ 出错、已退出 |
| blue | `#3f6ea8` | 看板卡片标题前「在跑」那根线。green/amber/red 各有旧含义，accent 是橙，只有蓝读作「它自己在动」 |
| inset | `#fffefa` | 输入框 / 折叠面板这类「下沉底」，比 surface 更亮一点点 |
| row_running / row_unread | `#dde7f1` / `#f6e7c9` | **项目列表行的状态底色**（2026-09-10）：淡蓝 = 在跑，淡黄 = 未读；已读无底色 |
| agent 色 | 已取消（2026-09-03）。v1.26 加回 agy 之后也没恢复：agent 在侧栏是一个字母小标，不是一种颜色 | — |

项目列表的状态（2026-09-10 用户拍板）：**整行的淡底色**，`row_running` 淡蓝 = 在跑、`row_unread` 淡黄 = 未读、没有底色 = 已读；选中的那一行标题用 accent、整行套一圈 1px accent 边框。**不再有竖线、圆点、转圈、下划线，也没有置顶底**。看板卡片上「在跑」仍是标题前一根 2px/2.5dp × 14px/16dp 的蓝竖线——那是一张卡不是一行，卡片有自己的底。连接状态行、工具步骤等处的小色点照旧。
终端字体：等宽（mac 端 Menlo，Android 端打包 JetBrains Mono NL）。
**终端 ANSI 16 色写死在令牌里，两端逐色相同**（v1.23 统一，v1.25 只剩这一套）：gruvbox-light——浅底上 8–15 比 0–7 更沉而不是更亮，7 white 给成暖灰 `#a89984`，15 bright white = ink。**没有「主题不带就用 xterm 默认」这条回退**（Android 此前深色主题正是走的 termux 出厂表，同一段输出在两端颜色完全不一样），共享向量 `fixtures/tokens.json` 钉着全部 20 个角色和 16 色。

两端逐字相同的两处**格式**：文件大小 `human_bytes` / `humanBytes` 一律**一位小数**（`B` / `1.5K` / `2.0M` / `1.2G`；Android 此前 K 不带小数、M 还按大小分两档，同一个文件两端显示不一样）；进度写作 **`3/7 完成`**（斜杠两边不留空格，与看板卡片的 `done/total` 同一种写法；mac 此前写 `3 / 7 完成`）。
