use clap::{Parser, Subcommand};
use flate2::read::GzDecoder;
use futures::StreamExt;
use reqwest::Client;
use serde::Deserialize;
use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use indicatif::{MultiProgress, ProgressBar, ProgressStyle};
use std::fs;
use std::io::Read;
use std::path::Path;
use std::process::Command;
use std::time::Duration;

#[derive(Parser)]
#[command(name = "fetcher", version = "0.4.0", about = "A configurable package fetcher")]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    Fetch {
        pkg: Option<String>,
        #[arg(long)]
        github: Option<String>,
        #[arg(long)]
        url: Option<String>,
        #[arg(long)]
        git: Option<String>,
        #[arg(long)]
        rev: Option<String>,
    },
}

// ── Manifest types ──────────────────────────────────────────────────────────

#[derive(Deserialize, Debug)]
#[allow(dead_code)]
struct Manifest {
    pkg_name: PkgInfo,
    source: Option<Source>,
    deps: HashMap<String, DepSpec>,
}

#[derive(Deserialize, Debug)]
#[allow(dead_code)]
struct PkgInfo {
    name: String,
    version: String,
}

#[derive(Deserialize, Debug)]
struct Source {
    github: Option<String>,
    registry: Option<String>,
    url: Option<String>,
}

#[derive(Deserialize, Debug)]
#[serde(untagged)]
enum DepSpec {
    Simple(String),
    Full {
        version: Option<String>,
        github: Option<String>,
        url: Option<String>,
        git: Option<String>,
        rev: Option<String>,
    },
}

impl DepSpec {
    fn version(&self) -> Option<&str> {
        match self {
            DepSpec::Simple(v) => Some(v.as_str()),
            DepSpec::Full { version, .. } => version.as_deref(),
        }
    }

    fn source_override(&self) -> Option<SourceOverride> {
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
}

#[derive(Debug)]
enum SourceOverride {
    Github(String),
    Url(String),
    Git(String, Option<String>),
}

// ── System repo types ───────────────────────────────────────────────────────

struct AptRepo {
    uri: String,
    suite: String,
    components: Vec<String>,
}

struct PacmanRepo {
    name: String,
    server: String,
}

struct ApkRepo {
    uri: String,
}

#[derive(Debug)]
struct FoundPkg {
    name: String,
    version: String,
    repo: String,
    download_url: String,
}

// ── Architecture detection ──────────────────────────────────────────────────

fn native_arch() -> String {
    let out = Command::new("uname").arg("-m").output()
        .ok().and_then(|o| String::from_utf8(o.stdout).ok()).unwrap_or_default();
    match out.trim() {
        "x86_64" => "amd64".to_string(),
        "aarch64" => "arm64".to_string(),
        "i686" | "i386" => "i386".to_string(),
        a => a.to_string(),
    }
}

fn pacman_arch() -> String {
    Command::new("uname").arg("-m").output()
        .ok().and_then(|o| String::from_utf8(o.stdout).ok())
        .map(|s| s.trim().to_string())
        .unwrap_or_else(|| "x86_64".to_string())
}

// ── Repo config readers ─────────────────────────────────────────────────────

fn read_apt_sources() -> Vec<AptRepo> {
    let mut repos = Vec::new();
    if let Ok(content) = fs::read_to_string("/etc/apt/sources.list") {
        parse_apt_lines(&content, &mut repos);
    }
    if let Ok(entries) = fs::read_dir("/etc/apt/sources.list.d") {
        for entry in entries.flatten() {
            let p = entry.path();
            if p.extension().map(|e| e == "list" || e == "sources").unwrap_or(false) {
                if let Ok(c) = fs::read_to_string(&p) {
                    parse_apt_lines(&c, &mut repos);
                }
            }
        }
    }
    repos
}

fn parse_apt_lines(content: &str, repos: &mut Vec<AptRepo>) {
    for line in content.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') || line.starts_with("deb-src") { continue; }
        let rest = line.strip_prefix("deb").map(|s| s.trim()).unwrap_or("");
        let rest = if rest.starts_with('[') {
            rest.find(']').map(|i| rest[i+1..].trim()).unwrap_or("")
        } else { rest };
        let parts: Vec<&str> = rest.split_whitespace().collect();
        if parts.len() < 2 { continue; }
        repos.push(AptRepo {
            uri: parts[0].trim_end_matches('/').to_string(),
            suite: parts[1].to_string(),
            components: parts[2..].iter().map(|s| s.to_string()).collect(),
        });
    }
}

