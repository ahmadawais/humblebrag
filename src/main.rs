//! humblebrag: npm downloads and GitHub stars for a user, org, or packages.
//!
//! Speed tricks:
//! * Bulk API: up to 128 unscoped packages × 365 days per request.
//! * Scoped packages (no bulk support): 540-day windows, fetched concurrently.
//! * HTTP/2 + connection reuse + gzip/brotli, bounded concurrency, 429-aware retry.
//! * Incremental on-disk cache: finalized days are never fetched twice, so
//!   re-runs only fetch the last few days.

use chrono::{Duration, NaiveDate, Utc};
use futures::stream::{self, StreamExt};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Instant;

mod config;
mod github;
mod onboard;
mod social;

// CI sets HUMBLEBRAG_VERSION to the release version; local builds use Cargo.toml.
const VERSION: &str = match option_env!("HUMBLEBRAG_VERSION") {
    Some(v) => v,
    None => env!("CARGO_PKG_VERSION"),
};
const API: &str = "https://api.npmjs.org/downloads/range";
const REGISTRY: &str = "https://registry.npmjs.org";
const BULK_MAX: usize = 128;
const BULK_DAYS: i64 = 365;
const SINGLE_DAYS: i64 = 540;
const FINAL_LAG: i64 = 3; // days before data is considered final & cacheable
const DEFAULT_RATE: f64 = 0.4; // sustained req/s to api.npmjs.org (0 = unlimited)
const DEFAULT_BURST: u64 = 10;
const REPORT_HTML: &str = include_str!("report.html");

fn epoch() -> NaiveDate {
    NaiveDate::from_ymd_opt(2015, 1, 10).unwrap() // first day npm has data for
}

// ---------- API types ----------
#[derive(Deserialize)]
struct Day {
    downloads: u64,
    day: String,
}
#[derive(Deserialize, Default)]
struct Range {
    #[serde(default)]
    downloads: Vec<Day>,
}
#[derive(Deserialize)]
struct SearchResp {
    total: usize,
    objects: Vec<SearchObj>,
}
#[derive(Deserialize)]
struct SearchObj {
    package: SearchPkg,
}
#[derive(Deserialize)]
struct SearchPkg {
    name: String,
}

// ---------- cache ----------
#[derive(Serialize, Deserialize)]
struct Cache {
    start: String,
    days: Vec<u64>,
    /// how many leading days are final (immutable)
    #[serde(default)]
    fin: Option<usize>,
    /// unix seconds when written
    #[serde(default)]
    at: u64,
    /// package creation date (immutable, saves a registry lookup)
    #[serde(default)]
    born: Option<String>,
}

fn now_secs() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs()
}

fn cache_dir() -> PathBuf {
    let base = std::env::var_os("XDG_CACHE_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(std::env::var_os("HOME").unwrap_or_default()).join(".cache"));
    // --exact keeps its own cache: pruned caches have zeros before first publish.
    base.join(if EXACT.load(Ordering::Relaxed) { "humblebrag/exact" } else { "humblebrag" })
}
static EXACT: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
fn cache_path(pkg: &str) -> PathBuf {
    cache_dir().join(format!("{}.json", pkg.replace('/', "__")))
}
/// Finalized days are always reused; the trailing not-yet-final days are reused
/// only while the cache entry is younger than `ttl` seconds.
fn load_cache(pkg: &str, ttl: u64) -> Option<(NaiveDate, Vec<u64>, Option<NaiveDate>)> {
    let mut c: Cache = serde_json::from_slice(&std::fs::read(cache_path(pkg)).ok()?).ok()?;
    if now_secs().saturating_sub(c.at) > ttl {
        let fin = c.fin.unwrap_or(c.days.len()).min(c.days.len());
        c.days.truncate(fin);
    }
    Some((parse_date(&c.start)?, c.days, c.born.as_deref().and_then(parse_date)))
}
fn save_cache(pkg: &str, start: NaiveDate, days: &[u64], fin: usize, born: Option<NaiveDate>) {
    let _ = std::fs::create_dir_all(cache_dir());
    let c = Cache {
        start: start.to_string(),
        days: days.to_vec(),
        fin: Some(fin.min(days.len())),
        at: now_secs(),
        born: born.map(|d| d.to_string()),
    };
    if let Ok(b) = serde_json::to_vec(&c) {
        let _ = std::fs::write(cache_path(pkg), b);
    }
}

fn parse_date(s: &str) -> Option<NaiveDate> {
    NaiveDate::parse_from_str(s, "%Y-%m-%d").ok()
}

// ---------- HTTP ----------
// Shared "don't send before" instant: one 429 pauses every worker, instead of
// all of them hammering the rate limiter in parallel.
static GATE: std::sync::Mutex<Option<std::time::Instant>> = std::sync::Mutex::new(None);

async fn wait_gate() {
    loop {
        let until = *GATE.lock().unwrap();
        match until {
            Some(t) if t > std::time::Instant::now() => tokio::time::sleep_until(t.into()).await,
            _ => return,
        }
    }
}
fn close_gate(ms: u64) {
    let t = std::time::Instant::now() + std::time::Duration::from_millis(ms);
    let mut g = GATE.lock().unwrap();
    if g.map_or(true, |cur| cur < t) {
        *g = Some(t);
    }
}

// Token bucket matched to api.npmjs.org's Cloudflare limit: hitting a 429
// costs far more (penalty window) than pacing just under the limit.
struct Bucket {
    tokens: f64,
    last: std::time::Instant,
}
static BUCKET: std::sync::Mutex<Option<Bucket>> = std::sync::Mutex::new(None);
static RATE: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0); // f64 bits, req/s
static THROTTLED: AtomicUsize = AtomicUsize::new(0);
static BURST: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

