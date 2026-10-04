//! periphery-daemon
//!
//! Listens to Hyprland's socket2.sock event stream and watches for windows being
//! dragged past the edge of a monitor. While dragging, the window's size is
//! continuously interpolated toward a discrete attractor (100/50/33/25/15/3% of
//! the monitor's shorter dimension). When the user releases the drag, the
//! window stays at that size ("peripheried" mode). The Omarchy shell plugin
//! (Service.qml) queries us over a Unix socket for the current list of
//! peripheried windows.
//!
//! Inspired by Scott Jenson's KDE Akademy 2026 talk "Are we really going to use
//! the same Desktop UX forever?".

mod attractors;
mod drag;
mod state;

use std::collections::HashMap;
use std::os::unix::net::UnixListener;
use std::path::PathBuf;
use std::sync::Arc;

use serde::{Deserialize, Serialize};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::{UnixListener as TokioUnixListener, UnixStream};
use tokio::sync::{mpsc, Mutex};
use tokio::time::{interval, Duration};

use state::WindowState;

/// Events from the Hyprland socket2 stream that we care about.
#[derive(Debug)]
enum HyprEvent {
    /// A window was opened. We need to start tracking its geometry.
    Opened { addr: String, ws: String, class: String, title: String },
    /// A window was moved (the user dragged it). We examine the new geometry
    /// to decide whether it's crossing a screen edge.
    Moved { addr: String },
    /// The active window changed. We use this to detect drag start/stop
    /// (a drag is a sequence of Moved events while the mouse is held).
    ActiveChanged(String),
    /// `mouse:251,318` — used to detect drag distance beyond the screen edge.
    MouseMoved { x: i32, y: i32 },
    /// A window was closed; drop it from state.
    Closed { addr: String },
    /// Any other event we don't handle. Ignored.
    Other,
}

/// A client (e.g. the Omarchy shell service) connected to our state socket.
#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "kind")]
enum ShellCommand {
    #[serde(rename = "query")]
    Query,
    #[serde(rename = "restore")]
    Restore { addr: String },
    #[serde(rename = "restore_all")]
    RestoreAll,
    #[serde(rename = "dismiss")]
    Dismiss { addr: String },
}

/// A snapshot we send back to the shell.
#[derive(Debug, Serialize)]
struct WindowSnapshot {
    addr: String,
    title: String,
    class: String,
    /// 1.0 = full screen, 0.03 = peripheried icon.
    size_frac: f32,
}

#[derive(Debug, Serialize)]
#[serde(tag = "kind")]
enum ShellMessage {
    #[serde(rename = "snapshot")]
    Snapshot { windows: Vec<WindowSnapshot> },
    #[serde(rename = "ack")]
    Ack,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "periphery_daemon=info".into()),
        )
        .init();

    let runtime_dir: PathBuf = std::env::var("XDG_RUNTIME_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from("/tmp"));
    let his = std::env::var("HYPRLAND_INSTANCE_SIGNATURE")
        .map_err(|_| anyhow::anyhow!("HYPRLAND_INSTANCE_SIGNATURE not set; this daemon must run under Hyprland"))?;

    let hypr_socket = runtime_dir.join(format!("hypr/{his}/.socket2.sock"));
    let state_socket = runtime_dir.join("omarchy-periphery/state.sock");
    if let Some(parent) = state_socket.parent() {
        tokio::fs::create_dir_all(parent).await?;
    }
    let _ = std::fs::remove_file(&state_socket); // best-effort stale cleanup

    // Channel from Hyprland events -> drag-detector task.
    let (tx, rx) = mpsc::channel::<HyprEvent>(512);

    // Per-window state. Shared across the Hyprland listener, the drag
    // detector, and the shell-facing socket.
    let state: Arc<Mutex<HashMap<String, WindowState>>> = Arc::new(Mutex::new(HashMap::new()));

    // Spawn the listener: Hyprland socket2 -> events.
    {
        let state_for_listener = Arc::clone(&state);
        tokio::spawn(async move {
            if let Err(e) = run_hyprland_listener(&hypr_socket, tx, state_for_listener).await {
                tracing::error!("hyprland listener exited: {e:?}");
            }
        });
    }

    // Spawn the drag detector + resize loop: consumes events, drives state.
    let _drag_handle = drag::spawn(rx, Arc::clone(&state));

    // Spawn the shell-facing socket.
    let state_socket_clone = state_socket.clone();
    {
        let state_for_shell = Arc::clone(&state);
        tokio::spawn(async move {
            if let Err(e) = run_shell_socket(&state_socket_clone, state_for_shell).await {
                tracing::error!("shell socket exited: {e:?}");
            }
        });
    }

    tracing::info!("periphery-daemon listening on {}", state_socket.display());

    // Keep the main task alive.
    let mut tick = interval(Duration::from_secs(3600));
    loop {
        tick.tick().await;
    }
}

