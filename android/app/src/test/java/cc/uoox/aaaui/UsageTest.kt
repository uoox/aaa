package cc.uoox.aaaui

import kotlinx.serialization.json.JsonNull
import kotlinx.serialization.json.JsonPrimitive
import org.junit.Assert.assertEquals
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test
import java.time.Instant
import java.time.ZoneId

class UsageTest {
    private val sh = ZoneId.of("Asia/Shanghai")
    // 2026-09-03 周四 10:00 上海
    private val now = Instant.parse("2026-09-03T02:00:00Z")

    @Test fun pctLevels() {
        assertEquals(PctLevel.NORMAL, pctColorLevel(null))
        assertEquals(PctLevel.NORMAL, pctColorLevel(69.9))
        assertEquals(PctLevel.WARN, pctColorLevel(70.0))
        assertEquals(PctLevel.WARN, pctColorLevel(89.9))
        assertEquals(PctLevel.CRIT, pctColorLevel(90.0))
        assertEquals(PctLevel.CRIT, pctColorLevel(100.0))
    }

    /** 会话顶栏那一行：只有模型和上下文占比（2026-09-08 起顶栏并成一行） */
    @Test fun headerSegmentsAreModelAndContextOnly() {
        val u = SessionUsage(model = "Fable 5.1", context_pct = 30.4, cache_hit_pct = 87.0, cost_usd = 1.25)
        assertEquals(listOf("Fable 5.1", "30%"), usageHeaderSegments(u).map { it.text })
        assertEquals(listOf("92%"), usageHeaderSegments(SessionUsage(context_pct = 91.6)).map { it.text })
        assertEquals(listOf(PctLevel.NORMAL, PctLevel.CRIT), usageHeaderSegments(SessionUsage(model = "M", context_pct = 95.0)).map { it.level })
        assertTrue(usageHeaderSegments(null).isEmpty())
        assertTrue(usageHeaderSegments(SessionUsage()).isEmpty())
        assertTrue(usageHeaderSegments(SessionUsage(model = "  ")).isEmpty())
    }

    /**
     * 栏表头右边那一段：**说剩多少**、不要 5h、重置时刻只出现一次。
     * 颜色仍按用掉的算（72% 用掉 = 琥珀），换算的只是那个数字。
     */
    @Test fun planLineFull() {
        val at = JsonPrimitive("2026-09-13T09:59:59Z")
        val plan = PlanUsage(
            five_hour = PlanWindow(32.0, JsonPrimitive("2026-09-10T18:00:00Z")),
            seven_day = PlanWindow(61.0, at),
            model_scoped = listOf(ModelScopedUsage("Fable", 40.0, at), ModelScopedUsage("Opus", 72.0, at)),
        )
        val now = Instant.parse("2026-09-11T00:00:00Z")
        val segs = planLineSegments(plan, now, ZoneId.of("UTC"))
        assertEquals(
            listOf("7d 剩 39%", "Fable 剩 60%", "Opus 剩 28%", "重置 周日 09:59"),
            segs.map { it.text },
        )
        assertEquals("5h 不进来", 3, segs.count { it.text.contains("剩") })
        assertEquals(PctLevel.WARN, segs.maxOf { it.level })
    }

    /** 按模型的窗口跟 7d 不是同一时刻时，它得自己报——不能拿 7d 的时刻替它说话。 */
    @Test fun planLineSplitsDifferingResets() {
        val plan = PlanUsage(
            seven_day = PlanWindow(61.0, JsonPrimitive("2026-09-13T09:59:59Z")),
            model_scoped = listOf(ModelScopedUsage("Fable", 40.0, JsonPrimitive("2026-09-14T09:59:59Z"))),
        )
        val segs = planLineSegments(plan, Instant.parse("2026-09-11T00:00:00Z"), ZoneId.of("UTC")).map { it.text }
        assertEquals(listOf("7d 剩 39%", "Fable 剩 60%", "重置 周日 09:59", "Fable 重置 周一 09:59"), segs)
    }

    @Test fun planLinePartial() {
        val only7d = PlanUsage(seven_day = PlanWindow(95.0))
        assertEquals(listOf("7d 剩 5%"), planLineSegments(only7d).map { it.text })
        assertEquals(PctLevel.CRIT, planLineSegments(only7d).maxOf { it.level })
        assertTrue(planLineSegments(null).isEmpty())
        assertTrue(planLineSegments(PlanUsage()).isEmpty())
        // 只有 5h：那一段不画，整行就是空的
        assertTrue(planLineSegments(PlanUsage(five_hour = PlanWindow(32.0))).isEmpty())
    }

    @Test fun parseResetsAtEpochSeconds() {
        assertEquals(Instant.parse("2026-09-03T06:30:00Z"), parseResetsAt(JsonPrimitive(1788417000)))
        assertEquals(Instant.parse("2026-09-03T06:30:00Z"), parseResetsAt(JsonPrimitive(1788417000.0)))
    }

    @Test fun parseResetsAtEpochMillisAndNumericString() {
        assertEquals(Instant.parse("2026-09-03T06:30:00Z"), parseResetsAt(JsonPrimitive(1788417000000L)))
        assertEquals(Instant.parse("2026-09-03T06:30:00Z"), parseResetsAt(JsonPrimitive("1788417000")))
    }

