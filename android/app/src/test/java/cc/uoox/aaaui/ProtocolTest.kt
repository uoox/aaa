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

    @Test fun diffResponseParses() {
        val resp = json.decodeFromString<DiffResponse>(
            """{"supported":true,"base":"refs/aaa-ckpt/s_1/0-start","files":[
                {"path":"src/main.rs","status":"modified","additions":10,"deletions":2,"patch":"@@ -1 +1 @@\n-a\n+b","truncated":false},
                {"path":"new.txt","status":"added","additions":5,"deletions":0,"patch":"","truncated":true}
            ]}""",
        )
        assertTrue(resp.supported)
        assertEquals(2, resp.files.size)
        assertEquals("modified", resp.files[0].status)
        assertEquals(10, resp.files[0].additions)
        assertTrue(resp.files[1].truncated)
    }

    @Test fun inboxItemsParse() {
        val items = json.decodeFromString<List<InboxItem>>(
            """[{"id":"i_1","text":"跑一遍测试","created_at":"2026-08-30T10:00:00Z"},{"id":"i_2","text":"更新文档","created_at":"2026-08-30T11:00:00Z"}]""",
        )
        assertEquals(2, items.size)
        assertEquals("跑一遍测试", items[0].text)
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
