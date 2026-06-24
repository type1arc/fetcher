use crate::index::{fetch_apt_index, fetch_pacman_index, fetch_apk_index};
use crate::repo::{AptRepo, PacmanRepo, ApkRepo, FoundPkg, read_apt_sources, read_pacman_repos, read_apk_repos};
use indicatif::{MultiProgress, ProgressBar, ProgressStyle};
use reqwest::Client;
use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

fn animate(pb: &ProgressBar) -> tokio::task::JoinHandle<()> {
    let pb = pb.clone();
    tokio::spawn(async move {
        loop {
            tokio::time::sleep(Duration::from_millis(30)).await;
            pb.tick();
        }
    })
}

pub async fn search_system_repos(client: &Client, name: &str) -> Vec<FoundPkg> {
    println!("searching for {} in system repos...", name);
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
        let pb = mp.add(ProgressBar::new(1));
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
                let anim = animate(pb);
                for (pkg, ver, url) in &fetch_apk_index(&client, repo, pb).await {
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