async fn take_token() {
    let rate = f64::from_bits(RATE.load(Ordering::Relaxed));
    let burst = BURST.load(Ordering::Relaxed) as f64;
    if rate <= 0.0 {
        return;
    }
    loop {
        let wait = {
            let mut g = BUCKET.lock().unwrap();
            let now = std::time::Instant::now();
            let b = g.get_or_insert(Bucket { tokens: burst, last: now });
            b.tokens = (b.tokens + now.duration_since(b.last).as_secs_f64() * rate).min(burst);
            b.last = now;
            if b.tokens >= 1.0 {
                b.tokens -= 1.0;
                return;
            }
            (1.0 - b.tokens) / rate
        };
        tokio::time::sleep(std::time::Duration::from_secs_f64(wait)).await;
    }
}

async fn get(client: &reqwest::Client, url: &str) -> Result<Option<Vec<u8>>, String> {
    let mut delay = 1000u64;
    for attempt in 0..10 {
        wait_gate().await;
        if url.starts_with(API) {
            take_token().await;
        }
        match client.get(url).send().await {
            Ok(r) => {
                let s = r.status();
                if s.is_success() {
                    return r.bytes().await.map(|b| Some(b.to_vec())).map_err(|e| e.to_string());
                }
                if s.as_u16() == 404 {
                    return Ok(None);
                }
                if s.as_u16() == 429 || s.is_server_error() {
                    THROTTLED.fetch_add(1, Ordering::Relaxed);
                    let ra = r
                        .headers()
                        .get("retry-after")
                        .and_then(|v| v.to_str().ok())
                        .and_then(|v| v.parse::<u64>().ok())
                        .map(|s| s * 1000);
                    let jitter = (std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().subsec_nanos() % 500) as u64;
                    close_gate(ra.unwrap_or(0).max(delay) + jitter);
                    delay = (delay * 2).min(30_000);
                    continue;
                }
                let t = r.text().await.unwrap_or_default();
                return Err(format!("{s} {url}: {t}"));
            }
            Err(e) => {
                if attempt == 9 {
                    return Err(e.to_string());
                }
                tokio::time::sleep(std::time::Duration::from_millis(delay)).await;
                delay = (delay * 2).min(30_000);
            }
        }
    }
    Err(format!("gave up: {url}"))
}

async fn user_packages(client: &reqwest::Client, user: &str) -> Result<Vec<String>, String> {
    // Two sources, fetched concurrently, unioned: the user's package list and maintainer search.
    let list = async {
        let url = format!("{REGISTRY}/-/user/{user}/package");
        match get(client, &url).await {
            Ok(Some(b)) => serde_json::from_slice::<HashMap<String, serde_json::Value>>(&b)
                .map(|m| m.into_keys().collect::<Vec<_>>())
                .unwrap_or_default(),
            _ => vec![],
        }
    };
    let search = async {
        let mut out = vec![];
        let mut from = 0;
        loop {
            let url = format!("{REGISTRY}/-/v1/search?text=maintainer:{user}&size=250&from={from}");
            let Ok(Some(b)) = get(client, &url).await else { break };
            let Ok(r) = serde_json::from_slice::<SearchResp>(&b) else { break };
            let n = r.objects.len();
            out.extend(r.objects.into_iter().map(|o| o.package.name));
            from += n;
            if n == 0 || from >= r.total {
                break;
            }
        }
        out
    };
    let (a, b) = tokio::join!(list, search);
    let set: BTreeSet<String> = a.into_iter().chain(b).collect();
    if set.is_empty() {
        return Err(format!("no packages found for user '{user}'"));
    }
    Ok(set.into_iter().collect())
}

#[derive(Deserialize)]
struct Packument {
    #[serde(default)]
    time: HashMap<String, serde_json::Value>,
}

/// Earliest timestamp the registry knows for a package (min of `created` and
/// every version's publish time). Used to skip windows where the package
/// could not have had downloads. Disabled with --exact.
async fn created(client: &reqwest::Client, pkg: &str) -> Option<NaiveDate> {
    let url = format!("{REGISTRY}/{}", pkg.replace('/', "%2F"));
    let b = get(client, &url).await.ok()??;
    let p: Packument = serde_json::from_slice(&b).ok()?;
    let min = p
        .time
        .iter()
        .filter(|(k, _)| k.as_str() != "modified")
        .filter_map(|(_, v)| v.as_str()?.get(..10))
        .min()?;
    parse_date(min)
}

#[derive(Serialize, Deserialize)]
struct LastDay {
    end: String,
    #[serde(default)]
    at: u64,
}

async fn last_available(client: &reqwest::Client, ttl: u64) -> Option<NaiveDate> {
    let path = cache_dir().join("_last-day.json");
    if let Some(c) = std::fs::read(&path).ok().and_then(|b| serde_json::from_slice::<LastDay>(&b).ok()) {
        if now_secs().saturating_sub(c.at) <= ttl {
            return parse_date(&c.end);
        }
    }
    let b = get(client, "https://api.npmjs.org/downloads/point/last-day").await.ok()??;
    let mut c: LastDay = serde_json::from_slice(&b).ok()?;
    c.at = now_secs();
    let _ = std::fs::create_dir_all(cache_dir());
    let _ = std::fs::write(&path, serde_json::to_vec(&c).ok()?);
    parse_date(&c.end)
}

