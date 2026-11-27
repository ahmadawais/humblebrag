# npmstat

Blazing-fast npm download stats for a user or a set of packages. Single Rust binary, terminal summary, JSON export, and a self-contained interactive HTML report.

```sh
./scripts/install.sh            # release build → ~/.local/bin/npmstat
npmstat user ahmadawais --html report.html --json report.json
npmstat pkg react,vue,svelte --from 2024-01-01
```

## Why it's faster than npm-stat.com

npm-stat.com runs a Java/Spring backend with a Postgres/Timescale cache and a jQuery + Highcharts 3 frontend. It fetches each package separately and draws one chart per package.

`npmstat`'s bottleneck is npm's rate limit (Cloudflare 429s after a burst of about 20 requests), so it's built to send as few requests as possible:

| Trick | Effect |
|---|---|
| **Bulk endpoint**: 128 unscoped packages × 365 days per request | ~170 packages × 12 years ≈ 24 requests instead of ~2,000 |
| **Scoped packages** (no bulk support): 540-day windows starting at the package's creation date | Skips empty pre-creation windows |
| **Incremental disk cache** (`~/.cache/npmstat`) | Finalized days are never fetched again |
| **TTL for recent days** (default 6h) | Re-runs within the TTL send **0** API requests |
| **Shared 429 gate + pacing** | One 429 pauses every worker instead of digging deeper into a ban |
| **Creation-date pruning** (registry lookups aren't rate-limited) | Older bulk windows only include packages that existed then. Use `--exact` to turn this off |
| **Token-bucket pacing** (burst 10, then 0.4 req/s) | Stays under the limit instead of triggering Cloudflare penalty windows |
| HTTP/2, connection reuse, gzip/brotli, LTO release build | Fast transfers, ~3 MB static binary |

## Options

```
--from YYYY-MM-DD   start date (default 2015-01-10, first day npm has data)
--to   YYYY-MM-DD   end date   (default: last day npm has data)
--json <file>       full JSON (daily totals + per-package monthly)
--html <file>       self-contained interactive HTML report
--top <n>           packages to list (default 25)
-c <n>              max concurrent requests
--ttl <minutes>     reuse not-yet-final recent days for this long (default 360)
--fresh             refetch recent days now (finalized history stays cached)
--no-cache          ignore the cache entirely
--rate <n>          sustained API req/s (default 0.4, 0 = unpaced)
--burst <n>         burst before pacing (default 10)
--exact             don't prune windows before first publish
```

## Benchmark (`user ahmadawais`, 179 packages, 2015 → today)

| | requests | time |
|---|---|---|
| cold (empty cache) | 42 | ~1–2 min, all of it npm's rate limit |
| warm (within TTL) | 0 | **~0.3 s** |
| warm (after TTL) | ~14 | ~1–2 s |

## Notes

- npm only has daily download data from **2015-01-10**. Anything earlier doesn't exist.
- `--to` defaults to the last day npm has published (`/downloads/point/last-day`). Days npm hasn't processed yet come back as zeros, so they're not counted.
- User packages = union of `registry.npmjs.org/-/user/<name>/package` and `maintainer:<name>` search.
