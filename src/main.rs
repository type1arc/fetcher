mod arch;
mod manifest;
mod repo;
mod index;
mod search;
mod download;
mod extract;
mod postinstall;

use clap::{Parser, Subcommand};
use manifest::{DepSpec, LockedDep, Lockfile, SourceOverride};
use reqwest::Client;
use std::collections::{HashMap, HashSet};
use std::time::Duration;

#[derive(Parser)]
#[command(name = "fetcher", version = "0.4.0", about = "A configurable package fetcher")]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Install a package or all dependencies from package.toml
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
        /// Build from source using this shell command (auto-detects if empty)
        #[arg(long)]
        build: Option<String>,
        /// Build from source instead of downloading binaries (auto-detects build system)
        #[arg(long)]
        from_source: bool,
    },
    /// Download source and build it (auto-detects build system if --build omitted)
    Source {
        /// GitHub repo (owner/repo) or URL
        src: String,
        #[arg(long)]
        version: Option<String>,
        /// Shell command to build and install (INSTALL_DIR env var available). Auto-detects if omitted.
        #[arg(long)]
        build: Option<String>,
    },
    /// Remove cached archives
    Clean,
    /// Unfetch (remove) an installed package
    Unfetch {
        /// Package name to unfetch
        pkg: Option<String>,
    },
}

fn home_bin() -> String {
    let home = std::env::var("HOME").unwrap_or_else(|_| ".fetcher".to_string());
    format!("{}/.local/bin", home)
}

