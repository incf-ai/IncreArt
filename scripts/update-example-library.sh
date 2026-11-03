#!/usr/bin/env bash
# Replaces the repo's example library, example-library/, with a library
# built by `demo_library build`:
#
#   scripts/update-example-library.sh <library> <demo-sources>
#
# Release zips ship it as "Example library" beside the app. Copied without
# the caches (cache/ names model files by their absolute paths; thumbnails
# are made again on open) or the lock, plus the sources' credits.json.
# library.rsn names no link mode: the app probes one on first open and
# turns the links into links of that kind (symlinks where it can), so one
# library serves every OS. Git keeps its symlinks as symlinks, or where it
# can't make them (Windows) as small files holding the target path, which
# the app takes as links too; the release zips hold those files, which every
# unzip tool keeps.
#
# The library is checked first with `increart --check-library` (the binary
# in $INCREART, else target/release or target/debug), which brings its
# metadata up to date: no problems and no seeds left to generate (they would
# be queued for whoever opens it).
set -euo pipefail

lib=${1:?library folder}
sources=${2:?demo sources folder (for credits.json)}
root=$(realpath "$(dirname "$0")/..")
dest=$root/example-library
[[ -f $lib/library.rsn ]] || { echo "$lib is not a library" >&2; exit 1; }
[[ -f $sources/credits.json ]] || { echo "no credits.json in $sources" >&2; exit 1; }
grep -q 'link_mode: Symlink' "$lib/library.rsn" || { echo "expected a symlink library" >&2; exit 1; }
increart=${INCREART:-}
for b in "$root/target/release/increart" "$root/target/debug/increart"; do
    [[ -n $increart || ! -x $b ]] || increart=$b
done
[[ -n $increart ]] || { echo "no increart binary: build it, or set INCREART" >&2; exit 1; }
report=$("$increart" --check-library "$lib") || { echo "$report" >&2; exit 1; }
echo "$report"
if grep -q 'seeds to generate' <<<"$report"; then
    echo "the library has runs still to generate: finish or cancel them" >&2
    exit 1
fi
grep -q 'metadata: 3' "$lib/library.rsn" || { echo "expected metadata 3 after the check" >&2; exit 1; }

stage=$(mktemp -d)
trap 'rm -rf "$stage"' EXIT
cp -r "$lib" "$stage/lib"   # symlinks stay symlinks
rm -rf "$stage/lib/cache" "$stage/lib/thumbs" "$stage/lib/library.lock"
cp "$sources/credits.json" "$stage/lib/credits.json"
sed -i '/link_mode: /d' "$stage/lib/library.rsn"

# nothing personal left behind: no absolute paths of this machine
if grep -rl -e "$HOME" -e /mnt/ -e /workspace "$stage/lib"; then
    echo "the files above name paths of this machine" >&2
    exit 1
fi
# every link resolves inside the library
if find "$stage/lib" -xtype l | grep .; then
    echo "the links above are broken" >&2
    exit 1
fi

rm -rf "$dest"
mv "$stage/lib" "$dest"
du -sh "$dest"
echo "now: git add -A example-library"
