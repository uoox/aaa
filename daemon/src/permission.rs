//! 替用户答**权限对话框**（Bash 授权、ExitPlanMode 批准…）。
//!
//! 客户端说答什么（`POST /sessions/:id/permission`），这里知道 Claude Code 的
//! 对话框怎么收：它是 Ink 的一个单选列表，第 1 项是 Yes、Esc 是 No。就两个键，
//! 不需要读屏确认——`meta.permission` 被 PostToolUse / Stop 清掉就是答成了。
//!
//! **AskUserQuestion 不在这里**（2026-09-11 用户拍板）。它曾经也走这个模块：
//! 客户端把选择发过来，daemon 翻译成对话框按键、再读屏确认对话框关掉了。那套东西
//! 是在盲操一个会变的 TUI——按键序列实测于 Claude Code 2.1.258，多选自填要按
//! 方向键下移选项数次、那时回车会把勾**取消**、Tab 在文本态和非文本态语义不同、
//! 按键之间还要留 70–160ms 节拍，最后靠屏幕上一行提示判断成没成。它一升级就可能
//! 答歪，而答歪（选错项还提交了）比答不了更糟。现在结构化提问一律去终端里答，
//! 那个对话框是 Claude Code 自己画的，你按什么就是什么。见 REMOVED.md。

use std::sync::Arc;
use std::time::Duration;

use crate::pool::Session;

/// One PTY write followed by a pause (ms) before the next.
pub type Step = (Vec<u8>, u64);

const BEAT: u64 = 70;

fn key(s: &str, pause: u64) -> Step {
    (s.as_bytes().to_vec(), pause)
}

/// 权限对话框（Bash 授权 / ExitPlanMode 批准…，Ink 的单选列表）：
/// allow = 第 1 项的数字（Yes；数字键即选中）再补一个 Return 兜底——对话框已经没了的话
/// Return 只是在空输入框上回车，无害；deny = Esc（No / 打断，回到输入框）。
pub fn permission_steps(behavior: &str) -> Result<Vec<Step>, String> {
    match behavior {
        "allow" => Ok(vec![key("1", 250), key("\r", BEAT)]),
        "deny" => Ok(vec![key("\x1b", BEAT)]),
        other => Err(format!("behavior 只能是 allow / deny，不是 {other}")),
    }
}

/// 把按键敲进 PTY，每一步之间留出节拍（Ink 一次 read 当一个输入事件）。
pub async fn type_steps(sess: &Arc<Session>, steps: Vec<Step>) -> Result<(), String> {
    for (bytes, pause) in steps {
        sess.write_input(&bytes).map_err(|e| format!("pty write: {e}"))?;
        tokio::time::sleep(Duration::from_millis(pause)).await;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn keys(steps: &[Step]) -> Vec<String> {
        steps.iter().map(|(b, _)| String::from_utf8(b.clone()).unwrap()).collect()
    }

    #[test]
    fn permission_is_two_keys_and_nothing_else() {
        assert_eq!(keys(&permission_steps("allow").unwrap()), vec!["1", "\r"]);
        assert_eq!(keys(&permission_steps("deny").unwrap()), vec!["\x1b"]);
        assert!(permission_steps("maybe").is_err());
    }
}
