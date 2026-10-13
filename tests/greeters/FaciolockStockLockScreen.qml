import QtQuick

Item {
    property bool viewVisible: false
    property QtObject authenticator: QtObject {
        property int starts: 0
        property int state: 0
        property string promptForSecret: ""
        property bool graceLocked: false
        signal failed(int kind)
        signal succeeded()
        function startAuthenticating() {
            if (state === 1 || graceLocked) {
                return
            }
            promptForSecret = ""
            starts += 1
            state = 1
        }
    }
}
