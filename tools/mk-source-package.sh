#!/usr/bin/env bash
# mk-source-package -- assemble a buildable source tree and build it.
#
#   ./tools/mk-source-package.sh            source package only (for dput)
#   ./tools/mk-source-package.sh --binary   also build the .debs here
#
# Launchpad's builders have no network, so every crate the workspace needs must
# already be inside the source package. That is the whole reason this script
# exists instead of a plain `dpkg-buildpackage`: vendor/ is ~194 MB, it must not
# live in git, and it must be in the tarball.
#
# The tree is assembled from what git tracks plus the models, which are
# gitignored on purpose (38 MB of weights). corpus/, spikes/, fuzz/ and target/
# are all excluded by not being tracked as source, so nothing has to remember to
# exclude them -- and `fuzz/` staying out is load-bearing: libfuzzer-sys must
# never reach the lockfile the PAM module's dependency-tree test rests on.

set -eu

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
out="${root}/../faciolock-build"
name="faciolock-$(dpkg-parsechangelog -l"$root/debian/changelog" -SVersion)"
tree="$out/$name"

command -v dpkg-buildpackage >/dev/null || {
	echo "needs: sudo apt install debhelper devscripts lintian" >&2; exit 2; }

# Before anything is assembled, because this is the one class of defect a build cannot
# find: the KCM's QML imports are inside a Qt resource, dh_shlibdeps reads ELF, and a
# missing declaration ships a package that installs fine and renders a blank page on any
# machine that is not this one.
"$root/tools/check-qml-deps.sh"
echo

echo "assembling $tree"
# Keep the vendored tree across rebuilds. It is byte-identical for a given
# Cargo.lock and re-fetching 156 crates to iterate on debian/rules is minutes
# per attempt for nothing. `cargo vendor` runs again the moment it is missing.
cache="$out/.vendor-cache"
[ -d "$tree/vendor" ] && { rm -rf "$cache"; mv "$tree/vendor" "$cache"; }
rm -rf "$tree"
mkdir -p "$tree"

# Tracked files plus anything new that .gitignore does not object to, so an
# uncommitted debian/ still packages.
(cd "$root" && git ls-files --cached --others --exclude-standard -z) \
	| (cd "$root" && rsync -a --files-from=- --from0 . "$tree/")

echo "models"
cp "$root/models"/*.onnx "$tree/models/"

mkdir -p "$tree/.cargo"
if [ -d "$cache" ]; then
	echo "vendor (cached)"
	mv "$cache" "$tree/vendor"
	printf '[source.crates-io]\nreplace-with = "vendored-sources"\n\n[source.vendored-sources]\ndirectory = "vendor"\n' \
		> "$tree/.cargo/config.toml"
else
	echo "vendoring (~194 MB, a few minutes on a cold cargo registry)"
	(cd "$tree" && cargo vendor --versioned-dirs vendor > .cargo/config.toml)
fi

echo "building"
cd "$tree"
if [ "${1:-}" = "--binary" ]; then
	dpkg-buildpackage -us -uc -b
else
	dpkg-buildpackage -us -uc -S
fi

echo
echo "artefacts in $out"
ls -1 "$out" | grep -v "^$name\$" || true
echo
echo "lintian:"
lintian "$out"/*.changes || true
