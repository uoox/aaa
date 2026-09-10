# 已移除

AAA 砍掉过的功能，连同砍它的理由。**这不是契约**——当下的契约在 [`PROTOCOL.md`](PROTOCOL.md)。
留着它是因为「为什么当初删了」比「删了什么」值钱：同一个想法过一阵会有人重新提，
理由还成立就别再做一遍，不成立就大方加回来（`GET /agents` 就是这么回来的）。

**2026-09-11（v1.39）：在消息流里替用户答 AskUserQuestion。** `POST /sessions/:id/answer`、daemon 的 `answer.rs`（`Answer` / `validate` / `plan` / `has_review_tab` / `drive` 与那套按键协议）、只服务于它的 `screen.rs`、两端的 `AnswerItem` 与表单的可交互态（mac 的 `Draft` / `form_complete` / `form_interactive` / `FormMode` / `toggle_option` / `submit` / 每题的「其它」输入框，Android 的 `QuestionCard` 选择与提交、`DaemonClient.answer`）全部删掉。`answer.rs` 缩成只管权限的 `permission.rs`。理由（用户 2026-09-11：「信息流对于 claude code 的 qa 问答，选择是不是比较差？这个部分建议是不要设计进通知，而是有通知点进进来到终端界面」）：

- **那是在盲操一个会变的 TUI**。按键协议实测于 Claude Code 2.1.258：多选自填要 `↓ × 选项数` 落到「Type something」行，而**那时按回车会把勾取消**，绝不能按；Tab 在文本态和非文本态语义不同；按键之间要留 70–160ms 节拍（Ink 一次 read 当一个输入事件）；最后靠屏幕上 `Ready to submit your answers?` 和 `Enter to select` 两行提示判断成没成，3 秒不消失就 409。Claude Code 一升级，这串序列就可能失配。
- **失配的后果是答歪，不是答不了**。选错项、把勾取消、或者提交了一个不是你要的答案——比「答不了、你去终端」糟得多。
- **终端里那个对话框是 Claude Code 自己画的**，按什么就是什么，没有中间层。

**留下的**：`asking` / `asking_seq` 照旧（daemon 仍然知道「它在等你答」，通知与卡片都靠它），消息流里那张卡也照旧画——**只是只读**：题面、选项、末尾那条「其它…」都在，看得见它在问什么，待答时露出一个「去终端答」。`POST /sessions/:id/permission` 与横幅上的「允许 / 拒绝」**也留着**：权限对话框只有两个键（数字 `1` / `Esc`），不脆，且「不进 app 直接答」的价值大。

**跟着走的**：通知点进去的落点从消息流改成**终端视图**（Android 的 `EXTRA_VIEW`、mac 同理）。

**顺手修掉的一个 bug**（用户 2026-09-11：「消息流选项卡出现的时候为什么还会有个上层的弹窗」）：Claude Code 对 `AskUserQuestion` 同样发 `PermissionRequest`，而 daemon 那个分支不看工具名，于是选项卡出现时消息流上还浮着一张「允许 / 拒绝」。**它不只是难看**——按「允许」发出去的是 `1` + 回车，等于替用户盲选了第一个选项。契约本来就写着「结构化提问不给按钮」，漏的是那一个判断。只有一句 message 的 `Notification(permission_prompt)` 兜底也一并挡住了。

**2026-09-11（v1.39）：Android 的设置页。** `SettingsScreen.kt` 整个文件、`settings` 路由、项目面板上的 ⚙ 入口，以及只有它在用的 `Modifier.surfaceCard` 一并删掉。理由（用户 2026-09-11）：「Android 这边不用设置，里面那些设置就是默认这些不用改了」。那一屏一共五样东西，逐样看下来确实没有一样是手机上要改的：

