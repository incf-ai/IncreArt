#!/usr/bin/env bash
# Downloads a URL to a file, for the workflows: up to 8 tries, each going
# on from what the last one got. A connection reset partway isn't among
# what curl's --retry tries again, and it failed a release's run 5.8 GiB of
# models in.
#
#   scripts/fetch.sh <url> <file>
set -euo pipefail
url=$1 file=$2
for try in 1 2 3 4 5 6 7 8; do
    curl -fsSL --retry 5 -C - -o "$file" "$url" && exit 0
    echo "$file: try $try stopped at $(wc -c < "$file" 2> /dev/null || echo 0) bytes" >&2
    sleep 10
done
echo "$file: gave up after 8 tries" >&2
exit 1
