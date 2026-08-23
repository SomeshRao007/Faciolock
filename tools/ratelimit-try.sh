#!/usr/bin/env bash
# ratelimit-try -- Phase 4's exit criteria, the parts no `cargo test` can reach.
#
#   "Kill the daemon mid-backoff, restart, backoff persists."
#   "Journal shows every attempt."
#
# `ratelimit::tests::state_survives_being_dropped_and_reloaded` proves a Limiter rebuilt from
# the same path remembers. It cannot prove what the criterion is about, which is that the
# write happens BEFORE the reply -- a limiter that flushed only on a clean shutdown would
# pass that test and fail this script, and `kill -9` is the only thing that tells them apart.
#
# Needs root, because the daemon's socket is SO_PEERCRED-gated (wiki/03 R2) and every leg
# below goes through the real socket. Needs NO camera and NO enrolment: `Limiter::decide`
# runs before the template load and before the burst, which is exactly the property being
# checked.
#
#   sudo ./tools/ratelimit-try.sh
#
# Not shipped, not a test. It exists so a human can watch it happen and write "observed" in
# wiki/09 rather than "pending".

set -u

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
run="${TMPDIR:-/tmp}/faciolock-ratelimit-try.$$"
sock="$run/faciolock.sock"
db="$run/ratelimit.db"
daemon="$root/target/debug/faciolockd"
client="$run/sock-try"

# The user to drive this against. Not root: uid 0 is unlikely to ever be enrolled, and using
# a real unprivileged account keeps the uid resolution honest.
user="${SUDO_USER:-$USER}"
uid="$(id -u "$user")"

cleanup() {
	[ -n "${pid:-}" ] && kill -9 "$pid" 2>/dev/null
	rm -rf "$run"
}
trap cleanup EXIT

pass=0
fail=0
check() { # check DESCRIPTION EXPECTED ACTUAL
	if [ "$2" = "$3" ]; then
		printf '  \033[32mok\033[0m   %-50s %s\n' "$1" "$3"
		pass=$((pass + 1))
	else
		printf '  \033[31mFAIL\033[0m %-50s got %s, wanted %s\n' "$1" "$3" "$2"
		fail=$((fail + 1))
	fi
}

ask() { # ask OP -> prints the verdict name, exits with the code
	"$client" "$sock" "$1" "$user" 2>&1 | awk '{print $1}'
}

# Wait for a real answer, not for the socket inode to exist.
#
# `kill -9` leaves the socket file behind -- the daemon's own cleanup never runs -- so
# `[ -S $sock ]` is true the instant the next daemon starts and long before it has bound.
# Polling with an actual request is the only readiness signal that means anything, and it
# doubles as a check that `serve::listen` recovers from a stale socket by renaming over it.
start() {
	"$daemon" --config "$run/faciolockd.toml" >>"$run/log" 2>&1 &
	pid=$!
	for _ in $(seq 60); do
		case "$("$client" "$sock" status "$user" 2>/dev/null | awk '{print $1}')" in
		NoEnrolment|Deny) return 0 ;;
		esac
		sleep 0.1
	done
	echo "daemon never answered on $sock; log:" >&2
	sed 's/^/  /' "$run/log" >&2
	exit 1
}

# Put the daemon into a locked state the only way that is honest without a camera: write the
# budget while it is DOWN, then start it.
#
# `Limiter::load` runs once at startup and the state is held in memory, written through on
# every change -- so editing the file behind a running daemon proves nothing and the first
# draft of this script was wrong to try. That design is also what the criterion is about:
# the file is what a restart reads.
relock() {
	[ -n "${pid:-}" ] && { kill -9 "$pid" 2>/dev/null; wait "$pid" 2>/dev/null; pid=""; }
	printf '%s 3 %s\n' "$uid" "$(date +%s)" >"$db"
	chmod 600 "$db"
	start
}

[ "$(id -u)" -eq 0 ] || { echo "needs root -- the socket is SO_PEERCRED-gated. try: sudo $0" >&2; exit 2; }
[ -x "$daemon" ] || { echo "build it first:  cargo build -p faciolock-daemon" >&2; exit 2; }

