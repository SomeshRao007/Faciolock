#!/bin/sh
set -eu

root=$(cd "$(dirname "$0")/.." && pwd)
stage=$(mktemp -d)
trap 'rm -rf "$stage"' EXIT

cp "$root/data/greeters/lockscreen/LockScreen.qml" \
   "$root/data/greeters/FaceScanIndicator.qml" \
   "$root/tests/greeters/FaciolockStockLockScreen.qml" \
   "$root/tests/greeters/tst_lockscreen.qml" "$stage/"

QT_QPA_PLATFORM=offscreen /usr/lib/qt6/bin/qmltestrunner -input "$stage" -platform offscreen

mkdir -p "$stage/sddm/themes/faciolock"
cp "$root/data/greeters/sddm/faciolock"/* \
   "$root/data/greeters/FaceScanIndicator.qml" \
   "$stage/sddm/themes/faciolock/"
ln -s /usr/share/sddm/themes/breeze "$stage/sddm/themes/breeze"

set +e
QT_QPA_PLATFORM=offscreen timeout 4s sddm-greeter-qt6 --test-mode \
    --theme "$stage/sddm/themes/faciolock" > "$stage/sddm.log" 2>&1
result=$?
set -e
cat "$stage/sddm.log"
[ "$result" -eq 0 ] || [ "$result" -eq 124 ]
! grep -E 'Main.qml:[0-9]+:[0-9]+:|QQmlApplicationEngine failed|Failed to load theme|Type .* unavailable|Error loading QML' "$stage/sddm.log"

mkdir -p "$stage/share/plasma/shells"
cp -a /usr/share/plasma/shells/org.kde.plasma.desktop \
    "$stage/share/plasma/shells/org.faciolock.testing"
lockdir="$stage/share/plasma/shells/org.faciolock.testing/contents/lockscreen"
mv "$lockdir/LockScreen.qml" "$lockdir/FaciolockStockLockScreen.qml"
sed 's/property bool faceEnabled: faceSettings.enabled/property bool faceEnabled: false/' \
    "$root/data/greeters/lockscreen/LockScreen.qml" > "$lockdir/LockScreen.qml"
cp "$root/data/greeters/FaceScanIndicator.qml" "$lockdir/"
sed -i 's/"Id": "org.kde.plasma.desktop"/"Id": "org.faciolock.testing"/' \
    "$stage/share/plasma/shells/org.faciolock.testing/metadata.json"

set +e
XDG_DATA_HOME="$stage/share" QT_QPA_PLATFORM=offscreen timeout 4s \
    /usr/lib/x86_64-linux-gnu/libexec/kscreenlocker_greet --testing \
    --shell org.faciolock.testing > "$stage/lockscreen.log" 2>&1
result=$?
set -e
cat "$stage/lockscreen.log"
[ "$result" -eq 0 ] || [ "$result" -eq 124 ]
! grep -E 'Error loading|is not a type|is not installed|Cannot assign|Cannot anchor|not found|FaciolockStockLockScreen.qml:[0-9]+:[0-9]+:|LockScreen.qml:[0-9]+:[0-9]+:' "$stage/lockscreen.log"

mkdir -p "$stage/dpkg" "$stage/diversion"
entry="$stage/diversion/LockScreen.qml"
stock="$stage/diversion/FaciolockStockLockScreen.qml"
cp /usr/share/plasma/shells/org.kde.plasma.desktop/contents/lockscreen/LockScreen.qml "$entry"
dpkg-divert --admindir "$stage/dpkg" --package plasma-faciolock-greeters \
    --add --rename --divert "$stock" "$entry"
[ "$(dpkg-divert --admindir "$stage/dpkg" --listpackage "$entry")" = plasma-faciolock-greeters ]
[ "$(dpkg-divert --admindir "$stage/dpkg" --truename "$entry")" = "$stock" ]
cp "$root/data/greeters/lockscreen/LockScreen.qml" "$entry"
rm -f "$entry"
dpkg-divert --admindir "$stage/dpkg" --package plasma-faciolock-greeters \
    --remove --rename "$entry"
cmp "$entry" /usr/share/plasma/shells/org.kde.plasma.desktop/contents/lockscreen/LockScreen.qml
