#!/usr/bin/env bash
# install-local -- put a built tree onto this machine the way a package would.
#
#   sudo ./tools/install-local.sh            install
#   sudo ./tools/install-local.sh --remove   take it all back off
#
# Every other harness runs the daemon out of
# target/debug against a throwaway store in $TMPDIR, which is exactly right for testing the
# code and useless for testing the *integration* -- socket activation, the sandbox, the
# `faciolock` user, and a PAM module something other than a test harness can dlopen.
#
# It is deliberately not a package; debian/ builds those. What this does
# is put the same files in the same places so the unit files and the pam-configs profile are
# exercised as written, and `--remove` puts the machine back.
#
# The package profile enables face authentication by default. This installer
# applies the same profile through pam-auth-update and keeps password fallback.

set -eu

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
libexec=/usr/libexec/faciolock
share=/usr/share/faciolock
secdir="$(ls -d /usr/lib/*/security 2>/dev/null | head -1)"
profile=release
lockscreen=/usr/share/plasma/shells/org.kde.plasma.desktop/contents/lockscreen/LockScreen.qml
stocklock=/usr/share/plasma/shells/org.kde.plasma.desktop/contents/lockscreen/FaciolockStockLockScreen.qml
greeterdir=/usr/share/sddm/themes/faciolock

[ "$(id -u)" -eq 0 ] || { echo "needs root. try: sudo $0 $*" >&2; exit 2; }

if [ "${1:-}" = "--remove" ]; then
	if [ "$(dpkg-divert --listpackage "$lockscreen")" = plasma-faciolock-greeters ]; then
		echo "the packaged greeter is installed; remove its package instead" >&2
		exit 2
	fi
	if [ "$(dpkg-divert --listpackage "$lockscreen")" = LOCAL ] &&
	   [ "$(dpkg-divert --truename "$lockscreen")" != "$stocklock" ]; then
		echo "the lock screen has another local diversion" >&2
		exit 2
	fi
	echo "stopping and disabling"
	pam-auth-update --package --remove faciolock
	if [ "$(dpkg-divert --listpackage "$lockscreen")" = LOCAL ] &&
	   [ "$(dpkg-divert --truename "$lockscreen")" = "$stocklock" ]; then
		rm -f "$lockscreen" /usr/share/plasma/shells/org.kde.plasma.desktop/contents/lockscreen/FaceScanIndicator.qml
		dpkg-divert --local --remove --rename "$lockscreen"
	fi
	if [ -f /etc/sddm.conf.d/zz-faciolock.conf ] &&
	   [ "$(cat /etc/sddm.conf.d/zz-faciolock.conf)" = "$(printf '[Theme]\nCurrent=faciolock')" ]; then
		rm -f /etc/sddm.conf.d/zz-faciolock.conf
	fi
	rm -rf "$greeterdir"
	systemctl disable --now faciolock.socket 2>/dev/null || true
	systemctl stop faciolock.service 2>/dev/null || true
	rm -f /etc/systemd/system/faciolock.socket /etc/systemd/system/faciolock.service
	rm -f /usr/lib/sysusers.d/faciolock.conf /usr/share/pam-configs/faciolock
	rm -f "$secdir/pam_faciolock.so" /usr/bin/faciolockctl
	rm -f /usr/share/polkit-1/actions/org.faciolock.policy
	rm -f /usr/share/applications/org.faciolock.panel.desktop
	rm -f /usr/share/applications/kcm_faciolock.desktop
	rm -f /usr/lib/*/qt6/plugins/plasma/kcms/systemsettings/kcm_faciolock.so
	rm -rf "$libexec" "$share"
	systemctl daemon-reload
	echo
	echo "left in place, on purpose:"
	echo "  /etc/faciolock/       your config"
	echo "  /var/lib/faciolock/   your enrolment and its key -- rm -rf it yourself if you mean it"
	echo "  the faciolock user    userdel faciolock"
	echo "  faciolock-auth        groupdel faciolock-auth (and your membership goes with it)"
	echo
	exit 0
fi

# A debug daemon is not worth installing: an enrolment pose cost 6.6 s in target/debug against
# ~85 ms for SFace optimised, because `tract` unoptimised is a different program. Anything
# measured against a debug install is a measurement of the profile.
[ -x "$root/target/$profile/faciolockd" ] || { echo "build it first:  cargo build --release" >&2; exit 2; }
[ -n "$secdir" ] || { echo "no PAM security directory found under /usr/lib/*/security" >&2; exit 2; }
[ -f "$lockscreen" ] || [ "$(dpkg-divert --listpackage "$lockscreen")" = LOCAL ] || { echo "Plasma lock screen file not found" >&2; exit 2; }
[ -f /usr/share/sddm/themes/breeze/Main.qml ] || { echo "Breeze SDDM theme not found" >&2; exit 2; }
owner=$(dpkg-divert --listpackage "$lockscreen")
[ -z "$owner" ] || [ "$owner" = LOCAL ] || { echo "lock screen is diverted by $owner" >&2; exit 2; }
[ "$owner" != LOCAL ] || [ "$(dpkg-divert --truename "$lockscreen")" = "$stocklock" ] || { echo "lock screen has another local diversion" >&2; exit 2; }

