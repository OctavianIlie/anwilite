use serde::Deserialize;
use serde_json::json;
use std::env;
use std::io::{self, BufRead, BufReader, Read, Write};
use std::net::Shutdown;
use std::os::unix::net::UnixStream;
use std::path::PathBuf;
use std::process::ExitCode;
use std::thread::sleep;
use std::time::Duration;

const SOCKET_FILE: &str = "anwilite-octlitch-0.sock";
// Workspaces 1-5 are always shown; 6-10 only when active or occupied.
const ALWAYS_SHOWN_WORKSPACES: usize = 5;
const TITLE_MAX_CHARS: usize = 80;

#[derive(Deserialize, Default)]
#[serde(default)]
struct State {
    #[serde(rename = "type")]
    kind: String,
    workspace: usize,
    mode: String,
    focused: Option<Focused>,
    workspaces: Vec<Workspace>,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct Focused {
    app_id: Option<String>,
    title: Option<String>,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct Workspace {
    id: usize,
    windows: usize,
    active: bool,
}

enum View {
    Workspace(usize),
    Mode,
    Title,
}

fn main() -> ExitCode {
    let args: Vec<String> = env::args().skip(1).collect();
    let args: Vec<&str> = args.iter().map(String::as_str).collect();

    match args.as_slice() {
        ["state"] => one_shot("state"),
        ["ping"] => one_shot("ping"),
        ["workspace", number] => one_shot(&format!("workspace {number}")),
        ["mode", mode] => one_shot(&format!("mode {mode}")),
        ["cycle-mode"] => cycle_mode(),
        ["waybar", "workspace", number] => match number.parse::<usize>() {
            Ok(number) if (1..=10).contains(&number) => waybar(View::Workspace(number)),
            _ => usage(),
        },
        ["waybar", "mode"] => waybar(View::Mode),
        ["waybar", "title"] => waybar(View::Title),
        _ => usage(),
    }
}

fn usage() -> ExitCode {
    eprintln!(
        "usage: anwilitectl state | ping | workspace <1-10> | mode <windowed|tiled|grid> | cycle-mode\n       anwilitectl waybar workspace <1-10> | waybar mode | waybar title"
    );
    ExitCode::from(2)
}

fn socket_path() -> Option<PathBuf> {
    if let Some(path) = env::var_os("ANWILITE_SOCKET") {
        return Some(PathBuf::from(path));
    }
    env::var_os("XDG_RUNTIME_DIR").map(|dir| PathBuf::from(dir).join(SOCKET_FILE))
}

/// Send one request, close our side, and read the whole reply.
fn request(line: &str) -> io::Result<String> {
    let path = socket_path().ok_or_else(|| io::Error::other("XDG_RUNTIME_DIR is not set"))?;
    let mut stream = UnixStream::connect(&path)?;
    stream.write_all(line.as_bytes())?;
    stream.write_all(b"\n")?;
    stream.shutdown(Shutdown::Write)?;
    let mut reply = String::new();
    stream.read_to_string(&mut reply)?;
    Ok(reply)
}

fn one_shot(line: &str) -> ExitCode {
    match request(line) {
        Ok(reply) => {
            print!("{reply}");
            if reply.contains(r#""ok":false"#) {
                ExitCode::FAILURE
            } else {
                ExitCode::SUCCESS
            }
        }
        Err(error) => {
            eprintln!("anwilitectl: cannot reach Anwilite: {error}");
            ExitCode::FAILURE
        }
    }
}

fn cycle_mode() -> ExitCode {
    let state = match request("state") {
        Ok(reply) => reply
            .lines()
            .find_map(|line| serde_json::from_str::<State>(line).ok())
            .filter(|state| state.kind == "state"),
        Err(error) => {
            eprintln!("anwilitectl: cannot reach Anwilite: {error}");
            return ExitCode::FAILURE;
        }
    };
    let Some(state) = state else {
        eprintln!("anwilitectl: unexpected reply from Anwilite");
        return ExitCode::FAILURE;
    };
    let next = match state.mode.as_str() {
        "windowed" => "tiled",
        "tiled" => "grid",
        _ => "windowed",
    };
    one_shot(&format!("mode {next}"))
}

/// Stream Waybar JSON for one module. Runs until Waybar closes our stdout.
fn waybar(view: View) -> ExitCode {
    let mut last = String::new();
    loop {
        if let Some(path) = socket_path()
            && let Ok(mut stream) = UnixStream::connect(&path)
            && stream.write_all(b"subscribe\n").is_ok()
        {
            for line in BufReader::new(stream).lines() {
                let Ok(line) = line else {
                    break;
                };
                let Ok(state) = serde_json::from_str::<State>(&line) else {
                    continue;
                };
                if state.kind != "state" {
                    continue;
                }
                emit(render(&view, &state), &mut last);
            }
        }
        // Anwilite is not running (or just restarted): hide, then retry.
        emit(hidden(), &mut last);
        sleep(Duration::from_secs(1));
    }
}

/// Print only when the module's content changed. If Waybar has gone away,
/// stdout is closed and there is nothing left to do.
fn emit(line: String, last: &mut String) {
    if *last == line {
        return;
    }
    let mut out = io::stdout().lock();
    if writeln!(out, "{line}").and_then(|_| out.flush()).is_err() {
        std::process::exit(0);
    }
    *last = line;
}

fn hidden() -> String {
    // Waybar hides a custom module whose text is empty.
    json!({ "text": "" }).to_string()
}

fn render(view: &View, state: &State) -> String {
    match view {
        View::Workspace(number) => render_workspace(*number, state),
        View::Mode => render_mode(state),
        View::Title => render_title(state),
    }
}

fn render_workspace(number: usize, state: &State) -> String {
    let entry = state.workspaces.iter().find(|ws| ws.id == number);
    let windows = entry.map_or(0, |ws| ws.windows);
    let active = state.workspace == number || entry.is_some_and(|ws| ws.active);

    if !active && windows == 0 && number > ALWAYS_SHOWN_WORKSPACES {
        return hidden();
    }

    let class = if active {
        "active"
    } else if windows > 0 {
        "occupied"
    } else {
        "empty"
    };
    let plural = if windows == 1 { "" } else { "s" };
    json!({
        "text": number.to_string(),
        "class": ["ws", class],
        "tooltip": format!("Workspace {number} — {windows} window{plural}"),
    })
    .to_string()
}

fn render_mode(state: &State) -> String {
    let label = match state.mode.as_str() {
        "tiled" => "Tiled",
        "grid" => "Grid",
        _ => "Windowed",
    };
    json!({
        "text": label,
        "class": ["mode", state.mode.as_str()],
        "tooltip": format!(
            "Workspace {} — {label}\nClick: Windowed → Tiled → Grid",
            state.workspace
        ),
    })
    .to_string()
}

fn render_title(state: &State) -> String {
    let Some(focused) = state.focused.as_ref() else {
        return hidden();
    };
    let Some(title) = focused
        .title
        .as_deref()
        .filter(|title| !title.is_empty())
        .or(focused.app_id.as_deref())
    else {
        return hidden();
    };

    let short: String = if title.chars().count() > TITLE_MAX_CHARS {
        let mut cut: String = title.chars().take(TITLE_MAX_CHARS - 1).collect();
        cut.push('…');
        cut
    } else {
        title.to_owned()
    };
    let app_id = focused.app_id.as_deref().unwrap_or("unknown app");

    json!({
        "text": pango_escape(&short),
        "class": ["title"],
        "tooltip": format!("{}\n{}", pango_escape(title), pango_escape(app_id)),
    })
    .to_string()
}

fn pango_escape(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for character in text.chars() {
        match character {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            other => out.push(other),
        }
    }
    out
}
