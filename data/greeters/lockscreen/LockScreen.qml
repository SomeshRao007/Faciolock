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
            }
            faceLockScreen.updateScan()
        }
        function onPromptForSecretChanged() {
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
        }
    }

    FaceScanIndicator {
        scanning: faceLockScreen.scanning
        anchors.horizontalCenter: parent.horizontalCenter
        anchors.bottom: parent.bottom
        anchors.bottomMargin: Kirigami.Units.gridUnit * 5
        z: 100
    }
}
