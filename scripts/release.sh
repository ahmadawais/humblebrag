#!/usr/bin/env bash
# Release humblebrag to npm from your own machine. No npm token lives in CI.
#
#   ./scripts/release.sh            release the version already in Cargo.toml
#   ./scripts/release.sh 0.2.0      bump Cargo.toml to 0.2.0 first, then release
#
# Steps: bump and tag, wait for GitHub Actions to build the 5 binaries into a
# GitHub release, download them, pack them into the one `humblebrag` npm package,
# then `npm publish` it. npm asks you to approve once in the browser (Touch ID).
# Safe to run again: it skips anything already tagged, built, or published.
set -euo pipefail
cd "$(dirname "$0")/.."

say() { printf '\033[1;35m==>\033[0m %s\n' "$*"; }
die() { printf '\033[1;31merror:\033[0m %s\n' "$*" >&2; exit 1; }

for bin in git gh npm node cargo; do command -v "$bin" >/dev/null || die "$bin not found"; done

[ "$(git branch --show-current)" = main ] || die "switch to main first"
[ -z "$(git status --porcelain)" ] || die "working tree is not clean"
git fetch -q origin main --tags
[ "$(git rev-parse HEAD)" = "$(git rev-parse origin/main)" ] || die "main is not in sync with origin/main"

current=$(grep -m1 '^version' Cargo.toml | cut -d'"' -f2)
version=${1:-$current}
version=${version#v}
tag="v$version"
[[ $version =~ ^[0-9]+\.[0-9]+\.[0-9]+(-[0-9A-Za-z.]+)?$ ]] || die "bad version: $version"

# 1. npm login first, so nothing gets tagged if you can't publish.
if ! npm whoami >/dev/null 2>&1; then
	say "Log in to npm (opens the browser)"
	npm login --auth-type=web
fi
say "npm user: $(npm whoami)"

# 2. Bump the version if asked.
if [ "$version" != "$current" ]; then
	say "Bump $current to $version"
	sed -i '' -e "1,/^version = /s/^version = \".*\"/version = \"$version\"/" Cargo.toml
	cargo check --quiet
	git commit -qm "chore: release $tag" Cargo.toml Cargo.lock
	git push -q origin main
fi

# 3. Tag and push, which starts the build workflow.
if git ls-remote --exit-code --tags origin "refs/tags/$tag" >/dev/null; then
	say "Tag $tag already on GitHub"
else
	say "Tag $tag and push"
	git tag -a "$tag" -m "$tag"
	git push -q origin "$tag"
fi

# 4. Wait for the release workflow to finish.
if ! gh release view "$tag" >/dev/null 2>&1; then
	say "Waiting for the release workflow"
	run=""
	for _ in $(seq 30); do
		run=$(gh run list --workflow release.yml --branch "$tag" --limit 1 --json databaseId -q '.[0].databaseId')
		[ -n "$run" ] && break
		sleep 2
	done
	[ -n "$run" ] || die "no workflow run found for $tag"
	gh run watch "$run" --exit-status --interval 10 || die "build failed: gh run view $run --log-failed"
fi

# 5. Download the binaries and build the npm packages.
dist=npm-dist
rm -rf "$dist" && mkdir -p "$dist/bins"
say "Download binaries from the $tag release"
gh release download "$tag" -D "$dist/bins" -p 'humblebrag-*' -p SHA256SUMS
(cd "$dist/bins" && shasum -a 256 -c --quiet SHA256SUMS) || die "checksum mismatch"
rm "$dist/bins/SHA256SUMS"
host="darwin-$(uname -m | sed 's/x86_64/x64/')"
chmod +x "$dist/bins/humblebrag-$host"
got=$("$dist/bins/humblebrag-$host" --version)
[ "$got" = "humblebrag $version" ] || die "downloaded binary says '$got', expected 'humblebrag $version'"
node scripts/npm-build.mjs --version "$version" --bins "$dist/bins" --out "$dist/pkg"

# 6. Publish. One browser approval.
if npm view "humblebrag@$version" version >/dev/null 2>&1; then
	say "humblebrag@$version already on npm"
else
	say "Publish humblebrag@$version (approve in the browser)"
	npm publish "$dist/pkg/humblebrag" --access public --auth-type=web
fi

say "Done: https://www.npmjs.com/package/humblebrag  (try: npx humblebrag@$version ahmadawais)"
