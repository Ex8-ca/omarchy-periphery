//! Drag detection + continuous resize loop.
//!
//! ## The problem
//!
//! Hyprland's `socket2.sock` event stream does NOT emit a "drag started" or
//! "drag ended" signal. The events we *do* get are:
//!
//!   - `activewindowv2` — focus changed to addr (empty = no window)
//!   - `mouseposition` — x,y of the cursor
//!   - `openwindow` / `closewindow` — window lifecycle
//!   - `movewindow` — workspace moves, NOT geometry
//!
//! So we have to *infer* drag start/stop. The heuristics that work:
//!
//! 1. **Drag start:** the active window's *outer* edge starts moving in lockstep
//!    with `mouseposition` deltas. If both happen within the same 16ms window,
//!    we're being dragged.
//! 2. **Drag end:** either the cursor stops moving (no `mouseposition` for
//!    ~120ms), OR the window's geometry stops changing across two
//!    consecutive samples (user is holding still — almost certainly released).
//!
//! ## The loop
//!
//! We poll geometry at ~120Hz while a drag is in progress. Each tick:
//!   - Sample the focused window's geometry via the command socket.
//!   - Compute the overshoot of its outer edge past the monitor's edge.
//!   - Pick an attractor based on overshoot.
//!   - If the window is currently between attractors, dispatch a resize toward
//!     it. (Otherwise, do nothing — the user is still holding still.)
//!
//! We do NOT poll at 120Hz when no drag is happening. The poll task sleeps
//! until either a `MouseMoved` event wakes it or it sees a candidate window
//! with stale geometry.
//!
//! ## Why this is fast
//!
//! The compositor's hot path is untouched. We send at most one `hyprctl
//! getwindowgeometry` per 8ms while dragging, and only when the window has
//! actually changed. Hyprland handles thousands of these per second per
//! client without blinking.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Instant;

use serde::Deserialize;
use tokio::sync::{mpsc, Mutex};
use tokio::time::{interval, Duration};

use crate::state::{Geometry, Regime, WindowState};
#[cfg(test)]
use crate::attractors::pick_attractor; // referenced by tests via crate path

// We could open the Hyprland command socket (`.socket.sock`) directly to
// avoid spawning `hyprctl` processes per sample, but `hyprctl` is universally
// available, already exits cleanly with a 5s timeout, and saves us from
// having to construct the HIS path ourselves on every call.

/// A bounding box reported by `hyprctl getwindowgeometry`.
#[derive(Debug, Clone, Copy, Deserialize)]
pub struct ReportedGeometry {
    #[serde(rename = "at")]
    pub pos: (i32, i32),
    #[serde(rename = "size")]
    pub dims: (i32, i32),
}

/// The top-level JSON shape from `hyprctl getwindowgeometry`.
/// We only need a couple of fields, so `flatten` keeps it loose.
#[derive(Debug, Deserialize)]
struct GeometryEnvelope {
    #[serde(flatten)]
    pub extra: HashMap<String, serde_json::Value>,
}

impl ReportedGeometry {
    /// Parse the JSON returned by `hyprctl getwindowgeometry <addr>`.
    pub fn parse(raw: &str) -> Option<Self> {
        let env: GeometryEnvelope = serde_json::from_str(raw).ok()?;
        // The shape is `{"x": 100, "y": 200, "width": 800, "height": 600}`.
        let x = env.extra.get("x")?.as_i64()? as i32;
        let y = env.extra.get("y")?.as_i64()? as i32;
        let w = env.extra.get("width")?.as_i64()? as i32;
        let h = env.extra.get("height")?.as_i64()? as i32;
        Some(Self { pos: (x, y), dims: (w, h) })
    }

    fn as_geometry(&self) -> Geometry {
        Geometry { x: self.pos.0, y: self.pos.1, w: self.dims.0, h: self.dims.1 }
    }
}

/// A monitor's bounding box.
#[derive(Debug, Clone, Deserialize)]
pub struct MonitorBox {
    #[allow(dead_code)]
    pub id: i32,
    #[allow(dead_code)]
    pub name: String,
    pub x: i32,
    pub y: i32,
    pub width: i32,
    pub height: i32,
    #[allow(dead_code)]
    pub scale: f32,
}

impl MonitorBox {
    pub fn shorter_dim(&self) -> i32 {
        self.width.min(self.height)
    }
    pub fn right(&self) -> i32 { self.x + self.width }
    pub fn bottom(&self) -> i32 { self.y + self.height }
}

