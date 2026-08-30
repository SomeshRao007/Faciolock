#!/usr/bin/env bash
# activation-try -- Phase 6's first exit criterion.
#
#   "zero RSS when idle (`pidof faciolockd` empty)"
#
#   sudo ./tools/activation-try.sh
#
# Nothing is installed and nothing on this machine is modified. `systemd-socket-activate` is
# systemd's own harness for exactly this: it binds the socket, sets LISTEN_PID/LISTEN_FDS and
# hands the daemon fd 3. That is the same code path faciolock.socket drives, so what is under
# test is the shipped one -- with no unit file, no `faciolock` user and no root beyond the
# SO_PEERCRED gate.
#
# ** Two things this harness cannot show, and it does not pretend to. **
#
# `systemd-socket-activate` **exec()s** the daemon rather than supervising it -- measured: the
# activator's pid and the daemon's pid are the same number. So when the daemon idle-exits,
# nothing is left holding the listening fd, and a second request gets ECONNREFUSED. PART 4
# starts a *fresh* activator, which shows that an exited daemon is replaceable; it does not
# show reactivation on the same inode. That is systemd's socket-unit behaviour and it needs
# `tools/install-local.sh` and `systemctl`.
#
# The sandbox is the other one: `systemd-analyze security` needs the real unit, and that
# measurement lives in wiki/03 R7.
#
# Parts 1-4 need no camera: every request is `status`, which answers from one file on disk and
# never opens the device, so they run in daylight unlike enrol-try. Part 5 needs a face in
# front of the lens and says so rather than failing when there isn't one.

set -u

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
run="${TMPDIR:-/tmp}/faciolock-activation-try.$$"
sock="$run/faciolock.sock"
daemon="$root/target/debug/faciolockd"
client="$run/sock-try"
user="${SUDO_USER:-$USER}"

# Short enough to watch, long enough that a slow model load cannot be mistaken for it. The
# shipped default is 30 s; what is under test is the mechanism, not the number.
idle=3

cleanup() {
	[ -n "${activator:-}" ] && kill "$activator" 2>/dev/null
	rm -rf "$run"
}
trap cleanup EXIT

pass=0
fail=0
check() { # check DESCRIPTION EXPECTED ACTUAL
	if [ "$2" = "$3" ]; then
		printf '  \033[32mok\033[0m   %-56s %s\n' "$1" "$3"
		pass=$((pass + 1))
	else
		printf '  \033[31mFAIL\033[0m %-56s got %s, wanted %s\n' "$1" "$3" "$2"
		fail=$((fail + 1))
	fi
}

ask() { # ask OP -> prints the verdict name
	"$client" "$sock" "$1" "$user" 2>&1 | awk '{print $1}'
}

# Two filters, and both are needed.
#
# `-x` on the process NAME rather than `-f` on the command line, because the activator's own
# argv carries the daemon's path and a command-line match would report a running daemon for the
# whole run. That is also how wiki/09 words the criterion: `pidof faciolockd` empty.
#
# And then /proc/PID/cmdline for OUR config, because `-x` alone matches the **installed**
# daemon too. Once faciolock.socket is active, anything at all can activate it mid-run -- a
# sudo, a screen lock, a login -- and this harness would report its own daemon as running when
# what it found was somebody else's.
running() {
	for pid in $(pgrep -x faciolockd 2>/dev/null); do
		if tr '\0' ' ' <"/proc/$pid/cmdline" 2>/dev/null | grep -qF "$run/faciolockd.toml"; then
			echo yes
			return
		fi
	done
	echo no
}

[ "$(id -u)" -eq 0 ] || { echo "needs root -- the socket is SO_PEERCRED-gated. try: sudo $0" >&2; exit 2; }
[ -x "$daemon" ] || { echo "build it first:  cargo build" >&2; exit 2; }

# **Refuse to run alongside the installed daemon.** There is one camera. This harness starts
# its own daemon on its own socket, but /dev/video2 is not private -- and once faciolock.socket
# is active, the `sudo` on the line above is itself enough to activate the installed daemon if
# the PAM profile is enabled. The two then fight for the device and the failures land on
# whichever asked second, which is not a bug in either.
if systemctl is-active --quiet faciolock.socket 2>/dev/null; then
	echo "faciolock.socket is active, and there is only one camera." >&2
	echo "  sudo systemctl stop faciolock.socket    then re-run, then start it again" >&2
	exit 2