mkdir -p "$run/templates" "$run/models"
cp "$root/models"/*.onnx "$root/models"/MANIFEST.sha256 "$run/models/" 2>/dev/null
cc -O2 -o "$client" "$root/tools/sock-try.c" || exit 2

cat >"$run/faciolockd.toml" <<EOF
socket = "$sock"
models = "$run/models"
templates = "$run/templates"
ratelimit = "$db"
EOF

echo
echo "PART 1 -- a locked-out user is refused before the camera"
echo

relock
check "daemon is up" "yes" "$([ -d "/proc/$pid" ] && echo yes || echo no)"

# The budget is checked before the store lookup and before the burst, so this answers
# without a template and with the camera dark. The NoEnrolment in PART 2 is what proves the
# ordering: same user, same daemon, and the only thing that changed is the lockout.
check "a locked uid gets RateLimited" "RateLimited" "$(ask auth)"

echo
echo "PART 2 -- Op::Clear releases the lockout (P4-b)"
echo

check "clear is acknowledged" "Ack" "$(ask clear)"

# The write-through is the interesting part. The daemon is still running, so this entry
# leaving the file means the change was persisted at the moment it happened rather than at
# shutdown -- the same `flush()` that `record_failure` calls, which is the ordering PART 3's
# criterion is about and the closest this script can get to it without a camera.
check "the entry left the file while the daemon still runs" "" \
	"$(awk -v u="$uid" '$1==u{print $2}' "$db")"
check "and the lockout is lifted -- so the budget answered first" "NoEnrolment" "$(ask auth)"
check "a second clear is quiet, not an error" "Ack" "$(ask clear)"

echo
echo "PART 3 -- the state survives kill -9"
echo

relock
check "locked again after a restart" "RateLimited" "$(ask auth)"

kill -9 "$pid"; wait "$pid" 2>/dev/null; pid=""
check "database outlived the kill" "3" "$(awk -v u="$uid" '$1==u{print $2}' "$db")"
check "database is owner-only" "600" "$(stat -c %a "$db")"
check "the socket outlived it too (stale)" "yes" "$([ -S "$sock" ] && echo yes || echo no)"

start
check "a restarted daemon renamed over the stale socket" "yes" \
	"$([ -d "/proc/$pid" ] && echo yes || echo no)"
check "and still refuses" "RateLimited" "$(ask auth)"
check "the clear still works after a restart" "Ack" "$(ask clear)"
check "leaving the user free" "NoEnrolment" "$(ask auth)"

echo
echo "PART 4 -- the journal line (wiki/03 R9)"
echo
grep -E '^<[0-9]>faciolockd: (auth|clear) ' "$run/log" | sed 's/^/  /'
echo

# The sequence above is fixed, so these counts are too:
#   auth  x5  -- RateLimited x3 (PART 1, PART 3 before and after the kill)
#                NoEnrolment x2 (PART 2, PART 3 tail)
#   clear x2  -- three were sent; the one with nothing to clear must stay silent
n="$(grep -cE '^<[0-9]>faciolockd: auth ' "$run/log")"

check "one audit line per request, and no more" "5" "$n"
check "  RateLimited" "3" "$(grep -cE '^<[0-9]>faciolockd: auth .*verdict=RateLimited' "$run/log")"
check "  NoEnrolment" "2" "$(grep -cE '^<[0-9]>faciolockd: auth .*verdict=NoEnrolment' "$run/log")"
check "each carries the uid" "$n" "$(grep -cE "^<[0-9]>faciolockd: auth uid=$uid " "$run/log")"
check "each says whether it counted" "$n" "$(grep -cE '^<[0-9]>faciolockd: auth .* counted=[01]$' "$run/log")"
check "none of these counted (all are exempt)" "$n" "$(grep -cE '^<[0-9]>faciolockd: auth .* counted=0$' "$run/log")"
check "every lockout is at err (<3>)" "3" "$(grep -cE '^<3>faciolockd: auth .*reason=lockout' "$run/log")"
check "a not-enrolled answer is at info, not warning" "0" \
	"$(grep -cE '^<[0-4]>faciolockd: auth .*verdict=NoEnrolment' "$run/log")"
check "a clear that did nothing logged nothing" "2" \
	"$(grep -cE '^<[0-9]>faciolockd: clear .*event=lockout-cleared' "$run/log")"

echo
echo "NOT PROVEN HERE, and it must not be written up as observed:"
cat <<'NOTE'
  the leg from "the daemon denied a real face" to "the counter moved". Every verdict this
  script can produce is exempt from counting by design (`counts_as_an_attempt`), because
  reaching a counting one needs a face in front of a real camera. With an enrolment:

    sudo ./target/debug/faciolockd --enroll "$USER"
    sudo ./target/sock-try /run/faciolock.sock auth "$USER"   # x3, face visible
    journalctl -t faciolockd | grep counted=1
    cat /var/lib/faciolock/ratelimit.db                        # failures climbing to 3
NOTE

echo
printf '%d passed, %d failed\n' "$pass" "$fail"
exit $((fail > 0))