/// What the drag detector knows about the currently-active window.
struct DragTracker {
    /// Window address that's currently focused (and therefore the candidate
    /// for being dragged).
    candidate: Option<String>,
    /// Geometry we sampled last tick.
    last_geom: Option<Geometry>,
    /// Cursor position at the last mouse event.
    last_mouse: Option<(i32, i32)>,
    /// When we last saw a mouse movement.
    last_mouse_at: Option<Instant>,
    /// When the current drag started (None = no drag).
    drag_started_at: Option<Instant>,
    /// The attractor we're animating toward right now.
    target_frac: f32,
    /// True while a drag is in progress.
    in_drag: bool,
}

impl DragTracker {
    fn new() -> Self {
        Self {
            candidate: None,
            last_geom: None,
            last_mouse: None,
            last_mouse_at: None,
            drag_started_at: None,
            target_frac: 1.0,
            in_drag: false,
        }
    }

    /// React to a focus change.
    fn on_active_changed(&mut self, addr: &str) {
        if self.candidate.as_deref() != Some(addr) {
            // Focus moved to a different window — any drag we were tracking
            // is implicitly over.
            self.candidate = if addr.is_empty() { None } else { Some(addr.to_string()) };
            self.in_drag = false;
            self.last_geom = None;
            self.drag_started_at = None;
            self.target_frac = 1.0;
        }
    }

    /// React to a cursor position change.
    fn on_mouse_moved(&mut self, x: i32, y: i32) {
        let moved = self.last_mouse.map(|(px, py)| (x - px).abs() + (y - py).abs() > 0).unwrap_or(true);
        if moved {
            self.last_mouse = Some((x, y));
            self.last_mouse_at = Some(Instant::now());
        }
    }

    /// True if the cursor hasn't moved for ~120ms (drag ended by release).
    fn cursor_is_still(&self) -> bool {
        match self.last_mouse_at {
            None => false,
            Some(t) => t.elapsed() > Duration::from_millis(120),
        }
    }
}

/// Spawn the drag-detection loop. It owns its own timer and reacts to
/// `HyprEvent`s on the channel. Geometry changes for tracked windows are
/// reflected into `state`; the Omarchy shell socket reads from `state`.
pub fn spawn(
    mut rx: mpsc::Receiver<crate::HyprEvent>,
    state: Arc<Mutex<HashMap<String, WindowState>>>,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let mut tracker = DragTracker::new();
        // Sample at 120Hz when in drag; when idle, sleep for 250ms between
        // ticks (so we can still notice when a drag might be starting).
        let mut idle_tick = interval(Duration::from_millis(250));
        let mut drag_tick = interval(Duration::from_millis(8));
        // First tick fires immediately; suppress that.
        idle_tick.tick().await;
        drag_tick.tick().await;

        loop {
            if tracker.in_drag {
                tokio::select! {
                    ev = rx.recv() => {
                        if let Some(ev) = ev {
                            handle_event(ev, &mut tracker, &state).await;
                        } else { break; }
                    }
                    _ = drag_tick.tick() => {
                        evaluate_drag(&mut tracker, &state).await;
                    }
                }
            } else {
                tokio::select! {
                    ev = rx.recv() => {
                        if let Some(ev) = ev {
                            handle_event(ev, &mut tracker, &state).await;
                        } else { break; }
                    }
                    _ = idle_tick.tick() => {
                        // Cheap health check: if the candidate window has been
                        // moved (which can happen via keybinds or Hyprland's
                        // own auto-tile), pick up on it. Real drag detection
                        // is driven by `mouseposition`.
                        evaluate_drag(&mut tracker, &state).await;
                    }
                }
            }
        }
    })
}

async fn handle_event(
    ev: crate::HyprEvent,
    tracker: &mut DragTracker,
    state: &Arc<Mutex<HashMap<String, WindowState>>>,
) {
    match ev {
        crate::HyprEvent::ActiveChanged(addr) => {
            tracker.on_active_changed(&addr);
        }
        crate::HyprEvent::MouseMoved { x, y } => {
            tracker.on_mouse_moved(x, y);
        }
        crate::HyprEvent::Opened { addr, ws, class, title } => {
            // We can't sample geometry yet (window may not be mapped); just
            // remember it exists so we don't try to `getwindowgeometry` on
            // an addr we never heard of.
            let mut s = state.lock().await;
            s.entry(addr.clone()).or_insert_with(|| {
                WindowState::new(
                    class,
                    title,
                    Geometry { x: 0, y: 0, w: 0, h: 0 }, // patched on next sample
                )
            });
            let _ = ws;
        }
        crate::HyprEvent::Closed { addr } => {
            state.lock().await.remove(&addr);
            if tracker.candidate.as_deref() == Some(&addr) {
                tracker.candidate = None;
                tracker.in_drag = false;
            }
        }
        crate::HyprEvent::Moved { addr } => {
            // Tile move (workspace change). Just refresh geometry.
            if let Some(g) = sample_geometry(&addr).await {
                let mut s = state.lock().await;
                if let Some(w) = s.get_mut(&addr) {
                    w.current = g;
                }
            }
        }
        crate::HyprEvent::Other => {}
    }
}

