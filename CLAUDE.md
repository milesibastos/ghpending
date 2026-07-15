# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

`AGENTS.md` holds detailed operational gotchas (proxy env vars, config paths, packaging, release CI). Read it too; this file is the architecture map, not a duplicate.

## Commands

- Test: `cargo test` (what release CI gates on). Single test by filter, e.g. `cargo test github::tests::item_cmp_sorts_prs_before_issues_then_number_desc`.
- Run: `cargo run --` (digest), `cargo run -- add [--user <name>|--all]`, `cargo run -- list`, `cargo run -- rm`. `add`/`rm` are interactive; `add` and the digest hit the live GitHub API.
- Snapshot output with `NO_COLOR=1`. Use `--config <path>` or `GHPENDING_CONFIG` with a throwaway file to avoid mutating the active global or repo-local watch list.

## Architecture

Single Rust CLI crate (edition 2024), not a workspace. `main.rs` wires everything; each module owns one concern and pushes I/O and env reads to the edges so the core logic stays pure and unit-testable.

Startup flow in `main.rs`: parse `Cli` → `config::resolve_path` (picks the active config file, notes non-global sources to stderr) → `config::load_from` → resolve theme (`theme::resolve_name` picks the name across flag/env/config precedence, `Theme::by_name` maps it to styles) → `github_client::build()` constructs the octocrab client once → dispatch to a `commands::*` handler, threading the resolved config `&Path`. No subcommand = digest.

Module responsibilities:
- `github_client.rs` — builds the octocrab `Octocrab`. Decides direct vs SOCKS-proxied transport, reads `GITHUB_TOKEN`. Pure URI/proxy-selection helpers are tested without network.
- `github.rs` — all GitHub domain logic and types (`RepoItem`, `RepoResult`/`RepoStatus`, `ListSource`, `AccountKind`). Fetching, pagination, 404→`NotFound` mapping, `item_cmp` sort, and the `resolve_list_source` decision table live here. This is the layer to extend for new API behavior.
- `commands/` — one file per subcommand (`digest`, `add`, `list`, `remove`). Orchestration only; they call into `github`/`config`/`display`.
- `config.rs` — TOML load/save of the watch list (`user`, `repos`, `theme`, `filters`); `0600` on Unix. `resolve_path` picks the active file (flag → env → local `.ghpending.toml` → global via the `directories` crate); `load_from`/`save_to` operate on that path. Pure `choose_source`/`find_local_config` helpers carry the precedence/discovery logic and the tests.
- `display.rs` / `format.rs` — rendering. `render_inner(results, theme, color, width)` is the pure core (color + terminal width injected as args); `render_digest` is the thin env-reading wrapper. `format.rs` has `truncate_title`/`relative_time`.
- `theme.rs` — palette definitions + name-resolution precedence.

Key design points (span multiple files, easy to break):
- **Digest concurrency** (`commands/digest.rs`): bounded to `MAX_CONCURRENT_FETCHES = 4` via a `FuturesUnordered` sliding window, with a 30s wall-clock deadline; repos not yet resolved when the deadline hits render as `timeout after 30s`. All-failed → non-zero exit (`all_repo_fetches_failed`).
- **Item fetch** (`github::fetch_items_inner`): issues and pulls are fetched from separate endpoints concurrently; PRs echoed by the issues endpoint (`issue.pull_request.is_some()`) are dropped. Sort is PRs-before-issues, each group by number descending (`item_cmp`).
- **PR enrichment** (`github::fetch_pr_extras` → `PrExtra`, rendered by `display::pr_detail_line`/`pr_extra_line`): a best-effort GraphQL query per PR-bearing repo adds unresolved-thread authors, the Codex reaction, and per-reviewer states. `pr_detail_line` combines that history with current REST review requests and suppresses redundant signals. Best-effort means a GraphQL failure leaves `pr_extra` `None` without downgrading the REST result. Pure helpers (`is_codex_actor`, `unresolved_by_author`, `codex_reaction`, `collapse_reviews`) carry the logic and the tests; the wire structs stay at the edge. See AGENTS.md for rendering and Codex-login gotchas.
- **`add` list source** (`github::resolve_list_source` / `resolve_source_for`): own login → authenticated listing (private included), org target → org listing, third-party user → public-only. `--all` ignores the saved user and lists everything the token reaches. Keep the pure `resolve_list_source` table and its tests in sync when changing this.
- **Digest omits zero-item repos** from the body but still counts them in the summary line.

## Testing convention

Unit tests live in `#[cfg(test)]` modules inline in each source file, and cover the pure decision functions (`resolve_name`, `resolve_list_source`, `resolve_user`, `item_cmp`, `split_repo`, `render_inner`). Prefer adding a pure helper + inline test over testing through the network.