async fn package_exists(client: &reqwest::Client, pkg: &str) -> bool {
    let url = format!("{REGISTRY}/{}", pkg.replace('/', "%2F"));
    client
        .head(&url)
        .header("accept", "application/vnd.npm.install-v1+json")
        .send()
        .await
        .map_or(false, |r| r.status().is_success())
}

/// Figure out what the user meant: `~name` = npm user, `@scope/x` or several
/// names = packages, a single bare name = check both (package wins, with a hint).
async fn resolve(client: &reqwest::Client, explicit: Option<String>, names: Vec<String>) -> Result<(String, String, Vec<String>), String> {
    let user = |n: &str| n.trim_start_matches('~').to_string();
    match explicit.as_deref() {
        Some("user") => {
            let u = user(&names[0]);
            let p = user_packages(client, &u).await?;
            return Ok(("user".into(), u, p));
        }
        Some(_) => return Ok(("pkg".into(), names.join(","), names)),
        None => {}
    }
    if names.len() > 1 || names[0].starts_with('@') {
        return Ok(("pkg".into(), names.join(","), names));
    }
    if names[0].starts_with('~') {
        let u = user(&names[0]);
        let p = user_packages(client, &u).await?;
        return Ok(("user".into(), u, p));
    }
    let n = names[0].clone();
    let (is_pkg, as_user) = tokio::join!(package_exists(client, &n), user_packages(client, &n));
    match (is_pkg, as_user) {
        (true, Ok(p)) => {
            eprintln!("  {n} is a package (also an npm user with {} packages: humblebrag npm ~{n})", p.len());
            Ok(("pkg".into(), n.clone(), vec![n]))
        }
        (true, Err(_)) => Ok(("pkg".into(), n.clone(), vec![n])),
        (false, Ok(p)) => Ok(("user".into(), n, p)),
        (false, Err(_)) => Err(format!("'{n}' is neither an npm package nor an npm user")),
    }
}

/// GitHub repos (`owner/repo`) named in the packages' `repository` fields.
async fn package_repos(client: &reqwest::Client, pkgs: &[String]) -> Vec<String> {
    let found = stream::iter(pkgs.iter().take(50))
        .map(|p| async move {
            let url = format!("{REGISTRY}/{}/latest", p.replace('/', "%2F"));
            let body = get(client, &url).await.ok().flatten()?;
            let v: serde_json::Value = serde_json::from_slice(&body).ok()?;
            github::repo_from_npm(&v["repository"])
        })
        .buffer_unordered(8)
        .collect::<Vec<_>>()
        .await;
    let mut out: Vec<String> = found.into_iter().flatten().collect();
    out.sort_by_key(|r| r.to_lowercase());
    out.dedup_by_key(|r| r.to_lowercase());
    out
}

// ---------- fetch planning ----------
struct Job {
    pkgs: Vec<String>,
    start: NaiveDate,
    end: NaiveDate,
}

/// Windows aligned backwards from `to`: the newest window is full-length, so
/// the short leftover window lands in the oldest period where few packages exist.
fn windows(from: NaiveDate, to: NaiveDate, len: i64) -> Vec<(NaiveDate, NaiveDate)> {
    let mut out = vec![];
    let mut e = to;
    while e >= from {
        let s = (e - Duration::days(len - 1)).max(from);
        out.push((s, e));
        e = s - Duration::days(1);
    }
    out
}

type JobOut = Vec<(String, Result<Vec<Day>, ()>)>;

async fn run_job(client: &reqwest::Client, job: &Job) -> JobOut {
    let url = format!("{API}/{}:{}/{}", job.start, job.end, job.pkgs.join(","));
    if job.pkgs.len() == 1 {
        let r = match get(client, &url).await {
            Ok(Some(b)) => Ok(serde_json::from_slice::<Range>(&b).unwrap_or_default().downloads),
            Ok(None) => Ok(vec![]),
            Err(e) => {
                eprintln!("\n  ! {e}");
                Err(())
            }
        };
        return vec![(job.pkgs[0].clone(), r)];
    }
    match get(client, &url).await {
        Ok(Some(b)) => match serde_json::from_slice::<HashMap<String, Option<Range>>>(&b) {
            Ok(m) => m.into_iter().map(|(k, v)| (k, Ok(v.unwrap_or_default().downloads))).collect(),
            Err(e) => {
                eprintln!("\n  ! bad bulk json: {e}");
                job.pkgs.iter().map(|p| (p.clone(), Err(()))).collect()
            }
        },
        // Bulk failed: fall back to one request per package for this window.
        _ => {
            let futs = job.pkgs.iter().map(|p| {
                let j = Job { pkgs: vec![p.clone()], start: job.start, end: job.end };
                async move { Box::pin(run_job(client, &j)).await }
            });
            futures::future::join_all(futs).await.into_iter().flatten().collect()
        }
    }
}

struct Series {
    start: NaiveDate,
    days: Vec<u64>,
}