fi
command -v systemd-socket-activate >/dev/null || { echo "needs systemd-socket-activate (package: systemd)" >&2; exit 2; }

mkdir -p "$run/templates" "$run/models"
cp "$root/models"/*.onnx "$root/models"/MANIFEST.sha256 "$run/models/" 2>/dev/null
cc -O2 -o "$client" "$root/tools/sock-try.c" || exit 2

cat >"$run/faciolockd.toml" <<EOF
socket = "$sock"
models = "$run/models"
templates = "$run/templates"
ratelimit = "$run/ratelimit.db"
idle_timeout_secs = $idle
EOF

echo
echo "PART 1 -- nothing runs until somebody connects"
echo

activate() {
	systemd-socket-activate --seqpacket --listen="$sock" \
		-- "$daemon" --config "$run/faciolockd.toml" >>"$run/log" 2>&1 &
	activator=$!
	# The socket is bound by the activator, so waiting for the inode is honest here in a way
	# it is not in the other harnesses -- there is no daemon yet to have left a stale one.
	for _ in $(seq 50); do [ -S "$sock" ] && return; sleep 0.1; done
}

activate
check "the socket exists" "yes" "$([ -S "$sock" ] && echo yes || echo no)"
check "and nothing is running behind it" "no" "$(running)"

echo
echo "PART 2 -- the first request starts it"
echo

first="$(ask status)"
check "a request is answered" "NoEnrolment" "$first"
check "the daemon is now up" "yes" "$(running)"
check "it knows the socket was inherited" "1" \
	"$(grep -c 'socket-activated' "$run/log")"
check "and it did not bind the path itself" "0" \
	"$(grep -c 'self-bound' "$run/log")"
check "idle-exit is armed" "1" "$(grep -c "idle-exit ${idle} s" "$run/log")"

echo
echo "PART 3 -- it goes away on its own"
echo

sleep "$((idle + 2))"
check "pidof faciolockd is empty" "no" "$(running)"
check "it said why" "1" "$(grep -c 'idle for .* exiting' "$run/log")"

# The bug this row exists for: `Daemon::serve` used to unlink the socket path unconditionally
# on the way out. Against an inherited fd that would delete systemd's inode, and the FIRST
# idle-exit would permanently break reactivation -- a daemon that works until it works.
check "the socket outlived the exit" "yes" "$([ -S "$sock" ] && echo yes || echo no)"

echo
echo "PART 4 -- and a fresh activation replaces it"
echo

# The `rm` is not tidiness, it is the price of the assertion above: `bind` fails EADDRINUSE on
# the inode the last check just proved survived, and the activator that exec'd the daemon went
# with it. Under a real faciolock.socket neither happens -- systemd holds the inode across
# every activation and never rebinds. See the header.
rm -f "$sock"
activate
check "the socket is back" "yes" "$([ -S "$sock" ] && echo yes || echo no)"
check "and still nothing behind it" "no" "$(running)"

second="$(ask status)"
check "a second request is answered" "NoEnrolment" "$second"
check "the daemon is up again" "yes" "$(running)"
check "activated twice, not restarted once" "2" \
	"$(grep -c 'socket-activated' "$run/log")"

echo
echo "PART 5 -- an enrolment in flight is not thrown away"
echo

# ENROL_EXPIRY is 60 s against an idle timeout of 3: one pose in, and the daemon must stay up
# past its idle deadline rather than discard the poses already given. Without the guard the
# next pose silently starts a fresh enrolment that never reaches three.
#
# This is the one leg that needs a face. A pose that was refused or could not be judged leaves
# no `Pending` behind, so there would be nothing for the guard to protect and asserting
# anything about it would be asserting the wrong thing. `may_exit` itself is covered without
# hardware by `an_enrolment_in_flight_keeps_the_daemon_alive`; what this adds is that
# `serve::run` actually consults it.
echo "  look at the camera --"
pose="$(ask enroll)"
if [ "$pose" = "Ack" ]; then
	check "one pose lands" "Ack" "$pose"
	sleep "$((idle + 2))"
	check "the daemon stayed up for the rest of it" "yes" "$(running)"
else
	printf '  \033[33mskip\033[0m %-56s %s\n' "no pose to protect (needs a face; B18 in daylight)" "$pose"
fi

echo
printf '  %d passed, %d failed\n\n' "$pass" "$fail"
[ "$fail" -eq 0 ] || exit 1
