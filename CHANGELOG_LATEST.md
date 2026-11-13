# teamcraft v0.3.1

Working time and leave for a small Austrian team, with Arbeitszeitgesetz checks. Not payroll. Local, one SQLite file. Windows x64.

## Downloads

- `teamcraft-app.exe`: the desktop window.
- `teamcraft.exe`: CLI and MCP server (`teamcraft mcp --allow-path <dir>`).
- `teamcraft-0.3.1-setup.exe`: per-user installer for both (no admin rights). Your records stay in `%LOCALAPPDATA%\teamcraft`.
- `teamcraft-v0.3.1.mcpb`: MCP bundle.

## What is new since 0.3.0

- A `candidate.hired` event from hirecraft opens one employee and does not copy a wage.

## What is new since 0.1.1

- Desktop window on the shared suite frame: time clock, AZG section 26 timesheet with target and balance, team absence calendar, leave request board, balances, capacity, AZG findings, team and KV check.
- Menu bar: Insert (clock in/out, time entry, leave, employee), Modify (employee, delete entry, decide leave, confirm week, adjust balance), Commands (AZG scan, KV check, capacity to taskcraft, vacation balances), plus the Help suite (shortcuts F1, docs, licenses, about).
- Every tool reachable from the window, CLI and MCP (25 tools).

Synthetic demo data only (`teamcraft demo-seed`). Rules are the general AZG and were not reviewed by a labour lawyer. See HELP.md and docs/PRD.md.