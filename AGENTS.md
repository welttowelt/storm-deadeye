# Deadeye Agent Instructions

This checkout uses the Deadeye Storm mailbox:

`/Users/olifreuler/Documents/New project/DEADEYE_AGENT_HANDOFF.md`

When working in this repo, read that mailbox before substantial edits,
validation, review, or any on-chain command. Append dated entries there for real
results, gate decisions, reviews, and handoffs.

Project call signs:

- Codex in this project: **Codex_Storm Deadeye**.
- Claude review lane: **Claude_Storm**.
- World Cup research/scout lane: **scout_claude**.
- Critique-before-action mode: **RCI_Storm**, signed by the runner.
- Adversarial review mode: **Sceptic_Storm**, signed by the runner.
- GPU execution lane: **GPU_Storm**, only after the mailbox contains the exact
  trigger `NOW GO FOR GPUs!!`.

Worktree rules:

- Codex_Storm Deadeye owns this worktree unless a mailbox entry declares a
  different one.
- Claude_Storm must review in a separate worktree named
  `/Users/olifreuler/Documents/New project/deadeye-claude-*`.
- scout_claude is a mailbox-only research resource. Send it concrete World Cup
  scouting jobs or research findings through the mailbox, always acknowledge
  its messages before acting, and always request read-ack on outbound notes.
- Do not run or edit in another agent's declared worktree.

Safety rules:

- Read-only commands, local builds/tests, market reads, doctor checks, quotes,
  and explicitly dry-run simulations are allowed.
- Oli approved autonomous leaderboard trading for this project. `deadeye trade
  quote`, `deadeye trade execute --dry-run`, and capped
  `deadeye trade execute --confirm` are allowed only for leaderboard trades that
  pass the local Storm Deadeye runner policy: fresh smoke/doctor, evidence-backed
  belief, quote accepted, dry-run attempted for the intended path, 100 STRK gas
  warning, 50 STRK strong warning, 25 STRK hard write stop, 1000 XP reserve, and
  trade caps.
- Do not run LP operations, admin operations, account deployment, grants,
  approval-only commands, settlement, pauses, unpauses, or runtime deployments
  without explicit approval for the exact command, account, network,
  market/contract, and caps.
- Never print, copy, commit, or summarize private keys or mnemonics.
- Do not commit wallet configs, journals, `.env` files, or secrets.

Default validation menu:

- `git status --short --branch`
- `git diff --check`
- `cargo check --locked -p <crate>`
- `cargo test --locked -p <crate>`
- `cargo test --locked -p deadeye-cli` when CLI behavior changes
- `deadeye markets list/show/info`, `deadeye doctor`, and `deadeye trade quote`
  for read-only live checks when relevant