async fn fetch_all(
    client: &reqwest::Client,
    pkgs: &[String],
    from: NaiveDate,
    to: NaiveDate,
    use_cache: bool,
    ttl: u64,
    concurrency: usize,
    exact: bool,
) -> (HashMap<String, Series>, usize, usize) {
    let today = Utc::now().date_naive();
    let final_through = today - Duration::days(FINAL_LAG);

    // Load caches, decide what each package still needs.
    let mut series: HashMap<String, Series> = HashMap::new();
    let mut need: BTreeMap<(NaiveDate, bool), Vec<String>> = BTreeMap::new();
    let mut born: HashMap<String, NaiveDate> = HashMap::new();
    for p in pkgs {
        let cached = if use_cache { load_cache(p, ttl) } else { None };
        if let (false, Some((_, _, Some(b)))) = (exact, &cached) {
            born.insert(p.clone(), *b);
        }
        let (start, days) = match cached {
            Some((s, d, _)) if s <= from => (s, d),
            _ => (from, vec![]),
        };
        let fetch_from = start + Duration::days(days.len() as i64);
        if fetch_from <= to {
            need.entry((fetch_from, p.starts_with('@'))).or_default().push(p.clone());
        }
        series.insert(p.clone(), Series { start, days });
    }

    // Creation dates (registry host: not rate-limited like the downloads API).
    // Lets us skip pre-creation windows and shrink bulk batches in older years.
    let unknown: Vec<&String> = if exact {
        vec![]
    } else {
        need.values().flatten().filter(|p| !born.contains_key(*p)).collect()
    };
    let fetched: Vec<(String, NaiveDate)> = stream::iter(unknown)
        .map(|p| async move { (p.clone(), created(client, p).await) })
        .buffer_unordered(32)
        .filter_map(|(p, d)| async move { d.map(|d| (p, d)) })
        .collect()
        .await;
    born.extend(fetched);

    // Build jobs.
    let mut jobs = vec![];
    for ((fetch_from, scoped), list) in &need {
        // Scoped (no bulk support) or a lone package: single-package windows are longer.
        if *scoped || list.len() == 1 {
            for p in list {
                let start = born.get(p).map_or(*fetch_from, |b| (*b).max(*fetch_from));
                for (s, e) in windows(start, to, SINGLE_DAYS) {
                    jobs.push(Job { pkgs: vec![p.clone()], start: s, end: e });
                }
            }
        } else {
            let oldest = list.iter().map(|p| born.get(p).copied().unwrap_or(*fetch_from)).min().unwrap_or(*fetch_from);
            for (s, e) in windows(oldest.max(*fetch_from), to, BULK_DAYS) {
                let alive: Vec<String> = list.iter().filter(|p| born.get(*p).map_or(true, |b| *b <= e)).cloned().collect();
                for chunk in alive.chunks(BULK_MAX) {
                    jobs.push(Job { pkgs: chunk.to_vec(), start: s, end: e });
                }
            }
        }
    }
    let total_jobs = jobs.len();
    let cache_hits = pkgs.len() - need.values().map(|v| v.len()).sum::<usize>();
    let done = Arc::new(AtomicUsize::new(0));

    let tty = std::io::IsTerminal::is_terminal(&std::io::stderr());
    let results: Vec<JobOut> = stream::iter(jobs)
        .map(|job| {
            let done = done.clone();
            async move {
                let r = run_job(client, &job).await;
                let d = done.fetch_add(1, Ordering::Relaxed) + 1;
                if tty {
                    eprint!("\r  fetching {d}/{total_jobs} requests…");
                }
                r
            }
        })
        .buffer_unordered(concurrency)
        .collect()
        .await;
    if total_jobs > 0 && tty {
        eprintln!();
    }

    let mut failed: BTreeSet<String> = BTreeSet::new();
    for (pkg, days) in results.into_iter().flatten() {
        let Ok(days) = days else {
            failed.insert(pkg);
            continue;
        };
        let Some(s) = series.get_mut(&pkg) else { continue };
        for d in days {
            let Some(date) = parse_date(&d.day) else { continue };
            if date < s.start {
                continue;
            }
            let i = (date - s.start).num_days() as usize;
            if i >= s.days.len() {
                s.days.resize(i + 1, 0);
            }
            s.days[i] = d.downloads;
        }
    }

    // Pad to `to`, persist finalized portion.
    for (pkg, s) in series.iter_mut() {
        let want = (to - s.start).num_days() as usize + 1;
        if s.days.len() < want {
            s.days.resize(want, 0);
        }
        if use_cache && !failed.contains(pkg) {
            let keep = ((final_through.min(to) - s.start).num_days() + 1).max(0) as usize;
            save_cache(pkg, s.start, &s.days, keep, born.get(pkg).copied());
        }
    }
    if !failed.is_empty() {
        eprintln!("  ! {} packages had failed requests (not cached, totals incomplete): {}", failed.len(), failed.iter().cloned().collect::<Vec<_>>().join(", "));
    }
    (series, total_jobs, cache_hits)
}

// ---------- output ----------
fn fmt(n: u64) -> String {
    let s = n.to_string();
    let mut out = String::new();
    for (i, c) in s.chars().enumerate() {
        if i > 0 && (s.len() - i) % 3 == 0 {
            out.push(',');
        }
        out.push(c);
    }
    out
}

