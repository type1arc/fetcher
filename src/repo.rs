use std::fs;

pub struct AptRepo {
    pub uri: String,
    pub suite: String,
    pub components: Vec<String>,
}

pub struct PacmanRepo {
    pub name: String,
    pub server: String,
}

pub struct ApkRepo {
    pub uri: String,
}

#[derive(Debug)]
pub struct FoundPkg {
    pub name: String,
    pub version: String,
    pub repo: String,
    pub download_url: String,
}

pub fn read_apt_sources() -> Vec<AptRepo> {
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

pub fn read_pacman_repos() -> Vec<PacmanRepo> {
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
        if let Some(val) = line.strip_prefix("Server = ") {
            let s = val.trim().to_string();
            if !section.is_empty() && !s.is_empty() {
                repos.push(PacmanRepo { name: section.clone(), server: s });
            }
        }
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

pub fn read_apk_repos() -> Vec<ApkRepo> {
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