    @Test fun parseResetsAtIso() {
        assertEquals(Instant.parse("2026-09-03T06:30:00Z"), parseResetsAt(JsonPrimitive("2026-09-03T06:30:00Z")))
        assertEquals(Instant.parse("2026-09-03T06:30:00Z"), parseResetsAt(JsonPrimitive("2026-09-03T14:30:00+08:00")))
        assertEquals(Instant.parse("2026-09-03T06:30:00.500Z"), parseResetsAt(JsonPrimitive("2026-09-03T06:30:00.500Z")))
    }

    @Test fun parseResetsAtGarbage() {
        assertNull(parseResetsAt(null))
        assertNull(parseResetsAt(JsonNull))
        assertNull(parseResetsAt(JsonPrimitive("")))
        assertNull(parseResetsAt(JsonPrimitive("soon")))
    }

    @Test fun resetLabelToday() {
        assertEquals("重置 14:30", resetLabel(Instant.parse("2026-09-03T06:30:00Z"), now, sh))
    }

    @Test fun resetLabelOtherDay() {
        // 2026-09-05 是周六
        assertEquals("重置 周六 08:05", resetLabel(Instant.parse("2026-09-05T00:05:00Z"), now, sh))
        // 上海的「今天」在 UTC 的前一天晚上：按时区算（已经过去了 → 月/日）
        assertEquals("重置 9/2 23:59", resetLabel(Instant.parse("2026-09-02T15:59:00Z"), now, sh))
        // 七天开外不说周几：十天后的「周六」是哪个周六说不清楚（与 mac 同口径）
        assertEquals("重置 9/13 08:05", resetLabel(Instant.parse("2026-09-13T00:05:00Z"), now, sh))
        assertNull(resetLabel(null, now, sh))
    }

    @Test fun artifactTime() {
        assertEquals("09:05", artifactTimeLabel("2026-09-03T01:05:00Z", now, sh))
        assertEquals("8/31", artifactTimeLabel("2026-08-31T01:05:00Z", now, sh))
        assertEquals("12/25", artifactTimeLabel("2025-12-25T10:00:00+08:00", now, sh))
        assertEquals("", artifactTimeLabel("", now, sh))
        assertEquals("", artifactTimeLabel("yesterday", now, sh))
    }

    @Test fun artifactsNewestFirst() {
        val list = listOf(
            ArtifactInfo("u1", ts = "2026-09-01T00:00:00Z"),
            ArtifactInfo("u3", ts = "2026-09-03T00:00:00Z"),
            ArtifactInfo("u2", ts = "2026-09-02T00:00:00Z"),
        )
        assertEquals(listOf("u3", "u2", "u1"), sortArtifacts(list).map { it.url })
    }

    @Test fun protocolDecodesUsageAndArtifacts() {
        val j = ProtocolJson.instance
        val s = j.decodeFromString(Session.serializer(), """{"id":"a","usage":{"model":"Fable 5.1","context_pct":30.5,"cost_usd":1.25,"effort":"high","unknown":1}}""")
        assertEquals("Fable 5.1", s.usage?.model)
        assertEquals(30.5, s.usage?.context_pct!!, 1e-9)
        assertNull(j.decodeFromString(Session.serializer(), """{"id":"b","usage":null}""").usage)

        val r = j.decodeFromString(UsageResponse.serializer(), """{"plan":{"five_hour":{"used_percentage":32,"resets_at":1788417000},"seven_day":{"used_percentage":61.5,"resets_at":"2026-09-05T00:00:00Z"},"model_scoped":[{"display_name":"Fable","utilization":40,"resets_at":null}],"updated_at":"x"}}""")
        assertEquals(Instant.parse("2026-09-03T06:30:00Z"), parseResetsAt(r.plan?.five_hour?.resets_at))
        assertEquals(Instant.parse("2026-09-05T00:00:00Z"), parseResetsAt(r.plan?.seven_day?.resets_at))
        assertNull(parseResetsAt(r.plan?.model_scoped?.first()?.resets_at))
        // 5h 不进那一行；Fable 的 resets_at 是 null，重置只报 7d 那一条
        assertEquals(
            listOf("7d 剩 38%", "Fable 剩 60%", "重置 周六 00:00"),
            planLineSegments(r.plan, Instant.parse("2026-09-03T00:00:00Z"), ZoneId.of("UTC")).map { it.text },
        )
        assertNull(j.decodeFromString(UsageResponse.serializer(), """{"plan":null}""").plan)

        val a = j.decodeFromString(ArtifactsResponse.serializer(), """{"artifacts":[{"url":"https://x/1","title":"T","description":"D","file_path":"/p","ts":"2026-09-03T01:05:00Z"}]}""")
        assertEquals("T", a.artifacts.single().title)
    }

    @Test fun usageFrameParses() {
        val f = EventFrame.parse("""{"t":"usage","plan":{"five_hour":{"used_percentage":10}}}""")
        assertTrue(f is EventFrame.UsageUpdate)
        assertEquals(10.0, (f as EventFrame.UsageUpdate).plan?.five_hour?.used_percentage!!, 1e-9)
        val n = EventFrame.parse("""{"t":"usage","plan":null}""")
        assertTrue(n is EventFrame.UsageUpdate && n.plan == null)
    }
}
