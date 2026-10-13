// ponytail: no i18n() wrapping. There is no translation catalogue and nothing loads one, so
// i18n("Enrol") is ceremony that fails loudly if the context is ever missing. Wrap these when
// a .po file exists to wrap them for.
import QtQuick
import QtQuick.Controls as Controls
import QtQuick.Layouts

import org.kde.kirigami as Kirigami
import org.kde.kcmutils

SimpleKCM {
    id: root

    // Nothing here is a setting that gets applied on OK. Every button acts immediately.
    ConfigModule.buttons: ConfigModule.NoAdditionalButton

    ColumnLayout {
        spacing: Kirigami.Units.largeSpacing

        Kirigami.InlineMessage {
            Layout.fillWidth: true
            type: Kirigami.MessageType.Error
            visible: kcm.trouble !== ""
            text: kcm.trouble
        }

        Kirigami.FormLayout {
            Layout.fillWidth: true

            Controls.Label {
                Kirigami.FormData.label: "Your face:"
                text: kcm.enrolment === "enrolled" ? "Enrolled"
                    : kcm.enrolment === "not-enrolled" ? "Not enrolled"
                    : "Unknown"
            }

            Controls.Switch {
                Kirigami.FormData.label: "Face unlock:"
                text: checked ? "On" : "Off"
                checked: kcm.pamEnabled
                enabled: !kcm.busy
                onClicked: {
                    kcm.run(kcm.pamEnabled ? "disable" : "enable")
                    checked = Qt.binding(function() { return kcm.pamEnabled })
                }
            }
        }

        // The pose prompt. faciolockctl is blocked on stdin until Capture is pressed, so
        // there is no timer here and no way to miss a pose by being slow.
        Kirigami.InlineMessage {
            Layout.fillWidth: true
            type: Kirigami.MessageType.Positive
            visible: kcm.pose !== ""
            text: kcm.pose
            actions: Kirigami.Action {
                text: "Capture"
                icon.name: "camera-photo"
                onTriggered: kcm.capture()
            }
        }

        RowLayout {
            spacing: Kirigami.Units.smallSpacing

            Controls.Button {
                text: kcm.enrolment === "enrolled" ? "Enrol again" : "Enrol"
                icon.name: "face-smile"
                enabled: !kcm.busy && kcm.trouble === ""
                onClicked: kcm.run("enroll")
            }

            Controls.Button {
                text: "Test"
                icon.name: "checkmark"
                enabled: !kcm.busy && kcm.enrolment === "enrolled"
                onClicked: kcm.run("test")
            }

            Controls.Button {
                text: "Remove"
                icon.name: "edit-delete"
                enabled: !kcm.busy && kcm.enrolment === "enrolled"
                onClicked: kcm.run("remove")
            }

            // Only enrolment can be stopped — it is the only thing that reads stdin, and
            // closing stdin is the only stop an unprivileged panel has over a root child.
            Controls.Button {
                text: "Stop"
                icon.name: "process-stop"
                visible: kcm.pose !== ""
                onClicked: kcm.cancel()
            }
        }

        // faciolockctl's own words, verbatim. The panel does not paraphrase advice that took
        // a phase of tuning to write ("sit square to the camera, about arm's length away").
        Controls.TextArea {
            Layout.fillWidth: true
            Layout.minimumHeight: Kirigami.Units.gridUnit * 8
            visible: kcm.log !== ""
            readOnly: true
            font.family: "monospace"
            text: kcm.log
            onTextChanged: cursorPosition = length
        }

        Item {
            Layout.fillHeight: true
        }
    }
}
