//! Shared handling for a session entering `waiting` (from the heuristic tick
//! or from a Claude hook): inbox auto-feed first; otherwise a deduplicated
//! high-priority ntfy push.

use std::sync::Arc;
use std::time::Instant;

use crate::api::SharedApp;
use crate::pool::{Session, State};
use crate::statemachine::Question;

/// v1.1 waiting-push dedup (pure): push only when the question text differs
/// from the last pushed one AND the per-session cooldown (5 min) has passed.
pub fn should_notify_waiting(
    last_question: Option<&str>,
    last_at: Option<Instant>,
    now: Instant,
    question_key: &str,
) -> bool {
    if last_question == Some(question_key) {
        return false; // same question: only ever push once
    }
    if let Some(t) = last_at {
        if now.duration_since(t).as_secs() < 300 {
            return false; // per-session cooldown
        }
    }
    true
}

pub async fn on_waiting(app: &SharedApp, sess: Arc<Session>, question: Option<Question>) {
    // 1) inbox auto-feed: once per session, when the box is non-empty.
    // 只对接受自由文本的 waiting 喂：输入框（question=None）或无选项的
    // 开放问题。带选项的对话框（trust dialog、权限确认——每个新项目的
    // 第一屏就是它）会丢弃自由文本，而 take_all 已把条目删掉——清单既
    // 没送达也不可恢复，附带的 \r 还会替用户确认当前高亮项。留到下一次
    // 输入框 waiting 再喂。
    let accepts_text = question.as_ref().is_none_or(|q| q.options.is_empty());
    let (project_path, can_feed, alive) = {
        let meta = sess.meta.lock().unwrap();
        let alive = sess.live.lock().unwrap().is_some();
        (
            meta.project_path.clone(),
            accepts_text && meta.feed_inbox && !meta.inbox_fed && meta.state == State::Waiting,
            alive,
        )
    };
    if can_feed && alive {
        let entries = {
            let mut inbox = app.inbox.lock().unwrap();
            if inbox.list(&project_path).is_empty() {
                Vec::new()
            } else {
                inbox.take_all(&project_path)
            }
        };
        if !entries.is_empty() {
            let mut text = crate::inbox::compose_feed(&entries);
            text.push('\r');
            if sess.write_input(text.as_bytes()).is_ok() {
                {
                    let mut meta = sess.meta.lock().unwrap();
                    meta.inbox_fed = true;
                }
                app.hub.inbox_changed(&project_path);
                sess.mark_dirty();
                return; // the agent got fresh input; no waiting push
            }
        }
    }

    // 2) deduplicated ntfy push (priority high)
    let Some(ntfy) = app.cfg.ntfy.clone() else { return };
    let question_key = question
        .as_ref()
        .map(|q| q.text.clone())
        .unwrap_or_default();
    let (do_push, title, body) = {
        let mut meta = sess.meta.lock().unwrap();
        let now = Instant::now();
        if !should_notify_waiting(
            meta.last_notified_question.as_deref(),
            meta.last_notify_at,
            now,
            &question_key,
        ) {
            (false, String::new(), String::new())
        } else {
            meta.last_notified_question = Some(question_key.clone());
            meta.last_notify_at = Some(now);
            let body = if question_key.is_empty() {
                meta.preview.clone()
            } else {
                question_key.clone()
            };
            (true, format!("等待输入: {}", meta.title), body)
        }
    };
    if do_push {
        tokio::task::spawn_blocking(move || {
            crate::ntfy::push_blocking(&ntfy, &title, &body, crate::ntfy::PRIO_HIGH);
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn waiting_push_dedup() {
        let t0 = Instant::now();
        // first push always allowed
        assert!(should_notify_waiting(None, None, t0, "要不要 iPad 布局?"));
        // same question never repeats, regardless of time
        assert!(!should_notify_waiting(
            Some("要不要 iPad 布局?"),
            Some(t0),
            t0 + Duration::from_secs(10_000),
            "要不要 iPad 布局?"
        ));
        // different question but within the 5-minute cooldown: suppressed
        assert!(!should_notify_waiting(
            Some("旧问题"),
            Some(t0),
            t0 + Duration::from_secs(299),
            "新问题"
        ));
        // different question after cooldown: allowed
        assert!(should_notify_waiting(
            Some("旧问题"),
            Some(t0),
            t0 + Duration::from_secs(300),
            "新问题"
        ));
        // questionless waiting (input box) keys as "": pushed once, then never
        assert!(should_notify_waiting(None, None, t0, ""));
        assert!(!should_notify_waiting(
            Some(""),
            Some(t0),
            t0 + Duration::from_secs(10_000),
            ""
        ));
    }
}
