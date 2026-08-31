#!/usr/bin/env bash
# install-local -- put a built tree onto this machine the way a package would.
#
#   sudo ./tools/install-local.sh            install
#   sudo ./tools/install-local.sh --remove   take it all back off
#
# Phase 6 needs this and Phases 1-5 did not: every harness so far ran the daemon out of
# target/debug against a throwaway store in $TMPDIR, which is exactly right for testing the
# code and useless for testing the *integration* -- socket activation, the sandbox, the
# `faciolock` user, and a PAM module something other than a test harness can dlopen.
#
# It is deliberately not a package. wiki/07's four .deb packages are Phase 7. What this does
# is put the same files in the same places so the unit files and the pam-configs profile are
# exercised as written, and `--remove` puts the machine back.
#
# **It does not touch /etc/pam.d and does not run pam-auth-update.** Enabling face
# authentication is a separate, deliberate step with its own recovery story -- wiki/07,
# "Rolling it out, and getting back out". Installing the module is not enabling it: nothing
# loads pam_faciolock.so until a PAM file names it.

set -eu

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
libexec=/usr/libexec/faciolock
share=/usr/share/faciolock
secdir="$(ls -d /usr/lib/*/security 2>/dev/null | head -1)"
profile=release

[ "$(id -u)" -eq 0 ] || { echo "needs root. try: sudo $0 $*" >&2; exit 2; }

if [ "${1:-}" = "--remove" ]; then
	echo "stopping and disabling"
	systemctl disable --now faciolock.socket 2>/dev/null || true
	systemctl stop faciolock.service 2>/dev/null || true
	rm -f /etc/systemd/system/faciolock.socket /etc/systemd/system/faciolock.service
	rm -f /usr/lib/sysusers.d/faciolock.conf /usr/share/pam-configs/faciolock
	rm -f "$secdir/pam_faciolock.so" /usr/bin/faciolockctl
	rm -f /usr/share/polkit-1/actions/org.faciolock.policy
	rm -rf "$libexec" "$share"
	systemctl daemon-reload
	echo
	echo "left in place, on purpose:"
	echo "  /etc/faciolock/       your config"
	echo "  /var/lib/faciolock/   your enrolment and its key -- rm -rf it yourself if you mean it"
	echo "  the faciolock user    userdel faciolock"
	echo "  faciolock-auth        groupdel faciolock-auth (and your membership goes with it)"
	echo
	echo "if you enabled PAM, undo that separately:  pam-auth-update --remove faciolock"
	exit 0
fi

# A debug daemon is not worth installing: an enrolment pose cost 6.6 s in target/debug against
# wiki/06's 85 ms for SFace, because `tract` unoptimised is a different program. Anything
# measured against a debug install is a measurement of the profile.
[ -x "$root/target/$profile/faciolockd" ] || { echo "build it first:  cargo build --release" >&2; exit 2; }
[ -n "$secdir" ] || { echo "no PAM security directory found under /usr/lib/*/security" >&2; exit 2; }

echo "user and group"
install -m 0644 "$root/data/sysusers.d/faciolock.conf" /usr/lib/sysusers.d/faciolock.conf
systemd-sysusers
id faciolock >/dev/null || { echo "sysusers did not create the user" >&2; exit 1; }

echo "binaries"
# `-s` because the release profile keeps `debug = true` for profiling: an unstripped
# faciolockd is 350 MB on disk. A package strips, so stripping here is what makes the
# installed tree resemble the packaged one -- and wiki/06's 164 KB resident figure for the
# PAM module is a claim about what is paged in, which stripping does not change.
install -d -m 0755 "$libexec"
install -s -m 0755 "$root/target/$profile/faciolockd" "$libexec/faciolockd"
install -s -m 0755 "$root/target/$profile/faciolockctl" /usr/bin/faciolockctl

echo "models"
install -d -m 0755 "$share/models"
install -m 0644 "$root/models"/*.onnx "$root/models/MANIFEST.sha256" "$share/models/"

echo "config directory (the file is written by diagnose, not by this script)"
# **The directory, and deliberately not the file.**
#
# An earlier version of this script created faciolockd.toml with a comment line in it, on the
# theory that a config file should exist after an install. That permanently broke
# `diagnose --write`: `probe::write_config` opens with `create_new(true)` to honour P5-e --
# never overwrite an operator's config -- so a placeholder is indistinguishable from a real
# one and the command can only ever refuse.
#
# A missing file is the designed fresh-install path, not an omission: `Config::load` and the
# CLI's `Settings::load` both read NotFound as "fresh install, use the defaults", and both say
# so in their own doc comments. So the correct thing for an installer to leave behind is
# nothing at all, and let the one command that has something to write create it.
install -d -m 0755 /etc/faciolock

echo "polkit"
install -d -m 0755 /usr/share/polkit-1/actions
install -m 0644 "$root/data/polkit/org.faciolock.policy" /usr/share/polkit-1/actions/

echo "PAM module (installed, NOT enabled)"
install -s -m 0644 "$root/target/$profile/libpam_faciolock.so" "$secdir/pam_faciolock.so"
install -d -m 0755 /usr/share/pam-configs
install -m 0644 "$root/data/pam-configs/faciolock" /usr/share/pam-configs/faciolock

echo "systemd"
install -m 0644 "$root/data/systemd/faciolock.socket" /etc/systemd/system/faciolock.socket
install -m 0644 "$root/data/systemd/faciolock.service" /etc/systemd/system/faciolock.service
systemctl daemon-reload
systemctl enable faciolock.socket
# `enable --now` is a no-op on an already-listening socket, so a reinstall that changes
# SocketMode or SocketGroup would leave the old inode in place and the new unit file unread --
# an install that reports success and did not take. Restart binds the shipped unit as written.
systemctl restart faciolock.socket

echo
systemd-analyze security faciolock.service | tail -1
echo
echo "installed. nothing is running yet -- the socket is listening and the daemon starts on"
echo "the first connect. next:"
echo
echo "  faciolockctl diagnose --write     find the camera, record the tier"
echo "  faciolockctl enroll \$USER         three poses"
echo "  faciolockctl test                 without touching any PAM file"
echo
echo "for anything that authenticates in your own session -- the KDE lock screen above all --"
echo "you also have to join the group the socket is gated on, and log out and back in:"
echo
echo "  gpasswd -a \$SUDO_USER faciolock-auth"
echo
echo "only then, and read wiki/07 first:  the /etc/pam.d/sudo line, or pam-auth-update"