fn usage() -> ! {
    eprintln!(
        "humblebrag (alias: hb): npm downloads and GitHub stars, all time

USAGE:
  hb <name> [options]              npm + GitHub for a user or package
  hb npm <name>... [options]       npm downloads only
  hb gh <login|owner/repo>...      GitHub only: stars, forks, watchers, followers
  hb contrib <login>               merged PRs to repos you don't own, kept in a record
  hb onboard                       set up a profile (npm, GitHub, social sites)

  <name> for npm is a package or user, auto-detected. ~<user> forces a user,
  several names are combined. `hb user <name>` and `hb pkg <name>` still work.
  For a package, the GitHub part uses its repository from package.json.

PROFILES:
  `hb onboard` saves a profile to ~/.config/humblebrag/config.json (or a path you
  pick, linked from there; $HUMBLEBRAG_CONFIG overrides). `hb <profile>` then
  totals its npm users and packages, GitHub accounts, merged PRs, and social
  followers, and adds a line per day to history-<profile>.jsonl next to the config.

OPTIONS:
  --gh <login|owner/repo>  GitHub target for `hb <name>` when it differs
  --no-profile        ignore a profile with this name
  --all               list every package, repo and contribution
  --no-record         `hb contrib`: don't update the record file
  --from YYYY-MM-DD   start date (default 2015-01-10, first day npm has data)
  --to   YYYY-MM-DD   end date   (default: latest day npm has data)
  --json <file>       write full JSON (npm daily totals, per package, GitHub)
  --html <file>       write a self-contained interactive HTML report
  --top <n>           packages and repos to list (default 25)
  -c <n>              max concurrent requests (default 8)
  --ttl <minutes>     reuse not-yet-final recent days for this long (default 360)
  --rate <n>          sustained npm API requests/sec (default 0.4, 0 = unpaced)
  --burst <n>         initial burst before pacing kicks in (default 10)
  --exact             don't skip windows before a package's first publish
                      (counts downloads of earlier, unpublished owners of a reused name)
  --fresh             refetch recent days now (finalized history still cached)
  --no-cache          ignore and don't write the on-disk cache
  -V, --version       print version

GITHUB TOKEN:
  Uses GITHUB_TOKEN, GH_TOKEN, or `gh auth token` when available (5,000
  requests/hour, watchers included). Without one: 60 requests/hour, no watchers."
    );
    std::process::exit(2)
}

#[derive(PartialEq, Clone, Copy)]
enum Mode {
    Both,
    Npm,
    Gh,
    Contrib,
}

struct NpmOpts {
    from: NaiveDate,
    to: NaiveDate,
    to_given: bool,
    top: usize,
    conc: usize,
    use_cache: bool,
    ttl: u64,
    exact: bool,
}