fn read_pacman_repos() -> Vec<PacmanRepo> {
    let mut repos = Vec::new();
    let content = match fs::read_to_string("/etc/pacman.conf") {
        Ok(c) => c,
        Err(_) => return repos,
    };
    let mut section = String::new();
    for line in content.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') { continue; }
        if line.starts_with('[') && line.ends_with(']') {
            section = line[1..line.len()-1].to_string();
            continue;
        }
        // Direct Server= line
        if let Some(val) = line.strip_prefix("Server = ") {
            let s = val.trim().to_string();
            if !section.is_empty() && !s.is_empty() {
                repos.push(PacmanRepo { name: section.clone(), server: s });
            }
        }
        // Include= directive – read the included file for Server= lines
        if let Some(val) = line.strip_prefix("Include = ") {
            if let Ok(inc) = fs::read_to_string(val.trim()) {
                for il in inc.lines() {
                    let il = il.trim();
                    if let Some(sv) = il.strip_prefix("Server = ") {
                        let s = sv.trim().to_string();
                        if !section.is_empty() && !s.is_empty() {
                            repos.push(PacmanRepo { name: section.clone(), server: s });
                        }
                    }
                }
            }
        }
    }
    repos
}

fn read_apk_repos() -> Vec<ApkRepo> {
    let content = match fs::read_to_string("/etc/apk/repositories") {
        Ok(c) => c,
        Err(_) => return Vec::new(),
    };
    content.lines().filter_map(|line| {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') { return None; }
        let start = line.find("http://").or_else(|| line.find("https://"))?;
        Some(ApkRepo { uri: line[start..].trim_end_matches('/').to_string() })
    }).collect()
}

// ── Index fetchers ──────────────────────────────────────────────────────────

async fn fetch_gz(client: &Client, url: &str) -> Result<Vec<u8>, Box<dyn std::error::Error + Send + Sync>> {
    let compressed = client.get(url).send().await?.bytes().await?;
    let mut out = Vec::new();
    match GzDecoder::new(&compressed[..]).read_to_end(&mut out) {
        Ok(_) => Ok(out),
        Err(_) => Ok(compressed.to_vec()),
    }
}

async fn fetch_apt_index(client: &Client, repo: &AptRepo, pb: &ProgressBar) -> Vec<(String, String, String)> {
    let arch = native_arch();
    let mut results = Vec::new();
    for comp in &repo.components {
        pb.set_message(format!("checking apt: {}/{} [component: {}]", repo.uri, repo.suite, comp));
        let url = format!("{}/dists/{}/{}/binary-{}/Packages.gz", repo.uri, repo.suite, comp, arch);
        let data = match fetch_gz(client, &url).await { Ok(d) => d, Err(_) => { pb.inc(1); continue; } };
        let text = match String::from_utf8(data) { Ok(t) => t, Err(_) => { pb.inc(1); continue; } };
        for record in text.split("\n\n") {
            let (mut name, mut ver, mut fname, mut parch) = (String::new(), String::new(), String::new(), String::new());
            for field in record.lines() {
                if let Some(v) = field.strip_prefix("Package: ") { name = v.to_string(); }
                else if let Some(v) = field.strip_prefix("Version: ") { ver = v.to_string(); }
                else if let Some(v) = field.strip_prefix("Filename: ") { fname = v.to_string(); }
                else if let Some(v) = field.strip_prefix("Architecture: ") { parch = v.to_string(); }
            }
            if !name.is_empty() && !fname.is_empty() && (parch == arch || parch == "all") {
                results.push((name, ver, format!("{}/{}", repo.uri.trim_end_matches('/'), fname)));
            }
        }
        pb.inc(1);
    }
    results
}