- **系统通知**（默认开）、**默认视图**（默认消息流）：默认值就是想要的值，一个开关摆在那儿只是让人多点两下确认它没被改过。
- **后台常驻**（默认关）：**明知的代价**——它是唯一启停前台服务的地方，删了之后这个值永远停在默认值，app 被系统回收之后收不到「跑完」通知。用户拍板时明说了默认就行。已经开过的机器上存的 `true` 还在，`AaaApplication` 起来照样把服务拉起来。
- **重启 daemon**：`aaa restart` 和 Mac App 各有一个入口，手机上按它的场景本来就少。
- **服务器 / 重新配对**：**这一件留着**，但换了地方（见下）。

**跟着走的**：`formatUptime` / `SettingRow` / `ToggleRow` / `Group` 四个只有那一屏在用的私有构件。`POST_NOTIFICATIONS` 的运行时授权本来就在 `MainActivity` 里要，不受影响。

**配对没有跟着删**：首次进 app 时 `gate` 看见 `server == null` 就直接跳 `pair`，那条路不经过设置页；配过之后想换主机 / 换 token，从**项目列表底下那一条**进——那一条只在连接不正常（连接中 / 已断开 / 未配对）或 SSD 掉了的时候才画，整条可点。连着好的时候它一行都不占。这是 2026-09-11 同一天里两次拍板叠出来的结果：先「设置按钮改成和 MacOS 一样 fix 在底部」，随即「不用设置」——底下那一条于是留了下来，但只剩「有事说一声 + 一条回配对页的路」。

**两端在这里故意不一样**：mac 侧栏底部那一条照旧常驻（状态点 + `daemon v…` + ⚙ 设置），mac 的设置页还有东西。

**2026-09-11（v1.38）：看板。** `GET /history/dashboard` 与 daemon 的 `dashboard()` / `SessionCard` / `Counts` / `Dashboard` / `LiveStatus` / `status_rank`、mac 的 `ui/history.rs` 与 `Page::History`、Android 的 `HistoryScreen.kt` 与 ▦ 入口、两端的 `Dashboard`/`SessionCard` DTO 与卡片过滤函数、共享向量 `fixtures/dashboard.json`、`daemon/examples/dashboard_debug.rs`、设计令牌里的 `blue`，全部删掉。理由（用户 2026-09-11，「我的使用次数非常少」）：

- **顶上那个数是假的**。删它那天本机实测：430 张卡，其中 **262 张已删除、412 张 paused**，而「未完成条目 338」是在这 430 张上算的。那些清单是 haiku 在某次 Stop 时写的，属于**已经结束**的会话——不是「还没做完的事」，是做完的事留下的渣。
- **真的那一半和项目列表是同一件事**。「在 AAA 里」那一节当天是 **18 行**，就是项目列表那 18 行；v1.36 之后项目列表已经三栏分好、整行底色说状态、行尾有时间。
- **待决策是同一件事的第三份**。当天是 0。真有的时候，消息流里已经原地出「允许 / 拒绝」卡片，手机也已经收到通知直达那条会话。
- 代价：mac 452 行 + Android 369 行 + 365 行共享向量，加上散在 9 个文件里的测试；从 git log 看它被返工过四次（v1.15 瀑布流、v1.17 砍计数条、v1.21 分两节、v1.27 加待决策）。

**跟着走的**：`POST /sessions/:id/checklist`（勾 / 取消勾只在看板上有）、daemon 的 `checklist_overrides` 与 `summary::set_item` / `set_item_at`、mac 与 Android 的 `kill_all` / `clean_exited` 客户端方法（`aaa` CLI 还在用这两条路由，daemon 侧留着）。**清单本身留着**，两端详情栏的「进度」一节照画，只是**只读**——用户 2026-09-11：「清单不需要开关，默认就是一定要的」。

