//! GitHub stats: stars, forks, watchers and followers for users, orgs and repos.
//!
//! With a token (GITHUB_TOKEN, GH_TOKEN, or `gh auth token`) it uses GraphQL:
//! 100 repos per request, watchers included. Without one it falls back to the
//! REST API (60 requests/hour), which has no cheap watcher count for owners.

use futures::future::join_all;
use serde_json::{json, Value};

const REST: &str = "https://api.github.com";
const GRAPHQL: &str = "https://api.github.com/graphql";

pub struct Repo {
    pub name: String,
    pub stars: u64,
    pub forks: u64,
    pub watchers: Option<u64>,
}

pub struct Owner {
    pub login: String,
    pub kind: &'static str, // "user" | "org" | "repo"
    pub followers: Option<u64>,
    pub repos: Vec<Repo>,
}

impl Owner {
    fn stars(&self) -> u64 {
        self.repos.iter().map(|r| r.stars).sum()
    }
    fn forks(&self) -> u64 {
        self.repos.iter().map(|r| r.forks).sum()
    }
    fn watchers(&self) -> Option<u64> {
        self.repos.iter().map(|r| r.watchers).sum()
    }
}

/// A token from the environment or the GitHub CLI. Never printed.
pub fn token() -> Option<String> {
    for k in ["GITHUB_TOKEN", "GH_TOKEN"] {
        if let Ok(t) = std::env::var(k) {
            if !t.trim().is_empty() {
                return Some(t.trim().to_string());
            }
        }
    }
    let out = std::process::Command::new("gh").args(["auth", "token"]).output().ok()?;
    let t = String::from_utf8(out.stdout).ok()?.trim().to_string();
    (out.status.success() && !t.is_empty()).then_some(t)
}

/// `owner/repo` from an npm `repository` field, if it points at GitHub.
pub fn repo_from_npm(repository: &Value) -> Option<String> {
    let url = repository.as_str().or_else(|| repository.get("url")?.as_str())?;
    let rest = if let Some(r) = url.strip_prefix("github:") {
        r
    } else if let Some(i) = url.find("github.com") {
        &url[i + "github.com".len() + 1..]
    } else if url.split('/').count() == 2 && !url.contains(':') {
        url // "owner/repo" shorthand
    } else {
        return None;
    };
    let mut parts = rest.split(['/', '#']).filter(|s| !s.is_empty());
    let owner = parts.next()?;
    let repo = parts.next()?.trim_end_matches(".git");
    Some(format!("{owner}/{repo}"))
}

async fn send(req: reqwest::RequestBuilder, token: &Option<String>) -> Result<Option<Value>, String> {
    let mut req = req.header("accept", "application/vnd.github+json").header("x-github-api-version", "2022-11-28");
    if let Some(t) = token {
        req = req.bearer_auth(t);
    }
    let r = req.send().await.map_err(|e| e.to_string())?;
    let s = r.status().as_u16();
    if s == 404 {
        return Ok(None);
    }
    if s == 401 {
        return Err("GitHub rejected the token (401). Check GITHUB_TOKEN or run `gh auth login`.".into());
    }
    if s == 403 || s == 429 {
        let hint = if token.is_some() { "" } else { " Set GITHUB_TOKEN or run `gh auth login` for 5,000 requests/hour." };
        return Err(format!("GitHub rate limit hit ({s}).{hint}"));
    }
    if !r.status().is_success() {
        return Err(format!("GitHub returned {s}"));
    }
    let b = r.bytes().await.map_err(|e| e.to_string())?;
    serde_json::from_slice(&b).map(Some).map_err(|e| e.to_string())
}

async fn graphql(client: &reqwest::Client, token: &Option<String>, query: &str, vars: Value) -> Result<Value, String> {
    let body = json!({ "query": query, "variables": vars });
    let v = send(client.post(GRAPHQL).header("content-type", "application/json").body(body.to_string()), token).await?.ok_or("GitHub GraphQL returned 404")?;
    if let Some(errs) = v.get("errors").and_then(|e| e.as_array()) {
        // "NOT_FOUND" comes back as an error with null data; let callers see null.
        if errs.iter().any(|e| e["type"] != "NOT_FOUND") {
            return Err(format!("GitHub: {}", errs[0]["message"].as_str().unwrap_or("GraphQL error")));
        }
    }
    Ok(v["data"].clone())
}

