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
- **Digest source** (`cli::Cli::repo` / `commands::digest::resolve_repos`): no positional argument uses the configured watch list; one validated `OWNER/REPO` replaces it in memory for that invocation. It never persists and remains orthogonal to configured/CLI filters and themes. Clap gives known subcommands precedence over this positional argument.
- **Item fetch** (`github::fetch_items_inner`): issues and pulls are fetched from separate endpoints concurrently; PRs echoed by the issues endpoint (`issue.pull_request.is_some()`) are dropped. Sort is PRs-before-issues, each group by number descending (`item_cmp`).
- **Repository metadata** (`github::fetch_repo_metadata` → `RepoMetadata`, rendered by `display::repo_metadata_segments`): an independent best-effort GraphQL query runs concurrently with item fetching and adds the most recently published non-draft release plus the tag whose target commit is most recent. Release/tag duplicates collapse; tag-only repos remain useful; incomplete release pages retain only the tag; metadata failure never affects `RepoStatus`. Header segments are removed from right to left when they do not fit the terminal.
- **PR enrichment** (`github::fetch_pr_extras` / `fetch_pr_checks` → `PrExtra`, rendered by `display::pr_status_segments`/`pr_detail_segments`/`pr_extra_line`): independent best-effort GraphQL streams per PR-bearing repo add merge readiness, head-commit check rollups and context names, unresolved-thread authors, the Codex reaction, per-reviewer states, and recent submitted-review history. Each stream pages open PRs from most recently updated and retains completed pages within its 8s budget; keeping checks separate prevents a check-permission error from suppressing review context. The renderer puts semantically styled merge/check segments in that order on the metadata line, combines review enrichment with current REST requests on the optional detail line, marks users with prior reviews as awaiting re-review, and suppresses redundant stale signals. Pure helpers carry the state mapping and tests; wire structs stay at the edge. See AGENTS.md for limits, rendering, and Codex-login gotchas.
- **`add` list source** (`github::resolve_list_source` / `resolve_source_for`): own login → authenticated listing (private included), org target → org listing, third-party user → public-only. `--all` ignores the saved user and lists everything the token reaches. Keep the pure `resolve_list_source` table and its tests in sync when changing this.
- **Digest omits zero-item repos** from the body but still counts them in the summary line.

## Testing convention

Unit tests live in `#[cfg(test)]` modules inline in each source file, and cover the pure decision functions (`resolve_name`, `resolve_list_source`, `resolve_user`, `item_cmp`, `split_repo`, `render_inner`). Prefer adding a pure helper + inline test over testing through the network.
