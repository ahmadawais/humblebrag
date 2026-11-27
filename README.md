# humblebrag

humblebrag counts every npm download of a package or of all packages by one npm user. It starts at the first day npm recorded downloads and ends at the last day npm has published.

```sh
npx humblebrag ahmadawais
```

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
```

## Usage

```sh
npx humblebrag <name>                  # a package or a user, detected for you
npx humblebrag ~<user>                 # a user, when the name is also a package
npx humblebrag react vue svelte        # several packages, added together
npx humblebrag ahmadawais --html report.html --json report.json
```

The `--html` file is one self-contained page. It has monthly, yearly, and cumulative charts, and a table of packages you can sort and filter. Click a package to chart it alone.

To install it globally, run `npm i -g humblebrag`. The command is then `humblebrag`.

When a name is both a package and a user, humblebrag picks the package and prints the command for the user.

## Options

```
--from YYYY-MM-DD   start date (default 2015-01-10, the first day npm has data)
--to   YYYY-MM-DD   end date (default: the last day npm has data)
--json <file>       write daily totals and per-package monthly totals as JSON
--html <file>       write an interactive HTML report
--top <n>           number of packages to list (default 25)
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

The `humblebrag` npm package holds a short Node script. It lists five optional dependencies, one per platform, and npm installs only the one that matches your OS and CPU. The script runs that binary.

| npm package | Platform |
|---|---|
| `humblebrag-darwin-arm64` | macOS on Apple silicon |
| `humblebrag-darwin-x64` | macOS on Intel |
| `humblebrag-linux-x64` | Linux x64, static musl build |
| `humblebrag-linux-arm64` | Linux arm64, static musl build |
| `humblebrag-win32-x64` | Windows x64 |

## Build from source

humblebrag is written in Rust.

```sh
cargo build --release
./target/release/humblebrag ahmadawais
```

`scripts/install.sh` builds it and copies the binary to `~/.local/bin`.

## Release

Run `./scripts/release.sh 0.2.0` on `main`. It does the following:

1. Logs you in to npm in the browser if needed.
2. Bumps `Cargo.toml`, commits, tags `v0.2.0`, and pushes.
3. Waits for GitHub Actions to build the five binaries and attach them to a GitHub release.
4. Downloads the binaries, checks their checksums, and builds the six npm packages.
5. Publishes each package. npm asks you to approve in the browser with a passkey or Touch ID.

No npm token is stored in GitHub. Each package is set to require two-factor auth and refuse tokens, so publishing always needs you. Running the script again skips steps that are already done.

## License

MIT
