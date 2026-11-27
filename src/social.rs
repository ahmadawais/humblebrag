//! Public follower counts from sites that expose them without a login, plus
//! numbers you keep by hand for sites that don't (X, Instagram, LinkedIn...).
//! Also a tiny history file: one JSON line per profile per day.

use futures::future::join_all;
use serde_json::{json, Map, Value};
use std::collections::BTreeMap;
use std::path::Path;

pub struct Stat {
    pub site: String,
    pub value: u64,
    pub unit: &'static str, // "followers" | "subscribers" | "reputation" | "karma"
    pub note: &'static str, // "" | "approx" | "manual"
}

impl Stat {
    /// Counts people (followers, subscribers), as opposed to points.
    pub fn is_audience(&self) -> bool {
        matches!(self.unit, "followers" | "subscribers")
    }
}

const UA: &str = "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/130 Safari/537.36";

async fn get_text(client: &reqwest::Client, url: &str, browser: bool) -> Result<String, String> {
    let mut req = client.get(url).header("accept-language", "en-US,en");
    if browser {
        req = req.header("user-agent", UA);
    }
    let r = req.send().await.map_err(|e| e.to_string())?;
    if !r.status().is_success() {
        return Err(format!("HTTP {}", r.status().as_u16()));
    }
    r.text().await.map_err(|e| e.to_string())
}

async fn get_json(client: &reqwest::Client, url: &str) -> Result<Value, String> {
    serde_json::from_str(&get_text(client, url, false).await?).map_err(|e| e.to_string())
}

/// "6.12K" -> 6120, "1.2M" -> 1200000, "3,611" -> 3611.
fn parse_count(s: &str) -> Option<u64> {
    let s = s.trim().replace(',', "");
    let (num, mul) = match s.chars().last()? {
        'K' | 'k' => (&s[..s.len() - 1], 1e3),
        'M' | 'm' => (&s[..s.len() - 1], 1e6),
        'B' | 'b' => (&s[..s.len() - 1], 1e9),
        _ => (&s[..], 1.0),
    };
    num.parse::<f64>().ok().map(|n| (n * mul).round() as u64)
}

fn id_str(v: &Value) -> Option<String> {
    v.as_str().map(|s| s.to_string()).or_else(|| v.as_u64().map(|n| n.to_string()))
}

async fn fetch_one(client: &reqwest::Client, site: &str, v: &Value) -> Result<Stat, String> {
    let stat = |value, unit, note| Ok(Stat { site: site.to_string(), value, unit, note });
    let id = id_str(v).ok_or("expected a handle or id")?;
    match site {
        "youtube" => {
            let handle = if id.starts_with('@') || id.starts_with("channel/") { id } else { format!("@{id}") };
            let html = get_text(client, &format!("https://www.youtube.com/{handle}"), true).await?;
            // The page embeds e.g. "6.12K subscribers" (YouTube rounds it).
            let n = html
                .split('"')
                .find_map(|s| s.strip_suffix(" subscribers").and_then(parse_count))
                .ok_or("subscriber count not found on the page")?;
            stat(n, "subscribers", "approx")
        }
        "bluesky" => {
            let d = get_json(client, &format!("https://public.api.bsky.app/xrpc/app.bsky.actor.getProfile?actor={id}")).await?;
            stat(d["followersCount"].as_u64().ok_or("no followersCount")?, "followers", "")
        }
        "mastodon" => {
            let (user, host) = id.trim_start_matches('@').split_once('@').ok_or("use user@instance")?;
            let d = get_json(client, &format!("https://{host}/api/v1/accounts/lookup?acct={user}")).await?;
            stat(d["followers_count"].as_u64().ok_or("no followers_count")?, "followers", "")
        }
        "stackoverflow" => {
            let d = get_json(client, &format!("https://api.stackexchange.com/2.3/users/{id}?site=stackoverflow")).await?;
            stat(d["items"][0]["reputation"].as_u64().ok_or("user not found")?, "reputation", "")
        }
        "hackernews" => {
            let d = get_json(client, &format!("https://hacker-news.firebaseio.com/v0/user/{id}.json")).await?;
            stat(d["karma"].as_u64().ok_or("user not found")?, "karma", "")
        }
        _ => Err("unknown site".into()),
    }
}

