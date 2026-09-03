//! Session pool: PTY spawn, server-side VT (vt100), broadcast to attached
//! clients, state machine, exited-session persistence.

use std::collections::HashMap;
use std::io::{Read, Write};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use bytes::Bytes;
use chrono::{DateTime, SecondsFormat, Utc};
use portable_pty::{ChildKiller, CommandBuilder, MasterPty, PtySize};
use serde::{Deserialize, Serialize};
use tokio::sync::broadcast;

use crate::events::EventHub;

pub const SCROLLBACK_LINES: usize = 5000;
pub const REPLAY_SCROLLBACK_TAIL: usize = 400;
pub const DEFAULT_ROWS: u16 = 40;
pub const DEFAULT_COLS: u16 = 120;
pub const SILENCE_SECS: f64 = 6.0;
/// Cap on exited sessions restored (and kept on disk) across a daemon
/// restart. Conservative retention: nothing is deleted while the daemon runs,
/// only the oldest records beyond the cap are dropped at startup — replay
/// semantics for everything kept are untouched.
pub const MAX_RESTORED_EXITED: usize = 200;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum State {
    Running,
    /// Alive and the screen has stopped changing: the agent finished its turn
    /// and is waiting for the user. (Persisted metas from before 2026-09-02
    /// may still say `idle`; it collapses into this.)
    #[serde(alias = "idle")]
    Waiting,
    Exited,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Meta {
    pub title: String,
    #[serde(default)]
    pub custom_title: bool,
    pub project_path: String,
    pub project_name: String,
    pub agent: String,
    pub state: State,
    pub rows: u16,
    pub cols: u16,
    pub pid: Option<u32>,
    pub exit_code: Option<i64>,
    pub resume_id: Option<String>,
    pub created_at: DateTime<Utc>,
    pub last_output_at: DateTime<Utc>,
    #[serde(default)]
    pub preview: String,
    /// v1.1: inbox auto-feed enabled for this session (POST /sessions)
    #[serde(default = "default_true")]
    pub feed_inbox: bool,
    /// v1.1: start-checkpoint ref (persisted so diff/rollback survive restarts)
    #[serde(default)]
    pub ckpt_start_ref: Option<String>,
    // -- volatile --
    #[serde(skip)]
    pub last_output_inst: Option<Instant>,
    #[serde(skip)]
    pub needs_name: bool,
    #[serde(skip)]
    pub inbox_fed: bool,
    /// 可见屏幕内容的哈希 + 上次内容变化时刻。「静默」按画面算而不是按
    /// 字节流：agy 这类 TUI 每几秒全清屏重绘（内容不变），按输出算它
    /// 永远是 Running、永不通知。
    #[serde(skip)]
    pub screen_hash: u64,
    #[serde(skip)]
    pub screen_changed_inst: Option<Instant>,
    /// The agent transcript holds an AskUserQuestion with no answer yet
    /// (claude only; structured, never guessed from the screen). Mirrored
    /// here from the message store so `/events` can carry it.
    #[serde(skip)]
    pub asking: bool,
    /// 用户主动 kill：客户端据此不弹「退出」通知（自己动的手，不用报告）
    #[serde(skip)]
    pub user_killed: bool,
    /// trust::on_tick 已替用户按过几次 Enter / 上次何时（只对 claude 会话）
    #[serde(skip)]
    pub trust_presses: u8,
    #[serde(skip)]
    pub trust_pressed_inst: Option<Instant>,
    /// 收到过至少一个 Claude Code hook 事件：状态由事件驱动，屏幕静默启发式退场
    #[serde(skip)]
    pub hooked: bool,
    /// StopFailure 报的错误类型（rate_limit / overloaded / authentication_failed…），
    /// 下一次提交清掉
    #[serde(skip)]
    pub error: Option<String>,
    /// PreCompact 到 PostCompact 之间：正在整理上下文
    #[serde(skip)]
    pub compacting: bool,
    /// PreToolUse(AskUserQuestion) 到达时刻：transcript 还没落盘的几秒内不许把
    /// asking 又压回 false
    #[serde(skip)]
    pub asking_hint_inst: Option<Instant>,
    /// statusLine 转来的本会话用量（模型 / 上下文占比 / 费用），持久化以便退出后还能看
    #[serde(default)]
    pub usage: Option<serde_json::Value>,
}

fn default_true() -> bool {
    true
}

pub struct Live {
    pub master: Box<dyn MasterPty + Send>,
    pub writer: Box<dyn Write + Send>,
    pub killer: Box<dyn ChildKiller + Send + Sync>,
    pub pid: Option<u32>,
}

pub struct Session {
    pub id: String,
    pub meta: Mutex<Meta>,
    pub parser: Mutex<Option<vt100::Parser>>,
    /// PTY output fan-out. `Bytes` so each chunk is allocated once and every
    /// attached client clones a refcount, not the buffer.
    pub out_tx: broadcast::Sender<Bytes>,
    pub live: Mutex<Option<Live>>,
    pub dirty: AtomicBool,
    /// v1.1: structured message stream (agent store tail)
    pub msgs: Mutex<crate::messages::MsgStore>,
    /// v1.1: git checkpoint runtime state
    pub ckpt: Mutex<crate::checkpoint::CkptState>,
}

pub struct PoolCtx {
    pub hub: EventHub,
    pub sessions_dir: PathBuf,
    pub ckpt_cfg: crate::config::CheckpointConfig,
}

pub struct SessionPool {
    pub map: Mutex<HashMap<String, Arc<Session>>>,
    pub ctx: Arc<PoolCtx>,
}

fn iso(dt: &DateTime<Utc>) -> String {
    dt.to_rfc3339_opts(SecondsFormat::Secs, true)
}

impl Session {
    /// 测试用：没有 PTY 的会话骨架（hooks / feed 单测）
    #[cfg(test)]
    pub fn for_test(agent: &str, project_path: &str) -> Session {
        let now = Utc::now();
        let (tx, _) = broadcast::channel(8);
        Session {
            id: "s_test".into(),
            meta: Mutex::new(Meta {
                title: "t".into(),
                custom_title: false,
                project_path: project_path.into(),
                project_name: "p".into(),
                agent: agent.into(),
                state: State::Running,
                rows: DEFAULT_ROWS,
                cols: DEFAULT_COLS,
                pid: None,
                exit_code: None,
                resume_id: None,
                created_at: now,
                last_output_at: now,
                preview: String::new(),
                feed_inbox: true,
                ckpt_start_ref: None,
                last_output_inst: None,
                needs_name: false,
                inbox_fed: false,
                screen_hash: 0,
                screen_changed_inst: None,
                asking: false,
                user_killed: false,
                trust_presses: 0,
                trust_pressed_inst: None,
                hooked: false,
                error: None,
                compacting: false,
                asking_hint_inst: None,
                usage: None,
            }),
            parser: Mutex::new(None),
            out_tx: tx,
            live: Mutex::new(None),
            dirty: AtomicBool::new(false),
            msgs: Mutex::new(crate::messages::MsgStore::for_agent(agent)),
            ckpt: Mutex::new(crate::checkpoint::CkptState::default()),
        }
    }

    pub fn to_json(&self) -> serde_json::Value {
        // refresh preview from the parser when we have one
        let preview = {
            let parser = self.parser.lock().unwrap();
            parser.as_ref().map(|p| crate::screen::preview(p.screen(), 4))
        };
        let mut meta = self.meta.lock().unwrap();
        if let Some(p) = preview {
            meta.preview = p;
        }
        serde_json::json!({
            "id": self.id,
            "title": meta.title,
            "project_path": meta.project_path,
            "project_name": meta.project_name,
            "agent": meta.agent,
            "state": meta.state,
            "asking": meta.asking,
            "preview": meta.preview,
            "rows": meta.rows,
            "cols": meta.cols,
            "pid": meta.pid,
            "exit_code": meta.exit_code,
            "resume_id": meta.resume_id,
            "created_at": iso(&meta.created_at),
            "last_output_at": iso(&meta.last_output_at),
            // v1.3（老客户端忽略未知字段）
            "hooked": meta.hooked,
            "error": meta.error,
            "compacting": meta.compacting,
            "user_killed": meta.user_killed,
            "usage": meta.usage,
        })
    }

    pub fn state(&self) -> State {
        self.meta.lock().unwrap().state
    }

    pub fn mark_dirty(&self) {
        self.dirty.store(true, Ordering::Relaxed);
    }

    fn meta_path(&self, ctx: &PoolCtx) -> PathBuf {
        ctx.sessions_dir.join(format!("{}.json", self.id))
    }
    fn replay_path(&self, ctx: &PoolCtx) -> PathBuf {
        ctx.sessions_dir.join(format!("{}.replay", self.id))
    }

    /// Build the reconnect-redraw byte stream: scrollback tail lines +
    /// clear screen + `contents_formatted()` of the current screen.
    pub fn build_replay(&self, ctx: &PoolCtx) -> Vec<u8> {
        let mut guard = self.parser.lock().unwrap();
        match guard.as_mut() {
            Some(parser) => build_replay_from_parser(parser),
            None => std::fs::read(self.replay_path(ctx)).unwrap_or_default(),
        }
    }

    /// Replay + broadcast receiver with no gap/overlap: subscribing while the
    /// parser lock is held means the reader thread (which locks the parser
    /// before broadcasting) cannot slip bytes between the two.
    pub fn attach_snapshot(&self, ctx: &PoolCtx) -> (Vec<u8>, broadcast::Receiver<Bytes>) {
        let mut guard = self.parser.lock().unwrap();
        let rx = self.out_tx.subscribe();
        let replay = match guard.as_mut() {
            Some(parser) => build_replay_from_parser(parser),
            None => std::fs::read(self.replay_path(ctx)).unwrap_or_default(),
        };
        (replay, rx)
    }

    /// Persist metadata + final screen so exited sessions survive daemon
    /// restarts.
    pub fn persist(&self, ctx: &PoolCtx) {
        if std::fs::create_dir_all(&ctx.sessions_dir).is_err() {
            return;
        }
        let replay = self.build_replay(ctx);
        let _ = std::fs::write(self.replay_path(ctx), &replay);
        let meta = self.meta.lock().unwrap().clone();
        if let Ok(body) = serde_json::to_string_pretty(&meta) {
            let _ = crate::paths::write_atomic(&self.meta_path(ctx), body.as_bytes());
        }
    }

    pub fn remove_persisted(&self, ctx: &PoolCtx) {
        let _ = std::fs::remove_file(self.meta_path(ctx));
        let _ = std::fs::remove_file(self.replay_path(ctx));
    }

    pub fn write_input(&self, bytes: &[u8]) -> std::io::Result<()> {
        let mut live = self.live.lock().unwrap();
        match live.as_mut() {
            Some(l) => {
                l.writer.write_all(bytes)?;
                l.writer.flush()
            }
            None => Err(std::io::Error::new(
                std::io::ErrorKind::NotConnected,
                "session not running",
            )),
        }
    }

    pub fn resize(&self, cols: u16, rows: u16) {
        {
            let live = self.live.lock().unwrap();
            if let Some(l) = live.as_ref() {
                let _ = l.master.resize(PtySize {
                    rows,
                    cols,
                    pixel_width: 0,
                    pixel_height: 0,
                });
            }
        }
        if let Some(p) = self.parser.lock().unwrap().as_mut() {
            p.screen_mut().set_size(rows, cols);
        }
        let mut meta = self.meta.lock().unwrap();
        meta.rows = rows;
        meta.cols = cols;
        drop(meta);
        self.mark_dirty();
    }

    /// TERM now; escalate to KILL after 2s if the process is still alive.
    pub async fn kill(self: &Arc<Self>) {
        // 用户/客户端主动终止：退出时不再推「退出」通知（自己动的手）
        self.meta.lock().unwrap().user_killed = true;
        let pid = {
            let live = self.live.lock().unwrap();
            live.as_ref().and_then(|l| l.pid)
        };
        if let Some(pid) = pid {
            unsafe {
                libc::kill(pid as libc::pid_t, libc::SIGTERM);
                libc::killpg(pid as libc::pid_t, libc::SIGTERM);
            }
        }
        let sess = Arc::clone(self);
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_secs(2)).await;
            if sess.state() != State::Exited {
                let mut live = sess.live.lock().unwrap();
                if let Some(l) = live.as_mut() {
                    let _ = l.killer.kill(); // SIGKILL
                    if let Some(pid) = l.pid {
                        unsafe {
                            libc::killpg(pid as libc::pid_t, libc::SIGKILL);
                        }
                    }
                }
            }
        });
    }
}

