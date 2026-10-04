#!/usr/bin/env bash
# install.sh - one-line installer for omarchy-periphery.
#
# Tested on a fresh Omarchy install (Arch + Hyprland + omarchy-shell).
# Idempotent: re-running re-installs everything to /usr/local.

set -euo pipefail

PLUGIN_ID="io.github.ex8-ca.omarchy-periphery"
PLUGIN_DIR_SYSTEM="/usr/share/omarchy-periphery"
PLUGIN_DIR_USER="${XDG_CONFIG_HOME:-$HOME/.config}/omarchy/plugins/${PLUGIN_ID}"
DAEMON_BIN="/usr/local/bin/periphery-daemon"
SYSTEMD_UNIT="/etc/systemd/user/periphery-daemon.service"
HYPR_SNIPPET_SRC="$(cd "$(dirname "$0")" && pwd)/hyprland/hyprland.conf.snippet"
HYPR_CONF="${XDG_CONFIG_HOME:-$HOME/.config}/hypr/hyprland.conf"

if [[ "$EUID" -eq 0 ]]; then
    SUDO=""
else
    SUDO="sudo"
fi

# 1. Install the Rust daemon.
echo "→ building periphery-daemon (release)…"
(cd "$(dirname "$0")/periphery-daemon" && cargo build --release --quiet)
echo "→ installing ${DAEMON_BIN}"
$SUDO install -m 0755 "$(dirname "$0")/periphery-daemon/target/release/periphery-daemon" "$DAEMON_BIN"

# 2. Install the systemd --user unit.
echo "→ installing systemd unit"
$SUDO tee "$SYSTEMD_UNIT" >/dev/null <<'UNIT'
[Unit]
Description=Omarchy Periphery daemon (Hyprland window shrinking)
After=graphical-session.target
PartOf=graphical-session.target

[Service]
Type=simple
ExecStart=/usr/local/bin/periphery-daemon
Restart=on-failure
RestartSec=2

[Install]
WantedBy=graphical-session.target
UNIT

# 3. Symlink the plugin into the user-plugins tree.
echo "→ symlinking plugin to ${PLUGIN_DIR_USER}"
mkdir -p "$(dirname "$PLUGIN_DIR_USER")"
if [[ ! -e "$PLUGIN_DIR_USER" ]]; then
    ln -s "$PWD" "$PLUGIN_DIR_USER"
fi

# 4. Append Hyprland snippet if not already present.
if [[ -f "$HYPR_CONF" ]] && ! grep -q "omarchy-periphery" "$HYPR_CONF"; then
    echo "→ appending Hyprland snippet to ${HYPR_CONF}"
    {
        echo ""
        echo "# >>> omarchy-periphery >>>"
        cat "$HYPR_SNIPPET_SRC"
        echo "# <<< omarchy-periphery <<<"
    } >> "$HYPR_CONF"
fi

# 5. Reload the user systemd + the Omarchy shell.
echo "→ enabling systemd unit"
systemctl --user daemon-reload
systemctl --user enable --now periphery-daemon.service
echo "→ rescan Omarchy shell"
omarchy-shell shell rescanPlugins || true
echo "→ enable bar widget"
omarchy plugin enable "$PLUGIN_ID" || true
omarchy bar move "$PLUGIN_ID" --section right || true

echo ""
echo "Periphery installed."
echo "  - Bar widget: right section, glyph ◐/◌"
echo "  - Try it: open any window, drag it past a screen edge, then keep pulling."
echo "  - Keybinds: SUPER+SHIFT+arrows nudge, SUPER+SHIFT+P toggle, SUPER+SHIFT+R restore."