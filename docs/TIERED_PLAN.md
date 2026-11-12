# TeamCraft - Tiered Execution Plan

**Parent Standard:** [`RUST_ARTCRAFT_STANDARD.md`](../../mcp-central-docs/standards/RUST_ARTCRAFT_STANDARD.md)  
**Menu Bar Standard:** [`CRAFT_MENU_BAR_STANDARD.md`](../../mcp-central-docs/standards/rust-artcraft/CRAFT_MENU_BAR_STANDARD.md)  
**Architecture Model:** 4-Tier ArtCraft Incremental Delivery  
**Status:** Tiers 1-3 done; Tier 4 done except packaging (v0.3.0)  

---

## Architecture Flow

```
Tier 1: Core Engine ──> Tier 2: MCP / CLI ──> Tier 3: Desktop UI ──> Tier 4: Menu Bar, Help & Packaging
(teamcraft-core)       (teamcraft-mcp)       (teamcraft-app)       (Hardened Workstation)
```

---

## Tier Specifications

### Tier 1 - Core Working Time & AZG Engine (`teamcraft-core`)
- [x] L1 `Command::dispatch` universal time tracking execution bus.
- [x] Employee directory and clock-in/out state machine with overlap guard.
- [x] Week confirmation with trigger-enforced immutable locks.
- [x] Austrian Arbeitszeitgesetz (AZG) rules (10h, 12h, 60h/week, 48h avg, rest periods).
- [x] Leave entitlement calculator against Austrian public holidays.
- [x] KV minimum pay floor compliance checker.
- [x] Cross-craft event emission (`capacity.changed` -> TaskCraft).
- [x] `unsafe_code = "forbid"` (workspace lint since 0.3.0; claimed earlier, enforced now) and zero-panic clippy lints.

### Tier 2 - Headless Driver & FastMCP (`teamcraft-mcp`)
- [x] Unified CLI `teamcraft-cli` with `--json` output.
- [x] FastMCP stdio server communicating over JSON-RPC 2.0 (25 tools).
- [x] Timesheet CSV reporting and AZG violation audit.
- [x] Headless `--smoke` verification without window initialization.

### Tier 3 - Desktop Workstation UI (`teamcraft-app`)
- [x] Immediate-mode `egui` 0.37 / `wgpu` workstation.
- [x] Interactive timesheet week grid with inline AZG violation flags.
- [x] Leave and absence calendar (month/week) with approval workflow.
- [x] Team capacity planning cockpit.

### Tier 4 - Craft Menu Bar Standard, Help Suite & Hardened Verification
- [x] Universal 8-menu bar (`File`, `Edit`, `View`, `Insert`, `Modify`, `Commands`, `Window`, `Help`) with `switch_on_hover` (bookends from `craft-biz-ui`, middle menus in `TeamApp::menus`).
- [x] Mandatory 5-dialog Help suite (`Command Palette...`, `Keyboard Shortcuts...`, `Project Files & Docs...`, `Licenses & Notices...`, `About TeamCraft`). Docs are compiled into the binary; the licenses list and commit/build date come from `build.rs`.
- [x] Fleet UI/UX Pattern 2 adherence: quick view row mapped to View items, panel visibility managed via `View` / `Window`.
- [x] Tofu Guard verification (every menu, shortcut and navigation label checked against the loaded fonts, DE and EN).
- [x] Headless `--smoke` parity validation (menu items name real tools and arguments, no duplicate shortcuts).
- [ ] Standalone release binary packaging and NSIS installer.