/// Extract the last `limit` scrollback lines as plain text by walking the
/// vt100 scrollback view offsets.
fn extract_scrollback(parser: &mut vt100::Parser, limit: usize) -> Vec<String> {
    parser.screen_mut().set_scrollback(usize::MAX); // clamped to actual length
    let n = parser.screen().scrollback();
    let (rows, cols) = parser.screen().size();
    let take = limit.min(n);
    let mut out: Vec<String> = Vec::with_capacity(take);
    let mut s = n - take; // logical index of first wanted scrollback line
    while s < n {
        let k = n - s; // offset whose view starts at logical line s
        parser.screen_mut().set_scrollback(k);
        let avail = (n - s).min(rows as usize);
        let lines: Vec<String> = parser.screen().rows(0, cols).take(avail).collect();
        let got = lines.len().max(1);
        out.extend(lines);
        s += got;
    }
    parser.screen_mut().set_scrollback(0);
    out
}

/// 屏幕纯文本：非备用屏时前面带最近 `scrollback_limit` 行回滚，然后是可见画面；
/// 尾部空行剥掉。给客户端「复制屏幕内容 / 抓链接」用，两端看到同一份。
pub fn screen_text(parser: &mut vt100::Parser, scrollback_limit: usize) -> String {
    let mut lines: Vec<String> = if parser.screen().alternate_screen() {
        Vec::new()
    } else {
        extract_scrollback(parser, scrollback_limit)
    };
    let (_, cols) = parser.screen().size();
    lines.extend(parser.screen().rows(0, cols));
    let mut out: Vec<&str> = lines.iter().map(|l| l.trim_end()).collect();
    while out.last().is_some_and(|l| l.is_empty()) {
        out.pop();
    }
    out.join("\n")
}

