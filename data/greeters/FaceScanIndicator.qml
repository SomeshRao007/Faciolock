import QtQuick
import org.kde.kirigami as Kirigami

Item {
    id: indicator
    property bool scanning: false
    visible: scanning
    width: 128
    height: 116
    Accessible.role: Accessible.Animation
    Accessible.name: qsTr("Scanning for your face")

    Rectangle {
        x: 28
        y: 4
        width: 72
        height: 84
        radius: 32
        color: "transparent"
        border.width: 2
        border.color: Kirigami.Theme.textColor

        Repeater {
            model: 2
            Rectangle {
                required property int index
                x: index === 0 ? 18 : 48
                y: 30
                width: 6
                height: 6
                radius: 3
                color: Kirigami.Theme.textColor
            }
        }

        Rectangle {
            x: 24
            y: 58
            width: 24
            height: 2
            radius: 1
            color: Kirigami.Theme.textColor
        }

        Rectangle {
            x: 4
            width: 64
            height: 2
            radius: 1
            color: Kirigami.Theme.highlightColor
            NumberAnimation on y {
                running: indicator.scanning
                loops: Animation.Infinite
                from: 12
                to: 72
                duration: 950
            }
        }
    }

    Text {
        anchors.horizontalCenter: parent.horizontalCenter
        anchors.bottom: parent.bottom
        text: qsTr("Scanning face")
        color: Kirigami.Theme.textColor
        font.pointSize: 10
    }
}
