//! `deadeye feedback` — post a structured feature request / bug report to
//! GitHub via the `gh` CLI.
//!
//! The CLI does the scaffolding (title prefix, standard label, environment
//! block, footer) so users and agents file consistent, well-tagged issues
//! without hand-writing the boilerplate. Posting is gated behind a y/N prompt
//! (skip with the global `--confirm`) since it's an outward-facing action.

use std::process::Command;

use anyhow::{Context as _, Result, bail};

use crate::cli::{FeedbackArgs, FeedbackKind};

/// Default target repo. Override with `--repo` or `DEADEYE_FEEDBACK_REPO`.
const DEFAULT_REPO: &str = "teddyjfpender/deadeye-rs";

pub(crate) async fn run(args: FeedbackArgs, confirm: bool) -> Result<()> {
    let repo = args.repo.clone().unwrap_or_else(|| DEFAULT_REPO.to_owned());
    let title = prefixed_title(args.kind, &args.title);
    let labels = labels_for(args.kind, &args.labels);
    let body = build_body(&args);

    if args.dry_run {
        println!("Would file this issue on {repo}:\n");
        println!("title : {title}");
        println!("labels: {}", labels.join(", "));
        println!("\n{body}");
        return Ok(());
    }

    ensure_gh()?;

    if !confirm {
        super::confirm_or_bail(&format!("Post this issue to {repo} (it is public)?"))?;
    }

    let url = create_issue(&repo, &title, &body, &labels)?;
    println!("Filed: {url}");
    Ok(())
}

/// Prefix the title so issues scan well in the list.
fn prefixed_title(kind: FeedbackKind, title: &str) -> String {
    let tag = match kind {
        FeedbackKind::Feature => "[Feature]",
        FeedbackKind::Bug => "[Bug]",
        FeedbackKind::Idea => "[Idea]",
    };
    format!("{tag} {title}")
}

/// Standard label (default GitHub labels that exist on any repo) + extras.
fn labels_for(kind: FeedbackKind, extra: &[String]) -> Vec<String> {
    let std = match kind {
        FeedbackKind::Bug => "bug",
        FeedbackKind::Feature | FeedbackKind::Idea => "enhancement",
    };
    let mut out = vec![std.to_owned()];
    for l in extra {
        if !out.contains(l) {
            out.push(l.clone());
        }
    }
    out
}

fn build_body(args: &FeedbackArgs) -> String {
    let kind = match args.kind {
        FeedbackKind::Feature => "feature request",
        FeedbackKind::Bug => "bug",
        FeedbackKind::Idea => "idea",
    };
    let component = args.component.as_deref().unwrap_or("unspecified");
    format!(
        "### Summary\n{title}\n\n\
         ### Type\n{kind}\n\n\
         ### Component\n{component}\n\n\
         ### Details\n{body}\n\n\
         ### Environment\n- deadeye {version}\n- {os}/{arch}\n\n\
         ---\nSubmitted via `deadeye feedback`.\n",
        title = args.title,
        body = args.body,
        version = env!("CARGO_PKG_VERSION"),
        os = std::env::consts::OS,
        arch = std::env::consts::ARCH,
    )
}

/// Fail early with a friendly message if `gh` isn't installed.
fn ensure_gh() -> Result<()> {
    let ok = Command::new("gh")
        .arg("--version")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false);
    if !ok {
        bail!(
            "the GitHub CLI `gh` is required to file issues. Install it from \
             https://cli.github.com and run `gh auth login`, then retry."
        );
    }
    Ok(())
}

/// Run `gh issue create`, returning the created issue URL. Retries without
/// labels if the repo rejects a label that doesn't exist.
fn create_issue(repo: &str, title: &str, body: &str, labels: &[String]) -> Result<String> {
    let out = gh_create(repo, title, body, labels)?;
    if out.status.success() {
        return Ok(stdout_url(&out.stdout));
    }
    let stderr = String::from_utf8_lossy(&out.stderr);
    // A missing label is the common failure — retry without labels so the
    // issue still gets filed, just untagged.
    if !labels.is_empty() && stderr.to_lowercase().contains("label") {
        eprintln!(
            "warning: a label was rejected ({}); retrying without labels",
            stderr.trim()
        );
        let retry = gh_create(repo, title, body, &[])?;
        if retry.status.success() {
            return Ok(stdout_url(&retry.stdout));
        }
        bail!(
            "gh issue create failed: {}",
            String::from_utf8_lossy(&retry.stderr).trim()
        );
    }
    bail!("gh issue create failed: {}", stderr.trim());
}

fn gh_create(
    repo: &str,
    title: &str,
    body: &str,
    labels: &[String],
) -> Result<std::process::Output> {
    let mut cmd = Command::new("gh");
    cmd.args([
        "issue", "create", "--repo", repo, "--title", title, "--body", body,
    ]);
    for l in labels {
        cmd.args(["--label", l]);
    }
    cmd.output().context("running `gh issue create`")
}

/// The created-issue URL is the last non-empty line gh prints to stdout.
fn stdout_url(stdout: &[u8]) -> String {
    String::from_utf8_lossy(stdout)
        .lines()
        .rev()
        .map(str::trim)
        .find(|l| !l.is_empty())
        .unwrap_or("")
        .to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cli::FeedbackKind;

    fn args(kind: FeedbackKind) -> FeedbackArgs {
        FeedbackArgs {
            title: "add CRPS scoring".into(),
            body: "After resolution I want a score command.".into(),
            kind,
            component: Some("forecast".into()),
            labels: vec![],
            repo: None,
            dry_run: true,
        }
    }

    #[test]
    fn title_is_prefixed_by_kind() {
        assert_eq!(prefixed_title(FeedbackKind::Feature, "x"), "[Feature] x");
        assert_eq!(prefixed_title(FeedbackKind::Bug, "x"), "[Bug] x");
    }

    #[test]
    fn standard_label_maps_to_default_github_labels() {
        assert_eq!(labels_for(FeedbackKind::Feature, &[]), vec!["enhancement"]);
        assert_eq!(labels_for(FeedbackKind::Bug, &[]), vec!["bug"]);
        let with_extra = labels_for(FeedbackKind::Idea, &["forecast".into()]);
        assert_eq!(
            with_extra,
            vec!["enhancement".to_owned(), "forecast".to_owned()]
        );
    }

    #[test]
    fn body_has_structured_sections() {
        let b = build_body(&args(FeedbackKind::Feature));
        for section in [
            "### Summary",
            "### Type",
            "### Component",
            "### Details",
            "### Environment",
        ] {
            assert!(b.contains(section), "missing {section}");
        }
        assert!(b.contains("forecast")); // component
        assert!(b.contains("deadeye feedback")); // footer
    }
}