fn build_client() -> Result<Client, Box<dyn std::error::Error>> {
    Ok(Client::builder()
        .user_agent("fetcher")
        .connect_timeout(Duration::from_secs(10))
        .timeout(Duration::from_secs(120))
        .build()?)
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let cli = Cli::parse();

    match &cli.command {
        Commands::Clean => {
            download::clean_cache().unwrap_or_else(|e| eprintln!("error: {e}"));
            return Ok(());
        }
        Commands::Unfetch { pkg } => {
            let client = build_client()?;
            let bin = home_bin();

            match pkg {
                Some(name) => {
                    let mut lockfile = manifest::read_lockfile().unwrap_or(Lockfile {
                        version: 1,
                        deps: HashMap::new(),
                    });
                    if lockfile.deps.contains_key(name) {
                        match download::uninstall_pkg(&client, name, &bin, &mut lockfile).await {
                            Ok(n) => {
                                println!("✓ removed {} file(s) for {}", n, name);
                                if let Err(e) = manifest::write_lockfile(&lockfile) {
                                    eprintln!("error writing lockfile: {e}");
                                }
                            }
                            Err(e) => eprintln!("error: {e}"),
                        }
                    } else {
                        match download::uninstall_pkg_by_name(name, &bin) {
                            Ok(n) => {
                                if n == 0 {
                                    eprintln!("no files found for '{}'", name);
                                } else {
                                    println!("✓ removed {} file(s) for {}", n, name);
                                }
                            }
                            Err(e) => eprintln!("error: {e}"),
                        }
                    }
                }
                None => {
                    let manifest = match manifest::load_manifest() {
                        Ok(m) => m,
                        Err(e) => { eprintln!("error: {e}"); return Ok(()); }
                    };
                    let mut lockfile = manifest::read_lockfile().unwrap_or(Lockfile {
                        version: 1,
                        deps: HashMap::new(),
                    });
                    let mut total: u32 = 0;
                    for name in manifest.deps.keys() {
                        if lockfile.deps.contains_key(name) {
                            match download::uninstall_pkg(&client, name, &bin, &mut lockfile).await {
                                Ok(n) => { total += n; }
                                Err(e) => eprintln!("error unfetching {}: {}", name, e),
                            }
                        } else {
                            match download::uninstall_pkg_by_name(name, &bin) {
                                Ok(n) => { total += n; }
                                Err(e) => eprintln!("error unfetching {}: {}", name, e),
                            }
                        }
                    }
                    if let Err(e) = manifest::write_lockfile(&lockfile) {
                        eprintln!("error writing lockfile: {e}");
                    }
                    println!("✓ removed {} file(s) total", total);
                }
            }
            return Ok(());
        }
        Commands::Source { src, version, build } => {
            let client = build_client()?;
            let version = version.clone().unwrap_or_else(|| "latest".to_string());
            // determine if it's a github repo or a URL
            let url = if src.contains("://") {
                src.replace("{version}", &version)
            } else if src.contains('/') {
                let (org, repo) = src.split_once('/').unwrap();
                if version == "latest" {
                    format!("https://github.com/{org}/{repo}/archive/HEAD.zip")
                } else {
                    format!("https://github.com/{org}/{repo}/archive/refs/tags/v{version}.zip")
                }
            } else {
                eprintln!("source must be a URL or GitHub 'owner/repo'");
                return Ok(());
            };
            let name = src.split('/').last().unwrap_or(&src).to_string();
            let build_cmd = build.as_deref().unwrap_or("");
            if let Err(e) = download::build_from_source(&client, &name, &version, &url, build_cmd).await {
                eprintln!("error: {e}");
            }
            postinstall::check_path();
            return Ok(());
        }
        Commands::Fetch { .. } => {}
    }

    let Commands::Fetch { pkg, github, url, git, rev, build, from_source } = &cli.command else { unreachable!() };

    let client = build_client()?;

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

            // source build (explicit command or auto-detect)
            if let Some(build_cmd) = build {
                if let Ok(url) = manifest::build_url(so.as_ref(), &None, name, version) {
                    if let Err(e) = download::build_from_source(&client, name, version, &url, build_cmd).await {
                        eprintln!("error: {e}");
                    }
                }
            } else if *from_source {
                let url = manifest::build_url(so.as_ref(), &None, name, version);
                match url {
                    Ok(u) => {
                        if let Err(e) = download::build_from_source(&client, name, version, &u, "").await {
                            eprintln!("error: {e}");
                        }
                    }
                    Err(_) => {
                        eprintln!("could not determine source URL for '{}'", name);
                        eprintln!("  provide --github <owner/repo> or --url <url>");
                        if github.is_none() && git.is_none() {
                            eprintln!("  example: fetcher fetch {} --from-source --github owner/{}", name, name);
                        }
                    }
                }
            } else if explicit {
                if manifest::is_git_source(so.as_ref()) {
                    let (gu, r) = match so.as_ref().unwrap() {
                        SourceOverride::Git(u, r) => (u.as_str(), r.as_deref()),
                        _ => unreachable!(),
                    };
                    if let Err(e) = download::git_clone(name, version, gu, r).await {
                        eprintln!("error: {e}");
                    }
                } else if let Ok(url) = manifest::build_url(so.as_ref(), &None, name, version) {
                    if let Err(e) = download::fetch_pkg(&client, name, version, &url, None).await {
                        eprintln!("error: {e}");
                    }
                }
            } else {
                let results = search::search_system_repos(&client, name).await;
                if !results.is_empty() {
                    let unique_repos: HashSet<&str> = results.iter().map(|r| r.repo.as_str()).collect();
                    println!("found {} package(s) in {} repo(s)", results.len(), unique_repos.len());
                    if let Err(e) = download::download_pkg(&client, &results[0], None).await {
                        eprintln!("error: {e}");
                    }
                } else {
                    match manifest::build_url(None, &None, name, version) {
                        Ok(u) => {
                            if let Err(e) = download::fetch_pkg(&client, name, version, &u, None).await {
                                eprintln!("error: {e}");
                            }
                        }
                        Err(_) => {
                            eprintln!("could not find '{}' in any system repo", name);
                        }
                    }
                }
            }
            postinstall::check_path();
        }
        None => {
            let manifest = manifest::load_manifest()?;

            let mut lockfile = manifest::read_lockfile().unwrap_or(Lockfile {
                version: 1,
                deps: HashMap::new(),
            });

            let mut installed: HashSet<String> = HashSet::new();

            if let Some(script) = &manifest.pre_install {
                if let Err(e) = postinstall::run_script(script, &manifest.pkg_name.name, &home_bin()) {
                    eprintln!("error: pre-install script: {e}");
                }
            }

            install_deps(
                &manifest.deps,
                &manifest.source,
                &client,
                &mut lockfile,
                &mut installed,
                *from_source,
            ).await;

            if let Err(e) = manifest::write_lockfile(&lockfile) {
                eprintln!("error writing lockfile: {e}");
            }

            if let Some(script) = &manifest.post_install {
                if let Err(e) = postinstall::run_script(script, &manifest.pkg_name.name, &home_bin()) {
                    eprintln!("error: post-install script: {e}");
                }
            }

            postinstall::check_path();
        }
    }

    Ok(())
}

