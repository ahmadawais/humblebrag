# humblebrag

humblebrag counts every npm download of a package or of all packages by one npm user, and the GitHub stars, forks, watchers, and followers that go with them. npm counts start at the first day npm recorded downloads and end at the last day npm has published.

```sh
npx humblebrag ahmadawais
```

`hb` is a shorter name for the same command.

```
  npm downloads · user ahmadawais · 2015-01-10 → 2026-11-25
  ─────────────────────────────────────────────
  TOTAL                 88,010,132
  packages                     179
  last 30 days           2,455,344
  last 365 days         25,405,225
  peak day                 156,749  (2026-06-02)

  by year
  2019         182,520
  2020         405,043  █
  2021       2,472,937  ███
  2022       8,658,909  ███████████
  2023      14,177,418  ██████████████████
  2024      18,294,253  ███████████████████████
  2025      20,123,210  ██████████████████████████
  2026      23,569,951  ██████████████████████████████

  top 5 packages
      16,077,593   18.3%  clear-any-console
      16,015,001   18.2%  cli-welcome
      15,497,063   17.6%  cli-handle-error
      15,464,833   17.6%  cli-handle-unhandled
      11,668,679   13.3%  cli-check-node

  github · user ahmadawais
  ─────────────────────────────────────────────
  stars                     12,820
  forks                      1,609
  watchers                     571
  followers                  7,126
  repos                        280  (public, not forks)

  top 3 repos
                  stars     forks  watchers
  corona-cli      1,833       181        37
  WPGulp          1,753       285        88
  Emoji-Log         902        72        15
```

## Usage

```sh
hb <name>                          # npm and GitHub for a user or a package
hb npm <name>                      # npm downloads only
hb gh <login>                      # GitHub only: a user or an org
hb gh <owner/repo>                 # GitHub only: one repo
hb gh vuejs vitejs                 # several GitHub targets, added together
hb contrib <login>                 # merged PRs to repos you don't own
hb onboard                         # set up a profile
hb npm react vue svelte            # several packages, added together
hb npm ~<user>                     # an npm user, when the name is also a package
hb ahmadawais --gh AhmadAwais      # GitHub login differs from the npm name
hb ahmadawais --html report.html --json report.json
```

Run it with `npx humblebrag`, or install it with `npm i -g humblebrag` to get the `humblebrag` and `hb` commands.

For an npm user, the GitHub part uses the same login. For a package, it uses the repository in the package's `package.json`. GitHub counts cover public repos the account owns and leave out forks. They match what [starz](https://github.com/yyx990803/starz) and [forkz](https://github.com/ahmadawais/forkz) report, plus watchers and followers.

GitHub allows 60 requests an hour without a token, which is enough for most users but leaves out watchers. humblebrag uses `GITHUB_TOKEN`, `GH_TOKEN`, or the GitHub CLI's login (`gh auth token`) when one is there. The token is only sent to api.github.com.

The `--html` file is one self-contained page. It has monthly, yearly, and cumulative charts, and a table of packages you can sort and filter. Click a package to chart it alone.

When a name is both a package and a user, humblebrag picks the package and prints the command for the user.

## Profiles

A profile groups everything that counts as your work under one name. Run `hb onboard` to make one. It asks for:

- npm users and packages
- GitHub users, orgs, and repos
- whether to track your merged PRs to other people's repos
- your handles on sites humblebrag can read: YouTube, Bluesky, Mastodon, Stack Overflow, Hacker News
- your follower counts on sites that need a login to read, such as X, Instagram, LinkedIn, Reddit, and Quora. You type these in and update them by running `hb onboard` again.

Then `hb <profile>` shows all of it with one total audience. Each run also writes one line per day to `history-<profile>.jsonl` and prints what changed since the last day you ran it. A year of daily runs is about 100 KB.

The config lives at `~/.config/humblebrag/config.json`. If you tell `hb onboard` to save it somewhere else, such as a dotfiles repo, it links the default path to that file. The history and contribution records go in the same folder as the config. `HUMBLEBRAG_CONFIG` overrides the path.

```json
{
  "profiles": {
    "ahmadawais": {
      "npm": ["~ahmadawais"],
      "github": ["ahmadawais", "CommandCodeAI"],
      "contributions": true,
      "social": { "youtube": "@AhmadAwais", "bluesky": "ahmadawais.com", "stackoverflow": "950111", "x": 100000 }
    }
  }
}
```

## Contributions

`hb contrib <login>` lists merged pull requests to public repos you don't own, sorted by the repo's stars. With a profile, repos owned by any of its GitHub accounts are left out too. Every run merges the results into `contributions-<login>.json`, so a PR stays in the record even if GitHub stops returning it. This needs a GitHub token. GitHub search returns at most 1,000 results per query, so humblebrag searches one year at a time.

