//! plan 配额的第二个来源：claude.ai 的 OAuth usage 接口。
//!
//! Claude Code 的 statusLine 只转来 `rate_limits.five_hour` / `seven_day` 两个窗口，
//! 按模型的周窗口（Fable 剩多少）它从来不给——那只在
//! `GET https://api.anthropic.com/api/oauth/usage` 的 `limits[]` 里（`kind=weekly_scoped`，
//! `scope.model.display_name` 是模型名），Claude Code 自己的 `/usage` 面板也是从这拿的。
//!
//! 令牌直接用 Claude Code 登录留下的那份：macOS 钥匙串条目 `Claude Code-credentials`，
//! 其它平台 `~/.claude/.credentials.json`（`CLAUDE_CONFIG_DIR` 优先）。只读不刷新——
//! refresh token 是轮换的，daemon 抢着刷会把用户的 Claude Code 登出；令牌过期就等
//! 下一次 Claude Code 跑起来自己续，期间沿用上一次的数。

use std::path::{Path, PathBuf};
use std::time::Duration;

use serde_json::{json, Value};

pub const USAGE_URL: &str = "https://api.anthropic.com/api/oauth/usage";
/// 轮询间隔；Claude Code 自己的面板也是分钟级刷新，更密没意义
pub const POLL_INTERVAL: Duration = Duration::from_secs(60);
/// 没有活的 claude 会话时每几个 tick 才拉一次（60s × 5 = 5 分钟）
pub const IDLE_POLL_EVERY: u64 = 5;
/// 被 429 限流后跳过的 tick 数（60s × 5 = 5 分钟）
pub const RATE_LIMIT_BACKOFF_TICKS: u64 = 5;
const KEYCHAIN_SERVICE: &str = "Claude Code-credentials";

/// 凭据文件路径：`$CLAUDE_CONFIG_DIR/.credentials.json`，没设就 `~/.claude/.credentials.json`
pub fn credentials_path(home: &Path) -> PathBuf {
    std::env::var("CLAUDE_CONFIG_DIR")
        .ok()
        .filter(|s| !s.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| home.join(".claude"))
        .join(".credentials.json")
}

/// 凭据 JSON（`{"claudeAiOauth":{accessToken,expiresAt,…}}`）里取还没过期的 access token
pub fn token_from_credentials(v: &Value, now_ms: i64) -> Option<String> {
    let o = v.get("claudeAiOauth")?;
    if let Some(exp) = o.get("expiresAt").and_then(Value::as_i64) {
        if exp <= now_ms {
            return None;
        }
    }
    o.get("accessToken").and_then(Value::as_str).filter(|s| !s.is_empty()).map(str::to_string)
}

fn read_credentials(home: &Path) -> Option<Value> {
    let p = credentials_path(home);
    if let Ok(s) = std::fs::read_to_string(&p) {
        if let Ok(v) = serde_json::from_str::<Value>(&s) {
            return Some(v);
        }
    }
    if cfg!(target_os = "macos") {
        let out = std::process::Command::new("security")
            .args(["find-generic-password", "-s", KEYCHAIN_SERVICE, "-w"])
            .output()
            .ok()?;
        if out.status.success() {
            return serde_json::from_slice(&out.stdout).ok();
        }
    }
    None
}

/// 当前可用的 access token；没登录 / 已过期 → None
pub fn access_token(home: &Path) -> Option<String> {
    let now_ms = chrono::Utc::now().timestamp_millis();
    read_credentials(home).and_then(|v| token_from_credentials(&v, now_ms))
}

