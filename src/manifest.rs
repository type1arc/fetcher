use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs;

#[derive(Deserialize, Debug)]
#[allow(dead_code)]
pub struct Manifest {
    pub pkg_name: PkgInfo,
    pub source: Option<Source>,
    pub deps: HashMap<String, DepSpec>,
    #[serde(default)]
    pub pre_install: Option<String>,
    #[serde(default)]
    pub post_install: Option<String>,
}

#[derive(Deserialize, Debug)]
#[allow(dead_code)]
pub struct PkgInfo {
    pub name: String,
    pub version: String,
}

#[derive(Deserialize, Debug)]
pub struct Source {
    pub github: Option<String>,
    pub registry: Option<String>,
    pub url: Option<String>,
}

#[derive(Deserialize, Debug)]
#[serde(untagged)]
pub enum DepSpec {
    Simple(String),
    Full {
        version: Option<String>,
        github: Option<String>,
        url: Option<String>,
        git: Option<String>,
        rev: Option<String>,
        #[serde(default)]
        sha256: Option<String>,
        #[serde(default)]
        build: Option<String>,
        #[serde(default)]
        pre_install: Option<String>,
        #[serde(default)]
        post_install: Option<String>,
    },
}

impl DepSpec {
    pub fn version(&self) -> Option<&str> {
        match self {
            DepSpec::Simple(v) => Some(v.as_str()),
            DepSpec::Full { version, .. } => version.as_deref(),
        }
    }

    pub fn source_override(&self) -> Option<SourceOverride> {
        match self {
            DepSpec::Simple(_) => None,
            DepSpec::Full { github, url, git, rev, .. } => {
                if let Some(repo) = github {
                    Some(SourceOverride::Github(repo.clone()))
                } else if let Some(u) = url {
                    Some(SourceOverride::Url(u.clone()))
                } else if let Some(g) = git {
                    Some(SourceOverride::Git(g.clone(), rev.clone()))
                } else {
                    None
                }
            }
        }
    }

    pub fn pre_install(&self) -> Option<&str> {
        match self {
            DepSpec::Simple(_) => None,
            DepSpec::Full { pre_install, .. } => pre_install.as_deref(),
        }
    }

    pub fn post_install(&self) -> Option<&str> {
        match self {
            DepSpec::Simple(_) => None,
            DepSpec::Full { post_install, .. } => post_install.as_deref(),
        }
    }

    pub fn sha256(&self) -> Option<&str> {
        match self {
            DepSpec::Simple(_) => None,
            DepSpec::Full { sha256, .. } => sha256.as_deref(),
        }
    }

    pub fn build(&self) -> Option<&str> {
        match self {
            DepSpec::Simple(_) => None,
            DepSpec::Full { build, .. } => build.as_deref(),
        }
    }
}

#[derive(Debug)]
pub enum SourceOverride {
    Github(String),
    Url(String),
    Git(String, Option<String>),
}

// ── Lockfile ─────────────────────────────────────────────────────────────────

#[derive(Serialize, Deserialize, Debug)]
pub struct Lockfile {
    pub version: u32,
    #[serde(default)]
    pub deps: HashMap<String, LockedDep>,
}

#[derive(Serialize, Deserialize, Debug)]
pub struct LockedDep {
    pub version: String,
    pub download_url: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub repo: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sha256: Option<String>,
}

pub fn load_manifest() -> Result<Manifest, Box<dyn std::error::Error>> {
    let content = fs::read_to_string("package.toml")?;
    let manifest: Manifest = toml::from_str(&content)?;
    Ok(manifest)
}

pub fn read_lockfile() -> Result<Lockfile, Box<dyn std::error::Error>> {
    let content = fs::read_to_string(".fetcher.lock")?;
    let lock: Lockfile = toml::from_str(&content)?;
    Ok(lock)
}

pub fn write_lockfile(lock: &Lockfile) -> Result<(), Box<dyn std::error::Error>> {
    let content = toml::to_string_pretty(lock)?;
    fs::write(".fetcher.lock", content)?;
    Ok(())
}

pub fn build_url(
    source_override: Option<&SourceOverride>,
    manifest_source: &Option<Source>,
    name: &str,
    version: &str,
) -> Result<String, Box<dyn std::error::Error + Send + Sync>> {
    fn github_archive(org: &str, repo: &str, version: &str) -> String {
        if version == "latest" {
            format!("https://github.com/{org}/{repo}/archive/HEAD.zip")
        } else {
            format!("https://github.com/{org}/{repo}/archive/refs/tags/v{version}.zip")
        }
    }

    if let Some(so) = source_override {
        match so {
            SourceOverride::Github(repo) => {
                let (org, repo_name) = repo.split_once('/')
                    .ok_or_else(|| format!("invalid github 'owner/repo': {repo}"))?;
                return Ok(github_archive(org, repo_name, version));
            }
            SourceOverride::Url(u) => return Ok(u.replace("{name}", name).replace("{version}", version)),
            SourceOverride::Git(_, _) => return Ok(String::new()),
        }
    }
    if let Some(src) = manifest_source {
        if let Some(ref github) = src.github {
            return Ok(github_archive(github, name, version));
        }
        if let Some(ref registry) = src.registry {
            return Ok(registry.replace("{name}", name).replace("{version}", version));
        }
        if let Some(ref url) = src.url {
            return Ok(url.replace("{name}", name).replace("{version}", version));
        }
    }
    if let Ok(org) = std::env::var("FETCHER_GITHUB_ORG") {
        return Ok(github_archive(&org, name, version));
    }
    Err("no source configured and no FETCHER_GITHUB_ORG set".into())
}

pub fn is_git_source(so: Option<&SourceOverride>) -> bool {
    matches!(so, Some(SourceOverride::Git(_, _)))
}
