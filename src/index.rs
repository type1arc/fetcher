use crate::arch::{native_arch, pacman_arch};
use crate::repo::{AptRepo, PacmanRepo, ApkRepo};
use flate2::read::GzDecoder;
use indicatif::ProgressBar;
use reqwest::Client;
use std::collections::HashMap;
use std::io::Read;

pub async fn fetch_gz(client: &Client, url: &str) -> Result<Vec<u8>, Box<dyn std::error::Error + Send + Sync>> {
    let compressed = client.get(url).send().await?.bytes().await?;
    let mut out = Vec::new();
    match GzDecoder::new(&compressed[..]).read_to_end(&mut out) {
        Ok(_) => Ok(out),
        Err(_) => Ok(compressed.to_vec()),
    }
}

pub async fn fetch_apt_index(client: &Client, repo: &AptRepo, pb: &ProgressBar) -> Vec<(String, String, String)> {
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

pub async fn fetch_pacman_index(client: &Client, repo: &PacmanRepo) -> Vec<(String, String, String)> {
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

pub async fn fetch_apk_index(client: &Client, repo: &ApkRepo, pb: &ProgressBar) -> Vec<(String, String, String)> {
    let arch = pacman_arch();
    let url = format!("{}/{}/APKINDEX.tar.gz", repo.uri.trim_end_matches('/'), arch);
    pb.set_message(format!("checking apk: {}", repo.uri));
    let data = match fetch_gz(client, &url).await { Ok(d) => d, Err(_) => { pb.inc(1); return Vec::new() } };

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
    pb.inc(1);
    results
}
