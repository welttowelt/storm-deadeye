#!/bin/sh
# Deadeye CLI installer.
#
#   curl -fsSL https://project-deadeye.vercel.app/install.sh | sh
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

# Default host for prebuilt release binaries (GitHub Releases). Overridable.
DEFAULT_RELEASE_BASE="https://github.com/${REPO}/releases/latest/download"

# ── 1. Install the binary ────────────────────────────────────────────────
# Prefer a prebuilt binary (no toolchain needed); fall back to a source build
# if none exists for this platform or the download fails.
install_binary() {
  base="${DEADEYE_RELEASE_BASE:-$DEFAULT_RELEASE_BASE}"
  if install_prebuilt "$base"; then
    return 0
  fi
  warn "no prebuilt binary available; building from source (needs Rust)…"
  install_from_source
}

# Prebuilt path. Returns non-zero (so the caller falls back) on unsupported
# platform or download failure.
install_prebuilt() {
  base="$1"
  os=$(uname -s); arch=$(uname -m)
  case "$os-$arch" in
    Darwin-arm64)            triple="aarch64-apple-darwin" ;;
    Darwin-x86_64)           triple="x86_64-apple-darwin" ;;
    Linux-x86_64)            triple="x86_64-unknown-linux-gnu" ;;
    Linux-aarch64|Linux-arm64) triple="aarch64-unknown-linux-gnu" ;;
    *) return 1 ;;
  esac
  url="${base%/}/deadeye-${triple}.tar.gz"
  info "Fetching prebuilt binary: $url"
  tmp=$(mktemp -d)
  if ! curl -fsSL "$url" | tar -xz -C "$tmp" 2>/dev/null || [ ! -f "$tmp/deadeye" ]; then
    rm -rf "$tmp"
    return 1
  fi
  bindir="${DEADEYE_BIN_DIR:-$HOME/.local/bin}"
  mkdir -p "$bindir"
  install -m 0755 "$tmp/deadeye" "$bindir/deadeye"
  rm -rf "$tmp"
  info "Installed deadeye to $bindir/deadeye"
  case ":$PATH:" in *":$bindir:"*) ;; *) warn "add $bindir to your PATH" ;; esac
  return 0
}

# Source path — fallback when no prebuilt binary fits the platform.
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

# ── 2. Install the agent skills ──────────────────────────────────────────
# The forecasting suite + the CLI usage skill.
SKILLS="deadeye-cli deadeye-superforecaster bayes-forecast-scratchpad evidence-ledger"

install_skill() {
  # Install for every supported agent whose home skills dir exists (always
  # do Claude). Source: local checkout if present, else raw GitHub.
  for dir in "$HOME/.claude/skills" "$HOME/.codex/skills"; do
    case "$dir" in
      "$HOME/.claude/skills") ;;                 # always install for Claude
      *) [ -d "$(dirname "$dir")" ] || continue ;;  # others: only if agent home exists
    esac
    for skill in $SKILLS; do
      dest="$dir/$skill"
      mkdir -p "$dest"
      if [ -f "skills/$skill/SKILL.md" ]; then
        cp "skills/$skill/SKILL.md" "$dest/SKILL.md"
      else
        curl -fsSL "${RAW_BASE}/skills/$skill/SKILL.md" -o "$dest/SKILL.md" \
          || { warn "could not fetch $skill SKILL.md for $dir"; continue; }
      fi
      info "Installed /$skill skill to $dest/SKILL.md"
    done
  done
  warn "Restart your agent app to pick up the new skills."
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
