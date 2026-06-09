//! `deadeye update` — check for a newer release and update in place.
//!
//! Queries the GitHub Releases API for the latest tag, compares it to the
//! running binary's version, and (unless `--check`) re-runs the installer to
//! pull the newest binary + refresh the agent skills. Updating a running
//! binary is safe on Unix — the OS keeps the old inode until the process exits.

use std::process::Command;

use anyhow::{Context as _, Result, bail};

/// Branded installer endpoint (same one the webapp serves).
const DEFAULT_INSTALL_URL: &str = "https://project-deadeye.vercel.app/install.sh";
/// GitHub Releases API for the latest tag.
const LATEST_RELEASE_API: &str =
    "https://api.github.com/repos/teddyjfpender/deadeye-rs/releases/latest";

pub(crate) async fn run(args: crate::cli::UpdateArgs, confirm: bool) -> Result<()> {
    let current = env!("CARGO_PKG_VERSION");
    println!("Installed: v{current}");

    let latest = latest_release_tag();
    match &latest {
        Some(tag) => {
            let newer = is_newer(normalize(tag), current);
            println!("Latest:    {tag}");
            if newer {
                println!("A newer version is available.");
            } else {
                println!("You are up to date.");
                if args.check {
                    return Ok(());
                }
            }
        },
        None => {
            println!("Latest:    (could not reach GitHub Releases — will reinstall latest)");
        },
    }

    if args.check {
        return Ok(());
    }

    let url = args.url.as_deref().unwrap_or(DEFAULT_INSTALL_URL);
    if !confirm {
        super::confirm_or_bail(&format!("Update now by running `{url}`?"))?;
    }
    run_installer(url)
}

/// Re-run the installer (`curl … | sh`) to update the binary + skills.
fn run_installer(url: &str) -> Result<()> {
    if Command::new("sh")
        .arg("-c")
        .arg("command -v curl")
        .output()
        .map(|o| !o.status.success())
        .unwrap_or(true)
    {
        bail!("`curl` is required to update; install it (or re-run the install command manually).");
    }
    println!("Updating via {url} …");
    let status = Command::new("sh")
        .arg("-c")
        .arg(format!("curl -fsSL {url} | sh"))
        .status()
        .context("running the installer")?;
    if !status.success() {
        bail!("installer exited with a non-zero status");
    }
    println!("Done. Restart your agent app to pick up refreshed skills.");
    Ok(())
}

/// Fetch the latest release tag via curl + the GitHub API. Best-effort.
fn latest_release_tag() -> Option<String> {
    let out = Command::new("curl")
        .args([
            "-fsSL",
            "-H",
            "Accept: application/vnd.github+json",
            "-H",
            "User-Agent: deadeye-cli",
            LATEST_RELEASE_API,
        ])
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let body = String::from_utf8_lossy(&out.stdout);
    // Avoid a JSON dep: pull "tag_name":"vX.Y.Z" out of the response.
    let key = "\"tag_name\"";
    let i = body.find(key)?;
    let rest = &body[i + key.len()..];
    let start = rest.find('"')? + 1;
    let end = rest[start..].find('"')? + start;
    Some(rest[start..end].to_owned())
}

/// Strip a leading `v` from a tag.
fn normalize(tag: &str) -> &str {
    tag.strip_prefix('v').unwrap_or(tag)
}

/// Compare dotted numeric versions: is `latest` strictly newer than `current`?
fn is_newer(latest: &str, current: &str) -> bool {
    let parse = |s: &str| -> Vec<u64> {
        s.split(['.', '-', '+'])
            .map(|p| p.parse::<u64>().unwrap_or(0))
            .collect()
    };
    let (l, c) = (parse(latest), parse(current));
    for i in 0..l.len().max(c.len()) {
        let lv = l.get(i).copied().unwrap_or(0);
        let cv = c.get(i).copied().unwrap_or(0);
        if lv != cv {
            return lv > cv;
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn version_comparison() {
        assert!(is_newer("0.1.6", "0.1.5"));
        assert!(is_newer("0.2.0", "0.1.9"));
        assert!(is_newer("1.0.0", "0.9.9"));
        assert!(!is_newer("0.1.5", "0.1.5"));
        assert!(!is_newer("0.1.4", "0.1.5"));
    }

    #[test]
    fn normalize_strips_v() {
        assert_eq!(normalize("v0.1.6"), "0.1.6");
        assert_eq!(normalize("0.1.6"), "0.1.6");
    }
}
