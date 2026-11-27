//! `hb onboard`: a few questions that write (or update) a profile in the config.
//! Enter keeps the value in [brackets], "-" clears it.

use crate::config::{self, Config, Profile};
use serde_json::Value;
use std::io::{BufRead, Write};
use std::path::{Path, PathBuf};

fn ask(q: &str, def: &str) -> String {
    if def.is_empty() {
        eprint!("  {q}: ");
    } else {
        eprint!("  {q} [{def}]: ");
    }
    let _ = std::io::stderr().flush();
    let mut line = String::new();
    if std::io::stdin().lock().read_line(&mut line).unwrap_or(0) == 0 {
        eprintln!();
        return def.to_string();
    }
    match line.trim() {
        "" => def.to_string(),
        "-" => String::new(),
        v => v.to_string(),
    }
}

fn ask_list(q: &str, def: &[String]) -> Vec<String> {
    ask(q, &def.join(", ")).split([',', ' ']).map(|s| s.trim().to_string()).filter(|s| !s.is_empty()).collect()
}

fn ask_yes(q: &str, def: bool) -> bool {
    let a = ask(&format!("{q} (y/n)"), if def { "y" } else { "n" });
    a.to_lowercase().starts_with('y')
}

fn val_str(v: Option<&Value>) -> String {
    match v {
        Some(Value::String(s)) => s.clone(),
        Some(Value::Number(n)) => n.to_string(),
        _ => String::new(),
    }
}

fn read_config(p: &Path) -> Config {
    std::fs::read_to_string(p)
        .ok()
        .map(|t| {
            serde_json::from_str(&t).unwrap_or_else(|e| {
                eprintln!("humblebrag: can't read {}: {e}", p.display());
                std::process::exit(2)
            })
        })
        .unwrap_or_default()
}

pub fn run() {
    let default_path = config::path();
    let current = std::fs::canonicalize(&default_path).unwrap_or_else(|_| default_path.clone());
    let mut cfg = read_config(&current);

    eprintln!();
    eprintln!("  humblebrag onboard: set up a profile, then run `hb <profile>`.");
    eprintln!("  Enter keeps [the value shown], \"-\" clears it.");
    eprintln!();

    let first = cfg.profiles.keys().next().cloned().unwrap_or_else(|| std::env::var("USER").unwrap_or_default());
    let name = ask("Profile name (what you type after hb)", &first);
    if name.is_empty() {
        eprintln!("  A profile needs a name.");
        std::process::exit(2);
    }
    let mut p: Profile = cfg.profile(&name).map(|(_, p)| p).unwrap_or_default();
    let def_npm = if p.npm.is_empty() { vec![format!("~{name}")] } else { p.npm.clone() };
    let def_gh = if p.github.is_empty() { vec![name.clone()] } else { p.github.clone() };

    eprintln!();
    p.npm = ask_list("npm users (~name) and packages, comma separated", &def_npm);
    p.github = ask_list("GitHub users, orgs, or owner/repo (first one is you)", &def_gh);
    p.contributions = ask_yes("Track your merged PRs to repos you don't own", p.contributions || p.github.len() > 0);

    eprintln!();
    eprintln!("  Sites read automatically (leave empty to skip):");
    for (site, hint) in [
        ("x", "X handle"),
        ("instagram", "Instagram handle"),
        ("linkedin", "LinkedIn handle (linkedin.com/in/<handle>)"),
        ("youtube", "YouTube handle, like @name"),
        ("bluesky", "Bluesky handle, like name.bsky.social"),
        ("mastodon", "Mastodon account, like user@mastodon.social"),
        ("stackoverflow", "Stack Overflow user id (the number in your profile URL)"),
        ("hackernews", "Hacker News username"),
        ("reddit", "Reddit username"),
    ] {
        // A number here was typed in by hand before; offer to switch to a handle.
        let cur = p.social.get(site).filter(|v| v.is_string() || site == "stackoverflow");
        match ask(hint, &val_str(cur)).as_str() {
            "" if cur.is_some() => p.social.remove(site),
            "" => None, // keep a hand-typed number, if any
            v => p.social.insert(site.to_string(), Value::String(v.to_string())),
        };
    }

    eprintln!();
    eprintln!("  Sites humblebrag can't read. Type your follower count; update it any time:");
    let mut manual: Vec<String> = vec!["quora".to_string()];
    for k in p.social.keys() {
        if !crate::social::FETCHED.contains(&k.as_str()) && !manual.contains(k) {
            manual.push(k.clone());
        }
    }
    for site in manual {
        let a = ask(&format!("{site} followers"), &val_str(p.social.get(&site)));
        match a.replace(',', "").parse::<u64>() {
            Ok(n) => {
                p.social.insert(site, Value::from(n));
            }
            Err(_) if a.is_empty() => {
                p.social.remove(&site);
            }
            Err(_) => eprintln!("    skipped: '{a}' is not a number"),
        }
    }
    for pair in ask_list("Other sites as name=followers, comma separated", &[]) {
        if let Some((k, v)) = pair.split_once('=') {
            if let Ok(n) = v.replace(',', "").parse::<u64>() {
                p.social.insert(k.trim().to_lowercase(), Value::from(n));
            }
        }
    }

    eprintln!();
    let target = config::expand(&ask("Save config to", &config::tilde(&current)));
    if target != current {
        // Keep other profiles that already live in the chosen file.
        let mut other = read_config(&target);
        for (k, v) in std::mem::take(&mut cfg.profiles) {
            other.profiles.entry(k).or_insert(v);
        }
        other.record_dir = other.record_dir.or(cfg.record_dir.take());
        cfg = other;
    }
    cfg.profiles.retain(|k, _| !k.eq_ignore_ascii_case(&name));
    cfg.profiles.insert(name.clone(), p);
    if let Some(dir) = target.parent() {
        std::fs::create_dir_all(dir).expect("create config folder");
    }
    std::fs::write(&target, serde_json::to_string_pretty(&cfg).unwrap() + "\n").expect("write config");
    eprintln!("  saved {}", config::tilde(&target));

    if target != default_path && std::env::var_os("HUMBLEBRAG_CONFIG").is_none() {
        link(&default_path, &target);
    }
    eprintln!("  records go to {}", config::tilde(&cfg.record_dir()));
    eprintln!();
    eprintln!("  Run: hb {name}");
    eprintln!();
}

/// Point the default config path at `target` so every run finds it.
fn link(default_path: &PathBuf, target: &Path) {
    if std::fs::read_link(default_path).ok().as_deref() == Some(target) {
        return;
    }
    if let Some(dir) = default_path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    if default_path.symlink_metadata().is_ok() {
        let bak = default_path.with_extension("json.bak");
        if std::fs::rename(default_path, &bak).is_ok() {
            eprintln!("  moved the old {} to {}", config::tilde(default_path), config::tilde(&bak));
        }
    }
    #[cfg(unix)]
    let r = std::os::unix::fs::symlink(target, default_path);
    #[cfg(not(unix))]
    let r = std::fs::copy(target, default_path).map(|_| ());
    match r {
        Ok(()) => eprintln!("  linked {} -> {}", config::tilde(default_path), config::tilde(target)),
        Err(e) => eprintln!("  couldn't link {}: {e}. Set HUMBLEBRAG_CONFIG={} instead.", config::tilde(default_path), target.display()),
    }
}
