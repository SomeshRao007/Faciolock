#!/usr/bin/env bash
# enrol-try -- Phase 5's exit criterion, and the row Phases 3 and 4 both left open.
#
#   "A person who has not read this wiki can enrol and use it from the CLI alone."
#   "A denied face moves the counter."  (wiki/09 Phase 4, last row)
#
# Needs a camera and needs YOUR FACE. That is the whole reason it exists: everything
# reachable without one is already a `cargo test`, and both open rows in wiki/09 are open
# because a burst of a real face is the one input a test cannot fabricate.
#
#   sudo ./tools/enrol-try.sh
#
# Nothing here is installed. The daemon runs out of target/debug against a throwaway store in
# $TMPDIR, so a failed run leaves your real /var/lib/faciolock alone -- and on a machine where
# Faciolock is not installed yet, there is no real one to leave alone.
#
# Not shipped, not a test. It exists so a human can watch it happen and write "observed" in
# wiki/09 rather than "pending".
#
# ** If it is daylight where you are, stop and come back after dark. ** B18: ambient infrared
# lifts the sensor floor above what the strobe checks can work against, and every capture
# answers Unavailable. That is the daemon being honest, not the harness being broken, and
# this script says so when it happens rather than leaving you to guess.

set -u

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
run="${TMPDIR:-/tmp}/faciolock-enrol-try.$$"
sock="$run/faciolock.sock"
db="$run/ratelimit.db"
templates="$run/templates"
daemon="$root/target/debug/faciolockd"
ctl="$root/target/debug/faciolockctl"
client="$run/sock-try"

# Not root: uid 0 is unlikely to ever be enrolled, and a real unprivileged account keeps the
# uid resolution honest.
user="${SUDO_USER:-$USER}"
uid="$(id -u "$user")"

cleanup() {
	[ -n "${pid:-}" ] && kill "$pid" 2>/dev/null
	rm -rf "$run"
}
trap cleanup EXIT

pass=0
fail=0
check() { # check DESCRIPTION EXPECTED ACTUAL
	if [ "$2" = "$3" ]; then
		printf '  \033[32mok\033[0m   %-52s %s\n' "$1" "$3"
		pass=$((pass + 1))
	else
		printf '  \033[31mFAIL\033[0m %-52s got %s, wanted %s\n' "$1" "$3" "$2"
		fail=$((fail + 1))
	fi
}

# `faciolockctl test` deliberately clears the failure budget when it finishes, so it is the
# wrong tool for watching the counter move. sock-try sends one op and says what came back.
ask() { # ask OP -> prints the verdict name
	"$client" "$sock" "$1" "$user" 2>&1 | awk '{print $1}'
}

# Wait for a real answer, not for the socket inode to exist -- a stale inode from a previous
# run is true the instant a new daemon starts and long before it has bound.
start() {
	"$daemon" --config "$run/faciolockd.toml" >>"$run/log" 2>&1 &
	pid=$!
	for _ in $(seq 100); do
		case "$(ask status)" in
		NoEnrolment | Deny) return 0 ;;
		esac
		sleep 0.1
	done
	echo "daemon never answered on $sock; log:" >&2
	sed 's/^/  /' "$run/log" >&2
	exit 1
}

[ "$(id -u)" -eq 0 ] || { echo "needs root -- the socket is SO_PEERCRED-gated. try: sudo $0" >&2; exit 2; }
[ -x "$daemon" ] && [ -x "$ctl" ] || { echo "build it first:  cargo build" >&2; exit 2; }