fn num(v: &Value) -> u64 {
    v.as_u64().unwrap_or(0)
}

const OWNER_Q: &str = "query($login:String!,$after:String){repositoryOwner(login:$login){login __typename
 ... on User{followers{totalCount}}
 repositories(first:100,after:$after,ownerAffiliations:OWNER,privacy:PUBLIC,isFork:false){
  pageInfo{hasNextPage endCursor} nodes{name stargazerCount forkCount watchers{totalCount}}}}}";

async fn owner_graphql(client: &reqwest::Client, token: &Option<String>, login: &str) -> Result<Owner, String> {
    let mut after = Value::Null;
    let mut owner: Option<Owner> = None;
    loop {
        let d = graphql(client, token, OWNER_Q, json!({ "login": login, "after": after })).await?;
        let o = &d["repositoryOwner"];
        if o.is_null() {
            return Err(format!("'{login}' is not a GitHub user or org"));
        }
        let own = owner.get_or_insert_with(|| Owner {
            login: o["login"].as_str().unwrap_or(login).to_string(),
            kind: if o["__typename"] == "Organization" { "org" } else { "user" },
            followers: o["followers"]["totalCount"].as_u64(),
            repos: vec![],
        });
        let page = &o["repositories"];
        for n in page["nodes"].as_array().into_iter().flatten() {
            own.repos.push(Repo {
                name: n["name"].as_str().unwrap_or("").to_string(),
                stars: num(&n["stargazerCount"]),
                forks: num(&n["forkCount"]),
                watchers: Some(num(&n["watchers"]["totalCount"])),
            });
        }
        if page["pageInfo"]["hasNextPage"] != true {
            break;
        }
        after = page["pageInfo"]["endCursor"].clone();
    }
    Ok(owner.unwrap())
}

async fn owner_rest(client: &reqwest::Client, login: &str) -> Result<Owner, String> {
    let u = send(client.get(format!("{REST}/users/{login}")), &None)
        .await?
        .ok_or_else(|| format!("'{login}' is not a GitHub user or org"))?;
    let is_org = u["type"] == "Organization";
    let pages = num(&u["public_repos"]).div_ceil(100).max(1);
    let lists = join_all((1..=pages).map(|p| {
        send(client.get(format!("{REST}/users/{login}/repos?per_page=100&type=owner&page={p}")), &None)
    }))
    .await;
    let mut repos = vec![];
    for l in lists {
        for r in l?.unwrap_or(Value::Null).as_array().into_iter().flatten() {
            if r["fork"] == true {
                continue;
            }
            repos.push(Repo {
                name: r["name"].as_str().unwrap_or("").to_string(),
                stars: num(&r["stargazers_count"]),
                forks: num(&r["forks_count"]),
                watchers: None, // REST's watchers_count is really stars
            });
        }
    }
    Ok(Owner {
        login: u["login"].as_str().unwrap_or(login).to_string(),
        kind: if is_org { "org" } else { "user" },
        followers: (!is_org).then(|| num(&u["followers"])),
        repos,
    })
}

async fn repo(client: &reqwest::Client, token: &Option<String>, full: &str) -> Result<Owner, String> {
    let r = send(client.get(format!("{REST}/repos/{full}")), token)
        .await?
        .ok_or_else(|| format!("GitHub repo '{full}' not found"))?;
    let name = r["full_name"].as_str().unwrap_or(full).to_string();
    Ok(Owner {
        login: name.clone(),
        kind: "repo",
        followers: None,
        repos: vec![Repo {
            name,
            stars: num(&r["stargazers_count"]),
            forks: num(&r["forks_count"]),
            watchers: Some(num(&r["subscribers_count"])),
        }],
    })
}