pub fn build_replay_from_parser(parser: &mut vt100::Parser) -> Vec<u8> {
    let sb = extract_scrollback(parser, REPLAY_SCROLLBACK_TAIL);
    let mut out = Vec::new();
    for line in sb {
        out.extend_from_slice(line.trim_end().as_bytes());
        out.extend_from_slice(b"\r\n");
    }
    // 备用屏要先切过去再画：客户端的模拟器才会把内容放进备用屏，TUI 退出时
    // 发的 1049l 也才能把它的主屏（上面的回滚）复原。
    if parser.screen().alternate_screen() {
        out.extend_from_slice(b"\x1b[?1049h");
    }
    out.extend_from_slice(b"\x1b[2J\x1b[H\x1b[0m");
    // state_formatted = 画面 + **终端状态**（鼠标上报 1000/1006、括号粘贴、
    // 应用光标键、键盘模式、光标显隐）。过去只发 contents_formatted，
    // 在 TUI 启动之后才 attach 的客户端永远不知道对方要鼠标，点击/滚轮
    // 都被当成本地选区——claude code 的鼠标在 AAA 里「不好使」的根源。
    out.extend_from_slice(&parser.screen().state_formatted());
    out
}

fn new_session_id() -> String {
    use rand::Rng;
    let n: u32 = rand::rng().random();
    format!("s_{n:08x}")
}

