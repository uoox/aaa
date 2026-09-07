package cc.uoox.aaaui

import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
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

    @Test fun dashboardParsingSearchAndGrouping() {
        val d = json.decodeFromString<Dashboard>(
            """{"today":{"sessions":2,"done":1,"open":2},"week":{"sessions":3,"done":3,"open":2},"active":1,
                "open":[{"session_id":"a","project_name":"Shop","title":"改登录页","text":"补测试","alive":true,"running":true},
                        {"session_id":"b","project_name":"Mail","title":"DKIM","text":"轮换"},
                        {"session_id":"c","project_name":"Shop","title":"改登录页","text":"发版"}],
                "days":[{"date":"2026-09-07","text":"- 修好登录","sessions":2,"done":1,"open":2,
                         "entries":[{"id":"a","title":"改登录页","project_name":"Shop","alive":true,"running":true,"open":1}]}],
                "spark":[0,1,2]}""",
        )
        assertEquals(2, d.today.sessions)
        assertEquals(1, d.active)
        assertEquals(3, d.open.size)
        assertTrue(d.open[0].alive && d.open[0].running && !d.open[1].alive)
        // 已退出但还在池子里的会话：alive 为真、running 为假（徽标不画，仍可点开）
        assertTrue(!json.decodeFromString<OpenItem>("""{"alive":true}""").running)
        assertEquals(listOf(0, 1, 2), d.spark)
        assertEquals(1, d.days.single().entries.single().open)
        // 缺字段用默认值，daemon 老版本 404 由界面兜底
        assertEquals(0, json.decodeFromString<Dashboard>("{}").active)

        val a = d.open[0]
        assertTrue(openItemMatches(a, "") && openItemMatches(a, "测试") && openItemMatches(a, "shop") && openItemMatches(a, "登录"))
        assertTrue(!openItemMatches(a, "支付"))
        val day = d.days.single()
        assertTrue(dayCardMatches(day, "") && dayCardMatches(day, "修好") && dayCardMatches(day, "shop"))
        assertTrue(!dayCardMatches(day, "支付"))

        // 同一会话里两条一模一样的未勾项（haiku 完全写得出来）：都要留着，
        // 列表 key 因此不能用文字（HistoryScreen 用 itemsIndexed 带序号）
        val dup = listOf(d.open[0], d.open[0])
        assertEquals(2, groupOpenByProject(dup).single().second.size)
        assertEquals(1, dup.map { "todo-${it.session_id}-${it.text}" }.toSet().size)
        assertEquals(2, dup.mapIndexed { i, it -> "todo-${it.session_id}-$i" }.toSet().size)

        // 按项目归并，项目内保持原序；项目按首次出现排
        val groups = groupOpenByProject(d.open)
        assertEquals(listOf("Shop", "Mail"), groups.map { it.first })
        assertEquals(listOf("补测试", "发版"), groups[0].second.map { it.text })
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
}