mkdir -p "$templates" "$run/models"
cp "$root/models"/*.onnx "$root/models"/MANIFEST.sha256 "$run/models/" 2>/dev/null
cc -O2 -o "$client" "$root/tools/sock-try.c" || exit 2

cat >"$run/faciolockd.toml" <<EOF
socket = "$sock"
models = "$run/models"
templates = "$templates"
ratelimit = "$db"
EOF

echo
echo "PART 0 -- what this machine has"
echo
"$ctl" --config "$run/faciolockd.toml" diagnose

start
check "daemon is up" "yes" "$([ -d "/proc/$pid" ] && echo yes || echo no)"
check "nobody is enrolled yet" "NoEnrolment" "$(ask status)"

echo
echo "PART 1 -- enrol, from the CLI alone"
echo "  Three poses. The daemon asks for a real burst each time, so give it a second."
echo
"$ctl" --config "$run/faciolockd.toml" enroll "$user"

if [ ! -f "$templates/$uid.tpl" ]; then
	echo
	echo "No template was written, so nothing below can be checked."
	if grep -q 'reason=ambient' "$run/log"; then
		echo
		echo "The daemon's log says why, and it is not a fault:"
		grep 'reason=ambient' "$run/log" | tail -3 | sed 's/^/  /'
		echo
		echo "That is B18. The room has more infrared in it than the strobe checks can"
		echo "work against -- almost always daylight. Close the blinds, or come back"
		echo "after dark, and run this again."
		exit 3
	fi
	echo "Daemon log:"
	tail -20 "$run/log" | sed 's/^/  /'
	exit 1
fi

check "a template exists for uid $uid" "yes" "$([ -s "$templates/$uid.tpl" ] && echo yes || echo no)"
check "it is encrypted, not a pile of floats" "FLT2" "$(head -c4 "$templates/$uid.tpl")"
check "the store wrote one line saying so" "1" \
	"$(grep -cE 'enrol uid='"$uid"' event=template-written' "$run/log")"
# Exactly three captured poses however many were attempted. A retry after a bad pose logs a
# refusal, not a capture, and a template built from four captures would mean the daemon
# finalised on the wrong count.
check "exactly three poses were captured" "3" \
	"$(grep -cE "enrol uid=$uid pose=[0-9]+/3 reason=captured" "$run/log")"
check "status now says enrolled" "Deny" "$(ask status)"

echo
echo "PART 2 -- list, and what it shows"
echo
"$ctl" --config "$run/faciolockd.toml" list
check "list finds the uid" "1" \
	"$("$ctl" --config "$run/faciolockd.toml" list | grep -cE "^ *$uid  +$user")"

echo
echo "PART 3 -- authenticate as yourself.  LOOK AT THE CAMERA."
echo
sleep 2
verdict="$(ask auth)"
check "your own face is an Allow" "Allow" "$verdict"
check "an allow is logged at notice (<5>)" "1" \
	"$(grep -cE '^<5>faciolockd: auth uid='"$uid"' verdict=Allow' "$run/log")"
check "and it counted" "1" \
	"$(grep -cE '^<5>faciolockd: auth uid='"$uid"' verdict=Allow.*counted=1' "$run/log")"

echo
echo "PART 4 -- the row wiki/09 has had open since Phase 4:"
echo "          \"a denied face moves the counter\""
echo
echo "  Point the camera at something that is NOT your face -- turn the laptop away,"
echo "  or have someone else sit in front of it. Three attempts."
echo
read -r -p "  [Enter] when the camera is looking at anything but you " _

# Spaced, because the point is to watch the counter move and the backoff is 1 s then 2 s
# after the first two failures. Back to back, attempts 2 and 3 come back `RateLimited` --
# which is the limiter working, and is confusing to read when what you came to see is the
# counter climbing to three.
for i in 1 2 3; do
	printf '    attempt %d: %s\n' "$i" "$(ask auth)"
	sleep 3
done

# The file is `uid failures timestamp`. Reading it is the point: `record_failure` flushes
# before the reply, so a counter that moved is a counter on disk.
after="$(awk -v u="$uid" '$1 == u {print $2}' "$db" 2>/dev/null)"
echo
echo "  ratelimit.db: $(cat "$db" 2>/dev/null || echo '(empty)')"
check "the failure counter moved off zero" "yes" "$([ -n "$after" ] && [ "$after" -gt 0 ] && echo yes || echo no)"
check "the denials are logged as counted" "yes" \
	"$([ "$(grep -cE 'auth uid='"$uid"'.*verdict=Deny.*counted=1' "$run/log")" -gt 0 ] && echo yes || echo no)"
check "nothing was counted that never saw a face" "0" \
	"$(grep -cE 'verdict=Unavailable.*counted=1' "$run/log")"

echo
echo "PART 5 -- Op::Clear gives it back, and remove takes the template with it"
echo
"$ctl" --config "$run/faciolockd.toml" remove "$user" --yes
check "the template is gone" "no" "$([ -f "$templates/$uid.tpl" ] && echo yes || echo no)"
check "the failure budget went with it" "" "$(awk -v u="$uid" '$1 == u {print $2}' "$db" 2>/dev/null)"
check "status is back to not-enrolled" "NoEnrolment" "$(ask status)"

echo
echo "NOT PROVEN HERE, and it must not be written up as observed:"
cat <<'NOTE'
  that `sudo` authenticates by face. That needs an edit to /etc/pam.d/sudo, which is a
  separate decision with its own recovery story, and wiki/09 Phase 3 keeps that row open
  deliberately until someone takes it.
NOTE

echo
printf '%d passed, %d failed\n' "$pass" "$fail"
exit $((fail > 0))