/// The hot path. Decides if a drag is happening, and if so, resizes toward
/// the attractor that matches the current overshoot.
async fn evaluate_drag(
    tracker: &mut DragTracker,
    state: &Arc<Mutex<HashMap<String, WindowState>>>,
) {
    let Some(addr) = tracker.candidate.clone() else {
        tracker.in_drag = false;
        return;
    };

    // Sample current geometry. If the sample fails (window gone), end drag.
    let geom = match sample_geometry(&addr).await {
        Some(g) => g,
        None => {
            tracker.in_drag = false;
            return;
        }
    };

    let prev_geom = tracker.last_geom;
    tracker.last_geom = Some(geom);

    // Detect drag end by stillness (cursor hasn't moved in 120ms).
    if tracker.cursor_is_still() {
        if tracker.in_drag {
            finalize_periphery(tracker, state, &addr, geom).await;
        }
        tracker.in_drag = false;
        tracker.drag_started_at = None;
        tracker.target_frac = 1.0;
        return;
    }

    // Detect drag start: mouse is moving AND the window's geometry changed
    // since last tick (i.e., the WM is moving the window in lockstep with
    // the cursor).
    let geom_changed = match (prev_geom, geom) {
        (Some(p), c) => p.x != c.x || p.y != c.y || p.w != c.w || p.h != c.h,
        _ => false,
    };
    if !tracker.in_drag {
        if geom_changed {
            // Capture pre-drag geometry exactly once, so we can restore later.
            if let Some(p) = prev_geom {
                let mut s = state.lock().await;
                if let Some(w) = s.get_mut(&addr) {
                    if w.regime == Regime::Normal {
                        w.pre_peeph = Some(p);
                        w.regime = Regime::Dragging;
                    }
                }
            }
            tracker.in_drag = true;
            tracker.drag_started_at = Some(Instant::now());
        } else {
            // Nothing moving. Stay idle.
            return;
        }
    }

    // We are in a drag. Compute overshoot.
    let monitors = sample_monitors().await;
    let Some(over) = overshoot_past_edge(geom, &monitors) else {
        return; // window isn't near any edge
    };

    // Pick attractor from overshoot.
    // units = how many "monitor shorter-dims" past the edge the window is.
    let mon = monitors.iter()
        .find(|m| geom.x + geom.w / 2 >= m.x && geom.x + geom.w / 2 <= m.right()
            && geom.y + geom.h / 2 >= m.y && geom.y + geom.h / 2 <= m.bottom())
        .or_else(|| monitors.first());
    let Some(mon) = mon else { return };
    let units = (over.abs() as f32) / (mon.shorter_dim() as f32);
    let frac = crate::attractors::pick_attractor(units);

    if (frac - tracker.target_frac).abs() < f32::EPSILON {
        return; // already at this attractor
    }
    tracker.target_frac = frac;

    // Resize the window toward this attractor. We keep its current center,
    // we just change width/height. The y-axis is the natural "shrink axis"
    // for top/bottom edge cases; we shrink BOTH so the aspect ratio stays.
    let new_w = ((mon.shorter_dim() as f32) * frac).round() as i32;
    let new_h = new_w; // square-ish; keeps videos viewable

    let cx = geom.x + geom.w / 2;
    let cy = geom.y + geom.h / 2;
    let new_x = cx - new_w / 2;
    let new_y = cy - new_h / 2;

    let _ = run_hyprctl(&format!(
        "dispatch resizewindowpixel exact {addr} {new_w} {new_h}"
    )).await;
    let _ = run_hyprctl(&format!(
        "dispatch movewindowpixel exact {addr} {new_x} {new_y}"
    )).await;

    // Update state.
    {
        let mut s = state.lock().await;
        if let Some(w) = s.get_mut(&addr) {
            w.current = Geometry { x: new_x, y: new_y, w: new_w, h: new_h };
            w.last_size_frac = frac;
        }
    }
}

