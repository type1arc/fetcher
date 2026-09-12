use crate::repo::{AptRepo, PacmanRepo, ApkRepo, read_apt_sources, read_pacman_repos, read_apk_repos};
use serde::Deserialize;
use std::fs;
use std::path::{Path, PathBuf};

#[derive(Deserialize, Debug, Default, Clone)]
pub struct FetcherConfig {
    /// Also scan system-configured repos (APT sources, pacman.conf, apk/repositories)
    #[serde(default)]
    pub include_system: bool,
    #[serde(default)]
    pub apt: Vec<AptRepo>,
    #[serde(default)]
    pub pacman: Vec<PacmanRepo>,
    #[serde(default)]
    pub apk: Vec<ApkRepo>,
}

impl FetcherConfig {
    pub fn path() -> Option<PathBuf> {
        if let Ok(p) = std::env::var("FETCHER_CONFIG") {
            let p = PathBuf::from(p);
            if p.is_file() { return Some(p); }
        }
        if Path::new("fetcher.conf").is_file() {
            return Some(PathBuf::from("fetcher.conf"));
        }
        if let Some(dir) = xdg_config_dir() {
            let p = dir.join("fetcher").join("fetcher.conf");
            if p.is_file() { return Some(p); }
        }
        let p = PathBuf::from("/etc/fetcher.conf");
        if p.is_file() { return Some(p); }
        None
    }
}

fn xdg_config_dir() -> Option<PathBuf> {
    if let Ok(d) = std::env::var("XDG_CONFIG_HOME") {
        if !d.is_empty() { return Some(PathBuf::from(d)); }
    }
    std::env::var("HOME").ok().map(|h| PathBuf::from(h).join(".config"))
}

/// Repo lists used for searching: config-file repos plus system repos when enabled.
///
/// Returns (apt, pacman, apk).
pub fn repo_lists() -> (Vec<AptRepo>, Vec<PacmanRepo>, Vec<ApkRepo>) {
    let (mut apt, mut pacman, mut apk, include_system) = match FetcherConfig::load() {
        Some(c) => (c.apt, c.pacman, c.apk, c.include_system),
        None => (Vec::new(), Vec::new(), Vec::new(), true),
    };
    if include_system {
        apt.extend(read_apt_sources());
        pacman.extend(read_pacman_repos());
        apk.extend(read_apk_repos());
    }
    (apt, pacman, apk)
}

impl FetcherConfig {
    fn load() -> Option<FetcherConfig> {
        let path = FetcherConfig::path()?;
        let content = fs::read_to_string(&path).ok()?;
        match toml::from_str::<FetcherConfig>(&content) {
            Ok(cfg) => Some(cfg),
            Err(e) => {
                eprintln!("warning: ignoring fetcher.conf at {}: {e}", path.display());
                None
            }
        }
    }
}