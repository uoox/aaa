package cc.uoox.aaaui

import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import kotlinx.serialization.json.jsonObject
import kotlinx.serialization.json.jsonArray
import kotlinx.serialization.json.jsonPrimitive
import org.junit.Test

class ProtocolTest {
    /** 「已完成」= 暂停且清单全勾完。v1.17 取消「做完的折起来」之后生产代码不再问这个问题，
     *  只有看板分组的测试还拿它当参照——所以 v1.22 从生产代码搬到这里（mac 那边一直挂着 `#[cfg(test)]`）。 */
    private fun cardIsFinished(c: SessionCard): Boolean = c.status == "paused" && c.open == 0

    private val json = ProtocolJson.instance

    @Test fun pairPayloadParsesHostsAndToken() {
        val payload = PairPayload.parse("aaa://pair?v=1&name=Mac%20mini&hosts=mac.tail:2730,100.64.0.1:2730&token=aaa_tk_x")
        assertEquals("Mac mini", payload.name)
        assertEquals(listOf("mac.tail:2730", "100.64.0.1:2730"), payload.hosts)
        assertEquals("aaa_tk_x", payload.token)
    }

    @Test fun pairPayloadRejectsBadInput() {
        for (bad in listOf(
            "https://example.com",
            "aaa://pair?v=2&hosts=h:1&token=t",
            "aaa://pair?v=1&hosts=&token=t",
            "aaa://pair?v=1&hosts=h:1&token=",
        )) {
            try { PairPayload.parse(bad); throw AssertionError("should reject: $bad") }
            catch (_: IllegalArgumentException) { }
        }
    }

    @Test fun sessionAskingParsesAndOldQuestionFieldIsIgnored() {
        val session = json.decodeFromString<Session>("""{"id":"s_1","title":"T","project_path":"/p","project_name":"p","agent":"claude","state":"waiting","asking":true,"question":{"text":"旧字段"}}""")
        assertEquals("waiting", session.state)
        assertTrue(session.asking)
        // 旧 daemon 没有 asking：默认 false
        assertFalse(json.decodeFromString<Session>("""{"id":"s_2","state":"idle"}""").asking)
    }

    @Test fun questionMessageCarriesTheForm() {
        val m = json.decodeFromString<ChatMessage>(
            """{"seq":7,"ts":"2026-09-02T10:00:00.000Z","role":"assistant","kind":"question","text":"Pick fruits",
                "tool":{"name":"AskUserQuestion","summary":"Fruits","status":"running"},
                "question":{"questions":[{"header":"Fruits","question":"Pick fruits","options":[{"label":"Apple","description":"Crisp"},{"label":"Cherry"}],"multi_select":true}]}}""",
        )
        val q = m.question!!.questions.single()
        assertEquals("Fruits", q.header)
        assertTrue(q.multi_select)
        assertEquals(listOf("Apple", "Cherry"), q.options.map { it.label })
        assertEquals("", q.options[1].description)
        // 普通消息没有 question
        assertNull(json.decodeFromString<ChatMessage>("""{"seq":8,"role":"user","kind":"answer","text":"Apple, Cherry"}""").question)
    }

    // ---------- v1.1 payloads ----------

    @Test fun messagesResponseParses() {
        val resp = json.decodeFromString<MessagesResponse>(
            """{"supported":true,"source":"claude","last_seq":42,"messages":[
                {"seq":41,"ts":"2026-08-30T12:00:00Z","role":"assistant","kind":"tool_use","text":"cargo build --release","tool":{"name":"Bash","summary":"cargo build","status":"ok"}},
                {"seq":42,"ts":"2026-08-30T12:00:01Z","role":"assistant","kind":"question","text":"要继续吗？","tool":null}
            ]}""",
        )
        assertTrue(resp.supported)
        assertEquals("claude", resp.source)
        assertEquals(42L, resp.last_seq)
        assertEquals(2, resp.messages.size)
        assertEquals("Bash", resp.messages[0].tool?.name)
        assertEquals("ok", resp.messages[0].tool?.status)
        assertNull(resp.messages[1].tool)
    }

    @Test fun messagesUnsupportedParses() {
        val resp = json.decodeFromString<MessagesResponse>("""{"supported":false,"source":"none","last_seq":0,"messages":[]}""")
        assertFalse(resp.supported)
        assertTrue(resp.messages.isEmpty())
    }

