// Service.qml - headless singleton that bridges the Omarchy shell to the periphery-daemon.
//
// The daemon runs as a long-lived user process (started by the systemd --user unit
// installed with the package). It listens to Hyprland's socket2.sock event stream
// and does the per-window geometry math. We just expose its current state.
//
// IPC: a single newline-delimited JSON stream over a Unix socket at
// $XDG_RUNTIME_DIR/omarchy-periphery/state.sock. Each message is one JSON object:
//   daemon -> shell: {"kind":"snapshot", "windows":[{"addr":"0xabc","size_frac":0.15,"title":"…","class":"…"}]}
//
// The shell is not allowed to run a second Quickshell instance for a plugin
// (see Omarchy plugin docs). This service lives inside the long-running
// omarchy-shell process and only opens the socket; no Qt timers tighter than 250ms.

import QtQuick
import Quickshell
import qs.Commons

Service {
  id: root
  moduleName: "io.github.ex8-ca.omarchy-periphery"

  // Public API used by BarWidget.qml / Panel.qml
  property var windows: []   // array of {addr, size_frac, title, class}
  property int count: 0
  readonly property bool daemonUp: socket.state === Socket.Connected

  // --- socket wiring -----------------------------------------------------
  Socket {
    id: socket
    path: Qt.application.arguments.indexOf("--periphery-sock") >= 0
      ? "" // overridden by caller for testing
      : `${Quickshell.env("XDG_RUNTIME_DIR")}/omarchy-periphery/state.sock`

    // We only consume; the daemon writes.
    onPacketReceived: function(packet) {
      try {
        const msg = JSON.parse(packet.toString());
        if (msg.kind === "snapshot") {
          root.windows = msg.windows || [];
          root.count = root.windows.length;
        }
      } catch (e) {
        Logger.warn("periphery: bad packet", packet, e);
      }
    }
  }

  // Poll the daemon once a second for the current state.
  // Hyprland events stream at hundreds of Hz; the shell only needs a coarse view.
  Timer {
    interval: 1000
    running: root.daemonUp
    repeat: true
    triggeredOnStart: true
    onTriggered: socket.send(JSON.stringify({kind: "query"}))
  }

  // Commands the panel can call
  function restoreWindow(addr) {
    socket.send(JSON.stringify({kind: "restore", addr: addr}));
  }

  function restoreAll() {
    socket.send(JSON.stringify({kind: "restore_all"}));
  }

  function dismiss(addr) {
    socket.send(JSON.stringify({kind: "dismiss", addr: addr}));
  }
}