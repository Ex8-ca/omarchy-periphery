# omarchy-periphery

> **Peripheral shrinking windows for [Omarchy](https://omarchy.org/).** Pull any window to the edge of the screen and it shrinks continuously — release it at a corner and it stays there as a peripheral glance: a video, a chat, a log. Pull it past a size threshold and it collapses further toward an icon. Inspired by [Scott Jenson's](https://jenson.org/) [KDE Akademy 2026 talk](https://media.ccc.de/v/kde2026-7-are_we_really_going_to_use_the_same_desktop_ux_forever), *"Are we really going to use the same Desktop UX forever?"*

## What it does

- Grab any window and drag it toward a screen edge.
- As you push past the edge, the window **shrinks continuously** — 100% → 50% → 33% → 25% → 15% → 3%.
- The further you pull, the smaller it gets. Let go at any size and it **stays there**, floating on top, fully interactive, but occupying peripheral vision.
- Below ~15% the chrome fades: it becomes a "peripheral glance" — perfect for a video call, a music visualizer, a tail of a build log.
- A bar widget shows how many windows are currently in periphery mode and a panel lets you recall any of them with one click.

Jenson's point is that desktop UX hasn't meaningfully changed in 20 years and tiling WMs are nibbling at the edges. This isn't tiling. The window lives wherever you put it, at whatever size you want, with no snapping zone or layout to fight against. It's a continuous surface, not a discrete grid.

## Install

```sh
# One line (assumes Omarchy + Hyprland already installed)
curl -fsSL https://raw.githubusercontent.com/Ex8-ca/omarchy-periphery/main/packaging/install.sh | bash
```

Or manually:

```sh
git clone https://github.com/Ex8-ca/omarchy-periphery ~/.config/omarchy/plugins/io.github.ex8-ca.omarchy-periphery
paru -S omarchy-periphery          # or use packaging/install.sh
omarchy-shell shell rescanPlugins
omarchy plugin enable io.github.ex8-ca.omarchy-periphery
omarchy bar move io.github.ex8-ca.omarchy-periphery --section right
```

## Components

| Component | What it is | Why |
|---|---|---|
| `periphery-daemon` | A small Rust binary | Hyprland event stream + continuous-resize loop. Rust keeps the WM hot path tight. |
| `manifest.json` + `*.qml` | An [Omarchy shell plugin](https://plugins.omarchy.org/develop.html) | Bar widget, panel, and headless service that talk to the daemon. |
| `hyprland/hyprland.conf.snippet` | Keybind + window rule snippet | Pulled into `~/.config/hypr/hyprland.conf` so the daemon knows about peripheried windows. |

## How it works

1. **`periphery-daemon`** subscribes to Hyprland's `socket2.sock` event stream.
2. Every time a window moves or resizes, it checks whether the window's *outer* edge crossed a monitor boundary.
3. If so, it animates the window size toward the nearest discrete attractor (`100/50/33/25/15/3%`) with a 60Hz loop until the user releases.
4. On release, it persists the geometry in a per-window tag (via `hyprctl dispatch setproperty ... tags`) so the bar widget knows about it.
5. The **Omarchy shell service** (`Service.qml`) connects to the daemon over a Unix socket and the **bar widget** (`BarWidget.qml`) shows the count. The **panel** (`Panel.qml`) lists them all.

## License

MIT. Concept attributed to Scott Jenson — see [his blog](https://jenson.org/) and the Akademy 2026 talk.