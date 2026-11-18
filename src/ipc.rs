use crate::window::DesktopMode;
use crate::workspace::WORKSPACE_COUNT;
use calloop::{Interest, LoopHandle, Mode, PostAction, RegistrationToken, generic::Generic};
use std::fmt::Write as _;
use std::io::{self, ErrorKind, Read, Write as _};
use std::os::fd::AsFd;
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::{env, fs};

const SOCKET_FILE: &str = "anwilite-octlitch-0.sock";
const MAX_CLIENTS: usize = 32;
const MAX_LINE: usize = 4096;
const MAX_PENDING_OUTPUT: usize = 256 * 1024;

/// Requests that change compositor state. `main.rs` executes them so they
/// follow exactly the same focus/redraw path as keyboard shortcuts.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IpcCommand {
    SwitchWorkspace(usize),
    SetMode(DesktopMode),
}

#[derive(Debug, PartialEq, Eq)]
enum Request {
    State,
    Subscribe,
    Ping,
    Command(IpcCommand),
}

/// Everything a status bar needs, gathered by `main.rs` once per loop pass.
pub struct Snapshot<'a> {
    pub workspace: usize,
    pub mode: DesktopMode,
    pub fullscreen: bool,
    pub focused_app_id: Option<&'a str>,
    pub focused_title: Option<&'a str>,
    pub window_counts: [usize; WORKSPACE_COUNT],
}

