#!/usr/bin/env python3
"""Promote ready Storm Deadeye templates into the candidate queue.

This is local queue plumbing only. It never calls Deadeye, never reads wallet
config, and never submits transactions. The execution runner still re-checks
doctor, quote, dry-run, gas, XP, EV, concentration, and trade caps.
"""

from __future__ import annotations

import argparse
import json
from pathlib import Path

import storm_deadeye_loop as loop

promote_templates = loop.promote_ready_templates


def build_arg_parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(description="Promote ready Storm Deadeye templates into the candidate queue.")
    parser.add_argument("--templates", type=Path, default=loop.DEFAULT_TEMPLATES)
    parser.add_argument("--candidates", type=Path, default=loop.DEFAULT_CANDIDATES)
    parser.add_argument("--append", action="store_true", help="Append ready templates to the candidate queue.")
    return parser


def main(argv: list[str] | None = None) -> int:
    args = build_arg_parser().parse_args(argv)
    result = loop.promote_ready_templates(args.templates, args.candidates, append=bool(args.append))
    print(json.dumps(result, indent=2, sort_keys=True))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