async fn fetch_pacman_index(client: &Client, repo: &PacmanRepo) -> Vec<(String, String, String)> {
    let arch = pacman_arch();
    let base = repo.server.replace("$repo", &repo.name).replace("$arch", &arch);
    let db_url = format!("{}/{}.db.tar.gz", base.trim_end_matches('/'), repo.name);
    let data = match fetch_gz(client, &db_url).await { Ok(d) => d, Err(_) => return Vec::new() };

    let mut archive = tar::Archive::new(&data[..]);
    let mut results = Vec::new();
    for mut entry in archive.entries().unwrap().filter_map(|e| e.ok()) {
        let path = entry.path().unwrap().to_string_lossy().to_string();
        if !path.ends_with("/desc") { continue; }
        let mut raw = Vec::new();
        let _ = Read::read_to_end(&mut entry, &mut raw);
        let text = String::from_utf8_lossy(&raw);
        let mut fields = HashMap::new();
        let mut key = String::new();
        let mut val = String::new();
        for line in text.lines() {
            if line.starts_with('%') && line.ends_with('%') {
                if !key.is_empty() { fields.insert(key.clone(), val.trim().to_string()); }
                key = line.trim_matches('%').to_string();
                val.clear();
            } else { val.push_str(line); val.push('\n'); }
        }
        if !key.is_empty() { fields.insert(key, val.trim().to_string()); }
        let name = fields.get("NAME").cloned().unwrap_or_default();
        let version = fields.get("VERSION").cloned().unwrap_or_default();
        let fname = fields.get("FILENAME").cloned().unwrap_or_default();
        if !name.is_empty() && !fname.is_empty() {
            let dl_url = format!("{}/{}", base.trim_end_matches('/'), fname);
            results.push((name, version, dl_url));
        }
    }
    results
}

async fn fetch_apk_index(client: &Client, repo: &ApkRepo) -> Vec<(String, String, String)> {
    let arch = pacman_arch();
    let url = format!("{}/{}/APKINDEX.tar.gz", repo.uri.trim_end_matches('/'), arch);
    let data = match fetch_gz(client, &url).await { Ok(d) => d, Err(_) => return Vec::new() };

    let mut archive = tar::Archive::new(&data[..]);
    let mut text = String::new();
    for mut entry in archive.entries().unwrap().filter_map(|e| e.ok()) {
        let path = entry.path().unwrap().to_string_lossy().to_string();
        if path == "APKINDEX" {
            let mut raw = Vec::new();
            let _ = Read::read_to_end(&mut entry, &mut raw);
            text = String::from_utf8_lossy(&raw).to_string();
            break;
        }
    }

    let mut results = Vec::new();
    for record in text.split("\n\n") {
        let (mut name, mut version) = (String::new(), String::new());
        for line in record.lines() {
            if let Some(v) = line.strip_prefix("P:") { name = v.trim().to_string(); }
            else if let Some(v) = line.strip_prefix("V:") { version = v.trim().to_string(); }
        }
        if !name.is_empty() && !version.is_empty() {
            results.push((name.clone(), version.clone(),
                format!("{}/{}/{}-{}.apk", repo.uri.trim_end_matches('/'), arch, name, version)));
        }
    }
    results
}

// ── Search + download ───────────────────────────────────────────────────────

fn animate(pb: &ProgressBar) -> tokio::task::JoinHandle<()> {
    let pb = pb.clone();
    tokio::spawn(async move {
        loop {
            tokio::time::sleep(Duration::from_millis(30)).await;
            pb.tick();
        }
    })
}

fn smooth_animate(pb: &ProgressBar, steps: u64) -> tokio::task::JoinHandle<()> {
    let pb = pb.clone();
    tokio::spawn(async move {
        for _ in 0..steps {
            tokio::time::sleep(Duration::from_millis(30)).await;
            pb.inc(1);
        }
        loop {
            tokio::time::sleep(Duration::from_millis(30)).await;
            pb.tick();
        }
    })
}

