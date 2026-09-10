package cc.uoox.aaaui

import kotlinx.serialization.json.JsonElement
import kotlinx.serialization.json.JsonNull
import kotlinx.serialization.json.JsonPrimitive
import kotlinx.serialization.json.contentOrNull
import kotlinx.serialization.json.doubleOrNull
import java.time.Instant
import java.time.LocalDate
import java.time.OffsetDateTime
import java.time.ZoneId
import java.time.format.DateTimeFormatter
import kotlin.math.roundToInt

// ============================================================
// 用量展示的纯函数：会话顶栏小字、首页套餐行、重置时间、产物时间
// ============================================================

/** 百分比的告警级别：≥70 琥珀、≥90 红，其它正常 */
enum class PctLevel { NORMAL, WARN, CRIT }

fun pctColorLevel(pct: Double?): PctLevel = when {
    pct == null -> PctLevel.NORMAL
    pct >= 90 -> PctLevel.CRIT
    pct >= 70 -> PctLevel.WARN
    else -> PctLevel.NORMAL
}

/** 一段文字及其告警级别（只有百分比段才会不是 NORMAL），UI 拿去拼 AnnotatedString */
data class UsageSegment(val text: String, val level: PctLevel = PctLevel.NORMAL)

fun pctText(pct: Double): String = "${pct.roundToInt()}%"

/**
 * 用掉 `used`% 之后还剩多少。**先把用掉的取整再相减**，不是 `round(100 - used)`：
 * 两边各自取整时 61.5 会同时显示成「用了 62%」和「剩 39%」，加起来 101。
 */
fun remainText(used: Double): String = "${100 - used.roundToInt()}%"

/**
 * 会话页顶栏那一行的用量：只要**模型**和**上下文占比**（2026-09-08 用户拍板：顶栏从三行
 * 并成一行，标题之外只留这两样）。缓存命中率和花费搬进详情屏——那是要坐下来看的数字，
 * 不该在手机顶栏跟标题抢宽度。
 */
fun usageHeaderSegments(usage: SessionUsage?): List<UsageSegment> {
    if (usage == null) return emptyList()
    return buildList {
        usage.model?.takeIf { it.isNotBlank() }?.let { add(UsageSegment(it)) }
        usage.context_pct?.let { add(UsageSegment(pctText(it), pctColorLevel(it))) }
    }
}

/**
 * 一栏表头行尾那一段：`剩 22% · Fable 9% · 2d16h 重置`
 * （2026-09-11 用户拍板的写法：`Claude: 剩22% Fable 9% xdxh 重置`，
 * `Antigravity: 剩22% Other 70% xdxh 重置`）。
 *
 * 四条：
 * - **说剩多少，不说用了多少**。接口给的是 `utilization`（用掉的百分比），这里换算成
 *   `100 - used`——「还能用多少」才是你要据以决定接下来干什么的数。颜色仍按**用掉的**
 *   算（用得越多越红），换算的只是那个数字。
 * - **5h 不进来**：它每五小时翻一次，看它没有意义；周窗口才是真会把人卡住的那个。
 * - **重置写成还剩多久**（`2d16h`），不写几点：那个差才是你要的数。
 * - **重置时刻一样就只说一次**，摆在行尾；真不一样了（差一分钟以上，Antigravity 的
 *   两组就是这样）**每一段各自带上自己的**——宁可长一点，也不能拿一组的时刻替另一组说话。
 *
 * v1.39 起这个函数**两栏共用**：daemon 把 agy 的配额压成了同一个 plan 形状
 * （`quota.rs::parse_agy_usage`），所以两栏只有这一套画法。
 *
 * 两端同一份口径（mac 的 `plan_header_segs`），两边的单测各盯各的一份同样的例子。
 */