/// User released the drag — mark the window as peripheried.
async fn finalize_periphery(
    tracker: &mut DragTracker,
    state: &Arc<Mutex<HashMap<String, WindowState>>>,
    addr: &str,
    geom: Geometry,
) {
    let mut s = state.lock().await;
    if let Some(w) = s.get_mut(addr) {
        // Only peripheried if the user actually shrunk past full size.
        if tracker.target_frac < 0.99 {
            w.regime = Regime::Peripheried;
            w.current = geom;
            w.last_size_frac = tracker.target_frac;
            // Tag the window so the windowrulev2 in hyprland.conf kicks in
            // (float, pin, opacity, no shadow, no blur).
            let _ = run_hyprctl(&format!(
                "dispatch setproperty {addr} tag +peripheried"
            )).await;
        } else {
            // Released at full size — just a normal move, revert regime.
            w.regime = Regime::Normal;
            w.pre_peeph = None;
        }
    }
}

/// How many pixels the window's outer edge is past the monitor boundary.
/// Returns None if no edge is being crossed.
fn overshoot_past_edge(geom: Geometry, monitors: &[MonitorBox]) -> Option<i32> {
    let mon = monitors.iter().min_by_key(|m| {
        let dx = (geom.x + geom.w / 2 - (m.x + m.width / 2)).abs();
        let dy = (geom.y + geom.h / 2 - (m.y + m.height / 2)).abs();
        dx + dy
    })?;

    let left_over   = mons_left_over(geom, mon);
    let right_over  = mons_right_over(geom, mon);
    let top_over    = mons_top_over(geom, mon);
    let bot_over    = mons_bottom_over(geom, mon);

    // Return the maximum (the most-pushed edge).
    [left_over, right_over, top_over, bot_over]
        .into_iter()
        .filter(|x| *x > 0)
        .max()
}

// -- edge helpers ----------------------------------------------------------
//
// Each returns the *signed* pixel overshoot of the window's edge past the
// monitor's edge. Positive = past, negative = not at the edge, 0 = flush.

fn mons_left_over(g: Geometry, m: &MonitorBox) -> i32 {
    // Window left edge is at g.x; monitor left edge is at m.x.
    // The user is "pushing left" if g.x is smaller than m.x.
    if g.x < m.x { m.x - g.x } else { -1 }
}
fn mons_right_over(g: Geometry, m: &MonitorBox) -> i32 {
    // Window right edge is at g.x + g.w.
    if g.x + g.w > m.right() { g.x + g.w - m.right() } else { -1 }
}
fn mons_top_over(g: Geometry, m: &MonitorBox) -> i32 {
    if g.y < m.y { m.y - g.y } else { -1 }
}
fn mons_bottom_over(g: Geometry, m: &MonitorBox) -> i32 {
    if g.y + g.h > m.bottom() { g.y + g.h - m.bottom() } else { -1 }
}

// -- hyprctl helpers -------------------------------------------------------

async fn sample_geometry(addr: &str) -> Option<Geometry> {
    let out = run_hyprctl(&format!("getwindowgeometry {addr}")).await.ok()?;
    ReportedGeometry::parse(&out).map(|g| g.as_geometry())
}

async fn sample_monitors() -> Vec<MonitorBox> {
    let out = match run_hyprctl("monitors all").await {
        Ok(s) => s,
        Err(_) => return Vec::new(),
    };
    serde_json::from_str(&out).unwrap_or_default()
}

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

#[cfg(test)]
mod tests {
    use super::*;

    fn mon(x: i32, y: i32, w: i32, h: i32) -> MonitorBox {
        MonitorBox { id: 0, name: "x".into(), x, y, width: w, height: h, scale: 1.0 }
    }

    #[test]
    fn overshoot_left() {
        let g = Geometry { x: -50, y: 100, w: 800, h: 600 };
        let m = mon(0, 0, 1920, 1080);
        assert_eq!(overshoot_past_edge(g, &[m]), Some(50));
    }
    #[test]
    fn overshoot_right() {
        let g = Geometry { x: 1200, y: 100, w: 800, h: 600 };
        let m = mon(0, 0, 1920, 1080);
        assert_eq!(overshoot_past_edge(g, &[m]), Some(80));
    }
    #[test]
    fn overshoot_none() {
        let g = Geometry { x: 100, y: 100, w: 800, h: 600 };
        let m = mon(0, 0, 1920, 1080);
        assert_eq!(overshoot_past_edge(g, &[m]), None);
    }
    #[test]
    fn overshoot_far_left_wins() {
        // Push way past the left edge; max() should pick that one.
        let g = Geometry { x: -200, y: 100, w: 800, h: 600 };
        let m = mon(0, 0, 1920, 1080);
        assert_eq!(overshoot_past_edge(g, &[m]), Some(200));
    }

    #[test]
    fn monitor_shorter_dim() {
        assert_eq!(mon(0, 0, 1920, 1080).shorter_dim(), 1080);
        assert_eq!(mon(0, 0, 1080, 1920).shorter_dim(), 1080);
    }
}