pub struct SpawnSpec {
    pub project_path: String,
    pub project_name: String,
    pub agent: String,
    pub title: String,
    pub cmd: String,
    pub resume_id: Option<String>,
    pub feed_inbox: bool,
}

impl SessionPool {
    pub fn new(ctx: PoolCtx) -> Self {
        SessionPool { map: Mutex::new(HashMap::new()), ctx: Arc::new(ctx) }
    }

    pub fn get(&self, id: &str) -> Option<Arc<Session>> {
        self.map.lock().unwrap().get(id).cloned()
    }

    /// All sessions, unordered — for internal sweeps that visit every session.
    pub fn all(&self) -> Vec<Arc<Session>> {
        self.map.lock().unwrap().values().cloned().collect()
    }

    /// All sessions, newest first (API-facing order).
    pub fn list(&self) -> Vec<Arc<Session>> {
        let mut v: Vec<(DateTime<Utc>, Arc<Session>)> = self
            .all()
            .into_iter()
            .map(|s| {
                let at = s.meta.lock().unwrap().created_at;
                (at, s)
            })
            .collect();
        v.sort_by_key(|(at, _)| std::cmp::Reverse(*at));
        v.into_iter().map(|(_, s)| s).collect()
    }

    pub fn remove(&self, id: &str) -> Option<Arc<Session>> {
        self.map.lock().unwrap().remove(id)
    }