    /**
     * 待发送来自会话对象的 `queued`——**Claude Code 自己的队列**（v1.22 用户拍板：
     * 排队发送按 claude code 逻辑，AAA 不另做一套）。以前这里测的是 `/inbox` 的条目。
     */
    @Test fun sessionCarriesClaudeCodeQueue() {
        val s = json.decodeFromString<Session>(
            """{"id":"s_1","queued":[{"ts":"2026-09-08T10:00:00Z","text":"跑一遍测试"},{"ts":"2026-09-08T10:01:00Z","text":"更新文档"}]}""",
        )
        assertEquals(2, s.queued.size)
        assertEquals("跑一遍测试", s.queued[0].text)
        // 老 daemon 不下发这个字段：空队列，不是崩
        assertTrue(json.decodeFromString<Session>("""{"id":"s_2"}""").queued.isEmpty())
    }

    @Test fun dashboardCardsParseSearchAndFinished() {
        val d = json.decodeFromString<Dashboard>(
            """{"counts":{"asking":1,"running":0,"background":1,"active":2,"paused":5,"open_items":7},
                "sessions":[{"id":"a","title":"改登录页","project_name":"Shop","status":"background","alive":true,"done":1,"open":1,
                             "items":[{"done":true,"text":"修登录"},{"done":false,"text":"补测试"}]},
                            {"id":"b","title":"旧活","project_name":"Mail","status":"paused","done":2,"open":0,"items":[]}]}""",
        )
        assertEquals(7, d.counts.open_items)
        assertEquals(2, d.sessions.size)
        val a = d.sessions[0]
        assertTrue(a.alive && a.status == "background" && a.items[1].text == "补测试" && !a.items[1].done)
        assertTrue(cardMatches(a, "") && cardMatches(a, "测试") && cardMatches(a, "shop") && cardMatches(a, "登录"))
        assertTrue(!cardMatches(a, "支付"))
        assertTrue(!cardIsFinished(a) && cardIsFinished(d.sessions[1]))
        assertTrue(cardRunning(a.copy(status = "running")) && cardRunning(a.copy(status = "background")))
        assertTrue(!cardRunning(a.copy(status = "active")) && !cardRunning(a.copy(status = "running", deleted = true)))
        // 老 daemon 的形状（没有 sessions）必须解码失败 → 界面报「请升级」，不能静默画空看板
        assertTrue(runCatching { json.decodeFromString<Dashboard>("""{"today":{"sessions":1},"days":[]}""") }.isFailure)
    }

    /** v1.22：清单由 daemon 解析好下发；客户端不再自己拆 `summary` 那串 markdown */
    @Test fun sessionChecklistAndAskingSeqComeFromDaemon() {
        val s = json.decodeFromString<Session>(
            """{"id":"s","summary":"- [x] 修好登录页\n- [ ] 补测试\n瞎话","asking_seq":42,
                "checklist":[{"done":true,"text":"修好登录页"},{"done":false,"text":"补测试"}]}""",
        )
        assertEquals(listOf(ChecklistItem(true, "修好登录页"), ChecklistItem(false, "补测试")), s.checklist)
        assertEquals(42L, s.asking_seq)
        // 老 daemon 不给这两样：清单空、没有待答（界面据此不画可答的表单卡片）
        val old = json.decodeFromString<Session>("""{"id":"s","summary":"- [ ] 补测试"}""")
        assertTrue(old.checklist.isEmpty())
        assertNull(old.asking_seq)
    }

    /** v1.22：项目行自带状态 / 代表会话 / 标题 / 排序时间；老 daemon 少这些字段，解码不能炸 */
    @Test fun projectRowCarriesDaemonComputedState() {
        val p = json.decodeFromString<Project>(
            """{"path":"/p/a","name":"a","mtime":"2026-09-01T00:00:00Z","session_id":"s_1","status":"asking",
                "title":"在等你回话","updated_at":"2026-09-08T12:00:00Z","registered":false}""",
        )
        assertEquals("s_1", p.session_id)
        assertEquals("asking", p.status)
        assertEquals("在等你回话", p.title)
        assertEquals("2026-09-08T12:00:00Z", p.updated_at)
        assertFalse(p.registered)
        val old = json.decodeFromString<Project>("""{"path":"/p/b","name":"b"}""")
        assertNull(old.session_id)
        assertEquals("", old.status)
        assertNull(old.title)
        // 老 daemon 不给 registered：注册表里的项目才会出现在它的 /projects 里，默认 true
        assertTrue(old.registered)
    }

