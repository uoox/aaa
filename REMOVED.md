# 已移除

AAA 砍掉过的功能，连同砍它的理由。**这不是契约**——当下的契约在 [`PROTOCOL.md`](PROTOCOL.md)。
留着它是因为「为什么当初删了」比「删了什么」值钱：同一个想法过一阵会有人重新提，
理由还成立就别再做一遍，不成立就大方加回来（`GET /agents` 就是这么回来的）。

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