#[tokio::main]
async fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.is_empty() || matches!(args[0].as_str(), "-h" | "--help" | "help") {
        usage();
    }
    if matches!(args[0].as_str(), "-V" | "--version") {
        println!("humblebrag {VERSION}");
        return;
    }
    if args[0] == "onboard" {
        onboard::run();
        return;
    }
    // Optional mode word, then names (anything before the first flag).
    let mut i = 0;
    let mut mode = Mode::Both;
    match args[0].as_str() {
        "npm" => (mode, i) = (Mode::Npm, 1),
        "gh" | "github" => (mode, i) = (Mode::Gh, 1),
        "contrib" | "contributions" => (mode, i) = (Mode::Contrib, 1),
        "user" | "pkg" => mode = Mode::Npm,
        _ => {}
    }
    let explicit = (mode == Mode::Npm && matches!(args.get(i).map(|s| s.as_str()), Some("user" | "pkg"))).then(|| {
        i += 1;
        args[i - 1].clone()
    });
    let mut names: Vec<String> = vec![];
    while i < args.len() && !args[i].starts_with('-') {
        names.extend(args[i].split(|c: char| c == ',' || c.is_whitespace()).map(|s| s.trim().to_string()).filter(|s| !s.is_empty()));
        i += 1;
    }
    if names.is_empty() {
        usage();
    }
    let today = Utc::now().date_naive();
    let mut o = NpmOpts {
        from: epoch(),
        to: today - Duration::days(1),
        to_given: false,
        top: 25,
        conc: 8,
        use_cache: true,
        ttl: 6 * 3600,
        exact: false,
    };
    let (mut json_out, mut html_out) = (None::<String>, None::<String>);
    let (mut rate, mut burst) = (DEFAULT_RATE, DEFAULT_BURST);
    let mut gh_override: Option<String> = None;
    let (mut no_profile, mut no_record) = (false, false);
    while i < args.len() {
        let val = || args.get(i + 1).cloned().unwrap_or_else(|| usage());
        match args[i].as_str() {
            "--gh" => gh_override = Some(val()),
            "--from" => o.from = parse_date(&val()).unwrap_or_else(|| usage()).max(epoch()),
            "--to" => {
                o.to = parse_date(&val()).unwrap_or_else(|| usage());
                o.to_given = true;
            }
            "--json" => json_out = Some(val()),
            "--html" => html_out = Some(val()),
            "--top" => o.top = val().parse().unwrap_or_else(|_| usage()),
            "-c" => o.conc = val().parse().unwrap_or_else(|_| usage()),
            "--ttl" => o.ttl = val().parse::<u64>().unwrap_or_else(|_| usage()) * 60,
            "--rate" => rate = val().parse().unwrap_or_else(|_| usage()),
            "--burst" => burst = val().parse().unwrap_or_else(|_| usage()),
            "--all" => {
                o.top = usize::MAX;
                i -= 1;
            }
            "--no-profile" => {
                no_profile = true;
                i -= 1;
            }
            "--no-record" => {
                no_record = true;
                i -= 1;
            }
            "--exact" => {
                o.exact = true;
                i -= 1;
            }
            "--fresh" => {
                o.ttl = 0;
                i -= 1;
            }
            "--no-cache" => {
                o.use_cache = false;
                i -= 1;
            }
            _ => usage(),
        }
        i += 2;
    }
    if o.from > o.to {
        eprintln!("--from is after --to");
        std::process::exit(2);
    }
    if matches!(mode, Mode::Gh | Mode::Contrib) && html_out.is_some() {
        eprintln!("--html needs npm data; use `hb <name>` or `hb npm <name>`");
        std::process::exit(2);
    }

    EXACT.store(o.exact, Ordering::Relaxed);
    RATE.store(f64::to_bits(rate), Ordering::Relaxed);
    BURST.store(burst, Ordering::Relaxed);
    let t0 = Instant::now();
    let client = reqwest::Client::builder()
        .user_agent(format!("humblebrag/{VERSION}"))
        .pool_max_idle_per_host(o.conc)
        .timeout(std::time::Duration::from_secs(60))
        .build()
        .unwrap();
    let token = if mode == Mode::Npm { None } else { github::token() };
    let cfg = config::load();
    let profile = (!no_profile && names.len() == 1 && explicit.is_none() && gh_override.is_none())
        .then(|| cfg.profile(&names[0]))
        .flatten();
    if let Some((name, _)) = &profile {
        eprintln!("  profile {name} ({})", config::path().display());
    }

    // Contributions only.
    if mode == Mode::Contrib {
        let login = names[0].trim_start_matches('@').to_string();
        let exclude: Vec<String> = match &profile {
            Some((_, p)) if !p.github.is_empty() => p.github.clone(),
            _ => vec![login.clone()],
        };
        let prs = contrib_report(&client, &token, &login, &exclude, &cfg, no_record, o.top).await;
        if let Some(f) = json_out {
            std::fs::write(&f, serde_json::to_vec(&serde_json::json!({ "contributions": prs })).unwrap()).expect("write json");
            eprintln!("  → {f}");
        }
        return;
    }

    // GitHub only.
    if mode == Mode::Gh {
        let targets = match &profile {
            Some((_, p)) if !p.github.is_empty() => p.github.clone(),
            _ => names.clone(),
        };
        let owners = gh_report(&client, &targets, &token, o.top).await;
        if owners.is_empty() {
            std::process::exit(1);
        }
        if let Some(f) = json_out {
            std::fs::write(&f, serde_json::to_vec(&serde_json::json!({ "github": github::to_json(&owners) })).unwrap()).expect("write json");
            eprintln!("  → {f}");
        }
        return;
    }

    // npm: resolve the name(s), then start GitHub alongside the download fetch.
    let last_day = async {
        if o.to_given {
            return None;
        }
        last_available(&client, o.ttl.min(3600)).await
    };
    let resolving = async {
        match &profile {
            Some((name, p)) if p.npm.is_empty() => Err(format!("profile {name} has no npm entries")),
            Some((name, p)) => resolve_many(&client, &p.npm).await.map(|pk| ("profile".to_string(), name.clone(), pk)),
            None => resolve(&client, explicit, names.clone()).await,
        }
    };
    let (last_day, resolved) = tokio::join!(last_day, resolving);
    if let Some(d) = last_day {
        o.to = d;
    }
    let npm = match resolved {
        Ok(r) => Some(r),
        Err(e) if mode == Mode::Both => {
            eprintln!("  npm: {e}");
            None
        }
        Err(e) => {
            eprintln!("{e}");
            std::process::exit(1)
        }
    };
    let gh_targets: Vec<String> = match (mode, &gh_override, &npm) {
        (Mode::Npm, _, _) => vec![],
        _ if profile.is_some() => profile.as_ref().unwrap().1.github.clone(),
        (_, Some(g), _) => vec![g.clone()],
        (_, None, Some((m, target, _))) if m == "user" => vec![target.clone()],
        (_, None, Some((_, target, pkgs))) => {
            let r = package_repos(&client, pkgs).await;
            if r.is_empty() {
                eprintln!("  github: no GitHub repository in {target}'s package.json (pass --gh <login|owner/repo>)");
            }
            r
        }
        (_, None, None) => names.clone(),
    };
    let contrib_task = match &profile {
        Some((name, p)) if p.contributions && mode == Mode::Both && !p.github.is_empty() => {
            let (client, token, login, exclude) = (client.clone(), token.clone(), p.github[0].clone(), p.github.clone());
            let _ = name;
            Some(tokio::spawn(async move { github::contributions(&client, &token, &login, &exclude).await.map(|v| (login, v)) }))
        }
        _ => None,
    };
    let gh_task = {
        let (client, token) = (client.clone(), token.clone());
        tokio::spawn(async move { github::fetch(&client, &gh_targets, &token).await })
    };
    let social_task = match &profile {
        Some((_, p)) if mode == Mode::Both && !p.social.is_empty() => {
            let (client, social) = (client.clone(), p.social.clone());
            Some(tokio::spawn(async move { social::fetch(&client, &social).await }))
        }
        _ => None,
    };

    let npm_data = match npm {
        Some((mode, target, pkgs)) => Some(npm_report(&client, &o, &mode, &target, pkgs, t0).await),
        None => None,
    };

    let mut owners = vec![];
    for r in gh_task.await.unwrap_or_default() {
        match r {
            Ok(x) => owners.push(x),
            Err(e) => eprintln!("  github: {e}"),
        }
    }
    github::print(&owners, o.top, token.is_some());
    let mut contribs = None;
    if let Some(t) = contrib_task {
        match t.await {
            Ok(Ok((login, fresh))) => contribs = Some(finish_contrib(&cfg, &login, fresh, no_record, o.top.min(10))),
            Ok(Err(e)) => eprintln!("  contributions: {e}"),
            Err(_) => {}
        }
    }
    let mut stats = vec![];
    if let Some(t) = social_task {
        for r in t.await.unwrap_or_default() {
            match r {
                Ok(x) => stats.push(x),
                Err(e) => eprintln!("  social: {e}"),
            }
        }
    }
    let gh_followers: Option<u64> = owners.iter().filter_map(|o| o.followers).reduce(|a, b| a + b);
    if let Some((name, p)) = &profile {
        let path = cfg.record_dir().join(format!("history-{}.jsonl", name.to_lowercase()));
        if let Some(last) = social::last_entry(&path) {
            social::fill_from_history(&mut stats, &p.social.keys().cloned().collect::<Vec<_>>(), &last);
        }
    }
    if let Some((name, _)) = &profile {
        social::print(name, &stats, gh_followers);
    }
    if npm_data.is_none() && owners.is_empty() {
        std::process::exit(1);
    }

    // One history line per profile per day, and what changed since the last one.
    if let (Some((name, _)), Mode::Both) = (&profile, mode) {
        let mut m = serde_json::Map::new();
        if let Some(d) = &npm_data {
            m.insert("npm_downloads".into(), d["total"].clone());
            m.insert("npm_packages".into(), serde_json::json!(d["packages"].as_array().map_or(0, |a| a.len())));
        }
        if !owners.is_empty() {
            let g = github::to_json(&owners);
            for k in ["stars", "forks", "watchers", "followers"] {
                if !g[k].is_null() {
                    m.insert(format!("gh_{k}"), g[k].clone());
                }
            }
        }
        if let Some(c) = &contribs {
            m.insert("merged_prs".into(), serde_json::json!(c.len()));
        }
        for st in &stats {
            // Several accounts on one site add up under the site's key.
            let k = st.key();
            let sum = m.get(&k).and_then(|v| v.as_u64()).unwrap_or(0) + st.value;
            m.insert(k, serde_json::json!(sum));
        }
        if !stats.is_empty() || gh_followers.is_some() {
            let aud: u64 = stats.iter().filter(|s| s.is_audience()).map(|s| s.value).sum::<u64>() + gh_followers.unwrap_or(0);
            m.insert("audience".into(), serde_json::json!(aud));
        }
        let path = cfg.record_dir().join(format!("history-{}.jsonl", name.to_lowercase()));
        match social::record_history(&path, &m) {
            Ok(Some(prev)) => social::print_deltas(&prev, &m),
            Ok(None) => eprintln!("  history {} (changes show from the next day)", config::tilde(&path)),
            Err(e) => eprintln!("  history: {e}"),
        }
    }

    if json_out.is_some() || html_out.is_some() {
        let mut data = npm_data.unwrap_or_else(|| serde_json::json!({}));
        if !owners.is_empty() {
            data["github"] = github::to_json(&owners);
        }
        if let Some(c) = &contribs {
            data["contributions"] = serde_json::to_value(c).unwrap();
        }
        if let Some(f) = json_out {
            std::fs::write(&f, serde_json::to_vec(&data).unwrap()).expect("write json");
            eprintln!("  → {f}");
        }
        if let Some(f) = html_out {
            if data.get("daily").is_none() {
                eprintln!("  --html skipped: no npm data");
            } else {
                let payload = serde_json::to_string(&data).unwrap().replace("</", "<\\/");
                std::fs::write(&f, REPORT_HTML.replace("/*__DATA__*/null", &payload)).expect("write html");
                eprintln!("  → {f}");
            }
        }
    }
}