/// usage 接口的响应 → `GET /usage` 的 plan 形状（不含 `updated_at`）。
/// 5h / 7d 取顶层窗口（`utilization` 归一成 `used_percentage`），按模型窗口取
/// `limits[]` 里的 `weekly_scoped`。三个都没有 → None。
pub fn parse_usage(body: &Value) -> Option<Value> {
    let window = |k: &str| -> Value {
        match body.get(k) {
            Some(w) if w.is_object() => json!({
                "used_percentage": w.get("utilization").cloned().unwrap_or(Value::Null),
                "resets_at": w.get("resets_at").cloned().unwrap_or(Value::Null),
            }),
            _ => Value::Null,
        }
    };
    let five_hour = window("five_hour");
    let seven_day = window("seven_day");
    let scoped: Vec<Value> = body
        .get("limits")
        .and_then(Value::as_array)
        .map(|ls| {
            ls.iter()
                .filter(|l| l.get("kind").and_then(Value::as_str) == Some("weekly_scoped"))
                .filter_map(|l| {
                    let name = l.pointer("/scope/model/display_name")?.as_str()?;
                    Some(json!({
                        "display_name": name,
                        "utilization": l.get("percent").cloned().unwrap_or(Value::Null),
                        "resets_at": l.get("resets_at").cloned().unwrap_or(Value::Null),
                    }))
                })
                .collect()
        })
        .unwrap_or_default();
    if five_hour.is_null() && seven_day.is_null() && scoped.is_empty() {
        return None;
    }
    Some(json!({
        "five_hour": five_hour,
        "seven_day": seven_day,
        "model_scoped": if scoped.is_empty() { Value::Null } else { Value::Array(scoped) },
    }))
}

/// 拉一次 usage 接口。阻塞；在 spawn_blocking 里调
pub fn fetch(token: &str) -> Result<Value, String> {
    let resp = ureq::get(USAGE_URL)
        .set("Authorization", &format!("Bearer {token}"))
        .set("anthropic-beta", "oauth-2025-04-20")
        .set("Accept", "application/json")
        .timeout(Duration::from_secs(10))
        .call()
        .map_err(|e| match e {
            ureq::Error::Status(code, _) => format!("HTTP {code}"),
            other => other.to_string(),
        })?;
    resp.into_json::<Value>().map_err(|e| e.to_string())
}

/// 三个窗口是否相同（忽略 `updated_at`，免得每次都广播）
pub fn same_windows(a: &Value, b: &Value) -> bool {
    a["five_hour"] == b["five_hour"] && a["seven_day"] == b["seven_day"] && a["model_scoped"] == b["model_scoped"]
}

// ── Antigravity（agy）那一份 ────────────────────────────────────────────────
//
// 与上面 claude 那一份是同一件事的第二份实现：读 agy 自己登录留下的令牌，问它自己的
// 配额接口，再压成**同一个 plan 形状**交出去——客户端因此只有一套画法（`plans` 这张
// 表按 agent 取，见 PROTOCOL「GET /usage」）。
//
// 令牌：agy 把它写进 macOS 钥匙串（service `gemini` / account `antigravity`，值是
// go-keyring 的 `go-keyring-base64:` + base64 的 JSON），钥匙串取不到时退回
// `~/.gemini/jetski-standalone-oauth-token`。**只读不刷新**，与 claude 那份同一条规矩：
// refresh token 是轮换的，daemon 抢着刷会把用户的 agy 登出；过期就等 agy 自己续。

pub const AGY_USAGE_URL: &str =
    "https://daily-cloudcode-pa.googleapis.com/v1internal:retrieveUserQuotaSummary";
const AGY_KEYCHAIN_SERVICE: &str = "gemini";
const AGY_KEYCHAIN_ACCOUNT: &str = "antigravity";
const GO_KEYRING_PREFIX: &str = "go-keyring-base64:";

/// **这个 User-Agent 是必须的**，不是礼貌：不带它，后端按 Gemini Code Assist 的企业
/// 授权判这个请求，个人账号一律 403 `SUBSCRIPTION_REQUIRED`；带上它才走 Antigravity
/// 自己那条线。改这个字符串前先确认接口还认。
pub const AGY_USER_AGENT: &str = "antigravity-cli/1.0";

/// agy 令牌文件：`~/.gemini/jetski-standalone-oauth-token`
pub fn agy_token_path(home: &Path) -> PathBuf {
    home.join(".gemini").join("jetski-standalone-oauth-token")
}

/// go-keyring 存的值：`go-keyring-base64:<base64(JSON)>`，也可能就是裸 JSON
pub fn decode_keyring_value(raw: &str) -> Option<Value> {
    let t = raw.trim();
    let json = match t.strip_prefix(GO_KEYRING_PREFIX) {
        Some(b64) => {
            use base64::Engine;
            String::from_utf8(base64::engine::general_purpose::STANDARD.decode(b64.trim()).ok()?).ok()?
        }
        None => t.to_string(),
    };
    serde_json::from_str(&json).ok()
}