async fn search_system_repos(client: &Client, name: &str) -> Vec<FoundPkg> {
    let mp = MultiProgress::new();
    let bar_sty = ProgressStyle::default_bar()
        .template("[{bar:20}] {spinner:.green} {msg}")
        .unwrap()
        .progress_chars("##-");

    let name = Arc::new(name.to_string());
    let client = Arc::new(client.clone());
    let done = Arc::new(AtomicBool::new(false));

    // ── Read all repo configs & create bars upfront ──

    let apt_repos = read_apt_sources();
    let mut apt_work: Vec<(ProgressBar, AptRepo)> = Vec::new();
    for repo in apt_repos {
        if repo.components.is_empty() { continue; }
        let pb = mp.add(ProgressBar::new(repo.components.len() as u64));
        pb.set_style(bar_sty.clone());
        pb.set_message(format!("apt: {}/{}", repo.uri, repo.suite));
        apt_work.push((pb, repo));
    }
    let apt_work = Arc::new(apt_work);

    let mut pacman_sections: HashMap<String, Vec<PacmanRepo>> = HashMap::new();
    for repo in read_pacman_repos() {
        pacman_sections.entry(repo.name.clone()).or_default().push(repo);
    }
    let mut pacman_work: Vec<(ProgressBar, String, Vec<PacmanRepo>)> = Vec::new();
    for (section, mirrors) in pacman_sections {
        let pb = mp.add(ProgressBar::new(mirrors.len() as u64));
        pb.set_style(bar_sty.clone());
        pb.set_message(format!("pacman: {}", section));
        pacman_work.push((pb, section, mirrors));
    }
    let pacman_work = Arc::new(pacman_work);

    let mut apk_work: Vec<(ProgressBar, ApkRepo)> = Vec::new();
    for repo in read_apk_repos() {
        let pb = mp.add(ProgressBar::new(100));
        pb.set_style(bar_sty.clone());
        pb.set_message(format!("apk: {}", repo.uri));
        apk_work.push((pb, repo));
    }
    let apk_work = Arc::new(apk_work);

    // ── Spawn concurrent search tasks ──

    let apt_task = tokio::spawn({
        let name = (*name).clone();
        let client = Client::clone(&client);
        let done = Arc::clone(&done);
        let apt_work = Arc::clone(&apt_work);
        async move {
            let mut found = Vec::new();
            for (pb, repo) in &*apt_work {
                if done.load(Ordering::Relaxed) { break; }
                let anim = animate(pb);
                for (pkg, ver, url) in &fetch_apt_index(&client, repo, pb).await {
                    if pkg == &name {
                        found.push(FoundPkg {
                            name: pkg.clone(), version: ver.clone(),
                            repo: format!("apt:{}", repo.uri), download_url: url.clone(),
                        });
                        done.store(true, Ordering::Relaxed);
                    }
                }
                anim.abort();
                pb.finish_with_message(format!(
                    "✓ {}  apt: {}/{}",
                    if found.iter().any(|f| f.repo.starts_with("apt:")) { "●" } else { "○" },
                    repo.uri, repo.suite,
                ));
            }
            found
        }
    });

    let pacman_task = tokio::spawn({
        let name = (*name).clone();
        let client = Client::clone(&client);
        let done = Arc::clone(&done);
        let pacman_work = Arc::clone(&pacman_work);
        async move {
            let mut found = Vec::new();
            for (pb, section, mirrors) in &*pacman_work {
                if done.load(Ordering::Relaxed) { break; }
                let anim = animate(pb);
                let mut section_found = false;
                'outer: for chunk in mirrors.chunks(10) {
                    if done.load(Ordering::Relaxed) { break; }
                    let tasks: Vec<_> = chunk.iter().map(|r| {
                        let c = client.clone();
                        let n = r.name.clone();
                        let s = r.server.clone();
                        tokio::spawn(async move {
                            fetch_pacman_index(&c, &PacmanRepo { name: n, server: s }).await
                        })
                    }).collect();
                    for task in tasks {
                        if done.load(Ordering::Relaxed) { break 'outer; }
                        pb.inc(1);
                        if let Ok(idx) = task.await {
                            if !idx.is_empty() {
                                for (pkg, ver, url) in &idx {
                                    if pkg == &name {
                                        found.push(FoundPkg {
                                            name: pkg.clone(), version: ver.clone(),
                                            repo: format!("pacman:{}", section),
                                            download_url: url.clone(),
                                        });
                                        section_found = true;
                                        done.store(true, Ordering::Relaxed);
                                    }
                                }
                                break 'outer;
                            }
                        }
                    }
                }
                anim.abort();
                if section_found {
                    pb.finish_with_message(format!("✓ ● found in pacman: {}", section));
                } else {
                    pb.finish_with_message(format!("✓ ○ pacman: {} (no match)", section));
                }
            }
            found
        }
    });

    let apk_task = tokio::spawn({
        let name = (*name).clone();
        let client = Client::clone(&client);
        let done = Arc::clone(&done);
        let apk_work = Arc::clone(&apk_work);
        async move {
            let mut found = Vec::new();
            for (pb, repo) in &*apk_work {
                if done.load(Ordering::Relaxed) { break; }
                let anim = smooth_animate(pb, 100);
                for (pkg, ver, url) in &fetch_apk_index(&client, repo).await {
                    if pkg == &name {
                        found.push(FoundPkg {
                            name: pkg.clone(), version: ver.clone(),
                            repo: format!("apk:{}", repo.uri), download_url: url.clone(),
                        });
                        done.store(true, Ordering::Relaxed);
                    }
                }
                anim.abort();
                pb.finish_with_message(format!(
                    "✓ {}  apk: {}",
                    if found.iter().any(|f| f.repo.starts_with("apk:")) { "●" } else { "○" },
                    repo.uri,
                ));
            }
            found
        }
    });

    let (apt, pacman, apk) = tokio::join!(apt_task, pacman_task, apk_task);
    let mut found = apt.unwrap_or_default();
    found.extend(pacman.unwrap_or_default());
    found.extend(apk.unwrap_or_default());
    found
}