echo "user and group"
install -m 0644 "$root/data/sysusers.d/faciolock.conf" /usr/lib/sysusers.d/faciolock.conf
systemd-sysusers
id faciolock >/dev/null || { echo "sysusers did not create the user" >&2; exit 1; }

echo "binaries"
# `-s` because the release profile keeps `debug = true` for profiling: an unstripped
# faciolockd is 350 MB on disk. A package strips, so stripping here is what makes the
# installed tree resemble the packaged one -- and the measured 164 KB resident figure for the
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
# `diagnose --write`: `probe::write_config` opens with `create_new(true)` --
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

echo "PAM module and default profile"
install -s -m 0644 "$root/target/$profile/libpam_faciolock.so" "$secdir/pam_faciolock.so"
install -d -m 0755 /usr/share/pam-configs
install -m 0644 "$root/data/pam-configs/faciolock" /usr/share/pam-configs/faciolock
pam-auth-update --package

echo "login and lock-screen themes"
install -d -m 0755 "$greeterdir"
install -m 0644 "$root/data/greeters/sddm/faciolock"/* "$greeterdir/"
install -m 0644 "$root/data/greeters/FaceScanIndicator.qml" "$greeterdir/"
install -d -m 0755 /etc/sddm.conf.d /etc/xdg
if [ ! -e /etc/sddm.conf.d/zz-faciolock.conf ]; then
	printf '[Theme]\nCurrent=faciolock\n' > /etc/sddm.conf.d/zz-faciolock.conf
fi
if [ "$owner" != LOCAL ]; then
	dpkg-divert --local --add --rename --divert "$stocklock" "$lockscreen"
fi
install -m 0644 "$root/data/greeters/lockscreen/LockScreen.qml" "$lockscreen"
install -m 0644 "$root/data/greeters/FaceScanIndicator.qml" "$(dirname "$lockscreen")/"
if awk '{ sub(/#.*/, ""); for (i = 1; i <= NF; i++) if ($i == "pam_faciolock.so") found=1 } END { exit !found }' /etc/pam.d/common-auth; then
	enabled=true
else
	enabled=false
fi
config=$(mktemp /etc/xdg/.faciolock-greeter.conf.XXXXXX)
chmod 0644 "$config"
printf '[FaceUnlock]\nenabled=%s\n' "$enabled" > "$config"
mv -f "$config" /etc/xdg/faciolock-greeter.conf

# Built here rather than by cargo, so it is skipped when the KF6 development packages are
# absent instead of failing an install that has nothing else wrong with it. The panel is the
# one part of this tree that is optional at runtime.
if cmake -S "$root/kcm" -B "$root/target/kcm" -DCMAKE_INSTALL_PREFIX=/usr >/dev/null 2>&1; then
	echo "KDE settings module"
	cmake --build "$root/target/kcm" >/dev/null
	cmake --install "$root/target/kcm" >/dev/null
else
	echo "KDE settings module -- skipped, no KF6 development packages"
fi

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
echo "installed. the socket is listening and the daemon starts on the first connect. next:"
echo
echo "  faciolockctl diagnose --write     find the camera, record the tier"
echo "  faciolockctl enroll \$USER         three poses"
echo "  faciolockctl test                 without touching any PAM file"
echo "  kcmshell6 kcm_faciolock           the same three, in a window"
echo
echo "for anything that authenticates in your own session -- the KDE lock screen above all --"
echo "you also have to join the group the socket is gated on, and log out and back in:"
echo
echo "  gpasswd -a \$SUDO_USER faciolock-auth"
echo
echo "face authentication is enabled by default; use faciolockctl disable to turn it off"