/// agy 凭据 JSON（`{"token":{access_token,expiry,…}}`）里取还没过期的 access token。
/// `expiry` 是 RFC3339；解析不动就当它还有效（宁可问一次拿 401，也不要平白不问）。
pub fn agy_token_from_json(v: &Value, now: chrono::DateTime<chrono::Utc>) -> Option<String> {
    let t = v.get("token")?;
    if let Some(exp) = t.get("expiry").and_then(Value::as_str) {
        if let Ok(at) = chrono::DateTime::parse_from_rfc3339(exp.trim()) {
            if at.with_timezone(&chrono::Utc) <= now {
                return None;
            }
        }
    }
    t.get("access_token").and_then(Value::as_str).filter(|s| !s.is_empty()).map(str::to_string)
}

fn read_agy_credentials(home: &Path) -> Option<Value> {
    if cfg!(target_os = "macos") {
        let out = std::process::Command::new("security")
            .args(["find-generic-password", "-s", AGY_KEYCHAIN_SERVICE, "-a", AGY_KEYCHAIN_ACCOUNT, "-w"])
            .output()
            .ok();
        if let Some(out) = out.filter(|o| o.status.success()) {
            if let Some(v) = String::from_utf8(out.stdout).ok().and_then(|s| decode_keyring_value(&s)) {
                return Some(v);
            }
        }
    }
    // 钥匙串没有（Linux、或者 agy 落回了文件）：读文件那一份
    let s = std::fs::read_to_string(agy_token_path(home)).ok()?;
    serde_json::from_str(&s).ok()
}

/// 当前可用的 agy access token；没登录 / 已过期 → None
pub fn agy_access_token(home: &Path) -> Option<String> {
    read_agy_credentials(home).and_then(|v| agy_token_from_json(&v, chrono::Utc::now()))
}

/// agy 的配额响应 → 与 claude 同一个 plan 形状。
///
/// 接口给的是**按模型分组**的桶：每组一个 `weekly` 和一个 `5h`，`remainingFraction`
/// 是**还剩**的比例（0–1）——这里换成 plan 形状里一贯的「用掉的百分比」。
///
/// **取第一个真有周窗口的组**（实测是 Gemini 那组，也就是 agy 默认在用的那一档）当
/// `seven_day` / `five_hour`——不死认 `groups[0]`：顺序和「首组会不会整个 disabled」
/// 都是对面说了算的。其余各组（Claude / GPT 那些）**整个不要**——
/// 2026-09-11 用户拍板：「antigravity 右边就不放 Other 了吧，反正也很少用」。
/// 于是 Antigravity 那一栏行尾就是 `剩 51% · 13h 重置` 两段，没有 `model_scoped`。
///
/// 一个桶都没有 → None（没有的东西不编一个出来）。
pub fn parse_agy_usage(body: &Value) -> Option<Value> {
    let groups = body.get("groups")?.as_array()?;
    let bucket = |g: &Value, window: &str| -> Option<Value> {
        let b = g.get("buckets")?.as_array()?.iter().find(|b| {
            b.get("window").and_then(Value::as_str).is_some_and(|w| w.eq_ignore_ascii_case(window))
        })?;
        // disabled 的桶不算数：那是「这一档你没有」，不是「用满了」
        if b.get("disabled").and_then(Value::as_bool).unwrap_or(false) {
            return None;
        }
        let frac = b.get("remainingFraction").and_then(Value::as_f64)?;
        Some(json!({
            "used_percentage": ((1.0 - frac.clamp(0.0, 1.0)) * 100.0 * 10.0).round() / 10.0,
            "resets_at": b.get("resetTime").cloned().unwrap_or(Value::Null),
        }))
    };
    // **取第一个真有周窗口的组**，不是死认 `groups[0]`（2026-09-11 agy 审阅指出）：
    // 实测第一组是 Gemini 那档，但组的顺序、以及首组会不会整个 `disabled`，都是对面
    // 说了算的。死认第一个的话，对面哪天换个顺序或把首档关掉，这一栏就整个空了——
    // 而那时看不出是「没配额」还是「没解析出来」。
    let first = groups.iter().find_map(|g| bucket(g, "weekly").map(|w| (g, w)));
    let (g, seven) = first?;
    Some(json!({
        "five_hour": bucket(g, "5h").unwrap_or(Value::Null),
        "seven_day": seven,
        "model_scoped": Value::Null,
    }))
}

