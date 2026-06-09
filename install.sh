#!/bin/sh
# Deadeye CLI installer.
#
#   curl -fsSL https://raw.githubusercontent.com/teddyjfpender/deadeye-rs/main/install.sh | sh
#
# Installs the `deadeye` binary and the `/deadeye-cli` agent skill so a coding
# agent can run the whole market loop for you. Re-run any time to update.
#
# Environment overrides:
#   DEADEYE_RELEASE_BASE  Base URL serving prebuilt binaries (when available).
#                         If unset, the script builds from source with cargo.
#   DEADEYE_BRANCH        Git branch/tag to install from source (default: main).
#   DEADEYE_SKIP_SKILL=1  Don't install the agent skill.
#   DEADEYE_SKIP_BIN=1    Don't install the binary (skill only).
set -eu

REPO="teddyjfpender/deadeye-rs"
BRANCH="${DEADEYE_BRANCH:-main}"
RAW_BASE="https://raw.githubusercontent.com/${REPO}/${BRANCH}"

info()  { printf '\033[1;36m==>\033[0m %s\n' "$1"; }
warn()  { printf '\033[1;33mwarning:\033[0m %s\n' "$1" >&2; }
die()   { printf '\033[1;31merror:\033[0m %s\n' "$1" >&2; exit 1; }
have()  { command -v "$1" >/dev/null 2>&1; }

# ── 1. Install the binary ────────────────────────────────────────────────
install_binary() {
  if [ -n "${DEADEYE_RELEASE_BASE:-}" ]; then
    install_prebuilt
  else
    install_from_source
  fi
}

# Prebuilt path — used once release artifacts exist at DEADEYE_RELEASE_BASE.
install_prebuilt() {
  os=$(uname -s); arch=$(uname -m)
  case "$os-$arch" in
    Darwin-arm64)  triple="aarch64-apple-darwin" ;;
    Darwin-x86_64) triple="x86_64-apple-darwin" ;;
    Linux-x86_64)  triple="x86_64-unknown-linux-gnu" ;;
    Linux-aarch64) triple="aarch64-unknown-linux-gnu" ;;
    *) die "no prebuilt binary for $os-$arch; unset DEADEYE_RELEASE_BASE to build from source" ;;
  esac
  bindir="${DEADEYE_BIN_DIR:-$HOME/.local/bin}"
  mkdir -p "$bindir"
  url="${DEADEYE_RELEASE_BASE%/}/deadeye-${triple}.tar.gz"
  info "Downloading $url"
  tmp=$(mktemp -d)
  curl -fsSL "$url" | tar -xz -C "$tmp" || die "download/extract failed"
  install -m 0755 "$tmp/deadeye" "$bindir/deadeye"
  rm -rf "$tmp"
  info "Installed deadeye to $bindir/deadeye"
  case ":$PATH:" in *":$bindir:"*) ;; *) warn "add $bindir to your PATH" ;; esac
}

# Source path — the real path until release CI lands.
install_from_source() {
  have cargo || die "cargo (Rust toolchain) not found. Install Rust from https://rustup.rs, \
or set DEADEYE_RELEASE_BASE to a prebuilt-binary host, then re-run."
  info "Building deadeye from source ($REPO@$BRANCH) — this can take a few minutes…"
  cargo install --git "https://github.com/${REPO}" --branch "$BRANCH" --locked deadeye-cli \
    || die "cargo install failed"
  info "Installed deadeye to $(cargo_bin)/deadeye"
  case ":$PATH:" in *":$(cargo_bin):"*) ;; *) warn "add $(cargo_bin) to your PATH" ;; esac
}

cargo_bin() { printf '%s' "${CARGO_HOME:-$HOME/.cargo}/bin"; }

# ── 2. Install the agent skill ───────────────────────────────────────────
install_skill() {
  # Install for every supported agent whose home skills dir exists (always
  # do Claude). Source: local checkout if present, else raw GitHub.
  for dir in "$HOME/.claude/skills" "$HOME/.codex/skills"; do
    case "$dir" in
      "$HOME/.claude/skills") ;;                 # always install for Claude
      *) [ -d "$(dirname "$dir")" ] || continue ;;  # others: only if agent home exists
    esac
    dest="$dir/deadeye-cli"
    mkdir -p "$dest"
    if [ -f "skills/deadeye-cli/SKILL.md" ]; then
      cp "skills/deadeye-cli/SKILL.md" "$dest/SKILL.md"
    else
      curl -fsSL "${RAW_BASE}/skills/deadeye-cli/SKILL.md" -o "$dest/SKILL.md" \
        || { warn "could not fetch SKILL.md for $dir"; continue; }
    fi
    info "Installed /deadeye-cli skill to $dest/SKILL.md"
  done
  warn "Restart your agent app to pick up the new skill."
}

# ── main ─────────────────────────────────────────────────────────────────
[ "${DEADEYE_SKIP_BIN:-0}" = "1" ]   || install_binary
[ "${DEADEYE_SKIP_SKILL:-0}" = "1" ] || install_skill

cat <<'EOF'

Deadeye installed. Get started:

    deadeye onboard --network mainnet     # create/recover a wallet, deploy account
    deadeye account show                  # confirm address + STRK balance
    deadeye collateral claim-grant --execute   # claim your XP grant
    deadeye markets list                  # find a market to trade

Or point a coding agent at it — the /deadeye-cli skill runs the whole loop:
fetch a market, forecast it, size the highest-EV trade, and submit.
EOF
