#!/usr/bin/env python3
"""Offline acceptance verifier for the Claude external smoke script.

The verifier runs a smoke script against a temporary fake `deadeye` binary. It
does not call the real CLI, RPC, indexer, wallet config, or any on-chain path.
Its purpose is narrow: prove the external smoke script enforces the same
`deadeye >= 0.1.20` floor as the Storm runner before doing market reads.
"""

from __future__ import annotations

import argparse
import json
import os
import stat
import subprocess
import tempfile
from pathlib import Path
from typing import Any


REPO_ROOT = Path(__file__).resolve().parents[1]
DEFAULT_SMOKE_SCRIPT = REPO_ROOT.parent / "deadeye-claude-smoke" / "smoke.sh"
DEFAULT_MARKET = "0x5e678bd092173e9ef0945f348d09e6c1c22f78e06c0ef380441444359193500"
MINIMUM_VERSION = "deadeye 0.1.20"
ACCEPTED_VERSION_REGEX = (
    r"^deadeye 0\.1\.(2[0-9]|[3-9][0-9]|[1-9][0-9]{2,})$"
    r"|^deadeye 0\.([2-9]|[1-9][0-9]+)\.[0-9]+$"
    r"|^deadeye ([1-9][0-9]*)\.[0-9]+\.[0-9]+$"
)
FIX_HINT = (
    "Add a deadeye --version gate before market reads; accept only "
    f"{MINIMUM_VERSION}+ and exit nonzero for missing, unparseable, or stale output."
)


FAKE_DEADEYE = """#!/usr/bin/env python3
import json
import os
import sys

log = os.environ["STORM_FAKE_DEADEYE_LOG"]
mode = os.environ["STORM_FAKE_DEADEYE_MODE"]
args = sys.argv[1:]
with open(log, "a", encoding="utf-8") as f:
    f.write(json.dumps(args) + "\\n")

if args == ["--version"]:
    if mode == "good":
        print("deadeye 0.1.20")
        raise SystemExit(0)
    if mode == "stale":
        print("deadeye 0.1.19")
        raise SystemExit(0)
    if mode == "missing":
        raise SystemExit(0)
    if mode == "unparseable":
        print("deadeye build-local")
        raise SystemExit(0)

if not args:
    raise SystemExit(64)

if args[:2] == ["markets", "list"]:
    print("0xabc Germany mock market")
    raise SystemExit(0)

if args[:2] == ["markets", "show"]:
    print(json.dumps({
        "family": "lognormal",
        "distribution": {"mu": 3.5, "sigma": 0.2},
        "status": {"kind": "Active"},
    }))
    raise SystemExit(0)

if args[:1] == ["doctor"]:
    print("all_ok true")
    raise SystemExit(0)

if args[:2] == ["trade", "quote"]:
    print("expected_value 1.0 candidate_mean 3.45 on_chain_will_accept true")
    raise SystemExit(0)

print("unexpected fake deadeye command: " + " ".join(args), file=sys.stderr)
raise SystemExit(65)
"""


def write_fake_deadeye(fake_bin: Path) -> Path:
    fake = fake_bin / "deadeye"
    fake.write_text(FAKE_DEADEYE, encoding="utf-8")
    fake.chmod(fake.stat().st_mode | stat.S_IXUSR)
    return fake


def read_command_log(log_path: Path) -> list[list[str]]:
    if not log_path.exists():
        return []
    return [json.loads(line) for line in log_path.read_text(encoding="utf-8").splitlines() if line.strip()]