    /// Restore exited sessions persisted by a previous daemon run.
    pub fn restore_persisted(&self) {
        self.restore_persisted_capped(MAX_RESTORED_EXITED);
    }

    /// Restore at most `cap` persisted sessions (newest by last_output_at);
    /// records beyond the cap have their meta + replay files removed so the
    /// state dir cannot grow without bound across restarts.
    fn restore_persisted_capped(&self, cap: usize) {
        let Ok(rd) = std::fs::read_dir(&self.ctx.sessions_dir) else { return };
        let mut restored: Vec<(String, Meta)> = Vec::new();
        for entry in rd.flatten() {
            let p = entry.path();
            if p.extension().and_then(|e| e.to_str()) != Some("json") {
                continue;
            }
            let Ok(body) = std::fs::read_to_string(&p) else { continue };
            let Ok(mut meta) = serde_json::from_str::<Meta>(&body) else { continue };
            let Some(id) = p
                .file_stem()
                .and_then(|s| s.to_str())
                .map(String::from)
                .filter(|s| s.starts_with("s_"))
            else {
                continue;
            };
            meta.state = State::Exited;
            meta.needs_name = false;
            restored.push((id, meta));
        }
        restored.sort_by_key(|(_, m)| std::cmp::Reverse(m.last_output_at));
        for (id, _) in restored.iter().skip(cap) {
            let _ = std::fs::remove_file(self.ctx.sessions_dir.join(format!("{id}.json")));
            let _ = std::fs::remove_file(self.ctx.sessions_dir.join(format!("{id}.replay")));
        }
        restored.truncate(cap);
        for (id, meta) in restored {
            let (tx, _) = broadcast::channel(64);
            let msgs = crate::messages::MsgStore::for_agent(&meta.agent);
            let ckpt = crate::checkpoint::CkptState {
                start_ref: meta.ckpt_start_ref.clone(),
                ..Default::default()
            };
            let sess = Arc::new(Session {
                id: id.clone(),
                meta: Mutex::new(meta),
                parser: Mutex::new(None),
                out_tx: tx,
                live: Mutex::new(None),
                dirty: AtomicBool::new(false),
                msgs: Mutex::new(msgs),
                ckpt: Mutex::new(ckpt),
            });
            self.map.lock().unwrap().insert(id, sess);
        }
    }