async fn download_pkg(client: &Client, pkg: &FoundPkg) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let dl_sty = ProgressStyle::default_bar()
        .template("[{bar:20}] {bytes}/{total_bytes} {msg}")
        .unwrap()
        .progress_chars("##-");
    let sp_sty = ProgressStyle::default_spinner()
        .template("{spinner:.green} {msg}")
        .unwrap();

    let response = client.get(&pkg.download_url).header("user-agent", "fetcher").send().await?;
    if !response.status().is_success() {
        return Err(format!("HTTP {} downloading {}", response.status(), pkg.download_url).into());
    }

    let total = response.content_length().unwrap_or(0);
    let pb = ProgressBar::new(total);
    pb.set_style(dl_sty);
    pb.set_message(format!("downloading {}", pkg.name));

    let mut bytes = Vec::new();
    let mut stream = response.bytes_stream();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk?;
        bytes.extend_from_slice(&chunk);
        pb.inc(chunk.len() as u64);
    }

    pb.finish_with_message(format!("✓ downloaded {}", pkg.name));
    fs::create_dir_all(".fcache")?;
    let ext = pkg.download_url.rsplit_once('.').map(|(_, e)| e).unwrap_or("pkg");
    let path = format!(".fcache/{}-{}.{}", pkg.name, pkg.version, ext);
    fs::write(&path, &bytes)?;

    let dest = format!(".fcache/root");
    let epb = ProgressBar::new_spinner();
    epb.set_style(sp_sty);
    epb.set_message(format!("extracting {} to {}/", pkg.name, dest));
    match extract_pkg(&path, &dest, &pkg.name, &pkg.version) {
        Ok(_) => epb.finish_with_message(format!("✓ extracted {} -> {}/", pkg.name, dest)),
        Err(e) => {
            epb.finish_with_message(format!("✗ extraction failed: {}", e));
            eprintln!("extraction failed for {}: {}", path, e);
        }
    }
    Ok(())
}

fn extract_pkg(path: &str, dest: &str, _name: &str, _version: &str) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let data = fs::read(path)?;
    if path.ends_with(".pkg.tar.zst") || path.ends_with(".zst") {
        let tar_bytes = zstd::stream::decode_all(&data[..])?;
        let root = Path::new(dest);
        fs::create_dir_all(root)?;
        let mut archive = tar::Archive::new(&tar_bytes[..]);
        archive.unpack(root)?;
    } else if path.ends_with(".apk") {
        let mut tar_bytes = Vec::new();
        GzDecoder::new(&data[..]).read_to_end(&mut tar_bytes)?;
        let root = Path::new(dest);
        fs::create_dir_all(root)?;
        let mut archive = tar::Archive::new(&tar_bytes[..]);
        archive.unpack(root)?;
    } else if path.ends_with(".deb") {
        extract_deb(&data, Path::new(dest))?;
    } else {
        return Err(format!("unknown package format: {}", path).into());
    }
    Ok(())
}