/// 拉一次 agy 的配额接口。阻塞；在 spawn_blocking 里调
pub fn agy_fetch(token: &str) -> Result<Value, String> {
    let resp = ureq::post(AGY_USAGE_URL)
        .set("Authorization", &format!("Bearer {token}"))
        .set("User-Agent", AGY_USER_AGENT)
        .set("Accept", "application/json")
        .timeout(Duration::from_secs(10))
        .send_json(json!({}))
        .map_err(|e| match e {
            ureq::Error::Status(code, _) => format!("HTTP {code}"),
            other => other.to_string(),
        })?;
    resp.into_json::<Value>().map_err(|e| e.to_string())
}

/// statusLine 转来的 plan 缺 `model_scoped`（它从来没有），把上一次轮询到的带上
pub fn carry_model_scoped(plan: &mut Value, cur: Option<&Value>) {
    if plan["model_scoped"].is_null() {
        if let Some(ms) = cur.map(|c| &c["model_scoped"]).filter(|v| !v.is_null()) {
            plan["model_scoped"] = ms.clone();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn token_respects_expiry() {
        let v = json!({"claudeAiOauth": {"accessToken": "sk-ant-oat01-x", "expiresAt": 2000}});
        assert_eq!(token_from_credentials(&v, 1000).as_deref(), Some("sk-ant-oat01-x"));
        assert!(token_from_credentials(&v, 2000).is_none(), "expired");
        assert!(token_from_credentials(&json!({}), 0).is_none());
        assert!(token_from_credentials(&json!({"claudeAiOauth": {"accessToken": ""}}), 0).is_none());
    }

    #[test]
    fn agy_token_respects_expiry_and_keyring_encoding() {
        let raw = r#"{"token":{"access_token":"ya29.x","expiry":"2026-09-11T02:44:25+08:00"},"auth_method":"consumer"}"#;
        let v: Value = serde_json::from_str(raw).unwrap();
        let before = chrono::DateTime::parse_from_rfc3339("2026-09-10T18:00:00Z").unwrap().with_timezone(&chrono::Utc);
        let after = chrono::DateTime::parse_from_rfc3339("2026-09-10T19:00:00Z").unwrap().with_timezone(&chrono::Utc);
        assert_eq!(agy_token_from_json(&v, before).as_deref(), Some("ya29.x"));
        assert!(agy_token_from_json(&v, after).is_none(), "过期了就当没有");
        // expiry 解析不动 → 照样给（宁可问一次拿 401）
        let odd: Value = serde_json::from_str(r#"{"token":{"access_token":"ya29.y","expiry":"whenever"}}"#).unwrap();
        assert_eq!(agy_token_from_json(&odd, after).as_deref(), Some("ya29.y"));
        assert!(agy_token_from_json(&json!({}), before).is_none());
        // 钥匙串里是 go-keyring 的 base64，也可能是裸 JSON
        use base64::Engine;
        let b64 = base64::engine::general_purpose::STANDARD.encode(raw);
        assert_eq!(decode_keyring_value(&format!("go-keyring-base64:{b64}")).unwrap(), v);
        assert_eq!(decode_keyring_value(raw).unwrap(), v);
        assert!(decode_keyring_value("not json").is_none());
    }

    #[test]
    fn agy_groups_become_the_same_plan_shape() {
        let body = json!({
            "groups": [
                {"displayName": "Gemini Models", "buckets": [
                    {"bucketId": "gemini-weekly", "window": "weekly", "remainingFraction": 0.5108519,
                     "resetTime": "2026-09-11T07:52:05Z"},
                    {"bucketId": "gemini-5h", "window": "5h", "remainingFraction": 0.676,
                     "resetTime": "2026-09-10T19:26:48Z"}
                ]},
                {"displayName": "Claude and GPT models", "buckets": [
                    {"bucketId": "3p-weekly", "window": "weekly", "remainingFraction": 0.90338373,
                     "resetTime": "2026-09-16T17:01:34Z"},
                    {"bucketId": "3p-5h", "window": "5h", "remainingFraction": 1}
                ]}
            ]
        });
        let p = parse_agy_usage(&body).unwrap();
        // remainingFraction 是**还剩**的，plan 形状里一贯存**用掉的**
        assert_eq!(p["seven_day"]["used_percentage"], 48.9);
        assert_eq!(p["seven_day"]["resets_at"], "2026-09-11T07:52:05Z");
        assert_eq!(p["five_hour"]["used_percentage"], 32.4);
        // 其余各组（Claude / GPT 那些）整个不要（2026-09-11 用户：「不放 Other 了」）
        assert!(p["model_scoped"].is_null(), "只有主的那一档");
        // disabled 的桶不算「用满了」，它整个不进来
        let off = json!({"groups": [{"displayName": "G", "buckets": [
            {"window": "weekly", "remainingFraction": 0.0, "disabled": true}
        ]}]});
        assert!(parse_agy_usage(&off).is_none());
        assert!(parse_agy_usage(&json!({})).is_none());
        assert!(parse_agy_usage(&json!({"groups": []})).is_none());
    }

    #[test]
    fn parses_scoped_windows_from_limits() {
        let body = json!({
            "five_hour": {"utilization": 5.0, "resets_at": "2026-09-05T01:00:00+00:00"},
            "seven_day": {"utilization": 1.0, "resets_at": "2026-09-06T10:00:00+00:00"},
            "seven_day_opus": null,
            "limits": [
                {"kind": "session", "percent": 5, "resets_at": "2026-09-05T01:00:00+00:00", "scope": null},
                {"kind": "weekly_all", "percent": 1, "scope": null},
                {"kind": "weekly_scoped", "percent": 2, "resets_at": "2026-09-06T10:00:00+00:00",
                 "scope": {"model": {"id": null, "display_name": "Fable"}, "surface": null}},
                {"kind": "weekly_scoped", "percent": 9, "scope": {"surface": "cowork"}}
            ]
        });
        let p = parse_usage(&body).unwrap();
        assert_eq!(p["five_hour"]["used_percentage"], 5.0);
        assert_eq!(p["seven_day"]["resets_at"], "2026-09-06T10:00:00+00:00");
        let ms = p["model_scoped"].as_array().unwrap();
        assert_eq!(ms.len(), 1, "only model-scoped rows, not surface-scoped");
        assert_eq!(ms[0]["display_name"], "Fable");
        assert_eq!(ms[0]["utilization"], 2);
        assert_eq!(ms[0]["resets_at"], "2026-09-06T10:00:00+00:00");
    }

    #[test]
    fn parse_without_windows_is_none() {
        assert!(parse_usage(&json!({"limits": []})).is_none());
        assert!(parse_usage(&json!({"error": "x"})).is_none());
        let only_top = parse_usage(&json!({"five_hour": {"utilization": 3}})).unwrap();
        assert_eq!(only_top["five_hour"]["used_percentage"], 3);
        assert!(only_top["model_scoped"].is_null());
    }

    #[test]
    fn statusline_keeps_polled_model_windows() {
        let cur = json!({"five_hour": {"used_percentage": 1}, "model_scoped": [{"display_name": "Fable", "utilization": 2}]});
        let mut plan = json!({"five_hour": {"used_percentage": 4}, "seven_day": null, "model_scoped": null});
        carry_model_scoped(&mut plan, Some(&cur));
        assert_eq!(plan["model_scoped"][0]["display_name"], "Fable");
        assert_eq!(plan["five_hour"]["used_percentage"], 4, "statusline windows win");
        let mut none = json!({"model_scoped": null});
        carry_model_scoped(&mut none, None);
        assert!(none["model_scoped"].is_null());
        assert!(same_windows(&plan, &json!({"five_hour": {"used_percentage": 4}, "seven_day": null,
            "model_scoped": [{"display_name": "Fable", "utilization": 2}], "updated_at": "later"})));
    }

    #[test]
    fn credentials_path_honours_config_dir() {
        let home = Path::new("/h");
        let p = credentials_path(home);
        // CLAUDE_CONFIG_DIR 可能由环境带进来；两种都合法，但文件名固定
        assert!(p.ends_with(".credentials.json"));
        if std::env::var("CLAUDE_CONFIG_DIR").ok().filter(|s| !s.is_empty()).is_none() {
            assert_eq!(p, PathBuf::from("/h/.claude/.credentials.json"));
        }
    }
}