/// npm packages for a profile. Each entry is detected like `hb <name>`: `~user`
/// is a user, `@scope/pkg` a package, and a bare name whichever exists.
async fn resolve_many(client: &reqwest::Client, entries: &[String]) -> Result<Vec<String>, String> {
    let lists = futures::future::join_all(entries.iter().map(|e| async move {
        resolve(client, None, vec![e.clone()]).await.map(|(_, _, pkgs)| pkgs)
    }))
    .await;
    let mut all = BTreeSet::new();
    for l in lists {
        all.extend(l?);
    }
    Ok(all.into_iter().collect())
}

async fn contrib_report(
    client: &reqwest::Client,
    token: &Option<String>,
    login: &str,
    exclude: &[String],
    cfg: &config::Config,
    no_record: bool,
    top: usize,
) -> Vec<github::Pr> {
    match github::contributions(client, token, login, exclude).await {
        Ok(fresh) => finish_contrib(cfg, login, fresh, no_record, top),
        Err(e) => {
            eprintln!("  contributions: {e}");
            std::process::exit(1)
        }
    }
}

/// Merge into the record file (unless --no-record) and print.
fn finish_contrib(cfg: &config::Config, login: &str, fresh: Vec<github::Pr>, no_record: bool, top: usize) -> Vec<github::Pr> {
    let prs = if no_record {
        fresh
    } else {
        let path = cfg.record_dir().join(format!("contributions-{}.json", login.to_lowercase()));
        match github::merge_record(&path, login, fresh.clone()) {
            Ok((all, added)) => {
                eprintln!("  record {} ({} PRs, {added} new)", path.display(), all.len());
                all
            }
            Err(e) => {
                eprintln!("  record: {e}");
                fresh
            }
        }
    };
    github::print_contributions(login, &prs, top);
    prs
}

