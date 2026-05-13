#![allow(
    clippy::print_stdout,
    clippy::print_stderr,
    reason = "xtask is a CLI binary — printing is the whole point"
)]

//! Workspace task runner — `cargo xtask <subcommand>`.
//!
//! Keep this binary intentionally small: every subcommand should be a
//! thin shell over functionality that lives in a library crate so the
//! same code is reachable from CI and from local development.

use std::{env, process::ExitCode, time::Duration};

use anyhow::{Context as _, Result, bail};
use deadeye_testkit::{Harness, devnet};
use url::Url;

const HELP: &str = "\
deadeye-rs xtask
USAGE: cargo xtask <command>

Commands:
  ci              Run the full CI pipeline locally (fmt + clippy + test).
  devnet-up       Verify a starknet-devnet is reachable at the default URL.
  devnet-reset    Reset the devnet back to genesis.
  help            Print this help.
";

#[tokio::main]
async fn main() -> ExitCode {
    match run().await {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) => {
            eprintln!("xtask: {err:#}");
            ExitCode::FAILURE
        },
    }
}

async fn run() -> Result<()> {
    let mut args = env::args().skip(1);
    let cmd = args.next().unwrap_or_else(|| "help".into());
    match cmd.as_str() {
        "help" | "-h" | "--help" => {
            print!("{HELP}");
            Ok(())
        },
        "ci" => ci(),
        "devnet-up" => devnet_up().await,
        "devnet-reset" => devnet_reset().await,
        other => {
            print!("{HELP}");
            bail!("unknown command: `{other}`");
        },
    }
}

fn ci() -> Result<()> {
    use std::process::Command;

    println!("xtask: cargo fmt --all -- --check");
    run_command(Command::new("cargo").args(["fmt", "--all", "--", "--check"]))?;

    println!("xtask: cargo clippy --workspace --all-targets --all-features -- -D warnings");
    run_command(Command::new("cargo").args([
        "clippy",
        "--workspace",
        "--all-targets",
        "--all-features",
        "--",
        "-D",
        "warnings",
    ]))?;

    println!("xtask: cargo test --workspace --all-features --lib --bins");
    run_command(Command::new("cargo").args([
        "test",
        "--workspace",
        "--all-features",
        "--lib",
        "--bins",
    ]))?;

    Ok(())
}

fn run_command(cmd: &mut std::process::Command) -> Result<()> {
    let status = cmd.status().context("failed to spawn cargo")?;
    if !status.success() {
        bail!("`{cmd:?}` failed with status {status}");
    }
    Ok(())
}

async fn devnet_up() -> Result<()> {
    let url = Url::parse(devnet::DEFAULT_URL)?;
    devnet::wait_until_ready(&url, 10, Duration::from_secs(1))
        .await
        .with_context(|| format!("devnet at {url} is not reachable"))?;
    let harness = Harness::devnet().await?;
    println!("devnet OK at {}", harness.url());
    Ok(())
}

async fn devnet_reset() -> Result<()> {
    let url = Url::parse(devnet::DEFAULT_URL)?;
    devnet::reset(&url).await.context("devnet reset failed")?;
    println!("devnet reset to genesis");
    Ok(())
}
