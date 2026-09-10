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
 * `Claude` 那一栏表头右边那一段：`7d 剩 22% · Fable 剩 9% · 重置 周六 18:00`
 * （2026-09-11 用户拍板：「订阅剩余额度直接放在列表上 Claude/Antigravity 这一行后面，
 * 不需要 5h，只需要 7d 和重置时间，claude 加个 Fable 剩余额度，重置时间应该是一样的
 * 所以不用显示」）。
 *
 * 三条：
 * - **说剩多少，不说用了多少**。接口给的是 `utilization`（用掉的百分比），这里换算成
 *   `100 - used` 并写个「剩」字——「还能用多少」才是你要据以决定接下来干什么的数。
 *   颜色仍按**用掉的**算（用得越多越红），换算的只是那个数字。
 * - **5h 不进来**：它每五小时翻一次，看它没有意义；周窗口才是真会把人卡住的那个。
 * - **重置时间只出现一次**：7d 与按模型的周窗口实测同一时刻。真不一样了（差一分钟以上）
 *   那一条才补自己的——宁可多一段，也不能拿 7d 的时刻替 Fable 说话。
 *
 * 两端同一份口径（mac 的 `plan_header_segs`），两边的单测各盯各的一份同样的例子。
 */
fun planLineSegments(
    plan: PlanUsage?,
    now: Instant = Instant.now(),
    zone: ZoneId = ZoneId.systemDefault(),
): List<UsageSegment> {
    if (plan == null) return emptyList()
    val out = mutableListOf<UsageSegment>()
    plan.seven_day?.used_percentage?.let { out += UsageSegment("7d 剩 " + remainText(it), pctColorLevel(it)) }
    plan.model_scoped.orEmpty().forEach { m ->
        m.utilization?.let {
            out += UsageSegment((m.display_name.ifBlank { "模型" }) + " 剩 " + remainText(it), pctColorLevel(it))
        }
    }
    if (out.isEmpty()) return out
    // 重置时刻：以 7d 的为准，没有 7d 就用第一个按模型窗口的
    val base = parseResetsAt(plan.seven_day?.resets_at)
        ?: plan.model_scoped.orEmpty().firstNotNullOfOrNull { parseResetsAt(it.resets_at) }
    resetLabel(base, now, zone)?.let { out += UsageSegment(it) }
    // 跟 7d 不是同一时刻的那些，各报各的（实测都一样，所以通常一条都不加）
    plan.model_scoped.orEmpty().forEach { m ->
        val t = parseResetsAt(m.resets_at) ?: return@forEach
        if (base != null && kotlin.math.abs(t.epochSecond - base.epochSecond) <= 60) return@forEach
        resetLabel(t, now, zone)?.let { out += UsageSegment((m.display_name.ifBlank { "模型" }) + " " + it) }
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

private val WEEKDAYS = arrayOf("周一", "周二", "周三", "周四", "周五", "周六", "周日")
private val HHMM = DateTimeFormatter.ofPattern("HH:mm")

/**
 * `重置 14:30`（今天）/ `重置 周四 14:30`（一周内）/ `重置 9/18 14:30`（更远或已过去）；
 * null 进 null 出。**七天开外不说周几**：十天后的「周四」是哪个周四说不清楚
 * （口径与 mac 的 `fmt_reset` 一字不差）。
 */
fun resetLabel(resetsAt: Instant?, now: Instant = Instant.now(), zone: ZoneId = ZoneId.systemDefault()): String? {
    if (resetsAt == null) return null
    val t = resetsAt.atZone(zone)
    val today: LocalDate = now.atZone(zone).toLocalDate()
    val hm = t.format(HHMM)
    if (t.toLocalDate() == today) return "重置 $hm"
    val days = java.time.temporal.ChronoUnit.DAYS.between(today, t.toLocalDate())
    if (days in 1..6) return "重置 ${WEEKDAYS[t.dayOfWeek.value - 1]} $hm"
    return "重置 ${t.monthValue}/${t.dayOfMonth} $hm"
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
