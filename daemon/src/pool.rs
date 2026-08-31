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

use crate::config::NtfyConfig;
use crate::events::EventHub;
use crate::statemachine::{self, Question, Verdict};

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
    Waiting,
    Idle,
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
    pub question: Option<Question>,
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
    pub hook_waiting: bool,
    #[serde(skip)]
    pub needs_name: bool,
    #[serde(skip)]
    pub inbox_fed: bool,
    #[serde(skip)]
    pub stalled_notified: bool,
    #[serde(skip)]
    pub last_notified_question: Option<String>,
    #[serde(skip)]
    pub last_notify_at: Option<Instant>,
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
    pub ntfy: Option<NtfyConfig>,
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
    pub fn to_json(&self) -> serde_json::Value {
        // refresh preview from the parser when we have one
        let preview = {
            let parser = self.parser.lock().unwrap();
            parser.as_ref().map(|p| statemachine::preview(p.screen(), 4))
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
            "question": meta.question,
            "preview": meta.preview,
            "rows": meta.rows,
            "cols": meta.cols,
            "pid": meta.pid,
            "exit_code": meta.exit_code,
            "resume_id": meta.resume_id,
            "created_at": iso(&meta.created_at),
            "last_output_at": iso(&meta.last_output_at),
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

pub fn build_replay_from_parser(parser: &mut vt100::Parser) -> Vec<u8> {
    let sb = extract_scrollback(parser, REPLAY_SCROLLBACK_TAIL);
    let mut out = Vec::new();
    for line in sb {
        out.extend_from_slice(line.trim_end().as_bytes());
        out.extend_from_slice(b"\r\n");
    }
    out.extend_from_slice(b"\x1b[2J\x1b[H\x1b[0m");
    out.extend_from_slice(&parser.screen().contents_formatted());
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
            meta.hook_waiting = false;
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

        let argv = crate::agents::spawn_argv(&spec.project_path, &spec.cmd);
        let mut cmd = CommandBuilder::new(&argv[0]);
        cmd.args(&argv[1..]);
        cmd.cwd(&spec.project_path);
        cmd.env("TERM", "xterm-256color");
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
            question: None,
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
            hook_waiting: false,
            needs_name: true,
            inbox_fed: false,
            stalled_notified: false,
            last_notified_question: None,
            last_notify_at: None,
        };
        // Backpressure: send never blocks; a client that can't keep up drops
        // to Lagged and gets a fresh full redraw (api::attach_loop), so a slow
        // phone can never stall the PTY reader or other clients.
        let (tx, _) = broadcast::channel(1024);
        let msgs = crate::messages::MsgStore::for_agent(&spec.agent);
        let sess = Arc::new(Session {
            id: new_session_id(),
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
                            if meta.state != State::Running {
                                meta.state = State::Running;
                                meta.needs_name = true;
                            }
                            meta.question = None;
                            meta.hook_waiting = false;
                            meta.stalled_notified = false;
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
                meta.question = None;
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
            if let Some(ntfy) = &rctx.ntfy {
                let meta = rsess.meta.lock().unwrap();
                crate::ntfy::push_blocking(
                    ntfy,
                    &format!("退出: {}", meta.title),
                    &format!(
                        "{} · {} exited (code {})",
                        meta.project_name,
                        meta.agent,
                        code.map(|c| c.to_string()).unwrap_or_else(|| "?".into())
                    ),
                    crate::ntfy::PRIO_DEFAULT,
                );
            }
        });

        Ok(sess)
    }

    /// 1s tick: silent running sessions -> waiting/idle by screen heuristics.
    /// Returns sessions that just transitioned into waiting (with the parsed
    /// question); the caller handles inbox feed + ntfy dedup.
    pub fn tick_states(&self) -> Vec<(Arc<Session>, Option<Question>)> {
        let mut entered_waiting = Vec::new();
        for sess in self.all() {
            let (is_running, silent) = {
                let meta = sess.meta.lock().unwrap();
                let silent = meta
                    .last_output_inst
                    .map(|t| t.elapsed().as_secs_f64() >= SILENCE_SECS)
                    .unwrap_or(false);
                (meta.state == State::Running && !meta.hook_waiting, silent)
            };
            if !is_running || !silent {
                continue;
            }
            let verdict = {
                let guard = sess.parser.lock().unwrap();
                guard.as_ref().map(|p| statemachine::analyze_screen(p.screen()))
            };
            let Some(verdict) = verdict else { continue };
            let mut became_waiting: Option<Option<Question>> = None;
            {
                let mut meta = sess.meta.lock().unwrap();
                if meta.state != State::Running {
                    continue;
                }
                match verdict {
                    Verdict::Waiting(q) => {
                        meta.state = State::Waiting;
                        meta.question = q.clone();
                        became_waiting = Some(q);
                    }
                    Verdict::Idle => {
                        meta.state = State::Idle;
                        meta.question = None;
                    }
                }
            }
            sess.mark_dirty();
            if let Some(q) = became_waiting {
                entered_waiting.push((Arc::clone(&sess), q));
            }
        }
        entered_waiting
    }

    /// v1.1 watchdog decision (pure, unit-testable). `waiting` never counts;
    /// shell sessions sit at a prompt forever and are exempt.
    pub fn watchdog_due(
        state: State,
        agent: &str,
        alive: bool,
        silence_s: u64,
        stall_minutes: u64,
        already_notified: bool,
    ) -> bool {
        stall_minutes > 0
            && alive
            && !already_notified
            && agent != "shell"
            && !matches!(state, State::Waiting | State::Exited)
            && silence_s >= stall_minutes * 60
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
    fn watchdog_decision() {
        let due = |state, agent: &str, silence| {
            SessionPool::watchdog_due(state, agent, true, silence, 10, false)
        };
        assert!(due(State::Running, "claude", 600));
        assert!(due(State::Idle, "claude", 600), "idle counts as stalled-capable");
        assert!(!due(State::Waiting, "claude", 600), "waiting never stalls");
        assert!(!due(State::Exited, "claude", 600));
        assert!(!due(State::Running, "claude", 599), "below threshold");
        assert!(!due(State::Running, "shell", 6000), "shell exempt");
        assert!(
            !SessionPool::watchdog_due(State::Running, "claude", false, 600, 10, false),
            "dead session exempt"
        );
        assert!(
            !SessionPool::watchdog_due(State::Running, "claude", true, 600, 10, true),
            "only one event per stall episode"
        );
        assert!(
            !SessionPool::watchdog_due(State::Running, "claude", true, 600, 0, false),
            "stall_minutes=0 disables"
        );
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
            ntfy: None,
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
