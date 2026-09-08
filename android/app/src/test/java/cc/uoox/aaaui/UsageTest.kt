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

    @Test fun planLineFull() {
        val plan = PlanUsage(
            five_hour = PlanWindow(32.0), seven_day = PlanWindow(61.0),
            model_scoped = listOf(ModelScopedUsage("Fable", 40.0), ModelScopedUsage("Opus", 72.0)),
        )
        assertEquals(
            listOf("5h 32%", "7d 61%", "Fable 40%", "Opus 72%"),
            planLineSegments(plan).map { it.text },
        )
        assertEquals(PctLevel.WARN, planLineSegments(plan).maxOf { it.level })
    }

    @Test fun planLinePartial() {
        assertEquals(listOf("7d 95%"), planLineSegments(PlanUsage(seven_day = PlanWindow(95.0))).map { it.text })
        assertEquals(PctLevel.CRIT, planLineSegments(PlanUsage(seven_day = PlanWindow(95.0))).maxOf { it.level })
        assertTrue(planLineSegments(null).isEmpty())
        assertTrue(planLineSegments(PlanUsage()).isEmpty())
        assertTrue(planLineSegments(PlanUsage(five_hour = PlanWindow(null))).isEmpty())
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
        // 上海的「今天」在 UTC 的前一天晚上：按时区算
        assertEquals("重置 周三 23:59", resetLabel(Instant.parse("2026-09-02T15:59:00Z"), now, sh))
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
        assertEquals(listOf("5h 32%", "7d 62%", "Fable 40%"), planLineSegments(r.plan).map { it.text })
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