async fn install_deps(
    deps: &HashMap<String, DepSpec>,
    manifest_source: &Option<manifest::Source>,
    client: &Client,
    lockfile: &mut Lockfile,
    installed: &mut HashSet<String>,
    from_source: bool,
) {
    for (dep_name, dep_spec) in deps {
        if installed.contains(dep_name) { continue; }

        let version = dep_spec.version().unwrap_or("latest").to_string();
        let so = dep_spec.source_override();

        // run dep-level pre_install
        if let Some(script) = dep_spec.pre_install() {
            let _ = postinstall::run_script(script, dep_name, &home_bin());
        }

        // build from source if requested (explicit build cmd, --from-source, or dep has build field)
        let build_requested = dep_spec.build().is_some() || from_source;
        if build_requested {
            let build_cmd = dep_spec.build().unwrap_or("");
            if let Ok(url) = manifest::build_url(so.as_ref(), manifest_source, dep_name, &version) {
                if let Err(e) = download::build_from_source(client, dep_name, &version, &url, build_cmd).await {
                    eprintln!("✗ error building {}: {}", dep_name, e);
                }
                installed.insert(dep_name.clone());
                continue;
            } else {
                eprintln!("could not determine source URL for '{}'", dep_name);
                eprintln!("  add github = \"<owner/{0}>\" or url = \"<url>\" to the dep in package.toml", dep_name);
                continue;
            }
        }

        let expected_sha256 = dep_spec.sha256();

        // check lockfile first
        let result = if let Some(pinned) = lockfile.deps.get(dep_name) {
            println!("→ {}@{} found in lockfile", dep_name, pinned.version);
            let fp = repo::FoundPkg {
                name: dep_name.clone(),
                version: pinned.version.clone(),
                repo: pinned.repo.clone().unwrap_or_default(),
                download_url: pinned.download_url.clone(),
            };
            download::download_pkg(client, &fp, pinned.sha256.as_deref()).await.map(|_| ())
        } else if manifest::is_git_source(so.as_ref()) {
            let (gu, r) = match so.as_ref().unwrap() {
                SourceOverride::Git(u, r) => (u.clone(), r.clone()),
                _ => unreachable!(),
            };
            download::git_clone(dep_name, &version, &gu, r.as_deref()).await.map(|_| ())
        } else if let Ok(url) = manifest::build_url(so.as_ref(), manifest_source, dep_name, &version) {
            // remote URL — download directly with checksum
            download::fetch_pkg(client, dep_name, &version, &url, expected_sha256).await.map(|_| ())
        } else {
            // search system repos
            let results = search::search_system_repos(client, dep_name).await;
            if results.is_empty() {
                eprintln!("could not find '{}' in any system repo", dep_name);
                continue;
            }
            let r = &results[0];
            let dl_result = download::download_pkg(client, r, expected_sha256).await;
            if let Ok(_) = &dl_result {
                let cache = format!(".fetcher/{}-{}.{}", dep_name, r.version,
                    r.download_url.rsplit_once('.').map(|(_, e)| e).unwrap_or("pkg"));
                let sha256 = postinstall::compute_sha256(&cache).ok();
                lockfile.deps.insert(dep_name.clone(), LockedDep {
                    version: r.version.clone(),
                    download_url: r.download_url.clone(),
                    repo: Some(r.repo.clone()),
                    sha256,
                });
            }
            dl_result.map(|_| ())
        };

        if let Err(e) = result {
            eprintln!("✗ error installing {}: {}", dep_name, e);
            continue;
        }

        installed.insert(dep_name.clone());

        // run dep-level post_install
        if let Some(script) = dep_spec.post_install() {
            let _ = postinstall::run_script(script, dep_name, &home_bin());
        }

        // check for nested package.toml in install dir
        let nested = format!("{}/package.toml", home_bin());
        if let Ok(content) = std::fs::read_to_string(&nested) {
            if let Ok(nested_manifest) = toml::from_str::<manifest::Manifest>(&content) {
                println!("found nested package.toml in {}, installing sub-deps...", dep_name);
                let _ = std::fs::remove_file(&nested);
                Box::pin(install_deps(
                    &nested_manifest.deps,
                    &nested_manifest.source,
                    client,
                    lockfile,
                    installed,
                    from_source,
                )).await;
            }
        }
    }
}