fn extract_deb(data: &[u8], dest: &Path) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let mut pos = 0;
    let magic = b"!<arch>\n";
    if data.get(..8) != Some(magic) {
        return Err("not a valid ar archive".into());
    }
    pos += 8;

    while pos + 60 <= data.len() {
        let header = &data[pos..pos+60];
        pos += 60;
        let name = String::from_utf8_lossy(&header[..16]).trim().to_string();
        let size_str = String::from_utf8_lossy(&header[48..58]).trim().to_string();
        let size: usize = size_str.parse().unwrap_or(0);
        let padded = size + (size % 2);
        if pos + padded > data.len() { break; }
        let content = &data[pos..pos+size];
        pos += padded;

        if name.starts_with("data.tar.") {
            if name.ends_with(".xz") {
                let mut tar_bytes = Vec::new();
                let mut decoder = xz2::read::XzDecoder::new(content);
                decoder.read_to_end(&mut tar_bytes)?;
                fs::create_dir_all(dest)?;
                let mut archive = tar::Archive::new(&tar_bytes[..]);
                archive.unpack(dest)?;
                return Ok(());
            } else if name.ends_with(".gz") {
                let mut tar_bytes = Vec::new();
                GzDecoder::new(content).read_to_end(&mut tar_bytes)?;
                fs::create_dir_all(dest)?;
                let mut archive = tar::Archive::new(&tar_bytes[..]);
                archive.unpack(dest)?;
                return Ok(());
            }
        }
    }
    Err("no data.tar found in .deb".into())
}

// ── Manifest loading ────────────────────────────────────────────────────────

fn load_manifest() -> Result<Manifest, Box<dyn std::error::Error>> {
    let content = fs::read_to_string("package.toml")?;
    let manifest: Manifest = toml::from_str(&content)?;
    Ok(manifest)
}

// ── Remote source fetching (GitHub / URL / Git) ─────────────────────────────

fn build_url(
    source_override: Option<&SourceOverride>,
    manifest_source: &Option<Source>,
    name: &str,
    version: &str,
) -> Result<String, Box<dyn std::error::Error + Send + Sync>> {
    if let Some(so) = source_override {
        match so {
            SourceOverride::Github(repo) => {
                let (org, repo_name) = repo.split_once('/')
                    .ok_or_else(|| format!("invalid github 'owner/repo': {repo}"))?;
                return Ok(format!("https://github.com/{org}/{repo_name}/archive/refs/tags/v{version}.zip"));
            }
            SourceOverride::Url(u) => return Ok(u.replace("{name}", name).replace("{version}", version)),
            SourceOverride::Git(_, _) => return Ok(String::new()),
        }
    }
    if let Some(src) = manifest_source {
        if let Some(ref github) = src.github {
            return Ok(format!("https://github.com/{github}/{name}/archive/refs/tags/v{version}.zip"));
        }
        if let Some(ref registry) = src.registry {
            return Ok(registry.replace("{name}", name).replace("{version}", version));
        }
        if let Some(ref url) = src.url {
            return Ok(url.replace("{name}", name).replace("{version}", version));
        }
    }
    if let Ok(org) = std::env::var("FETCHER_GITHUB_ORG") {
        return Ok(format!("https://github.com/{org}/{name}/archive/refs/tags/v{version}.zip"));
    }
    Err("no source configured and no FETCHER_GITHUB_ORG set".into())
}

fn is_git_source(so: Option<&SourceOverride>) -> bool {
    matches!(so, Some(SourceOverride::Git(_, _)))
}

async fn fetch_pkg(client: &Client, name: &str, version: &str, url: &str) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let dl_sty = ProgressStyle::default_bar()
        .template("[{bar:20}] {bytes}/{total_bytes} {msg}")
        .unwrap()
        .progress_chars("##-");

    let response = client.get(url).header("user-agent", "fetcher").send().await?;
    if !response.status().is_success() {
        return Err(format!("failed to fetch {}@{} ({}): HTTP {}", name, version, url, response.status()).into());
    }

    let total = response.content_length().unwrap_or(0);
    let pb = ProgressBar::new(total);
    pb.set_style(dl_sty);
    pb.set_message(format!("downloading {}@{}", name, version));

    let mut bytes = Vec::new();
    let mut stream = response.bytes_stream();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk?;
        bytes.extend_from_slice(&chunk);
        pb.inc(chunk.len() as u64);
    }
    pb.finish_with_message(format!("✓ downloaded {}@{}", name, version));

    fs::create_dir_all(".fcache")?;
    let path = format!(".fcache/{}-{}.zip", name, version);
    fs::write(&path, &bytes)?;
    Ok(())
}