    /// Spawn `zsh -lc 'cd <dir> && <cmd>'` on a fresh PTY.
    pub fn spawn(&self, spec: SpawnSpec) -> Result<Arc<Session>, String> {
        let pty_system = portable_pty::native_pty_system();
        let pair = pty_system
            .openpty(PtySize {
                rows: DEFAULT_ROWS,
                cols: DEFAULT_COLS,
                pixel_width: 0,
                pixel_height: 0,
            })
            .map_err(|e| format!("openpty: {e}"))?;

        let id = new_session_id();
        let argv = crate::agents::spawn_argv(&spec.project_path, &spec.cmd);
        let mut cmd = CommandBuilder::new(&argv[0]);
        cmd.args(&argv[1..]);
        cmd.cwd(&spec.project_path);
        cmd.env("TERM", "xterm-256color");
        // hooks 用它在请求头里报出自己是哪个会话（hooks.rs）
        cmd.env(crate::hooks::SESSION_ENV, &id);
        cmd.env("LANG", "en_US.UTF-8");
        // Without this every agent dies with "command not found" under
        // launchd: the inherited PATH is bare, and `zsh -lc` does not source
        // .zshrc to fix it. See agents::agent_path.
        cmd.env("PATH", crate::agents::spawn_path());
        // Agents must start as if launched by hand. When the daemon itself was
        // started from inside an agent (running it in the foreground to debug,
        // say), it inherits that agent's session markers, and a child Claude
        // Code then disables transcript saving — which silently empties the
        // message stream, since that view is built by tailing the transcript.
        for marker in [
            "CLAUDE_CODE_CHILD_SESSION",
            "CLAUDE_CODE_SESSION_ID",
            "CLAUDE_CODE_ENTRYPOINT",
            "CLAUDECODE",
        ] {
            cmd.env_remove(marker);
        }

        let mut child = pair
            .slave
            .spawn_command(cmd)
            .map_err(|e| format!("spawn: {e}"))?;
        drop(pair.slave);

        let pid = child.process_id();
        let killer = child.clone_killer();
        let reader = pair
            .master
            .try_clone_reader()
            .map_err(|e| format!("clone reader: {e}"))?;
        let writer = pair
            .master
            .take_writer()
            .map_err(|e| format!("take writer: {e}"))?;

        let now = Utc::now();
        let meta = Meta {
            title: spec.title,
            custom_title: false,
            project_path: spec.project_path.clone(),
            project_name: spec.project_name.clone(),
            agent: spec.agent.clone(),
            state: State::Running,
            rows: DEFAULT_ROWS,
            cols: DEFAULT_COLS,
            pid,
            exit_code: None,
            resume_id: spec.resume_id.clone(),
            created_at: now,
            last_output_at: now,
            preview: String::new(),
            feed_inbox: spec.feed_inbox,
            ckpt_start_ref: None,
            last_output_inst: Some(Instant::now()),
            needs_name: true,
            inbox_fed: false,
            screen_hash: 0,
            screen_changed_inst: None,
            asking: false,
            user_killed: false,
            trust_presses: 0,
            trust_pressed_inst: None,
            hooked: false,
            error: None,
            compacting: false,
            asking_hint_inst: None,
            usage: None,
        };
        // Backpressure: send never blocks; a client that can't keep up drops
        // to Lagged and gets a fresh full redraw (api::attach_loop), so a slow
        // phone can never stall the PTY reader or other clients.
        let (tx, _) = broadcast::channel(1024);
        let msgs = crate::messages::MsgStore::for_agent(&spec.agent);
        let sess = Arc::new(Session {
            id,
            meta: Mutex::new(meta),
            parser: Mutex::new(Some(vt100::Parser::new(
                DEFAULT_ROWS,
                DEFAULT_COLS,
                SCROLLBACK_LINES,
            ))),
            out_tx: tx,
            live: Mutex::new(Some(Live { master: pair.master, writer, killer, pid })),
            dirty: AtomicBool::new(true),
            msgs: Mutex::new(msgs),
            ckpt: Mutex::new(crate::checkpoint::CkptState::default()),
        });
        self.map
            .lock()
            .unwrap()
            .insert(sess.id.clone(), Arc::clone(&sess));

        // Reader thread: PTY -> vt100 parser + broadcast; EOF -> exited.
        let rsess = Arc::clone(&sess);
        let rctx = Arc::clone(&self.ctx);
        std::thread::spawn(move || {
            let mut reader = reader;
            let mut buf = [0u8; 8192];
            loop {
                match reader.read(&mut buf) {
                    Ok(0) | Err(_) => break,
                    Ok(n) => {
                        let data = &buf[..n];
                        if let Some(p) = rsess.parser.lock().unwrap().as_mut() {
                            p.process(data);
                        }
                        {
                            let mut meta = rsess.meta.lock().unwrap();
                            meta.last_output_at = Utc::now();
                            meta.last_output_inst = Some(Instant::now());
                            // hooked 会话的 running/waiting 由 UserPromptSubmit/Stop 决定：
                            // 等待中的光标闪烁、时钟重绘不算「在跑」
                            if meta.state != State::Running && !meta.hooked {
                                meta.state = State::Running;
                                meta.needs_name = true;
                            }
                        }
                        rsess.mark_dirty();
                        let _ = rsess.out_tx.send(Bytes::copy_from_slice(data));
                    }
                }
            }
            // child exited (or PTY closed)
            let code = child
                .wait()
                .ok()
                .map(|st| st.exit_code() as i64);
            {
                let mut meta = rsess.meta.lock().unwrap();
                meta.state = State::Exited;
                meta.exit_code = code;
                meta.asking = false;
                meta.needs_name = true;
            }
            *rsess.live.lock().unwrap() = None;
            // v1.1: end checkpoint (agent sessions with a start checkpoint)
            {
                let (agent, project_path, has_start) = {
                    let meta = rsess.meta.lock().unwrap();
                    (
                        meta.agent.clone(),
                        meta.project_path.clone(),
                        meta.ckpt_start_ref.is_some(),
                    )
                };
                if rctx.ckpt_cfg.enabled && agent != "shell" && has_start {
                    let mut state = rsess.ckpt.lock().unwrap();
                    let _ = crate::checkpoint::make_checkpoint(
                        std::path::Path::new(&project_path),
                        &rsess.id,
                        &mut state,
                        "end",
                    );
                }
            }
            rsess.persist(&rctx);
            rsess.mark_dirty();
        });

        Ok(sess)
    }