/// Connect to Hyprland's socket2.sock and translate each event into our `HyprEvent`.
async fn run_hyprland_listener(
    path: &PathBuf,
    tx: mpsc::Sender<HyprEvent>,
    state: Arc<Mutex<HashMap<String, WindowState>>>,
) -> anyhow::Result<()> {
    let stream = UnixStream::connect(path).await?;
    let mut reader = BufReader::new(stream);
    let mut buf = String::new();

    loop {
        buf.clear();
        let n = reader.read_line(&mut buf).await?;
        if n == 0 {
            anyhow::bail!("hyprland socket2 closed");
        }
        let line = buf.trim_end();
        let parsed = parse_event(line, &state).await;
        if let Some(ev) = parsed {
            if tx.send(ev).await.is_err() {
                anyhow::bail!("event channel closed");
            }
        }
    }
}

/// Parse one `EVENT>>DATA` line from Hyprland.
async fn parse_event(
    line: &str,
    state: &Arc<Mutex<HashMap<String, WindowState>>>,
) -> Option<HyprEvent> {
    let mut split = line.splitn(2, ">>");
    let evt = split.next()?.trim();
    let data = split.next().unwrap_or("").trim();

    match evt {
        "openwindow" => {
            // addr,workspace,class,title
            let mut parts = data.splitn(4, ',');
            let addr = parts.next()?.to_string();
            let ws = parts.next()?.to_string();
            let class = parts.next()?.to_string();
            let title = parts.next()?.to_string();
            Some(HyprEvent::Opened { addr, ws, class, title })
        }
        "closewindow" | "kill" => {
            let addr = data.to_string();
            state.lock().await.remove(&addr);
            Some(HyprEvent::Closed { addr })
        }
        "movewindow" => {
            // addr,ws,class,title  (geometry doesn't change for tile; we listen for activewindowv2)
            let addr = data.split(',').next()?.to_string();
            Some(HyprEvent::Moved { addr })
        }
        "activewindowv2" => {
            // addr (empty if no window)
            let addr = data.to_string();
            if addr.is_empty() {
                None
            } else {
                Some(HyprEvent::ActiveChanged(addr))
            }
        }
        "mouseenter" => Some(HyprEvent::Other),
        "mouseposition" => {
            // x,y  (cursor in screen coords)
            let mut it = data.split(',');
            let x: i32 = it.next()?.parse().ok()?;
            let y: i32 = it.next()?.parse().ok()?;
            Some(HyprEvent::MouseMoved { x, y })
        }
        _ => Some(HyprEvent::Other),
    }
}

/// Accept connections on the shell-facing Unix socket. Each connection is a
/// line-delimited JSON RPC. The connection also receives a snapshot any time
/// the windows list changes.
async fn run_shell_socket(
    path: &PathBuf,
    state: Arc<Mutex<HashMap<String, WindowState>>>,
) -> anyhow::Result<()> {
    // Std listener is fine because we hand off to tokio on accept.
    let std_listener = UnixListener::bind(path)?;
    std_listener.set_nonblocking(true)?;
    let listener = TokioUnixListener::from_std(std_listener)?;
    loop {
        match listener.accept().await {
            Ok((stream, _)) => {
                let state = Arc::clone(&state);
                tokio::spawn(async move {
                    if let Err(e) = handle_shell_client(stream, state).await {
                        tracing::warn!("shell client error: {e:?}");
                    }
                });
            }
            Err(e) => {
                tracing::warn!("accept error: {e}");
            }
        }
    }
}

