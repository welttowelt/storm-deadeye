#!/usr/bin/env python3
import sys
import tempfile
import textwrap
import unittest
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
import verify_external_smoke_floor as verifier


def write_script(path: Path, body: str) -> None:
    path.write_text(textwrap.dedent(body).lstrip(), encoding="utf-8")
    path.chmod(0o755)


class VerifyExternalSmokeFloorTests(unittest.TestCase):
    def test_compliant_script_passes_offline_verifier(self):
        with tempfile.TemporaryDirectory() as tmpdir:
            script = Path(tmpdir) / "smoke.sh"
            write_script(
                script,
                """
                #!/bin/zsh
                MKT="${1:-0x1}"
                V="$(deadeye --version 2>&1)"
                [[ "$V" =~ '^deadeye 0\\.1\\.(2[0-9]|[3-9][0-9])$|^deadeye 0\\.[2-9]\\.|^deadeye [1-9]\\.' ]] || exit 7
                deadeye markets list --limit 3 --output plain >/dev/null || exit 1
                deadeye markets show "$MKT" --output json >/dev/null || exit 1
                deadeye doctor --market "$MKT" --output plain >/dev/null || exit 1
                deadeye trade quote "$MKT" --family lognormal --belief 3.4 --belief-sigma 0.2 --budget 50 >/dev/null || exit 1
                """,
            )

            result = verifier.verify_smoke_script(script, timeout=3)

        self.assertTrue(result["ok"])
        self.assertTrue(all(case["ok"] for case in result["cases"]))

    def test_script_that_continues_after_stale_version_fails(self):
        with tempfile.TemporaryDirectory() as tmpdir:
            script = Path(tmpdir) / "smoke.sh"
            write_script(
                script,
                """
                #!/bin/zsh
                V="$(deadeye --version 2>&1)"
                [[ "$V" == *deadeye* ]] || true
                deadeye markets list --limit 3 --output plain >/dev/null
                exit 1
                """,
            )

            result = verifier.verify_smoke_script(script, timeout=3)

        self.assertFalse(result["ok"])
        stale = next(case for case in result["cases"] if case["mode"] == "stale")
        self.assertFalse(stale["ok"])
        self.assertIn("ran commands after version check", stale["reason"])

    def test_script_that_rejects_good_version_fails(self):
        with tempfile.TemporaryDirectory() as tmpdir:
            script = Path(tmpdir) / "smoke.sh"
            write_script(
                script,
                """
                #!/bin/zsh
                deadeye --version >/dev/null
                exit 9
                """,
            )

            result = verifier.verify_smoke_script(script, timeout=3)

        self.assertFalse(result["ok"])
        good = next(case for case in result["cases"] if case["mode"] == "good")
        self.assertFalse(good["ok"])
        self.assertIn("good deadeye 0.1.20 path failed", good["reason"])


if __name__ == "__main__":
    unittest.main()
