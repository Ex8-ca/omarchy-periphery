// Panel.qml - opens when the bar widget is clicked. Shows a list of
// peripheried windows with a restore button and a "restore all" footer.

import QtQuick
import Quickshell
import qs.Commons
import qs.Ui

Panel {
  id: root
  moduleName: "io.github.ex8-ca.omarchy-periphery"
  manageIpc: false

  property var anchorItem: null
  property var hostWidget: null

  readonly property var service: Service.find("io.github.ex8-ca.omarchy-periphery") || null

  function open()  { root.controller.show() }
  function close() { root.controller.hide() }

  function switchPanel(direction) {
    if (root.bar && typeof root.bar.switchPanelFrom === "function")
      return root.bar.switchPanelFrom(root.hostWidget || root, direction);
    return false;
  }

  KeyboardPanel {
    id: panel
    anchorItem: root.anchorItem
    owner: root.hostWidget || root
    bar: root.bar
    open: root.opened
    focusTarget: keyCatcher
    contentWidth: panel.fittedContentWidth(Style.space(320))
    contentHeight: panel.fittedContentHeight(content.implicitHeight)

    PanelKeyCatcher {
      id: keyCatcher
      anchors.fill: parent
      onCloseRequested: root.close()
      onTabRequested: function(direction) { root.switchPanel(direction) }

      Column {
        id: content
        width: parent.width
        spacing: Style.space(8)

        // Header
        Text {
          width: parent.width
          text: root.service && root.service.daemonUp
            ? "Peripheral windows"
            : "Periphery daemon is not running"
          color: root.barForeground
          font.family: root.bar ? root.bar.fontFamily : Style.font.family
          font.pixelSize: Style.font.subtitle
          font.bold: true
          wrapMode: Text.WordWrap
        }

        // Empty state
        Text {
          width: parent.width
          visible: root.service && root.service.count === 0
          text: "Pull any window to a screen edge to shrink it. " +
                "Release at any size to leave it in your peripheral vision."
          color: root.barForeground
          opacity: 0.7
          font.family: root.bar ? root.bar.fontFamily : Style.font.family
          font.pixelSize: Style.font.body
          wrapMode: Text.WordWrap
        }

        // List of peripheried windows
        Repeater {
          model: root.service ? root.service.windows : []
          delegate: Row {
            width: parent.width
            spacing: Style.space(8)

            Column {
              width: parent.width - restoreBtn.width - Style.space(8)
              spacing: 2

              Text {
                width: parent.width
                text: modelData.title || modelData.class || modelData.addr
                color: root.barForeground
                font.family: root.bar ? root.bar.fontFamily : Style.font.family
                font.pixelSize: Style.font.body
                font.bold: true
                elide: Text.ElideRight
              }
              Text {
                width: parent.width
                text: Math.round(modelData.size_frac * 100) + "% of screen"
                color: root.barForeground
                opacity: 0.6
                font.family: root.bar ? root.bar.fontFamily : Style.font.family
                font.pixelSize: Style.font.caption
              }
            }

            WidgetButton {
              id: restoreBtn
              text: "Restore"
              onPressed: function() {
                if (root.service) root.service.restoreWindow(modelData.addr);
              }
            }
          }
        }

        // Footer: restore all
        WidgetButton {
          visible: root.service && root.service.count > 0
          width: parent.width
          text: "Restore all"
          onPressed: function() {
            if (root.service) root.service.restoreAll();
            root.close();
          }
        }
      }
    }
  }
}