async fn handle_shell_client(
    mut stream: UnixStream,
    state: Arc<Mutex<HashMap<String, WindowState>>>,
) -> anyhow::Result<()> {
    let (read_half, mut write_half) = stream.split();
    let mut lines = BufReader::new(read_half).lines();

    // Send initial snapshot immediately.
    let snap = snapshot(&state).await;
    let _ = write_half.write_all(serde_json::to_string(&snap)?.as_bytes()).await;
    let _ = write_half.write_all(b"\n").await;

    while let Some(line) = lines.next_line().await? {
        let cmd: ShellCommand = match serde_json::from_str(&line) {
            Ok(c) => c,
            Err(e) => {
                tracing::warn!("bad shell command: {e}");
                continue;
            }
        };
        match cmd {
            ShellCommand::Query => {
                let snap = snapshot(&state).await;
                let _ = write_half.write_all(serde_json::to_string(&snap)?.as_bytes()).await;
                let _ = write_half.write_all(b"\n").await;
            }
            ShellCommand::Restore { addr } => {
                restore_window(&state, &addr).await;
                let _ = write_half.write_all(serde_json::to_string(&ShellMessage::Ack)?.as_bytes()).await;
                let _ = write_half.write_all(b"\n").await;
            }
            ShellCommand::RestoreAll => {
                let addrs: Vec<String> = state.lock().await
                    .iter()
                    .filter(|(_, w)| w.is_peripheried())
                    .map(|(a, _)| a.clone())
                    .collect();
                for a in addrs {
                    restore_window(&state, &a).await;
                }
                let _ = write_half.write_all(serde_json::to_string(&ShellMessage::Ack)?.as_bytes()).await;
                let _ = write_half.write_all(b"\n").await;
            }
            ShellCommand::Dismiss { addr } => {
                state.lock().await.remove(&addr);
            }
        }
    }
    Ok(())
}

async fn snapshot(state: &Arc<Mutex<HashMap<String, WindowState>>>) -> ShellMessage {
    let guard = state.lock().await;
    let mut out = Vec::with_capacity(guard.len());
    for (addr, w) in guard.iter() {
        if w.is_peripheried() {
            out.push(WindowSnapshot {
                addr: addr.clone(),
                title: w.title.clone(),
                class: w.class.clone(),
                size_frac: w.last_size_frac(),
            });
        }
    }
    ShellMessage::Snapshot { windows: out }
}

/// Restore a window to its pre-peripheried size & position via hyprctl.
async fn restore_window(state: &Arc<Mutex<HashMap<String, WindowState>>>, addr: &str) {
    let geom = state.lock().await.get(addr).and_then(|w| w.pre_peeph_geometry());
    if let Some(g) = geom {
        // hyprctl dispatch resizewindowpixel "exact" <addr> <w> <h> — then move
        let _ = run_hyprctl(&format!(
            "dispatch resizewindowpixel exact {addr} {} {}",
            g.w, g.h
        )).await;
        let _ = run_hyprctl(&format!(
            "dispatch movewindowpixel exact {addr} {} {}",
            g.x, g.y
        )).await;
        state.lock().await.get_mut(addr).map(|w| w.mark_restored());
    }
}

/// Spawn a one-shot `hyprctl` invocation. We could use the hyprctl socket
/// directly but `hyprctl` is universally available and already exits cleanly.
async fn run_hyprctl(args: &str) -> anyhow::Result<String> {
    let out = tokio::process::Command::new("hyprctl")
        .args(args.split_whitespace())
        .output()
        .await?;
    if !out.status.success() {
        anyhow::bail!("hyprctl {args} failed: {}", String::from_utf8_lossy(&out.stderr));
    }
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}

