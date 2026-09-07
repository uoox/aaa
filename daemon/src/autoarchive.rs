//! 自动归档（2026-09-07 用户拍板）：暂停超过 N 天、清单全勾完（或没清单）的项目自动进
//! 归档——列表里一百多个暂停的项目大部分再也不会碰，不该一直占着屏。每小时扫一次，
//! `auto_archive_days = 0` 关掉。只看经 AAA 跑过的项目（会话日志里有记录），没跑过的
//! 目录不动。

use std::collections::{HashMap, HashSet};

use chrono::{DateTime, Utc};

use crate::history::{Entry, parse_checklist};

/// 该归档的项目路径：没有活会话、最近一条会话结束超过 `days` 天、且最后一版清单没有
/// 没勾的项。`live_paths` = 此刻有非 exited 会话的项目。
pub fn candidates(entries: &[Entry], live_paths: &HashSet<String>, now: DateTime<Utc>, days: u32) -> Vec<String> {
    if days == 0 {
        return Vec::new();
    }
    // 每个项目最近的一条（按 ended_at / created_at）
    let mut newest: HashMap<&str, &Entry> = HashMap::new();
    for e in entries.iter().filter(|e| e.agent != "shell" && e.deleted_at.is_none()) {
        let cur = newest.entry(e.project_path.as_str()).or_insert(e);
        if last_ts(e) > last_ts(cur) {
            *cur = e;
        }
    }
    let cutoff = now - chrono::Duration::days(days as i64);
    let mut out: Vec<String> = newest
        .into_iter()
        .filter(|(path, _)| !live_paths.contains(*path))
        .filter(|(_, e)| e.ended_at.is_some())
        .filter(|(_, e)| DateTime::parse_from_rfc3339(last_ts(e)).map(|t| t.with_timezone(&Utc) < cutoff).unwrap_or(false))
        .filter(|(_, e)| parse_checklist(&e.summary).iter().all(|i| i.done))
        .map(|(p, _)| p.to_string())
        .collect();
    out.sort();
    out
}

fn last_ts(e: &Entry) -> &str {
    e.ended_at.as_deref().unwrap_or(&e.created_at)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn e(id: &str, path: &str, ended: Option<&str>, summary: &str) -> Entry {
        Entry {
            id: id.into(),
            project_path: path.into(),
            project_name: "p".into(),
            agent: "claude".into(),
            title: "t".into(),
            created_at: "2026-08-01T00:00:00Z".into(),
            ended_at: ended.map(String::from),
            exit_code: Some(0),
            deleted_at: None,
            summary: summary.into(),
            last_state: "exited".into(),
        }
    }

    #[test]
    fn old_finished_projects_qualify_but_live_open_or_recent_ones_do_not() {
        let now = DateTime::parse_from_rfc3339("2026-09-07T00:00:00Z").unwrap().with_timezone(&Utc);
        let entries = vec![
            e("a1", "/p/old-done", Some("2026-08-01T00:00:00Z"), "- [x] 完成"),
            e("b1", "/p/old-open", Some("2026-08-01T00:00:00Z"), "- [ ] 还没做"),
            e("c1", "/p/recent", Some("2026-09-06T00:00:00Z"), "- [x] 完成"),
            e("d1", "/p/live", Some("2026-08-01T00:00:00Z"), ""),
            e("e1", "/p/nolist", Some("2026-08-01T00:00:00Z"), ""),
            // 同项目更新的一条还没做完：以最新的为准
            e("f1", "/p/mixed", Some("2026-08-01T00:00:00Z"), "- [x] 早"),
            e("f2", "/p/mixed", Some("2026-08-20T00:00:00Z"), "- [ ] 晚"),
        ];
        let live: HashSet<String> = ["/p/live".to_string()].into_iter().collect();
        assert_eq!(candidates(&entries, &live, now, 14), vec!["/p/nolist", "/p/old-done"]);
        assert!(candidates(&entries, &live, now, 0).is_empty(), "0 = 关掉");
        assert!(candidates(&entries, &live, now, 60).is_empty(), "都没到 60 天");
    }
}
