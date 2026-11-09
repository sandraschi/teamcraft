# AGENTS.md - teamcraft

Working time and leave for a small Austrian team (replaces Clockodo / Personio-lite / timesheet Excel). Not payroll. Read first:
`mcp-central-docs/standards/RUST_ARTCRAFT_STANDARD.md`,
`mcp-central-docs/projects/craft-fleet-expansion/BACKOFFICE_CRAFT_SUITE.md` (section 4.3).

## Layout

- `crates/teamcraft-store` (L1): employees, time entries, week locks, leave, adjustments, KV minimum table, AZG findings on SQLite WAL. All data invariants live here.
- `crates/teamcraft-core` (L1): `Command` bus, `dispatch`, MCP catalog, `azg.rs` (pure AZG engine), timesheet, KV check, capacity (emits `capacity.changed` to taskcraft), demo seed.
- `crates/teamcraft-mcp` (L2): `teamcraft` binary. CLI verbs map onto the same tool calls as MCP.
- No L3 (egui) yet. Planned: clock panel, week view with AZG flags, leave calendar.
- Shared kernel: `../craft-biz` (holidays, money, bus, MCP loop, confinement).

## Invariants (never weaken)

- One open entry per employee (unique partial index); entries of one employee never overlap.
- A confirmed ISO week is read-only. Enforced in Rust and by SQLite triggers on INSERT/UPDATE/DELETE.
- Sick leave stores day counts only. No reason, no note: refused in Rust and by a CHECK constraint. Never add a diagnosis field.
- No SV number, address, bank details or other payroll data on employees. Gross salary only for the KV check.
- Leave counts working days (Mon-Fri minus Austrian public holidays from `craft_biz::holidays`) in half-days. No floats in balances.
- AZG findings are recorded, never auto-corrected; findings that stop applying are marked cleared, not deleted.
- No KV figures ship with the app. The demo KV table is labelled fictional.

## Docs (keep current in the same commit)

- `README.md`: what it does, quick start, MCP config, build.
- `HELP.md`: task-oriented how-to; update when a verb or workflow changes.
- `docs/PRD.md`: purpose, boundaries, honest gaps, next steps; update "Today" and "Gaps" with every release.
- `docs/TOOLS.md`: generated. After any tool change run `just docs`; the `tools_doc_is_current` test fails otherwise.
- `CHANGELOG.md`: one entry per change set.
- CLI help: `about`, argument doc comments and `AFTER_HELP` in the mcp crate's `main.rs`.

## Rules

- Zero-panic: unwrap/expect/panic/todo/unimplemented denied outside tests.
- MCP mode: stdout is JSON-RPC only; diagnostics to stderr.
- File tools (`kv_import`, `timesheet --out`) go through `Confinement::check`.
- Demo data only via `demo-seed`, only on an empty database, flagged demo. Mock names only.
- Every bug fix ships with a regression test.

## Repo hygiene

- LF line endings only (`.gitattributes`). CRLF breaks commit hooks.
- Single trunk `main`. No agent branches or leftover WIP branches.
- No em dash characters in code, docs or commit messages.
- No `*.bak`, `*.db`, `*.csv`, `target/`, binaries in git.

## Gates

1. `cargo clippy --workspace --all-targets --all-features -- -D warnings`
2. `cargo test --workspace`
3. `cargo fmt --all --check`
4. MCP probe: `initialize` -> `tools/list` -> `tools/call`, stdout pure JSON
5. `Cargo.lock` committed, clean `git status`
6. `CHANGELOG.md` entry; README / HELP / PRD updated if behaviour changed