    /// 1s tick: a running session whose *visible screen* has not changed for
    /// `SILENCE_SECS` is done with its turn → `waiting`. No screen reading
    /// beyond the hash: what the agent is asking, if anything, comes from
    /// the transcript (`asking`). Returns the sessions that just flipped, so
    /// the caller can run the inbox feed.
    pub fn tick_states(&self) -> Vec<Arc<Session>> {
        let mut entered_waiting = Vec::new();
        for sess in self.all() {
            // 「静默」按可见内容算，不按字节流：agy 这类 TUI 每几秒全清屏
            // 重绘一遍（画面不变），按输出算它永远 Running、永不完成。
            let hash = {
                let guard = sess.parser.lock().unwrap();
                guard.as_ref().map(|p| {
                    use std::hash::{Hash, Hasher};
                    let mut h = std::collections::hash_map::DefaultHasher::new();
                    let screen = p.screen();
                    let (_, cols) = screen.size();
                    for row in screen.rows(0, cols) {
                        row.hash(&mut h);
                    }
                    h.finish()
                })
            };
            let flipped = {
                let mut meta = sess.meta.lock().unwrap();
                if let Some(hash) = hash {
                    if meta.screen_hash != hash {
                        meta.screen_hash = hash;
                        meta.screen_changed_inst = Some(Instant::now());
                    }
                }
                let silent = meta
                    .screen_changed_inst
                    .or(meta.last_output_inst)
                    .map(|t| t.elapsed().as_secs_f64() >= SILENCE_SECS)
                    .unwrap_or(false);
                if meta.state == State::Running && silent && !meta.hooked {
                    meta.state = State::Waiting;
                    true
                } else {
                    false
                }
            };
            if flipped {
                sess.mark_dirty();
                entered_waiting.push(Arc::clone(&sess));
            }
        }
        entered_waiting
    }