## Options

```
--gh <target>       GitHub login or owner/repo, when it differs from the npm name
--no-profile        ignore a profile with the same name
--all               list every package, repo and contribution
--no-record         hb contrib: don't update the record file
--from YYYY-MM-DD   start date (default 2015-01-10, the first day npm has data)
--to   YYYY-MM-DD   end date (default: the last day npm has data)
--json <file>       write npm daily and per-package monthly totals, and GitHub counts, as JSON
--html <file>       write an interactive HTML report
--top <n>           number of packages and repos to list (default 25)
-c <n>              max concurrent requests (default 8)
--ttl <minutes>     reuse cached recent days for this long (default 360)
--fresh             fetch recent days again now
--no-cache          don't read or write the cache
--rate <n>          sustained requests per second to the downloads API (default 0.4)
--burst <n>         requests sent before pacing starts (default 10)
--exact             count downloads from before a package's first publish
-V, --version       print the version
```

## How it fetches the data

The npm downloads API at api.npmjs.org blocks a client after about 20 quick requests. After that it allows about one request every 2.5 seconds. Each request costs time, so humblebrag sends as few as it can.

1. The bulk endpoint takes up to 128 packages and 365 days in one request. A user with 170 packages and 12 years of history needs about 24 requests. One request per package per period would take about 2,000.
2. The bulk endpoint rejects scoped packages like `@scope/name`. humblebrag requests each scoped package alone, 540 days at a time, which is the longest range the API accepts.
3. humblebrag reads each package's first publish date from registry.npmjs.org. That host has no such limit. It then skips the years before the first publish, so older bulk requests carry fewer packages.
4. humblebrag stores daily counts in `~/.cache/humblebrag`. npm doesn't change a day's count after 3 days, so humblebrag never fetches those days again. It reuses the most recent days for 6 hours.
5. A token bucket paces requests to stay under the limit. When npm still answers 429, every request waits, so the client doesn't dig deeper into the block.

These numbers come from `ahmadawais`, which has 179 packages and data from 2015 to 2026:

| Run | Requests | Time |
|---|---|---|
| First run, empty cache | 42 | 1 to 2 minutes, almost all of it waiting on the rate limit |
| Again within 6 hours | 0 | 0.3 seconds |
| Again after 6 hours | about 14 | 1 to 2 seconds |

A user with many scoped packages takes longer on the first run, because each scoped package needs its own requests.

## Notes

- npm has daily download counts from 2015-01-10. It has no counts for earlier days.
- npm returns zeros for days it hasn't processed yet. humblebrag asks npm for the last processed day and stops there.
- A user's packages are the packages the registry lists for that user plus the packages a `maintainer:<user>` search returns.
- npm lets a new owner reuse the name of a deleted package. By default, humblebrag leaves out downloads from before the current package's first publish. `--exact` counts them.

## How npx runs a native binary

The `humblebrag` npm package is a short Node script plus the SHA-256 sum of each prebuilt binary. On first run the script downloads the binary for your OS and CPU from the matching GitHub release, checks it against the sum, and caches it in `~/.cache/humblebrag/bin/`. Later runs start the cached binary directly. It needs Node 18 or newer.

| Binary | Platform |
|---|---|
| `humblebrag-darwin-arm64` | macOS on Apple silicon |
| `humblebrag-darwin-x64` | macOS on Intel |
| `humblebrag-linux-x64` | Linux x64, static musl build |
| `humblebrag-linux-arm64` | Linux arm64, static musl build |
| `humblebrag-win32-x64.exe` | Windows x64 |

## Build from source

humblebrag is written in Rust.

```sh
cargo build --release
./target/release/humblebrag ahmadawais
```

`scripts/install.sh` builds it and copies the binary to `~/.local/bin`.

## Release

Every push to `main` builds the five binaries and publishes them as a GitHub release, with a `SHA256SUMS` file. The version is the major and minor from `Cargo.toml` plus the next free patch number. With `Cargo.toml` at 0.1.0 and v0.1.4 released, the next push makes v0.1.5. To start 0.2, set `Cargo.toml` to 0.2.0.

Publishing to npm is manual. Run `pnpm release` on your machine. It finds the GitHub releases newer than the version on npm and publishes each one. For each, it builds the package, installs it in a temp folder, runs it to test the download, then publishes. npm asks you to approve each publish in the browser with a passkey or Touch ID. No npm token is stored anywhere.

- `pnpm release --latest` publishes only the newest release.
- `pnpm release --dry-run` builds and tests without publishing.

## License

MIT