    /** schema 是唯一的兼容闸门：缺了就是 0，界面挂降级横幅 */
    @Test fun healthSchemaGate() {
        assertEquals(2, json.decodeFromString<Health>("""{"version":"1.22.0","schema":2}""").schema)
        assertEquals(0, json.decodeFromString<Health>("""{"version":"1.21.0"}""").schema)
        assertTrue(json.decodeFromString<Health>("""{"version":"1.21.0"}""").schema < SCHEMA_PROJECT_STATUS)
    }

    @Test fun uploadResultParses() {
        val r = json.decodeFromString<UploadResult>("""{"saved_path":"/p/demo/_inbox/20260830-shot.png"}""")
        assertTrue(r.saved_path.endsWith("shot.png"))
    }

    // ---------- /events frames ----------

    @Test fun eventFramesParse() {
        val snap = EventFrame.parse("""{"t":"snapshot","sessions":[{"id":"s_1","title":"t","project_path":"/p","project_name":"p","agent":"shell","state":"running"}]}""")
        assertTrue(snap is EventFrame.Snapshot && snap.sessions.single().id == "s_1")

        val upd = EventFrame.parse("""{"t":"session","session":{"id":"s_2","title":"","project_path":"/p","project_name":"p","agent":"claude","state":"waiting"}}""")
        assertTrue(upd is EventFrame.SessionUpdate && upd.session.state == "waiting")

        val rem = EventFrame.parse("""{"t":"session_removed","id":"s_3"}""")
        assertTrue(rem is EventFrame.SessionRemoved && rem.id == "s_3")

        assertTrue(EventFrame.parse("""{"t":"projects_changed"}""") is EventFrame.ProjectsChanged)

        val health = EventFrame.parse("""{"t":"health","ssd_mounted":false}""")
        assertTrue(health is EventFrame.HealthUpdate && !health.ssdMounted)

        val mc = EventFrame.parse("""{"t":"messages_changed","id":"s_4","last_seq":99}""")
        assertTrue(mc is EventFrame.MessagesChanged && mc.id == "s_4" && mc.lastSeq == 99L)

        // inbox_changed：v1.22 起没人读了（待发送随会话对象来），当未知帧忽略
        assertTrue(EventFrame.parse("""{"t":"inbox_changed","path":"/p/x"}""") is EventFrame.Unknown)

        // 2026-09-02 移除的帧：老 daemon 还会发，当未知帧忽略
        assertTrue(EventFrame.parse("""{"t":"session_stalled","id":"s_5","quiet_s":900}""") is EventFrame.Unknown)

        assertTrue(EventFrame.parse("""{"t":"future_frame","x":1}""") is EventFrame.Unknown)
        assertTrue(EventFrame.parse("not json") is EventFrame.Unknown)
    }

    @Test fun rerootKeepsRelativePathAndLeavesOthersAlone() {
        assertEquals("/Volumes/SSD/Agents/aaaproject/x", rerootPath("/Volumes/SSD/project/x", "/Volumes/SSD/project", "/Volumes/SSD/Agents/aaaproject"))
        assertEquals("/Volumes/SSD/Agents/aaaproject", rerootPath("/Volumes/SSD/project", "/Volumes/SSD/project/", "/Volumes/SSD/Agents/aaaproject"))
        assertEquals("同前缀不同目录不算", "/Volumes/SSD/projectX/y", rerootPath("/Volumes/SSD/projectX/y", "/Volumes/SSD/project", "/new"))
        assertEquals("/elsewhere", rerootPath("/elsewhere", "/Volumes/SSD/project", "/new"))
    }

    /** 三端共享向量 fixtures/dashboard.json：客户端的过滤口径（已完成 / 搜索）必须得到 expect */
    @Test fun sharedFixtureFilters() {
        val fx = json.parseToJsonElement(java.io.File("../../fixtures/dashboard.json").readText()).jsonObject
        val d = json.decodeFromString<Dashboard>("""{"sessions":${fx["sessions"]}}""")
        fun expect(k: String) = fx["expect"]!!.jsonObject[k]!!.jsonArray.map { it.jsonPrimitive.content }
        assertEquals(expect("finished"), d.sessions.filter { cardIsFinished(it) && !it.deleted }.map { it.id })
        assertEquals(expect("match_测试"), d.sessions.filter { cardMatches(it, "测试") }.map { it.id })
        assertEquals(listOf("ask", "run", "bg", "act", "poolpau", "pau", "fin", "old", "del"), d.sessions.map { it.id })
        assertEquals(expect("visible_default"), d.sessions.filter { !it.deleted }.map { it.id })
    }
}