async fn gh_report(client: &reqwest::Client, targets: &[String], token: &Option<String>, top: usize) -> Vec<github::Owner> {
    let mut owners = vec![];
    for r in github::fetch(client, targets, token).await {
        match r {
            Ok(x) => owners.push(x),
            Err(e) => eprintln!("  github: {e}"),
        }
    }
    github::print(&owners, top, token.is_some());
    owners
}

/// Fetch and print npm downloads; returns the JSON data for --json/--html.
async fn npm_report(client: &reqwest::Client, o: &NpmOpts, mode: &str, target: &str, pkgs: Vec<String>, t0: Instant) -> serde_json::Value {
    let (from, to, top) = (o.from, o.to, o.top);
    if from > to {
        eprintln!("--from is after the last available day ({to})");
        std::process::exit(2);
    }
    let t_list = t0.elapsed();
    eprintln!("  {} packages ({:.0?})", pkgs.len(), t_list);

    let (series, reqs, hits) = fetch_all(client, &pkgs, from, to, o.use_cache, o.ttl, o.conc, o.exact).await;
    let t_fetch = t0.elapsed();

    // Aggregate within [from, to].
    let n_days = (to - from).num_days() as usize + 1;
    let mut daily = vec![0u64; n_days];
    let mut per_pkg: Vec<(String, u64, Vec<u64>)> = vec![]; // name, total, monthly
    let months: Vec<String> = {
        let mut m = BTreeSet::new();
        for k in 0..n_days {
            m.insert((from + Duration::days(k as i64)).format("%Y-%m").to_string());
        }
        m.into_iter().collect()
    };
    let month_idx: HashMap<&str, usize> = months.iter().enumerate().map(|(i, m)| (m.as_str(), i)).collect();
    let day_month: Vec<usize> = (0..n_days)
        .map(|k| month_idx[(from + Duration::days(k as i64)).format("%Y-%m").to_string().as_str()])
        .collect();
    for p in &pkgs {
        let s = &series[p];
        let off = (from - s.start).num_days() as usize;
        let slice = &s.days[off..off + n_days];
        let mut monthly = vec![0u64; months.len()];
        let mut tot = 0;
        for (k, v) in slice.iter().enumerate() {
            daily[k] += v;
            monthly[day_month[k]] += v;
            tot += v;
        }
        per_pkg.push((p.clone(), tot, monthly));
    }
    per_pkg.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
    let total: u64 = daily.iter().sum();

    let mut yearly: BTreeMap<i32, u64> = BTreeMap::new();
    for (k, v) in daily.iter().enumerate() {
        *yearly.entry(chrono::Datelike::year(&(from + Duration::days(k as i64)))).or_default() += v;
    }
    let (peak_i, peak_v) = daily.iter().enumerate().max_by_key(|x| x.1).map(|(i, v)| (i, *v)).unwrap_or((0, 0));
    let last = |n: usize| daily[n_days.saturating_sub(n)..].iter().sum::<u64>();

    println!();
    println!("  npm downloads · {mode} {target} · {from} → {to}");
    println!("  ─────────────────────────────────────────────");
    println!("  TOTAL           {:>16}", fmt(total));
    println!("  packages        {:>16}", pkgs.len());
    println!("  last 30 days    {:>16}", fmt(last(30)));
    println!("  last 365 days   {:>16}", fmt(last(365)));
    println!("  peak day        {:>16}  ({})", fmt(peak_v), from + Duration::days(peak_i as i64));
    println!();
    println!("  by year");
    let ymax = yearly.values().copied().max().unwrap_or(1).max(1);
    for (y, v) in yearly.iter().skip_while(|(_, v)| **v == 0) {
        let bar = "█".repeat(((*v as f64 / ymax as f64) * 30.0).round() as usize);
        println!("  {y}  {:>14}  {bar}", fmt(*v));
    }
    println!();
    println!("  top {} packages", top.min(per_pkg.len()));
    for (name, t, _) in per_pkg.iter().take(top) {
        let pct = if total > 0 { *t as f64 * 100.0 / total as f64 } else { 0.0 };
        println!("  {:>14}  {:>5.1}%  {name}", fmt(*t), pct);
    }
    println!();
    eprintln!(
        "  ⚡ {reqs} API requests ({} throttled), {hits}/{} packages fully cached · list {:.0?} · fetch {:.0?} · total {:.0?}",
        THROTTLED.load(Ordering::Relaxed),
        pkgs.len(),
        t_list,
        t_fetch - t_list,
        t0.elapsed()
    );

    serde_json::json!({
        "mode": mode, "target": target,
        "from": from.to_string(), "to": to.to_string(),
        "generated": Utc::now().to_rfc3339(),
        "total": total,
        "months": months,
        "daily": daily,
        "packages": per_pkg.iter().map(|(n, t, m)| serde_json::json!({"name": n, "total": t, "monthly": m})).collect::<Vec<_>>(),
    })
}
