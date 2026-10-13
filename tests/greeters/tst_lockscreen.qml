import QtQuick
import QtTest

TestCase {
    name: "FaciolockLockScreen"
    when: windowShown

    Component {
        id: lockScreen
        LockScreen { width: 800; height: 600 }
    }

    function test_disabled_does_not_start() {
        const screen = lockScreen.createObject(this, { faceEnabled: false })
        verify(screen !== null)
        screen.viewVisible = true
        compare(screen.authenticator.starts, 0)
        compare(screen.scanning, false)
        screen.destroy()
    }

    function test_visible_attempt_stops_at_password_prompt() {
        const screen = lockScreen.createObject(this, { faceEnabled: true })
        verify(screen !== null)
        compare(screen.authenticator.starts, 0)
        screen.viewVisible = true
        compare(screen.authenticator.starts, 1)
        compare(screen.scanning, true)
        screen.authenticator.promptForSecret = "Password:"
        compare(screen.scanning, false)
        screen.authenticator.promptForSecret = ""
        compare(screen.scanning, false)
        screen.viewVisible = false
        screen.viewVisible = true
        compare(screen.authenticator.starts, 1)
        screen.destroy()
    }

    function test_later_attempt_and_failure() {
        const screen = lockScreen.createObject(this, { faceEnabled: true })
        verify(screen !== null)
        screen.viewVisible = true
        screen.authenticator.failed(1)
        compare(screen.scanning, true)
        screen.authenticator.state = 0
        compare(screen.scanning, false)
        screen.authenticator.startAuthenticating()
        compare(screen.authenticator.starts, 2)
        compare(screen.scanning, true)
        screen.authenticator.succeeded()
        compare(screen.scanning, false)
        screen.destroy()
    }

    function test_grace_lock_does_not_claim_scanning() {
        const screen = lockScreen.createObject(this, { faceEnabled: true })
        verify(screen !== null)
        screen.authenticator.graceLocked = true
        screen.viewVisible = true
        compare(screen.authenticator.starts, 0)
        compare(screen.scanning, false)
        screen.authenticator.graceLocked = false
        screen.locked = true
        compare(screen.authenticator.starts, 1)
        compare(screen.scanning, true)
        screen.locked = false
        screen.locked = true
        compare(screen.authenticator.starts, 1)
        screen.destroy()
    }
}