def run_case(smoke_script: Path, mode: str, market: str, timeout: float) -> dict[str, Any]:
    with tempfile.TemporaryDirectory(prefix="storm-smoke-floor-") as tmp:
        tmpdir = Path(tmp)
        fake_bin = tmpdir / "bin"
        fake_bin.mkdir()
        log_path = tmpdir / "deadeye-commands.jsonl"
        write_fake_deadeye(fake_bin)

        env = os.environ.copy()
        env["HOME"] = str(tmpdir / "home")
        env["PATH"] = f"{fake_bin}{os.pathsep}{env.get('PATH', '')}"
        env["STORM_FAKE_DEADEYE_LOG"] = str(log_path)
        env["STORM_FAKE_DEADEYE_MODE"] = mode

        proc = subprocess.run(
            ["zsh", str(smoke_script), market],
            text=True,
            capture_output=True,
            timeout=timeout,
            env=env,
            check=False,
        )
        commands = read_command_log(log_path)
        return {
            "mode": mode,
            "returncode": proc.returncode,
            "commands": commands,
            "post_version_commands": commands[1:] if commands[:1] == [["--version"]] else commands,
            "stdout_tail": proc.stdout.strip().splitlines()[-5:],
            "stderr_tail": proc.stderr.strip().splitlines()[-5:],
        }


def evaluate_case(case: dict[str, Any]) -> tuple[bool, str]:
    mode = case["mode"]
    commands = case["commands"]
    post_version = case["post_version_commands"]
    rc = case["returncode"]
    if commands[:1] != [["--version"]]:
        return False, "did not check `deadeye --version` first"
    if mode == "good":
        if rc != 0:
            return False, f"good deadeye 0.1.20 path failed rc={rc}"
        required_prefixes = [
            ["markets", "list"],
            ["markets", "show"],
            ["doctor"],
            ["trade", "quote"],
        ]
        missing = [
            prefix
            for prefix in required_prefixes
            if not any(cmd[: len(prefix)] == prefix for cmd in commands)
        ]
        if missing:
            return False, f"good path skipped expected read-only probes: {missing}"
        return True, "accepted deadeye 0.1.20 and reached read-only probes"
    if rc == 0:
        return False, f"{mode} version path passed but should fail"
    if post_version:
        return False, f"{mode} version path ran commands after version check: {post_version}"
    return True, f"{mode} version failed closed before market reads"


def verify_smoke_script(smoke_script: Path, market: str = DEFAULT_MARKET, timeout: float = 12.0) -> dict[str, Any]:
    smoke_script = smoke_script.expanduser().resolve()
    if not smoke_script.exists():
        return {
            "ok": False,
            "smoke_script": str(smoke_script),
            "error": "smoke script does not exist",
            "minimum_version": MINIMUM_VERSION,
            "accepted_version_regex": ACCEPTED_VERSION_REGEX,
            "fix_hint": FIX_HINT,
            "cases": [],
        }
    cases = [run_case(smoke_script, mode, market, timeout) for mode in ("good", "stale", "missing", "unparseable")]
    evaluated = []
    ok = True
    for case in cases:
        case_ok, reason = evaluate_case(case)
        ok = ok and case_ok
        evaluated.append({**case, "ok": case_ok, "reason": reason})
    return {
        "ok": ok,
        "smoke_script": str(smoke_script),
        "minimum_version": MINIMUM_VERSION,
        "accepted_version_regex": ACCEPTED_VERSION_REGEX,
        "network_free": True,
        "real_deadeye_invoked": False,
        "fix_hint": None if ok else FIX_HINT,
        "cases": evaluated,
    }


def build_arg_parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(description="Verify external smoke enforces deadeye >= 0.1.20 offline.")
    parser.add_argument("--smoke-script", type=Path, default=DEFAULT_SMOKE_SCRIPT)
    parser.add_argument("--market", default=DEFAULT_MARKET)
    parser.add_argument("--timeout", type=float, default=12.0)
    return parser


def main(argv: list[str] | None = None) -> int:
    args = build_arg_parser().parse_args(argv)
    result = verify_smoke_script(args.smoke_script, args.market, args.timeout)
    print(json.dumps(result, indent=2, sort_keys=True))
    return 0 if result["ok"] else 1


if __name__ == "__main__":
    raise SystemExit(main())
