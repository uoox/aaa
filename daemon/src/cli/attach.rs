//! Attach the local terminal to a daemon-hosted PTY.
//!
//! This is what makes the new CLI different from the old script: the agent is
//! not a child of this terminal. Closing the window, losing the SSH link or
//! detaching leaves the session running in the daemon, and the phone is
//! looking at the very same PTY.

use std::io::Write;
use std::net::TcpStream;
use std::os::fd::AsRawFd;
use std::time::Duration;

use tungstenite::{Message, WebSocket};

use crate::client::{bracket, Client};
use crate::tty::{self, Raw, BOLD, DIM, RESET};

/// Ctrl-] — telnet's escape, and one no TUI agent binds.
const DETACH: u8 = 0x1d;

pub enum Outcome {
    /// user pressed Ctrl-]; the session keeps running
    Detached,
    /// the PTY ended (agent exited) or the daemon closed the socket
    Ended,
}

pub fn attach(c: &Client, id: &str, title: &str) -> Result<Outcome, String> {
    if !tty::is_tty() {
        return Err("attach 需要交互式终端".into());
    }
    let mut ws = connect(c, id)?;
    let fd = ws.get_ref().as_raw_fd();

    // Full raw: the remote PTY has already done its own output processing, so
    // a second pass here would mangle cursor motion.
    let raw = Raw::enter(false).ok_or("无法进入 raw 模式")?;
    tty::clear();
    print!(
        "{DIM}── {BOLD}{title}{RESET}{DIM} · Ctrl-] 脱离（会话继续运行）──{RESET}\r\n"
    );
    let _ = std::io::stdout().flush();

    let (mut cols, mut rows) = tty::size();
    send_resize(&mut ws, cols, rows);

    let outcome = pump(&mut ws, fd, &mut cols, &mut rows);
    let _ = ws.close(None);
    drop(raw);
    println!();
    Ok(outcome)
}

fn pump(ws: &mut WebSocket<TcpStream>, fd: i32, cols: &mut u16, rows: &mut u16) -> Outcome {
    let mut buf = [0u8; 4096];
    let mut out = std::io::stdout();
    loop {
        let (stdin_ready, sock_ready) = tty::poll(Some(fd), 50);

        // window resize: polling the size beats a SIGWINCH handler here —
        // it costs one ioctl per 50ms and needs no global signal state.
        let (c, r) = tty::size();
        if (c, r) != (*cols, *rows) {
            *cols = c;
            *rows = r;
            send_resize(ws, c, r);
        }

        if stdin_ready {
            let n = tty::read_stdin(&mut buf);
            if n == 0 {
                return Outcome::Detached;
            }
            if let Some(i) = buf[..n].iter().position(|b| *b == DETACH) {
                // forward whatever preceded the escape, then leave
                if i > 0 {
                    let _ = ws.write(Message::Binary(buf[..i].to_vec().into()));
                    let _ = ws.flush();
                }
                return Outcome::Detached;
            }
            if ws.write(Message::Binary(buf[..n].to_vec().into())).is_err() {
                return Outcome::Ended;
            }
            ignore_would_block(ws.flush());
        }

        if sock_ready {
            loop {
                match ws.read() {
                    Ok(Message::Binary(data)) => {
                        let _ = out.write_all(&data);
                    }
                    // the hello frame and any future control JSON: the
                    // terminal view has no use for it
                    Ok(Message::Text(_)) | Ok(Message::Ping(_)) | Ok(Message::Pong(_)) => {}
                    Ok(Message::Close(_)) | Ok(Message::Frame(_)) => return Outcome::Ended,
                    Err(tungstenite::Error::Io(e)) if e.kind() == std::io::ErrorKind::WouldBlock => break,
                    Err(_) => return Outcome::Ended,
                }
            }
            let _ = out.flush();
        }
    }
}

fn send_resize(ws: &mut WebSocket<TcpStream>, cols: u16, rows: u16) {
    let frame = format!(r#"{{"t":"resize","cols":{cols},"rows":{rows}}}"#);
    let _ = ws.write(Message::Text(frame.into()));
    ignore_would_block(ws.flush());
}

fn ignore_would_block(r: Result<(), tungstenite::Error>) {
    if let Err(tungstenite::Error::Io(e)) = &r {
        if e.kind() == std::io::ErrorKind::WouldBlock {
            return;
        }
    }
    let _ = r;
}

fn connect(c: &Client, id: &str) -> Result<WebSocket<TcpStream>, String> {
    use std::net::ToSocketAddrs;
    let authority = format!("{}:{}", bracket(&c.host), c.port);
    let addr = authority
        .to_socket_addrs()
        .map_err(|e| format!("解析 {authority} 失败: {e}"))?
        .next()
        .ok_or_else(|| format!("解析 {authority} 无结果"))?;
    let stream = TcpStream::connect_timeout(&addr, Duration::from_secs(if c.local { 2 } else { 6 }))
        .map_err(|e| format!("连接 {authority} 失败: {e}"))?;
    stream.set_nodelay(true).ok();

    let req = tungstenite::http::Request::builder()
        .method("GET")
        .uri(format!("ws://{authority}/api/v1/sessions/{id}/attach"))
        .header("Host", &authority)
        .header("Connection", "Upgrade")
        .header("Upgrade", "websocket")
        .header("Sec-WebSocket-Version", "13")
        .header("Sec-WebSocket-Key", tungstenite::handshake::client::generate_key())
        .header("Authorization", format!("Bearer {}", c.token))
        .body(())
        .map_err(|e| format!("构造握手请求失败: {e}"))?;

    let (ws, _) = tungstenite::client::client(req, stream)
        .map_err(|e| format!("WebSocket 握手失败: {e}"))?;
    // Non-blocking only after the handshake: the handshake itself wants a
    // plain blocking socket, and tungstenite keeps any bytes it over-read.
    ws.get_ref()
        .set_nonblocking(true)
        .map_err(|e| format!("set_nonblocking: {e}"))?;
    Ok(ws)
}