/// Fetch every target concurrently. A target is a login (user or org) or `owner/repo`.
pub async fn fetch(client: &reqwest::Client, targets: &[String], token: &Option<String>) -> Vec<Result<Owner, String>> {
    join_all(targets.iter().map(|t| async move {
        let t = t.trim_start_matches('@');
        if t.contains('/') {
            repo(client, token, t).await
        } else if token.is_some() {
            owner_graphql(client, token, t).await
        } else {
            owner_rest(client, t).await
        }
    }))
    .await
}

fn fmt(n: u64) -> String {
    crate::fmt(n)
}

pub fn print(owners: &[Owner], top: usize, has_token: bool) {
    if owners.is_empty() {
        return;
    }
    let label = owners.iter().map(|o| format!("{} {}", o.kind, o.login)).collect::<Vec<_>>().join(", ");
    let stars: u64 = owners.iter().map(|o| o.stars()).sum();
    let forks: u64 = owners.iter().map(|o| o.forks()).sum();
    let watchers: Option<u64> = owners.iter().map(|o| o.watchers()).sum();
    let followers: Vec<u64> = owners.iter().filter_map(|o| o.followers).collect();
    let n_repos: usize = owners.iter().map(|o| o.repos.len()).sum();
    let owned = owners.iter().any(|o| o.kind != "repo");

    println!();
    println!("  github · {label}");
    println!("  ─────────────────────────────────────────────");
    println!("  stars           {:>16}", fmt(stars));
    println!("  forks           {:>16}", fmt(forks));
    match watchers {
        Some(w) => println!("  watchers        {:>16}", fmt(w)),
        None if !has_token => println!("  watchers        {:>16}  (needs a token: GITHUB_TOKEN or `gh auth login`)", "n/a"),
        None => {}
    }
    if !followers.is_empty() {
        println!("  followers       {:>16}", fmt(followers.iter().sum()));
    }
    if owned {
        println!("  repos           {:>16}  (public, not forks)", n_repos);
    }

    let multi = owners.len() > 1;
    let mut rows: Vec<(String, &Repo)> = owners
        .iter()
        .flat_map(|o| {
            o.repos.iter().map(move |r| {
                let name = if multi && o.kind != "repo" { format!("{}/{}", o.login, r.name) } else { r.name.clone() };
                (name, r)
            })
        })
        .collect();
    if rows.len() > 1 {
        rows.sort_by(|a, b| b.1.stars.cmp(&a.1.stars).then(b.1.forks.cmp(&a.1.forks)).then(a.0.cmp(&b.0)));
        let w = rows.iter().take(top).map(|r| r.0.chars().count()).max().unwrap_or(4).max(4);
        println!();
        println!("  top {} repos", top.min(rows.len()));
        let wh = if watchers.is_some() { format!("  {:>8}", "watchers") } else { String::new() };
        println!("  {:<w$}  {:>9}  {:>8}{wh}", "", "stars", "forks");
        for (name, r) in rows.iter().take(top) {
            let wv = r.watchers.map(|v| format!("  {:>8}", fmt(v))).unwrap_or_default();
            println!("  {name:<w$}  {:>9}  {:>8}{wv}", fmt(r.stars), fmt(r.forks));
        }
    }
    println!();
}

pub fn to_json(owners: &[Owner]) -> Value {
    json!({
        "stars": owners.iter().map(|o| o.stars()).sum::<u64>(),
        "forks": owners.iter().map(|o| o.forks()).sum::<u64>(),
        "watchers": owners.iter().map(|o| o.watchers()).sum::<Option<u64>>(),
        "followers": owners.iter().filter_map(|o| o.followers).reduce(|a, b| a + b),
        "owners": owners.iter().map(|o| json!({
            "login": o.login, "kind": o.kind, "followers": o.followers,
            "stars": o.stars(), "forks": o.forks(), "watchers": o.watchers(),
            "repos": o.repos.iter().map(|r| json!({"name": r.name, "stars": r.stars, "forks": r.forks, "watchers": r.watchers})).collect::<Vec<_>>(),
        })).collect::<Vec<_>>(),
    })
}
