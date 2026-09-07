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

    @Test fun agentTerminalFieldDefaultsAndParses() {
        assertTrue(json.decodeFromString<Agent>("""{"id":"shell","label":"终端","terminal":true}""").terminal)
        assertFalse(json.decodeFromString<Agent>("""{"id":"claude","label":"Claude"}""").terminal)
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

    @Test fun inboxItemsParse() {
        val items = json.decodeFromString<List<InboxItem>>(
            """[{"id":"i_1","text":"跑一遍测试","created_at":"2026-08-30T10:00:00Z"},{"id":"i_2","text":"更新文档","created_at":"2026-08-30T11:00:00Z"}]""",
        )
        assertEquals(2, items.size)
        assertEquals("跑一遍测试", items[0].text)
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
        assertTrue(cardSpinning(a.copy(status = "running")) && cardSpinning(a.copy(status = "background")))
        assertTrue(!cardSpinning(a.copy(status = "active")) && !cardSpinning(a.copy(status = "running", deleted = true)))
        // 老 daemon 的形状（没有 sessions）必须解码失败 → 界面报「请升级」，不能静默画空看板
        assertTrue(runCatching { json.decodeFromString<Dashboard>("""{"today":{"sessions":1},"days":[]}""") }.isFailure)
    }

    @Test fun sessionChecklistParses() {
        val s = json.decodeFromString<Session>("""{"id":"s","summary":"- [x] 修好登录页\n- [ ] 补测试\n瞎话"}""")
        val items = parseChecklist(s.summary)
        assertEquals(listOf(ChecklistItem(true, "修好登录页"), ChecklistItem(false, "补测试")), items)
        assertTrue(parseChecklist(json.decodeFromString<Session>("""{"id":"s"}""").summary).isEmpty())
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

        val ic = EventFrame.parse("""{"t":"inbox_changed","path":"/p/x"}""")
        assertTrue(ic is EventFrame.InboxChanged && ic.path == "/p/x")

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
        assertEquals(listOf("ask", "run", "bg", "act", "pau", "fin", "old", "del"), d.sessions.map { it.id })
        assertEquals(expect("visible_default"), d.sessions.filter { !it.deleted }.map { it.id })
    }
}
