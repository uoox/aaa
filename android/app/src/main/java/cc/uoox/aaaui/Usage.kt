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
import java.util.Locale
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

fun List<UsageSegment>.joined(): String = joinToString(" · ") { it.text }

fun pctText(pct: Double): String = "${pct.roundToInt()}%"

/** 会话顶栏：`Fable 5.1 · 上下文 30% · $1.25`，缺哪段省哪段；全缺返回空列表 */
fun usageSubtitleSegments(usage: SessionUsage?): List<UsageSegment> {
    if (usage == null) return emptyList()
    return buildList {
        usage.model?.takeIf { it.isNotBlank() }?.let { add(UsageSegment(it)) }
        usage.context_pct?.let { add(UsageSegment("上下文 " + pctText(it), pctColorLevel(it))) }
        // 提示缓存命中率（最近一次调用：缓存读 ÷ 全部输入）；读为 0 = 缓存已失效。
        // 一位小数：99.6% 四舍五入成 100% 会让人以为「全命中了」；它跟要不要 compact 无关（看上下文那格）
        usage.cache_hit_pct?.let { add(UsageSegment("缓存 " + String.format(Locale.US, "%.1f%%", it))) }
        usage.cost_usd?.let { add(UsageSegment(String.format(Locale.US, "$%.2f", it))) }
    }
}

fun usageSubtitle(usage: SessionUsage?): String? = usageSubtitleSegments(usage).takeIf { it.isNotEmpty() }?.joined()

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

/** 首页套餐行：`5h 32% · 7d 61% · Fable 40%`；没有任何窗口返回空列表 */
fun planLineSegments(plan: PlanUsage?): List<UsageSegment> {
    if (plan == null) return emptyList()
    return buildList {
        plan.five_hour?.used_percentage?.let { add(UsageSegment("5h " + pctText(it), pctColorLevel(it))) }
        plan.seven_day?.used_percentage?.let { add(UsageSegment("7d " + pctText(it), pctColorLevel(it))) }
        plan.model_scoped.orEmpty().forEach { m ->
            m.utilization?.let { add(UsageSegment((m.display_name.ifBlank { "模型" }) + " " + pctText(it), pctColorLevel(it))) }
        }
    }
}

fun planLine(plan: PlanUsage?): String? = planLineSegments(plan).takeIf { it.isNotEmpty() }?.joined()

/** 整行的颜色跟最高的那个百分比走 */
fun planMaxLevel(plan: PlanUsage?): PctLevel = planLineSegments(plan).maxOfOrNull { it.level } ?: PctLevel.NORMAL

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

/** `重置 14:30`（今天）/ `重置 周四 14:30`（其它日子）；null 进 null 出 */
fun resetLabel(resetsAt: Instant?, now: Instant = Instant.now(), zone: ZoneId = ZoneId.systemDefault()): String? {
    if (resetsAt == null) return null
    val t = resetsAt.atZone(zone)
    val today = now.atZone(zone).toLocalDate()
    val hm = t.format(HHMM)
    return if (t.toLocalDate() == today) "重置 $hm" else "重置 ${WEEKDAYS[t.dayOfWeek.value - 1]} $hm"
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

/** 产物列表新的在前 */
fun sortArtifacts(list: List<ArtifactInfo>): List<ArtifactInfo> = list.sortedByDescending { it.ts }