fun planLineSegments(
    plan: PlanUsage?,
    now: Instant = Instant.now(),
): List<UsageSegment> {
    if (plan == null) return emptyList()
    // (要显示的字, 用掉的百分比, 这一段自己的重置时刻)
    data class Row(val text: String, val used: Double, val at: Instant?)
    val rows = mutableListOf<Row>()
    plan.seven_day?.let { w ->
        w.used_percentage?.let { rows += Row("剩 " + remainText(it), it, parseResetsAt(w.resets_at)) }
    }
    plan.model_scoped.orEmpty().forEach { m ->
        m.utilization?.let {
            rows += Row((m.display_name.ifBlank { "模型" }) + " " + remainText(it), it, parseResetsAt(m.resets_at))
        }
    }
    if (rows.isEmpty()) return emptyList()
    // 各段的重置是不是同一时刻（没有时刻的那些不参与判断：它们本来就没什么可说的）
    val times = rows.mapNotNull { it.at }
    val base = times.firstOrNull()
    val same = base == null || times.all { kotlin.math.abs(it.epochSecond - base.epochSecond) <= 60 }
    val out = mutableListOf<UsageSegment>()
    if (same) {
        rows.forEach { out += UsageSegment(it.text, pctColorLevel(it.used)) }
        base?.let { out += UsageSegment(resetCountdown(it, now) + " 重置") }
    } else {
        // 不一样：谁的重置跟着谁走，紧挨在它后面
        rows.forEach { r ->
            out += UsageSegment(r.text, pctColorLevel(r.used))
            r.at?.let { out += UsageSegment(resetCountdown(it, now) + " 重置") }
        }
    }
    return out
}

/** resets_at 既可能是 unix 秒（或毫秒）数字，也可能是 ISO-8601 字串；解不出返回 null */
fun parseResetsAt(el: JsonElement?): Instant? {
    if (el == null || el is JsonNull || el !is JsonPrimitive) return null
    if (!el.isString) {
        val n = el.doubleOrNull ?: return null
        // 2001-09-09 之后的秒数都 < 1e10；更大的按毫秒算
        return if (n > 1e11) Instant.ofEpochMilli(n.toLong()) else Instant.ofEpochMilli((n * 1000).toLong())
    }
    val s = el.contentOrNull?.trim().orEmpty()
    if (s.isEmpty()) return null
    s.toDoubleOrNull()?.let { return parseResetsAt(JsonPrimitive(it)) }
    return runCatching { Instant.parse(s) }.getOrNull()
        ?: runCatching { OffsetDateTime.parse(s).toInstant() }.getOrNull()
}

private val HHMM = DateTimeFormatter.ofPattern("HH:mm")

/**
 * 到重置还有多久，写成 `2d16h` / `13h` / `40m`（2026-09-11 用户拍板：
 * 「xdxh 指的是重置日还剩下 x 日 x 小时」）。**说还剩多久，不说几点重置**：
 * 「周日 17:59」要你自己去减，而你想知道的本来就是那个差。
 * 已经过了（轮询还没跟上）说 `0m`，不给负数。
 * 口径与 mac 的 `fmt_reset_in` 一字不差。
 */
fun resetCountdown(resetsAt: Instant, now: Instant = Instant.now()): String {
    val mins = maxOf(0L, java.time.Duration.between(now, resetsAt).toMinutes())
    val d = mins / 1440
    val h = (mins % 1440) / 60
    val m = mins % 60
    return when {
        d > 0 && h > 0 -> "${d}d${h}h"
        d > 0 -> "${d}d"
        h > 0 -> "${h}h"
        else -> "${m}m"
    }
}

/** 产物时间：今天只给 HH:mm，其它日子给 M/d；解析失败给空串 */
fun artifactTimeLabel(ts: String, now: Instant = Instant.now(), zone: ZoneId = ZoneId.systemDefault()): String {
    val inst = runCatching { Instant.parse(ts) }.getOrNull()
        ?: runCatching { OffsetDateTime.parse(ts).toInstant() }.getOrNull()
        ?: return ""
    val t = inst.atZone(zone)
    val today: LocalDate = now.atZone(zone).toLocalDate()
    return if (t.toLocalDate() == today) t.format(HHMM) else "${t.monthValue}/${t.dayOfMonth}"
}

/**
 * Markdown 的改动时间：`mtime` 是 epoch 秒（daemon 的 `docs` 就给这个），
 * 走与 [artifactTimeLabel] 同一套「今天给时分，别的日子给月日」。
 */
fun epochTimeLabel(mtime: Double, now: Instant = Instant.now(), zone: ZoneId = ZoneId.systemDefault()): String {
    if (mtime <= 0) return ""
    val t = Instant.ofEpochSecond(mtime.toLong()).atZone(zone)
    val today: LocalDate = now.atZone(zone).toLocalDate()
    return if (t.toLocalDate() == today) t.format(HHMM) else "${t.monthValue}/${t.dayOfMonth}"
}

/** 产物列表新的在前 */
fun sortArtifacts(list: List<ArtifactInfo>): List<ArtifactInfo> = list.sortedByDescending { it.ts }
