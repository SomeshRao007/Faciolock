#!/usr/bin/env bash
# check-qml-deps -- every QML module the panel imports must be a declared dependency.
#
# `kcmutils_add_qml_kcm` compiles ui/ into a Qt resource
# inside the plugin, so the imports are string data in the .so and `dh_shlibdeps` -- which
# reads DT_NEEDED out of ELF -- has nothing to find. The C++ half of a QML KCM declares its
# dependencies. The QML half does not, and nothing warns you.
#
# The failure it prevents is silent: the package installs, registers, launches, and shows a
# blank page, with import errors on a stderr nobody reads. It cannot fail on a machine that
# has Plasma installed, which is every machine that builds it.
#
# Resolves against what is installed here, so it needs the KF6/Qt6 QML modules present --
# true on any machine that can build the KCM at all.

set -eu

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
qml="$root/kcm/ui/main.qml"
control="$root/debian/control"
qmldir="$(qmake6 -query QT_INSTALL_QML 2>/dev/null || echo /usr/lib/x86_64-linux-gnu/qt6/qml)"

depends="$(awk '/^Package: plasma-faciolock-kcm$/{p=1} p&&/^Depends:/{d=1} d{print} d&&/^[A-Z][a-z-]*:/&&!/^Depends:/{exit}' "$control")"

missing=0
while read -r module; do
	path="$qmldir/${module//.//}"
	[ -f "$path/qmldir" ] || { echo "unresolvable: $module (no $path/qmldir)"; missing=1; continue; }

	pkg="$(dpkg -S "$path/qmldir" 2>/dev/null | cut -d: -f1)"
	[ -n "$pkg" ] || { echo "unowned: $module at $path -- not from a package?"; missing=1; continue; }

	case "$depends" in
		*"$pkg"*) printf '  ok  %-28s %s\n' "$module" "$pkg" ;;
		*) echo "MISSING from Depends: $module needs $pkg"; missing=1 ;;
	esac
done < <(grep -oP '^import \K[A-Za-z][A-Za-z0-9._]*' "$qml" | sort -u)

[ "$missing" -eq 0 ] || {
	echo
	echo "Add the package(s) above to plasma-faciolock-kcm's Depends in debian/control."
	echo "dh_shlibdeps cannot find these; they are not ELF dependencies."
	exit 1
}

echo "every QML import is a declared dependency"
