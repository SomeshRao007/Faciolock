import QtQuick
import QtCore
import org.kde.kirigami as Kirigami

FaciolockStockLockScreen {
    id: faceLockScreen
    property bool scanning: false
    property bool autoStarted: false
    property bool passwordPrompted: false
    property bool faceEnabled: faceSettings.enabled
    property bool locked: false
    property bool faceMatched: false
    property bool secretAnswered: false

    function updateScan() {
        // PamAuthenticators::Authenticating is 1; password entry keeps that state.
        scanning = faceEnabled && authenticator.state === 1
                   && !passwordPrompted && !authenticator.promptForSecret
        if (scanning) {
            scanTimeout.restart()
        } else {
            scanTimeout.stop()
        }
    }

    function startFaceScan() {
        if (autoStarted || !viewVisible || !faceEnabled) {
            return
        }
        if (authenticator.state !== 1) {
            authenticator.startAuthenticating()
        }
        autoStarted = authenticator.state === 1
        updateScan()
    }

    Settings {
        id: faceSettings
        location: "file:///etc/xdg/faciolock-greeter.conf"
        category: "FaceUnlock"
        property bool enabled: false
        property string indicator: "landmarks"
        property string position: "top"
    }

    Timer {
        id: scanTimeout
        interval: 7000
        onTriggered: faceLockScreen.scanning = false
    }

    Component.onCompleted: startFaceScan()
    onViewVisibleChanged: startFaceScan()
    onLockedChanged: {
        if (locked) {
            startFaceScan()
        }
    }

    Connections {
        target: authenticator
        function onStateChanged() {
            if (authenticator.state === 0) {
                faceLockScreen.passwordPrompted = false
                faceLockScreen.faceMatched = false
            }
            faceLockScreen.updateScan()
        }
        // pam_faciolock's notice on a match (lib.rs MATCHED).
        function onInfoMessageChanged() {
            if (authenticator.infoMessage === "Face recognised") {
                faceLockScreen.faceMatched = true
            }
        }
        function onPromptForSecretChanged() {
            // After a match the prompt is pam_kwallet5's, and "" makes it step aside. Once per
            // lock, so a second factor that fails on "" falls back to typing, not a loop.
            if (faceLockScreen.faceMatched && !faceLockScreen.secretAnswered
                    && authenticator.promptForSecret) {
                faceLockScreen.secretAnswered = true
                authenticator.respond("")
            }
            if (authenticator.promptForSecret) {
                faceLockScreen.passwordPrompted = true
            }
            faceLockScreen.updateScan()
        }
        function onFailed(kind) {
            if (kind === 0) {
                faceLockScreen.scanning = false
                scanTimeout.stop()
            }
        }
        function onSucceeded() {
            faceLockScreen.scanning = false
            scanTimeout.stop()
            // Stock UI shows an Unlock button when nothing prompted; the greeter only
            // honours quit once PAM has succeeded.
            if (faceLockScreen.faceMatched) {
                Qt.quit()
            }
        }
    }

    FaceScanIndicator {
        readonly property bool atTop: faceSettings.position !== "bottom"
        scanning: faceLockScreen.scanning
        style: faceSettings.indicator
        anchors.horizontalCenter: parent.horizontalCenter
        anchors.top: atTop ? parent.top : undefined
        anchors.bottom: atTop ? undefined : parent.bottom
        anchors.topMargin: Kirigami.Units.gridUnit
        anchors.bottomMargin: Kirigami.Units.gridUnit * 5
        z: 100
    }
}
