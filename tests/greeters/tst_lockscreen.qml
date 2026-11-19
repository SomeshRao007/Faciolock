import QtQuick
import QtTest

TestCase {
    name: "FaciolockLockScreen"
    when: windowShown

    Component {
        id: lockScreen
        LockScreen { width: 800; height: 600 }
    }

    Component {
        id: indicator
        FaceScanIndicator {}
    }

    // The style comes from a root-owned file a newer release may write; an unknown one must
    // still draw something rather than an empty chip.
    function test_indicator_style_falls_back_to_landmarks() {
        const chip = indicator.createObject(this, { style: "pulse", scanning: true })
        verify(chip !== null)
        compare(chip.look, "pulse")
        verify(chip.width > chip.height)
        chip.style = "sparkles"
        compare(chip.look, "landmarks")
        chip.style = ""
        compare(chip.look, "landmarks")
        chip.destroy()
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

    // After a match the next secret prompt is pam_kwallet5's; answer it once per lock only.
    function test_match_answers_the_wallet_prompt_once() {
        const screen = lockScreen.createObject(this, { faceEnabled: true })
        verify(screen !== null)
        screen.viewVisible = true
        screen.authenticator.promptForSecret = "Password: "
        compare(screen.authenticator.responses.length, 0)
        screen.authenticator.state = 0
        screen.authenticator.startAuthenticating()
        screen.authenticator.infoMessage = "Face recognised"
        compare(screen.faceMatched, true)
        screen.authenticator.promptForSecret = "Password: "
        compare(screen.authenticator.responses, [""])
        compare(screen.scanning, false)
        screen.authenticator.state = 0
        compare(screen.faceMatched, false)
        screen.authenticator.startAuthenticating()
        screen.authenticator.infoMessage = "Face recognised"
        screen.authenticator.promptForSecret = "Verification code: "
        compare(screen.authenticator.responses.length, 1)
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