    /// 250ms flush: emit throttled `session` events for dirty sessions.
    pub fn flush_dirty(&self) {
        for sess in self.all() {
            if sess.dirty.swap(false, Ordering::Relaxed) {
                self.ctx.hub.session(sess.to_json());
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn screen_text_joins_scrollback_and_screen_on_main_screen_only() {
        let mut parser = vt100::Parser::new(3, 20, 100);
        parser.process(b"one\r\ntwo\r\nthree\r\nfour\r\n");
        // 3 行屏 + 1 行回滚（"one"）；尾部空行剥掉
        assert_eq!(screen_text(&mut parser, 500), "one\ntwo\nthree\nfour");
        assert_eq!(screen_text(&mut parser, 0), "three\nfour"); // 只看可见 3 行（第 3 行空）
        // 备用屏：只有可见画面，回滚不算
        parser.process(b"\x1b[?1049h\x1b[2J\x1b[Halt");
        assert!(parser.screen().alternate_screen());
        assert_eq!(screen_text(&mut parser, 500), "alt");
    }

    #[test]
    fn replay_contains_scrollback_tail_and_screen() {
        let mut parser = vt100::Parser::new(5, 40, 100);
        for i in 0..30 {
            parser.process(format!("line-{i}\r\n").as_bytes());
        }
        let replay = build_replay_from_parser(&mut parser);
        let text = String::from_utf8_lossy(&replay);
        // 30 lines printed on a 5-row screen: lines 0..25 scrolled out
        assert!(text.contains("line-0\r\n"), "oldest scrollback line present");
        assert!(text.contains("line-24"), "newest scrollback line present");
        assert!(text.contains("\x1b[2J"), "clear screen between scrollback and live screen");
        assert!(text.contains("line-29"), "current screen content present");
        // scrollback must be ordered
        let p0 = text.find("line-0\r\n").unwrap();
        let p24 = text.find("line-24").unwrap();
        assert!(p0 < p24);
    }

    #[test]
    fn replay_carries_terminal_modes_for_late_attachers() {
        // claude code 的真实开场：备用屏 + 鼠标上报 1000/1006 + 括号粘贴
        let mut parser = vt100::Parser::new(24, 80, 100);
        parser.process(b"\x1b[?1049h\x1b[?1000h\x1b[?1006h\x1b[?2004hhello");
        let replay = build_replay_from_parser(&mut parser);
        let text = String::from_utf8_lossy(&replay);
        for mode in ["\x1b[?1049h", "\x1b[?1000h", "\x1b[?1006h", "\x1b[?2004h"] {
            assert!(text.contains(mode), "replay must re-enable {mode:?}: {text:?}");
        }
        assert!(text.contains("hello"));
        // 没开鼠标的普通 shell：不能凭空给客户端打开鼠标上报
        let mut plain = vt100::Parser::new(24, 80, 100);
        plain.process(b"$ ls\r\n");
        let replay = build_replay_from_parser(&mut plain);
        let text = String::from_utf8_lossy(&replay);
        assert!(!text.contains("\x1b[?1000h") && !text.contains("\x1b[?1049h"), "{text:?}");
    }

    #[test]
    fn pty_broadcast_shares_one_buffer_across_clients() {
        let (tx, mut rx1) = broadcast::channel::<Bytes>(8);
        let mut rx2 = tx.subscribe();
        tx.send(Bytes::copy_from_slice(b"chunk")).unwrap();
        let a = rx1.try_recv().unwrap();
        let b = rx2.try_recv().unwrap();
        // same allocation: N attached clients cost N refcounts, not N copies
        assert_eq!(a.as_ptr(), b.as_ptr());
    }

    #[test]
    fn restore_prunes_oldest_beyond_cap() {
        let dir = tempfile::tempdir().unwrap();
        let sessions_dir = dir.path().join("sessions");
        std::fs::create_dir_all(&sessions_dir).unwrap();
        for i in 0..5 {
            let meta = serde_json::json!({
                "title": format!("t{i}"),
                "project_path": "/p",
                "project_name": "p",
                "agent": "shell",
                "state": "exited",
                "question": null,
                "rows": 40, "cols": 120,
                "pid": null, "exit_code": 0, "resume_id": null,
                "created_at": format!("2026-01-0{}T00:00:00Z", i + 1),
                "last_output_at": format!("2026-01-0{}T00:00:00Z", i + 1),
            });
            std::fs::write(sessions_dir.join(format!("s_{i:08x}.json")), meta.to_string()).unwrap();
            std::fs::write(sessions_dir.join(format!("s_{i:08x}.replay")), b"replay").unwrap();
        }
        let pool = SessionPool::new(PoolCtx {
            hub: EventHub::new(),
            sessions_dir: sessions_dir.clone(),
            ckpt_cfg: crate::config::CheckpointConfig::default(),
        });
        pool.restore_persisted_capped(3);
        assert_eq!(pool.all().len(), 3, "only the newest `cap` sessions restored");
        // the two oldest records (and their replays) are gone from disk
        assert!(!sessions_dir.join("s_00000000.json").exists());
        assert!(!sessions_dir.join("s_00000000.replay").exists());
        assert!(!sessions_dir.join("s_00000001.json").exists());
        // the newest survive, replay intact
        assert!(sessions_dir.join("s_00000004.json").exists());
        assert!(sessions_dir.join("s_00000004.replay").exists());
        assert!(pool.get("s_00000004").is_some());
        assert!(pool.get("s_00000000").is_none());
    }

    #[test]
    fn scrollback_extraction_respects_limit() {
        let mut parser = vt100::Parser::new(5, 40, 1000);
        for i in 0..200 {
            parser.process(format!("l{i}\r\n").as_bytes());
        }
        let sb = extract_scrollback(&mut parser, 10);
        assert_eq!(sb.len(), 10);
        // screen shows l196..l199 + cursor row; l195 is the last scrolled-out line
        assert_eq!(sb[9].trim_end(), "l195");
        // parser back at live view
        assert_eq!(parser.screen().scrollback(), 0);
    }
}
