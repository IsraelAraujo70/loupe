# Agent notes for Loupe

Read [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md) first: crates, fixed contracts, write sets, and gates. For any UI work, read [docs/DESIGN.md](docs/DESIGN.md) and the GPUI Kit Design Guides it points to; project skills live in `.claude/skills/` (see its SOURCE.md).

- Code, identifiers, commit messages, and UI strings are English.
- `loupe-app` links GPUI: on Linux install its system libraries first (the list is in `.github/workflows/release.yml`). The macOS build and the `.app` bundle need a Mac with Xcode and the Metal toolchain.
- GPUI is `gpui-kit = "=0.9.0"` (re-exports `gpui`, `gpui_component` as `gpui_kit::component`). Read component sources under `~/.cargo/registry/src/*/gpui-component-0.9.0/src` before guessing an API. `use gpui_kit::*` also imports GPUI's `test` attribute: in test modules import items by name instead of `use super::*`.
- Comments only for a non-obvious why. JSDoc-style `///` on public items whose behavior is not obvious.
- No hardcoded product data or fixtures in the app. Tests may use fixtures.
- Never print, log, or commit a token.
