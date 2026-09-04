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