impl Snapshot<'_> {
    pub fn to_json(&self) -> String {
        let mut out = String::with_capacity(512);
        let _ = write!(
            out,
            r#"{{"type":"state","workspace":{},"mode":"{}","fullscreen":{},"focused":"#,
            self.workspace,
            mode_name(self.mode),
            self.fullscreen,
        );
        if self.focused_app_id.is_some() || self.focused_title.is_some() {
            out.push_str(r#"{"app_id":"#);
            push_json_optional(&mut out, self.focused_app_id);
            out.push_str(r#","title":"#);
            push_json_optional(&mut out, self.focused_title);
            out.push('}');
        } else {
            out.push_str("null");
        }
        out.push_str(r#","workspaces":["#);
        for (index, count) in self.window_counts.iter().enumerate() {
            if index > 0 {
                out.push(',');
            }
            let id = index + 1;
            let _ = write!(
                out,
                r#"{{"id":{id},"windows":{count},"active":{}}}"#,
                id == self.workspace
            );
        }
        out.push_str("]}");
        out
    }
}

pub fn mode_name(mode: DesktopMode) -> &'static str {
    match mode {
        DesktopMode::Floating => "windowed",
        DesktopMode::Tiled => "tiled",
        DesktopMode::Grid => "grid",
    }
}

fn parse_request(line: &str) -> Result<Request, String> {
    let mut words = line.split_whitespace();
    let Some(verb) = words.next() else {
        return Err("empty request".into());
    };
    let argument = words.next();
    if words.next().is_some() {
        return Err("too many arguments".into());
    }

    match (verb, argument) {
        ("state", None) => Ok(Request::State),
        ("subscribe", None) => Ok(Request::Subscribe),
        ("ping", None) => Ok(Request::Ping),
        ("workspace", Some(number)) => number
            .parse::<usize>()
            .ok()
            .filter(|number| (1..=WORKSPACE_COUNT).contains(number))
            .map(|number| Request::Command(IpcCommand::SwitchWorkspace(number)))
            .ok_or_else(|| format!("workspace must be 1-{WORKSPACE_COUNT}")),
        ("mode", Some(mode)) => match mode {
            "windowed" | "floating" => {
                Ok(Request::Command(IpcCommand::SetMode(DesktopMode::Floating)))
            }
            "tiled" => Ok(Request::Command(IpcCommand::SetMode(DesktopMode::Tiled))),
            "grid" => Ok(Request::Command(IpcCommand::SetMode(DesktopMode::Grid))),
            _ => Err("mode must be windowed, tiled or grid".into()),
        },
        ("state" | "subscribe" | "ping", Some(_)) => Err(format!("{verb} takes no argument")),
        ("workspace" | "mode", None) => Err(format!("{verb} needs an argument")),
        _ => Err(format!("unknown request: {verb}")),
    }
}

fn push_json_string(out: &mut String, value: &str) {
    out.push('"');
    for character in value.chars() {
        match character {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            control if (control as u32) < 0x20 => {
                let _ = write!(out, "\\u{:04x}", control as u32);
            }
            other => out.push(other),
        }
    }
    out.push('"');
}

fn push_json_optional(out: &mut String, value: Option<&str>) {
    match value {
        Some(value) => push_json_string(out, value),
        None => out.push_str("null"),
    }
}

fn reply_json(result: &Result<(), String>) -> String {
    match result {
        Ok(()) => r#"{"type":"reply","ok":true}"#.to_owned(),
        Err(error) => {
            let mut out = String::from(r#"{"type":"reply","ok":false,"error":"#);
            push_json_string(&mut out, error);
            out.push('}');
            out
        }
    }
}

struct Client {
    id: u64,
    stream: UnixStream,
    // Wakes the event loop when this client sends data or hangs up.
    token: Option<RegistrationToken>,
    input: Vec<u8>,
    output: Vec<u8>,
    subscribed: bool,
    wants_state: bool,
    // The peer closed its writing side (e.g. `echo state | ncat -U ...`).
    eof: bool,
    // Close once all queued replies are written.
    close_after_flush: bool,
    // I/O failed or the client fell too far behind: drop immediately.
    broken: bool,
}

impl Client {
    fn queue_line(&mut self, line: &str) {
        self.output.extend_from_slice(line.as_bytes());
        self.output.push(b'\n');
    }

    fn read_available(&mut self) {
        let mut buffer = [0u8; 1024];
        loop {
            match self.stream.read(&mut buffer) {
                Ok(0) => {
                    self.eof = true;
                    break;
                }
                Ok(count) => {
                    self.input.extend_from_slice(&buffer[..count]);
                    // Stop reading a flooding client; the rest waits.
                    if self.input.len() > MAX_LINE * 4 {
                        break;
                    }
                }
                Err(error) if error.kind() == ErrorKind::WouldBlock => break,
                Err(error) if error.kind() == ErrorKind::Interrupted => continue,
                Err(_) => {
                    self.broken = true;
                    break;
                }
            }
        }
    }

    fn take_lines(&mut self) -> Vec<String> {
        let mut lines = Vec::new();
        while let Some(end) = self.input.iter().position(|&byte| byte == b'\n') {
            let raw: Vec<u8> = self.input.drain(..=end).collect();
            lines.push(String::from_utf8_lossy(&raw[..end]).trim().to_owned());
        }
        // `printf state | ...` sends no trailing newline before EOF.
        if self.eof && !self.input.is_empty() {
            let raw = std::mem::take(&mut self.input);
            lines.push(String::from_utf8_lossy(&raw).trim().to_owned());
        }
        if self.input.len() > MAX_LINE {
            self.input.clear();
            self.queue_line(&reply_json(&Err("request line too long".into())));
            self.close_after_flush = true;
        }
        lines
    }

    fn flush(&mut self) {
        while !self.output.is_empty() {
            match self.stream.write(&self.output) {
                Ok(0) => {
                    self.broken = true;
                    break;
                }
                Ok(count) => {
                    self.output.drain(..count);
                }
                Err(error) if error.kind() == ErrorKind::WouldBlock => break,
                Err(error) if error.kind() == ErrorKind::Interrupted => continue,
                Err(_) => {
                    self.broken = true;
                    break;
                }
            }
        }
        if self.output.len() > MAX_PENDING_OUTPUT {
            eprintln!(
                "Anwilite IPC: client {} is not reading; disconnecting",
                self.id
            );
            self.broken = true;
        }
    }

    fn finished(&self) -> bool {
        self.broken
            || (self.output.is_empty()
                && (self.close_after_flush || (self.eof && !self.subscribed)))
    }
}

pub struct IpcServer {
    listener: Option<UnixListener>,
    path: Option<PathBuf>,
    clients: Vec<Client>,
    next_id: u64,
    last_snapshot: Option<String>,
}

impl IpcServer {
    /// Bind the socket and register it with the event loop. Failure only
    /// disables IPC; the compositor keeps running.
    pub fn new<D>(handle: &LoopHandle<'_, D>) -> Self {
        let mut server = Self {
            listener: None,
            path: None,
            clients: Vec::new(),
            next_id: 1,
            last_snapshot: None,
        };

        let Some(path) = env::var_os("XDG_RUNTIME_DIR")
            .map(PathBuf::from)
            .filter(|dir| dir.is_absolute())
            .map(|dir| dir.join(SOCKET_FILE))
        else {
            eprintln!("Anwilite IPC: XDG_RUNTIME_DIR missing; IPC disabled");
            return server;
        };

        let listener = match bind_socket(&path).and_then(|listener| {
            listener.set_nonblocking(true)?;
            Ok(listener)
        }) {
            Ok(listener) => listener,
            Err(error) => {
                eprintln!("Anwilite IPC: cannot bind {}: {error}", path.display());
                return server;
            }
        };

        if register_wakeup(handle, &listener).is_none() {
            let _ = fs::remove_file(&path);
            return server;
        }

        println!("Anwilite IPC: listening on {}", path.display());
        server.listener = Some(listener);
        server.path = Some(path);
        server
    }

    pub fn has_clients(&self) -> bool {
        !self.clients.is_empty()
    }

    /// Accept new clients and read requests. Returns the state-changing
    /// commands; answer each one with `reply`.
    pub fn poll<D>(&mut self, handle: &LoopHandle<'_, D>) -> Vec<(u64, IpcCommand)> {
        self.accept_new(handle);

        let mut commands = Vec::new();
        for client in &mut self.clients {
            if client.broken || client.eof {
                continue;
            }
            client.read_available();

            if client.eof
                && let Some(token) = client.token.take()
            {
                // A hung-up socket stays readable forever; stop waking on it.
                handle.remove(token);
            }

            for line in client.take_lines() {
                if line.is_empty() {
                    continue;
                }
                match parse_request(&line) {
                    Ok(Request::State) => client.wants_state = true,
                    Ok(Request::Subscribe) => {
                        client.subscribed = true;
                        client.wants_state = true;
                    }
                    Ok(Request::Ping) => client.queue_line(&reply_json(&Ok(()))),
                    Ok(Request::Command(command)) => commands.push((client.id, command)),
                    Err(error) => client.queue_line(&reply_json(&Err(error))),
                }
            }
        }
        commands
    }

    pub fn reply(&mut self, client_id: u64, result: Result<(), String>) {
        if let Some(client) = self
            .clients
            .iter_mut()
            .find(|client| client.id == client_id)
        {
            client.queue_line(&reply_json(&result));
        }
    }

    /// Answer `state` requests, notify subscribers if the snapshot changed,
    /// write pending output and drop finished clients.
    pub fn publish<D>(&mut self, handle: &LoopHandle<'_, D>, snapshot: &str) {
        let changed = self.last_snapshot.as_deref() != Some(snapshot);

        for client in &mut self.clients {
            if client.wants_state || (client.subscribed && changed) {
                client.queue_line(snapshot);
                client.wants_state = false;
            }
            client.flush();
        }

        if changed {
            self.last_snapshot = Some(snapshot.to_owned());
        }

        let mut index = 0;
        while index < self.clients.len() {
            if self.clients[index].finished() {
                let mut client = self.clients.swap_remove(index);
                if let Some(token) = client.token.take() {
                    handle.remove(token);
                }
            } else {
                index += 1;
            }
        }
    }

    fn accept_new<D>(&mut self, handle: &LoopHandle<'_, D>) {
        let Some(listener) = self.listener.as_ref() else {
            return;
        };

        loop {
            let stream = match listener.accept() {
                Ok((stream, _)) => stream,
                Err(error) if error.kind() == ErrorKind::WouldBlock => break,
                Err(error) if error.kind() == ErrorKind::Interrupted => continue,
                Err(error) => {
                    eprintln!("Anwilite IPC: accept failed: {error}");
                    break;
                }
            };

            if self.clients.len() >= MAX_CLIENTS {
                // Best effort: tell the client why, then drop it.
                let _ = stream.set_nonblocking(true);
                let _ = (&stream).write_all(
                    format!("{}\n", reply_json(&Err("too many IPC clients".into()))).as_bytes(),
                );
                continue;
            }

            if stream.set_nonblocking(true).is_err() {
                continue;
            }
            let Some(token) = register_wakeup(handle, &stream) else {
                continue;
            };

            let id = self.next_id;
            self.next_id += 1;
            self.clients.push(Client {
                id,
                stream,
                token: Some(token),
                input: Vec::new(),
                output: Vec::new(),
                subscribed: false,
                wants_state: false,
                eof: false,
                close_after_flush: false,
                broken: false,
            });
        }
    }
}

impl Drop for IpcServer {
    fn drop(&mut self) {
        if let Some(path) = &self.path {
            let _ = fs::remove_file(path);
        }
    }
}

/// Bind, replacing a stale socket file left by a crashed instance, but never
/// one that a running instance still answers on.
fn bind_socket(path: &Path) -> io::Result<UnixListener> {
    match UnixListener::bind(path) {
        Ok(listener) => Ok(listener),
        Err(error) if error.kind() == ErrorKind::AddrInUse => {
            if UnixStream::connect(path).is_ok() {
                return Err(io::Error::new(
                    ErrorKind::AddrInUse,
                    "another Anwilite instance owns this IPC socket",
                ));
            }
            fs::remove_file(path)?;
            UnixListener::bind(path)
        }
        Err(error) => Err(error),
    }
}

/// Register a duplicate of `socket`'s fd so readiness wakes the event loop.
/// The actual reading happens in the main loop, with full compositor state.
fn register_wakeup<D>(handle: &LoopHandle<'_, D>, socket: &impl AsFd) -> Option<RegistrationToken> {
    let fd = match socket.as_fd().try_clone_to_owned() {
        Ok(fd) => fd,
        Err(error) => {
            eprintln!("Anwilite IPC: cannot duplicate socket fd: {error}");
            return None;
        }
    };
    match handle.insert_source(Generic::new(fd, Interest::READ, Mode::Level), |_, _, _| {
        Ok(PostAction::Continue)
    }) {
        Ok(token) => Some(token),
        Err(error) => {
            eprintln!("Anwilite IPC: cannot register socket: {}", error.error);
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_queries_and_commands() {
        assert_eq!(parse_request("state"), Ok(Request::State));
        assert_eq!(parse_request("  subscribe  "), Ok(Request::Subscribe));
        assert_eq!(parse_request("ping"), Ok(Request::Ping));
        assert_eq!(
            parse_request("workspace 3"),
            Ok(Request::Command(IpcCommand::SwitchWorkspace(3)))
        );
        assert_eq!(
            parse_request("workspace 10"),
            Ok(Request::Command(IpcCommand::SwitchWorkspace(10)))
        );
        assert_eq!(
            parse_request("mode grid"),
            Ok(Request::Command(IpcCommand::SetMode(DesktopMode::Grid)))
        );
        assert_eq!(
            parse_request("mode windowed"),
            Ok(Request::Command(IpcCommand::SetMode(DesktopMode::Floating)))
        );
    }

    #[test]
    fn rejects_bad_requests() {
        for bad in [
            "workspace 0",
            "workspace 11",
            "workspace x",
            "workspace",
            "mode",
            "mode stacked",
            "state now",
            "workspace 1 2",
            "reboot",
        ] {
            assert!(parse_request(bad).is_err(), "{bad} should be rejected");
        }
    }

    #[test]
    fn escapes_json_strings() {
        let mut out = String::new();
        push_json_string(&mut out, "a \"b\" \\ c\nd\u{1}");
        assert_eq!(out, r#""a \"b\" \\ c\nd\u0001""#);
    }

    #[test]
    fn snapshot_is_one_json_line() {
        let mut counts = [0; WORKSPACE_COUNT];
        counts[0] = 2;
        let json = Snapshot {
            workspace: 1,
            mode: DesktopMode::Tiled,
            fullscreen: false,
            focused_app_id: Some("org.kde.kate"),
            focused_title: Some("notes \"draft\""),
            window_counts: counts,
        }
        .to_json();
        assert!(json.starts_with(r#"{"type":"state","workspace":1,"mode":"tiled""#));
        assert!(json.contains(r#""focused":{"app_id":"org.kde.kate","title":"notes \"draft\""}"#));
        assert!(json.contains(r#"{"id":1,"windows":2,"active":true}"#));
        assert!(json.contains(r#"{"id":10,"windows":0,"active":false}"#));
        assert!(json.ends_with("]}"));
        assert_eq!(json.lines().count(), 1);
    }

    #[test]
    fn snapshot_without_focus_uses_null() {
        let json = Snapshot {
            workspace: 4,
            mode: DesktopMode::Floating,
            fullscreen: true,
            focused_app_id: None,
            focused_title: None,
            window_counts: [0; WORKSPACE_COUNT],
        }
        .to_json();
        assert!(json.contains(r#""mode":"windowed","fullscreen":true,"focused":null"#));
    }

    #[test]
    fn replies_are_tagged() {
        assert_eq!(reply_json(&Ok(())), r#"{"type":"reply","ok":true}"#);
        assert_eq!(
            reply_json(&Err("no".into())),
            r#"{"type":"reply","ok":false,"error":"no"}"#
        );
    }
}