pub const FETCHED: [&str; 5] = ["youtube", "bluesky", "mastodon", "stackoverflow", "hackernews"];

/// Sites in FETCHED take a handle or id. Any other site takes a number you keep up to date.
pub async fn fetch(client: &reqwest::Client, social: &BTreeMap<String, Value>) -> Vec<Result<Stat, String>> {
    join_all(social.iter().map(|(site, v)| async move {
        if FETCHED.contains(&site.as_str()) {
            fetch_one(client, site, v).await.map_err(|e| format!("{site}: {e}"))
        } else {
            match v.as_u64() {
                Some(n) => Ok(Stat { site: site.clone(), value: n, unit: "followers", note: "manual" }),
                None => Err(format!("{site}: can't fetch this site; set a number of followers instead")),
            }
        }
    }))
    .await
}

pub fn print(name: &str, stats: &[Stat], gh_followers: Option<u64>) {
    if stats.is_empty() {
        return;
    }
    println!();
    println!("  social · {name}");
    println!("  ─────────────────────────────────────────────");
    for s in stats {
        let note = match s.note {
            "approx" => " (rounded by the site)",
            "manual" => " (from config)",
            _ => "",
        };
        println!("  {:<15} {:>16}  {}{note}", s.site, crate::fmt(s.value), s.unit);
    }
    let audience: u64 = stats.iter().filter(|s| s.is_audience()).map(|s| s.value).sum::<u64>() + gh_followers.unwrap_or(0);
    println!("  {:<15} {:>16}  followers + subscribers{}", "AUDIENCE", crate::fmt(audience), if gh_followers.is_some() { ", GitHub included" } else { "" });
    println!();
}

// ---------- history ----------

/// Write today's numbers (replacing an earlier run today) and return the most
/// recent earlier entry, for "since last time" deltas. One line per day.
pub fn record_history(path: &Path, metrics: &Map<String, Value>) -> Result<Option<Map<String, Value>>, String> {
    let today = chrono::Utc::now().date_naive().to_string();
    let mut lines: Vec<Map<String, Value>> = std::fs::read_to_string(path)
        .unwrap_or_default()
        .lines()
        .filter_map(|l| serde_json::from_str::<Value>(l).ok()?.as_object().cloned())
        .collect();
    lines.retain(|l| l.get("date").and_then(|d| d.as_str()) != Some(today.as_str()));
    let prev = lines.last().cloned();
    let mut row = Map::new();
    row.insert("date".into(), json!(today));
    row.extend(metrics.clone());
    lines.push(row);
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    }
    let body: String = lines.iter().map(|l| serde_json::to_string(l).unwrap() + "\n").collect();
    std::fs::write(path, body).map_err(|e| e.to_string())?;
    Ok(prev)
}

pub fn print_deltas(prev: &Map<String, Value>, now: &Map<String, Value>) {
    let date = prev.get("date").and_then(|d| d.as_str()).unwrap_or("?");
    let mut rows = vec![];
    for (k, v) in now {
        let (Some(a), Some(b)) = (prev.get(k).and_then(|x| x.as_i64()), v.as_i64()) else { continue };
        if a != b {
            let d = b - a;
            rows.push(format!("  {:<15} {:>16}", k, format!("{}{}", if d > 0 { "+" } else { "-" }, crate::fmt(d.unsigned_abs()))));
        }
    }
    println!("  since {date}");
    println!("  ─────────────────────────────────────────────");
    if rows.is_empty() {
        println!("  no change");
    }
    for r in rows {
        println!("{r}");
    }
    println!();
}
