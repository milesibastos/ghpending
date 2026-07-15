# ghpending

See open issues and pull requests across the GitHub repos you care about, at a glance.

![ghpending output](https://raw.githubusercontent.com/akitaonrails/ghpending/main/docs/screenshot.png)

## Install

### Homebrew (macOS / Linux)

```sh
brew tap akitaonrails/tap && brew install ghpending
```

### Arch Linux (AUR)

```sh
yay -S ghpending-bin    # prebuilt x86_64 binary, fastest
yay -S ghpending        # builds from source, x86_64/aarch64
```

### Cargo

```sh
cargo install ghpending
```

### mise

```sh
mise use -g github:akitaonrails/ghpending
```

### From source

```sh
git clone https://github.com/akitaonrails/ghpending
cargo install --path ghpending
```

## Upgrading

```sh
# Homebrew
brew upgrade ghpending

# AUR (any helper that respects upstream changes)
yay -Syu ghpending-bin

# Cargo
cargo install ghpending --force

# mise
mise upgrade ghpending

# From source
cd ghpending && git pull && cargo install --path .
```

## Usage

```sh
ghpending add                # pick repos from the saved user/org to track
ghpending add --user <name>  # switch to a different user/org (replaces the saved one)
ghpending add --all          # pick from every repo your token can reach (private included)
ghpending        # print the digest
ghpending --author <login>                  # only items authored by this user
ghpending --review-requested <login>        # only PRs awaiting this user
ghpending list   # show tracked repos
ghpending rm     # remove repos from the list
```

- `ghpending add` — lists repos and lets you select which to track. The username is saved so subsequent `add` runs skip the prompt. Pass `--user <name>` to switch to a different user/org without editing the config; it replaces the saved one.
  - **Private repos:** with a `GITHUB_TOKEN` that has the `repo` scope, `add` includes private repos automatically when the target is your own account or an org you belong to. For a third-party user only their public repos are visible.
  - `--all` lists every repo your token can reach — owned, collaborator and organization-member, private included — in a single picker, ignoring the saved user. Use it to grab private repos you collaborate on across different owners.
- `ghpending` — fetches all tracked repos concurrently and prints a digest of open issues and pull requests.
- `ghpending list` — prints the repos currently in your watch list.
- `ghpending rm` — opens an interactive menu to select repos to remove from tracking.

## Authentication (optional)

Everything works unauthenticated for public repos, subject to GitHub's default 60 requests/hour rate limit. Set `GITHUB_TOKEN` to raise that to 5,000 requests/hour:

```sh
GITHUB_TOKEN=$(gh auth token) ghpending
```

The token is read silently at startup — no configuration needed. To track **private** repos (and have them show up in `ghpending add`), the token needs the `repo` scope (classic) or read access to the repo's Contents, Issues and Pull requests (fine-grained).

### GitHub API proxy (optional)

If a SOCKS proxy is already listening at `127.0.0.1:9050`, `ghpending` uses it for GitHub API calls and falls back to direct API access when it is not available. You can also force a SOCKS proxy with `GHPENDING_GITHUB_PROXY=socks5h://host:port`; existing `HTTPS_PROXY` / `ALL_PROXY` values are honored when they use `socks5` or `socks5h`.

## Config

The global config file lives at:

- Linux: `~/.config/ghpending/config.toml`
- macOS: `~/Library/Application Support/ghpending/config.toml`

Example:

```toml
user = "akitaonrails"
repos = ["ratatui-org/ratatui", "tokio-rs/tokio"]
```

Run `ghpending add --user <name>` to change the `user` field, or edit the file directly to reorder repos.

### Per-project config

Drop a `.ghpending.toml` in a project (same format as above) to watch a
different repo set while you're inside that directory. ghpending walks up from
the current directory to the git root looking for it, so it works from any
subfolder. The local file **fully replaces** the global one — `add`/`rm` write
to whichever file is active, and a `using config <path>` note is printed to
stderr so you know which one is in effect.

Precedence (highest first): `--config <path>` flag, then the `GHPENDING_CONFIG`
environment variable, then the nearest `.ghpending.toml`, then the global config.
The flag and env var take a path directly and bypass the walk-up search — handy
for scripting or pointing at a throwaway config.

### Filtering the digest

Filter the digest to items authored by particular users or PRs currently
awaiting review. These are current review requests, not users who submitted a
review in the past.

```toml
[filters]
authors = ["alice"]
review_requested = ["bob", "team:my-org/backend"]
match = "any"
```

Values are case-insensitive. Multiple values within one role match any listed
value. `match = "any"` (the default) includes an item matching either enabled
role; `match = "all"` requires every enabled role. Issues can match authors but
not review requests. Team requests must be explicit as `team:ORG/SLUG` — a
request to a team is not treated as a request to every individual member.

For one-off filtering, repeat the CLI options as needed:

```sh
ghpending --author alice --author bob
ghpending --review-requested bob
ghpending --author alice --review-requested bob --match all
```

If either CLI role option is present, the CLI author and review-request lists
replace both configured role lists for that invocation. The configured matching
mode remains in effect unless `--match` overrides it.

### Review context

PR detail lines condense the current review state into segments such as:

```text
approved (2): alice, bob · awaiting review (2): carol, team:my-org/backend
1 unresolved by alice · awaiting review (1): bob
```

Human reviewers are grouped by their latest state, with approvals first. `N` is
the number of reviewers in a state group, but the number of current request
targets for `awaiting review`; a team counts as one target. A matching aggregate
GitHub decision is omitted, as is `review required` when current requests already
show who or which team is awaited. A neutral `commented` state is hidden only
when the same login already appears in `unresolved`.

Review states and unresolved threads are best-effort GraphQL enrichment. Current
review requests come from REST and still render if enrichment fails.

## Themes

Pass `--theme nerv` on the command line (any subcommand) or set `theme = "nerv"` in the config file to switch to the NERV interface palette. The older purple-accent Evangelion palette is still available as `evangelion`.

```toml
user = "akitaonrails"
repos = ["ratatui-org/ratatui"]
theme = "nerv"
```

The environment variables `GHPENDING_THEME` (specific) and `TCLOCK_WIDGET_THEME` (generic, set by [tclock](https://github.com/akitaonrails/clock-tui) for its widget subprocesses) are also honored, so running ghpending as a tclock widget follows the clock's theme cycling automatically.

Precedence: `--theme` flag, then `GHPENDING_THEME`, then `TCLOCK_WIDGET_THEME`, then the config file, then `default`. An unknown name in an env var is skipped with a warning; an unknown name in the flag is an error.

## License

MIT