**留着的**：`GET /history`（账本，删项目后「记录还在吗」只有它看得见，没有客户端画它，要看就 curl）、`POST /sessions/kill_all` 与 `/sessions/clean_exited`（CLI 在用）。**明知的代价**：看板是唯一能看到**已经停掉的**会话的清单的地方，这条路没了；用户拍板时的原话是「代价不用管，我需要的时候手动去看就好」。

**2026-09-10（v1.35）：「浏览」——会话的第三种看法。** `GET /files`（列目录）、`files.rs` 的 `list_dir` / `Entry` / `parent_of`、mac 的 `FilesView`、Android 的 `FilesView`、两端视图轮换里的那一档全部删掉。理由（用户 2026-09-10）：「不需要『浏览』功能，取而代之是项目生成的 Markdown 也显示在产物里面，并支持阅读」。手机上真正会去翻的只有报告和笔记，不是一个 `.git/objects` 也点得进去的文件管理器；而报告本来就该和「发布过的产物链接」排在一起。**留下的是读文件那一半**：`GET /files/read` 还在（守卫也还是那一条：出项目根 404），`GET /sessions/:id/artifacts` 多带一段 `docs`（项目里的 Markdown，最近改的在前），两端在详情栏「产物」一节里画它、点开就读，见 PROTOCOL「产物」。⌘E 因此回到两档（终端 ⇄ 消息流）。

**2026-09-10（v1.35）：Android 详情屏的收件箱。** 那一节、`InboxEntry` DTO、`inboxList` / `inboxAdd` / `inboxDelete` 三个调用、`inbox_changed` 帧的解析与订阅一并删掉。理由（用户 2026-09-10）：「Android 详情里面的收件箱也可以去掉」。v1.32 删 mac 那一节时留下的理由是「排队是人不在跟前才需要的，那正是手机的场景」——用下来手机上也不需要：**排队是 Claude Code 自己的行为**（消息流末尾那些「待发送」），AAA 这一套只是另一个要维护的队列。**daemon 那一套照旧跑着**（`feed.rs` 每秒重试、`GET/POST/DELETE /inbox` 三条路由都在、`inbox_changed` 照发），「信任对话框挡着屏幕时把整句话收下」那条岔路仍靠它落地；现在没有任何客户端入口。

**2026-09-10（v1.32）：mac 详情栏的收件箱。** 那一节、`inbox` 状态表、输入框、`refresh_inbox` / `submit_inbox` / `drop_inbox` 与 mac 侧的三个 `/inbox` 调用一并删掉。理由（用户 2026-09-10）：坐在 Mac 前面的时候直接在消息流里说话就行——排队是「人不在跟前」才需要的东西，那正是手机的场景。**daemon 那一套照旧跑着**（`feed.rs` 每秒重试、`GET/POST/DELETE /inbox` 三条路由都在），入口只剩 Android 的会话详情屏与 CLI。v1.28 给 mac 加这一节时的理由是「daemon 里跑着却没人能往里加东西」，那个理由已经由 Android 那一侧承担了。

**2026-09-10（v1.30）：换 agent。** `POST /projects/agent`、mac 项目行尾的 agent 字母小标、Android 长按单里的「换成 X」、两端的 `swap_project_agent` / `setProjectAgent` 全部拆掉。理由（用户 2026-09-10）：「不要有切换agent的功能，这个永远不要实现」。**保留**的是新建项目时选 agent 那一处——一个项目跑哪个 agent 在建它的时候定，之后不改；真要换就新建一个项目。注册表第三列（上一次 resume 的对话 id）还在，它是 resume 的兜底，与换 agent 无关。`GET /agents` 也还在：新建那个入口要靠它知道装了哪些。

**2026-09-10（v1.30）：消息流里的「✻ Claude」署名。** 两端 agent 那一侧上方那行强调色小字去掉。理由（用户 2026-09-10）：「已经用气泡做分隔了，这样的显示已经能让人知道那是 claude 的回复」——一边靠右有底、一边通栏无底，版式已经把两个人分开了，再挂个署名只是噪音。

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
