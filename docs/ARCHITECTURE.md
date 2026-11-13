# Loupe architecture

Loupe is a native macOS client for reviewing GitHub pull requests next to the Linear issues they close and the T3 Code threads that wrote them. It is built with GPUI through `gpui-kit` (GPUI plus gpui-component).

## Crates

| Crate | Owns | Depends on GPUI |
|---|---|---|
| `loupe-github` | GitHub GraphQL/REST client, domain types, diff parsing | no |
| `loupe-linear` | Linear GraphQL client and domain types | no |
| `loupe-t3` | T3 Code server client | no |
| `loupe-app` | The GPUI app (binary `loupe`) | yes |

The API crates are blocking (`ureq`) and pure: they parse wire JSON into domain types at the boundary and never expose wire types. The app runs every call on a background thread with `services::background` and awaits it from `cx.spawn`.

## Contracts inside `loupe-app`

These signatures are fixed. A front changes its own bodies freely; a change to a signature below goes through the CTO.

- `route::Route` lists every screen. `shell::router::build` maps each variant to the front that owns it.
- `shell::navigate(route, window, cx)` and `shell::go_back(cx)` are the only way to change screens.
- `services::Services` (a GPUI global) holds `Option<Arc<Client>>` for GitHub, Linear, and T3. `None` means not connected. The shell builds and replaces it.
- `store::Settings` (a GPUI global) holds preferences; change it only with `store::update_settings`. Secrets go through `store::{load_secret, save_secret, delete_secret}`.
- `links::PrLinkContext` is what the PR view hands to `linear::pr_linked_issues` and `t3::pr_linked_threads`, which return the views for the PR view's side column.
- `github::palette_search` and `linear::palette_search` return `shell::PaletteItem`s for the command palette.
- Sign-in belongs to the shell: GitHub token discovery, OAuth device flow (two plain HTTP calls in `settings/`), PAT and Linear key entry, and the T3 server URL and token. The shell builds `Services` from them.
- `ui::avatar(url, size)` renders a remote avatar; the shell installs the HTTP client that loads remote images.
- `theme::apply` picks the palette Settings > Appearance asks for (a theme file's light or dark variant, `theme::themes`), maps it onto gpui-component's theme, and makes it the one `theme::tokens` reads, so a change repaints every window. `theme::tokens` holds colors the stock theme has no slot for. Views use `cx.theme()` first.

## Fronts and write sets

| Front | Writes |
|---|---|
| GitHub | `crates/loupe-github/**`, `crates/loupe-app/src/github/**`, `schemas/github.graphql` |
| Linear | `crates/loupe-linear/**`, `crates/loupe-app/src/linear/**`, `schemas/linear.graphql` |
| T3 | `crates/loupe-t3/**`, `crates/loupe-app/src/t3/**` |
| Shell | `crates/loupe-app/src/{main.rs,shell/**,settings/**,store.rs,services.rs,theme.rs,theme/**,ui.rs}`, `crates/loupe-app/themes/**`, `scripts/make-themes.py`, `scripts/bundle-macos.sh`, `assets/**`, `.github/**`, `README.md` |

`Cargo.toml` files: each front may add dependencies to its own crate's manifest and to `[workspace.dependencies]`; the CTO resolves conflicts when integrating.

## Building and running

macOS is the main target; Linux builds too. Build and run with `cargo run`, test with `cargo test --workspace`, using the pinned toolchain (`rust-toolchain.toml`, Rust 1.99.1). `LOUPE_DATA_DIR=<dir>` keeps settings and tokens in that directory instead of the system keychain, so a development run never touches your real data. `scripts/bundle-macos.sh` builds `dist/Loupe.app` on a Mac.

## Verification gates

- Every GraphQL document validates against the checked-in SDL in `schemas/` (offline test in each API crate).
- Read queries run live against the real API in an `#[ignore]` smoke test that reads the token from the environment.
- Mutations are validated live without side effects: GitHub with a bogus node id (the API reports "Could not resolve to a node" only after the document validates), Linear unauthenticated (the API reports `AUTHENTICATION_ERROR` only after the document validates).
- The merge goes through GitHub's asynchronous merge REST endpoint, the only one that merges stacked pull requests. It is checked live without side effects on an already merged and a closed pull request (`tests/live_merge.rs`).
- The GUI is checked by running the app and reading its accessibility tree and screenshots.