async fn git_clone(name: &str, version: &str, git_url: &str, rev: Option<&str>) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let cache_dir = Path::new(".fcache").join(format!("{name}-{version}"));
    if cache_dir.exists() { fs::remove_dir_all(&cache_dir)?; }
    let mut cmd = Command::new("git");
    cmd.arg("clone");
    if let Some(r) = rev { cmd.arg("--branch").arg(r); }
    cmd.arg(git_url).arg(&cache_dir);
    if !cmd.status()?.success() { return Err(format!("git clone failed for {name}").into()); }
    println!("cloned {}@{} from git -> {:?}", name, version, cache_dir);
    Ok(())
}

// ── Main ────────────────────────────────────────────────────────────────────

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let cli = Cli::parse();

    match &cli.command {
        Commands::Fetch { pkg, github, url, git, rev } => {
            let client = Client::builder()
                .user_agent("fetcher")
                .connect_timeout(std::time::Duration::from_secs(3))
                .timeout(std::time::Duration::from_secs(10))
                .build()?;

            match pkg {
                Some(pkg_str) => {
                    let (name, version) = if let Some(at) = pkg_str.find('@') {
                        (&pkg_str[..at], &pkg_str[at+1..])
                    } else { (pkg_str.as_str(), "latest") };

                    let explicit = github.is_some() || url.is_some() || git.is_some();
                    let so = if let Some(r) = github { Some(SourceOverride::Github(r.clone())) }
                        else if let Some(u) = url { Some(SourceOverride::Url(u.clone())) }
                        else if let Some(g) = git { Some(SourceOverride::Git(g.clone(), rev.clone())) }
                        else { None };

                    if explicit {
                        if is_git_source(so.as_ref()) {
                            let (gu, r) = match so.as_ref().unwrap() {
                                SourceOverride::Git(u, r) => (u.as_str(), r.as_deref()),
                                _ => unreachable!(),
                            };
                            if let Err(e) = git_clone(name, version, gu, r).await { eprintln!("error: {e}"); }
                        } else if let Ok(url) = build_url(so.as_ref(), &None, name, version) {
                            if let Err(e) = fetch_pkg(&client, name, version, &url).await { eprintln!("error: {e}"); }
                        }
                    } else {
                        let results = search_system_repos(&client, name).await;
                        if results.is_empty() {
                            match build_url(None, &None, name, version) {
                                Ok(u) => { if let Err(e) = fetch_pkg(&client, name, version, &u).await { eprintln!("error: {e}"); } }
                                Err(_) => { eprintln!("could not find '{}' in any system repo", name); }
                            }
                        } else {
                            if let Err(e) = download_pkg(&client, &results[0]).await { eprintln!("error: {e}"); }
                        }
                    }
                }
                None => {
                    let manifest = load_manifest()?;
                    let mut tasks = Vec::new();
                    for (dep_name, dep_spec) in &manifest.deps {
                        let name = dep_name.clone();
                        let version = dep_spec.version().unwrap_or("latest").to_string();
                        let so = dep_spec.source_override();
                        if is_git_source(so.as_ref()) {
                            let (gu, r) = match so.as_ref().unwrap() {
                                SourceOverride::Git(u, r) => (u.clone(), r.clone()),
                                _ => unreachable!(),
                            };
                            tasks.push(tokio::spawn(async move { git_clone(&name, &version, &gu, r.as_deref()).await }));
                        } else if let Ok(url) = build_url(so.as_ref(), &manifest.source, &name, &version) {
                            let cl = client.clone();
                            tasks.push(tokio::spawn(async move { fetch_pkg(&cl, &name, &version, &url).await }));
                        }
                    }
                    for r in futures::future::join_all(tasks).await {
                        if let Err(e) = r { eprintln!("error: {e}"); }
                    }
                }
            }
        }
    }
    Ok(())
}
