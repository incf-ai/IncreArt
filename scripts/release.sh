#!/usr/bin/env bash
# Releases main's commit as a tag, once CI has passed on it: checks it here
# (clippy and the tests, with rust-toolchain.toml's toolchain, CI's), pushes
# main, waits for the CI run of that commit (Linux, macOS and Windows on
# links, hard links and copies), and only then makes the annotated tag and
# pushes it, which starts the release (.github/workflows/release.yml; that
# runs CI on the tag again and publishes nothing unless it passes).
#
#   scripts/release.sh v0.1.0-rc21 notes.txt
#
# notes.txt is the tag's message. Tags used to be pushed along with main,
# and what only another system shows (Windows' paths and error wording, a
# slower runner) failed CI after the release was out.
#
# GITHUB_TOKEN, when set, is used to ask GitHub (60 asks an hour without).
set -euo pipefail

tag=${1:?usage: scripts/release.sh <tag> <notes file>}
notes=${2:?usage: scripts/release.sh <tag> <notes file>}
cd "$(dirname "$0")/.."

# (a remote's address may hold a token: never shown)
hide() { sed -E 's#//[^@/]*@#//#g'; }
fail() { echo "release: $*" >&2; exit 1; }

[[ -s $notes ]] || fail "$notes is empty or missing"
[[ $(git branch --show-current) == main ]] || fail "not on main"
[[ -z $(git status --porcelain --untracked-files=no) ]] || fail "uncommitted changes"
git rev-parse -q --verify "refs/tags/$tag" > /dev/null && fail "tag $tag exists"
version=$(sed -nE 's/^version = "(.*)"/\1/p' crates/increart-gui/Cargo.toml | head -1)
[[ $tag == "v$version" || $tag =~ ^v${version//./\\.}-rc[0-9]+$ ]] || fail "tag $tag, but increart-gui is version $version"
repo=$(git remote get-url origin | sed -E 's#^.*github\.com[:/]##; s#\.git$##')
sha=$(git rev-parse HEAD)

echo "release: checking ${sha:0:7} here"
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace --no-fail-fast
cargo clippy -p increart-apitest --all-targets --features fake -- -D warnings
cargo test -p increart-apitest --features fake

echo "release: pushing main"
git push origin main 2>&1 | hide

api() {
    local auth=()
    [[ -n ${GITHUB_TOKEN:-} ]] && auth=(-H "Authorization: Bearer $GITHUB_TOKEN")
    curl -fsS "${auth[@]}" -H "Accept: application/vnd.github+json" "https://api.github.com/repos/$repo/$1"
}
# the newest CI run of this commit: "<id> <status> <conclusion>"
ci_run() {
    api "actions/workflows/ci.yml/runs?head_sha=$sha&per_page=5" | python3 -c '
import json, sys
runs = json.load(sys.stdin)["workflow_runs"]
if runs:
    r = max(runs, key=lambda r: r["run_number"])
    print(r["id"], r["status"], r["conclusion"] or "-")'
}

echo "release: waiting for CI on ${sha:0:7} (https://github.com/$repo/actions)"
deadline=$((SECONDS + 5400))
while :; do
    read -r id status conclusion <<< "$(ci_run || true)"
    if [[ ${status:-} == completed ]]; then break; fi
    ((SECONDS < deadline)) || fail "CI hasn't finished in 90 min"
    sleep 90
done
if [[ $conclusion != success ]]; then
    api "actions/runs/$id/jobs?per_page=50" | python3 -c '
import json, sys
for j in json.load(sys.stdin)["jobs"]:
    if j["conclusion"] not in ("success", "skipped"):
        steps = [s["name"] for s in j["steps"] if s["conclusion"] == "failure"]
        print("  " + j["name"] + ": " + j["conclusion"] + (" in " + "; ".join(steps) if steps else ""))' >&2 || true
    fail "CI $conclusion on ${sha:0:7}: not tagged (https://github.com/$repo/actions/runs/$id)"
fi

echo "release: CI passed; tagging $tag"
git tag -a "$tag" -F "$notes" "$sha"
git push origin "$tag" 2>&1 | hide
echo "release: $tag pushed (https://github.com/$repo/actions/workflows/release.yml)"
