#!/usr/bin/env bash
# Publish GitHub releases to npm from your own machine: `pnpm release`.
#
# GitHub Actions makes a GitHub release (binaries + SHA256SUMS) on every push to
# main. This script finds the releases newer than what's on npm and publishes
# each one, oldest first, so `latest` ends on the newest. npm asks you to approve
# each publish in the browser (passkey or Touch ID). No npm token is involved.
#
#   pnpm release              publish every release newer than npm's latest
#   pnpm release --latest     publish only the newest release
#   pnpm release --dry-run    build and test, don't publish
set -euo pipefail
cd "$(dirname "$0")/.."

say() { printf '\033[1;35m==>\033[0m %s\n' "$*"; }
die() { printf '\033[1;31merror:\033[0m %s\n' "$*" >&2; exit 1; }

only_latest=0 dry=0
for arg in "$@"; do
	case $arg in
		--latest) only_latest=1 ;;
		--dry-run) dry=1 ;;
		*) die "unknown option: $arg" ;;
	esac
done
for bin in gh npm node shasum; do command -v "$bin" >/dev/null || die "$bin not found"; done

# Is version $1 newer than $2? Plain x.y.z only.
newer() { [ "$1" != "$2" ] && [ "$(printf '%s\n%s\n' "$1" "$2" | sort -V | tail -1)" = "$1" ]; }

npm_latest=$(npm view humblebrag version 2>/dev/null || true)
say "npm latest: ${npm_latest:-none}"

todo=""
for tag in $(gh release list --limit 100 --exclude-drafts --exclude-pre-releases --json tagName -q '.[].tagName' | sort -V); do
	v=${tag#v}
	[[ $v =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]] || continue
	if [ -z "$npm_latest" ] || newer "$v" "$npm_latest"; then todo="$todo $v"; fi
done
todo=${todo# }
[ -n "$todo" ] || { say "Nothing to publish"; exit 0; }
[ "$only_latest" = 1 ] && todo=${todo##* }
say "To publish: $todo"

if [ "$dry" = 0 ] && ! npm whoami >/dev/null 2>&1; then
	say "Log in to npm (opens the browser)"
	npm login --auth-type=web
fi

host="darwin-$(uname -m | sed 's/x86_64/x64/')"
for version in $todo; do
	tag="v$version"
	dist="npm-dist/$version"
	rm -rf "$dist" && mkdir -p "$dist/bins"

	say "$tag: fetch checksums and the $host binary"
	gh release download "$tag" -D "$dist/bins" -p SHA256SUMS -p "humblebrag-$host"
	(cd "$dist/bins" && grep " humblebrag-$host\$" SHA256SUMS | shasum -a 256 -c --quiet) || die "checksum mismatch"
	node scripts/npm-build.mjs --version "$version" --sums "$dist/bins/SHA256SUMS" --out "$dist/pkg"

	# Install the packed package and run it, which downloads the binary from
	# GitHub the way a user's first run would.
	say "$tag: test npm install and first run"
	t=$(mktemp -d)
	(cd "$dist/pkg/humblebrag" && npm pack -q --pack-destination "$t" >/dev/null)
	(cd "$t" && npm init -y >/dev/null && npm install -q --no-audit --no-fund "./humblebrag-$version.tgz" >/dev/null)
	got=$(cd "$t" && XDG_CACHE_HOME="$t/cache" ./node_modules/.bin/humblebrag --version 2>/dev/null || true)
	rm -rf "$t"
	[ "$got" = "humblebrag $version" ] || die "installed package says '$got', expected 'humblebrag $version'"
	echo "  ok: $got"

	if [ "$dry" = 1 ]; then
		say "$tag: dry run, not publishing"
	else
		say "$tag: publish (approve in the browser)"
		npm publish "$dist/pkg/humblebrag" --access public --auth-type=web
	fi
done

say "Done: https://www.npmjs.com/package/humblebrag"
