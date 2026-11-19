import QtQuick
import QtQuick.Shapes
import org.kde.kirigami as Kirigami

// A compact chip rather than a tall badge: at the top of the screen it has to clear the stock
// clock, whose top edge sits about 70 px down on a 768-line or scaled display.
Rectangle {
    id: indicator
    property bool scanning: false
    property string style: "landmarks"
    // A value this build does not draw -- a typo, a config from a newer release -- gets the default.
    readonly property string look: ["viewfinder", "head", "pulse"].includes(style) ? style : "landmarks"
    readonly property color ink: Kirigami.Theme.textColor
    readonly property color accent: "#ff3355"
    readonly property string bracketPath: "M2 11V2h9M25 2h9v9M34 25v9h-9M11 34H2v-9"
    property real phase: 0

    visible: scanning
    width: row.implicitWidth + 20
    height: 44
    radius: height / 2
    color: Qt.rgba(Kirigami.Theme.backgroundColor.r, Kirigami.Theme.backgroundColor.g,
                   Kirigami.Theme.backgroundColor.b, 0.6)
    border.color: Qt.rgba(ink.r, ink.g, ink.b, 0.15)
    Accessible.role: Accessible.Animation
    Accessible.name: qsTr("Scanning for your face")

    NumberAnimation on phase {
        running: indicator.scanning
        loops: Animation.Infinite
        from: 0
        to: 1
        duration: 1500
    }

    component Strokes: Shape {
        property alias path: svg.path
        property alias color: line.strokeColor
        property alias lineWidth: line.strokeWidth
        anchors.fill: parent
        preferredRendererType: Shape.CurveRenderer
        ShapePath {
            id: line
            strokeWidth: 2
            fillColor: "transparent"
            capStyle: ShapePath.RoundCap
            joinStyle: ShapePath.RoundJoin
            PathSvg { id: svg }
        }
    }

    component Sweep: Rectangle {
        id: sweep
        property bool running
        x: 7
        width: 22
        height: 2
        radius: 1
        Rectangle {
            anchors.centerIn: parent
            width: parent.width
            height: 8
            color: parent.color
            opacity: 0.18
        }
        SequentialAnimation on y {
            running: sweep.running
            loops: Animation.Infinite
            NumberAnimation { from: 6; to: 28; duration: 950; easing.type: Easing.InOutSine }
            NumberAnimation { from: 28; to: 6; duration: 950; easing.type: Easing.InOutSine }
        }
    }

    Row {
        id: row
        x: 4
        anchors.verticalCenter: parent.verticalCenter
        spacing: 8

        Loader {
            width: 36
            height: 36
            sourceComponent: ({ landmarks: landmarks, viewfinder: viewfinder, head: head, pulse: pulse })[indicator.look]
        }

        Text {
            anchors.verticalCenter: parent.verticalCenter
            text: qsTr("Scanning face")
            color: indicator.ink
        }
    }

    // The five points the pipeline aligns on: eyes, nose tip, mouth corners.
    Component {
        id: landmarks
        Item {
            Strokes { color: indicator.ink; path: indicator.bracketPath }
            Strokes {
                color: indicator.accent
                lineWidth: 1
                opacity: 0.5
                path: "M11 12H25L18 19ZM11 12L12.5 26.5L18 19L23.5 26.5L25 12M12.5 26.5H23.5"
            }
            Repeater {
                model: [[11, 12], [25, 12], [18, 19], [12.5, 26.5], [23.5, 26.5]]
                Rectangle {
                    required property var modelData
                    required property int index
                    // Each point peaks a fifth of a cycle after the one before it.
                    readonly property real t: (indicator.phase + 1 - index / 5) % 1
                    x: modelData[0] - 2.5
                    y: modelData[1] - 2.5
                    width: 5
                    height: 5
                    radius: 2.5
                    color: indicator.ink
                    border.color: indicator.accent
                    opacity: 0.3 + 0.7 * (1 - Math.abs(2 * t - 1))
                }
            }
        }
    }

    Component {
        id: viewfinder
        Item {
            Strokes { color: indicator.ink; path: indicator.bracketPath }
            Sweep { running: indicator.scanning; color: indicator.accent }
        }
    }

    // The README banner's head, scaled to the chip.
    Component {
        id: head
        Item {
            Strokes { color: indicator.ink; path: indicator.bracketPath; opacity: 0.5 }
            Strokes {
                color: indicator.ink
                lineWidth: 1.2
                path: "M18 5C23.5 5 27.4 8.6 27.4 14.6C27.4 19.7 26.7 23.5 24.7 26.6C22.9 29.3 20.5 30.8 18 30.8C15.5 30.8 13.1 29.3 11.3 26.6C9.3 23.5 8.6 19.7 8.6 14.6C8.6 8.6 12.5 5 18 5ZM8.6 14.6C7.2 14.1 6.7 16.3 7.2 18C7.6 19.2 8.3 19.5 8.8 19.3M27.4 14.6C28.8 14.1 29.3 16.3 28.8 18C28.4 19.2 27.7 19.5 27.2 19.3M12.5 16Q14.2 14.6 16 16Q14.2 17 12.5 16ZM23.5 16Q21.8 14.6 20 16Q21.8 17 23.5 16ZM17.3 16.8C17.2 18.7 16.2 20.2 16.5 21.2C16.7 21.9 17.5 21.9 18 21.7M18.7 16.8C18.8 18.7 19.8 20.2 19.5 21.2C19.3 21.9 18.5 21.9 18 21.7M14.6 25.1Q18 26.1 21.4 25.1"
            }
            Sweep { running: indicator.scanning; color: indicator.accent }
        }
    }

    // An emitter ring: no face drawn, reads as "the sensor is working".
    Component {
        id: pulse
        Item {
            Rectangle {
                anchors.centerIn: parent
                width: 30
                height: 30
                radius: 15
                color: "transparent"
                border.width: 1.5
                border.color: indicator.ink
                opacity: 0.25
            }
            Repeater {
                model: 2
                Rectangle {
                    required property int index
                    readonly property real t: (indicator.phase + index / 2) % 1
                    anchors.centerIn: parent
                    width: 6 + 20 * t
                    height: width
                    radius: width / 2
                    color: "transparent"
                    border.width: 1.5
                    border.color: indicator.accent
                    opacity: 0.7 * (1 - t)
                }
            }
            Strokes {
                color: indicator.accent
                lineWidth: 2.5
                path: "M18 3A15 15 0 0 1 32.1 12.9"
                RotationAnimation on rotation {
                    running: indicator.scanning
                    loops: Animation.Infinite
                    from: 0
                    to: 360
                    duration: 1400
                }
            }
            Rectangle {
                anchors.centerIn: parent
                width: 6
                height: 6
                radius: 3
                color: indicator.accent
            }
        }
    }
}
