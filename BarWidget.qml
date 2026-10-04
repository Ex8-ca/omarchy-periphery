// BarWidget.qml - the bar entry point. Shows a glyph + count of peripheried windows.
// Inspired by the structure of omarchy.clock's BarWidget + Panel pair (see
// https://plugins.omarchy.org/develop.html).

import QtQuick
import Quickshell
import qs.Ui

BarWidget {
  id: root
  moduleName: "io.github.ex8-ca.omarchy-periphery"

  readonly property bool opened: panelLoader.item
    ? panelLoader.item.opened === true
    : false

  readonly property var service: Service.find("io.github.ex8-ca.omarchy-periphery") || null

  function open()  { if (panelLoader.item) panelLoader.item.open() }
  function close() { if (panelLoader.item) panelLoader.item.close() }
  function toggle(){ if (panelLoader.item) panelLoader.item.toggle() }

  function injectPanel() {
    if (!panelLoader.item) return;
    panelLoader.item.bar = root.bar;
    panelLoader.item.anchorItem = button;
    panelLoader.item.hostWidget = root;
  }

  implicitWidth: button.implicitWidth
  implicitHeight: button.implicitHeight

  onBarChanged: injectPanel()

  Loader {
    id: panelLoader
    active: true
    source: Qt.resolvedUrl("Panel.qml")
    visible: false
    onLoaded: {
      root.injectPanel();
      Qt.callLater(root.injectPanel);
    }
  }

  WidgetButton {
    id: button
    anchors.fill: parent
    bar: root.bar
    // We render two glyphs: an empty "no windows" state and a filled state.
    text: (root.service && root.service.count > 0)
      ? "◐ " + root.service.count
      : "◌"
    tooltipText: root.service && root.service.count > 0
      ? `${root.service.count} peripheried window(s)`
      : "No peripheried windows"
    onPressed: function(buttonCode) {
      if (buttonCode === Qt.LeftButton) root.toggle();
    }
  }
}