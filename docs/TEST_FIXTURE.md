# Loupe test fixture

Permanent draft pull requests in this repository that Loupe can be tested against end to end, writes included. They hold generic sample code (a "pantry planner": recipes, stock, a small web client), never real data. **Never merge, close, or retarget them.**

## Pull requests

| PR | Base ← head | What it covers |
|---|---|---|
| [#186](https://github.com/IsraelAraujo70/loupe/pull/186) | `loupe-fixture/base` ← `loupe-fixture/stack-1` | Stack 1/3. The rich diff, the seeded comments and threads, the Linear ticket, the checks. **The only pull request a test may write to.** |
| [#187](https://github.com/IsraelAraujo70/loupe/pull/187) | `loupe-fixture/stack-1` ← `loupe-fixture/stack-2` | Stack 2/3: metric conversion (Rust and TSX), a new test file. |
| [#188](https://github.com/IsraelAraujo70/loupe/pull/188) | `loupe-fixture/stack-2` ← `loupe-fixture/stack-3` | Stack 3/3: shopping list export; deletes `web/src/api/client.test.ts` with no replacement, for the Guide's removed tests. |
| [#189](https://github.com/IsraelAraujo70/loupe/pull/189) | `loupe-fixture/base` ← `loupe-fixture/large-diff` | 26 generated CSV files (78k lines): GitHub leaves 16 diffs out of the file list (`huge.csv` alone, then `part-10` to `part-24`), so Files shows Load diff and Load all. |
| [#190](https://github.com/IsraelAraujo70/loupe/pull/190) | `loupe-fixture/base` ← `loupe-fixture/many-commits` | 260 one-line commits: more than the 250 GitHub lists for a pull request, so the commit list pages through the compare API. |

Every title starts with `[Loupe test fixture] não mergear` and every one carries the `loupe-fixture` label. #186 → #187 → #188 is a chain of branches (each base is the head of the one below), so Reviews folds it into one stack, the pull request view shows the stack and "Merge stack", and the Guide generates one chapter per pull request. It is not a native GitHub stack (`PullRequest.stack` is null).

## What #186 holds

- **Files**: added, removed (`src/pantry/legacy_import.rs`), renamed without edits (`src/recipe/parse.rs` → `parser.rs`, no patch), renamed with edits (`docs/guide/getting-started.md` → `quickstart.md`), binary added and modified (`assets/badge.png`, `assets/icon.png`), a mode change only (`scripts/seed.sh`), an empty file (`web/src/features/planner/.gitkeep`), and a generated file (`src/generated/units_table.rs`, marked `linguist-generated` in `.gitattributes`).
- **Long file**: `src/catalog/ingredients.rs`, about 3,000 lines with edits near lines 60, 1,100, and 2,900 and a new function at the end, for expanding context and searching (`Sumac`, `Saffron, whole threads`).
- **Word-level edits**: `src/units.rs` (`28.35` → `28.349_523`, `US` → `metric`, `"pc"` → `"pcs"`), `web/src/api/client.ts` (`id` → `recipeId`), `RecipeCard.css`, `README.md`.
- **Tree**: nested folders (`web/src/features/planner/week/day/`) and look-alike siblings (`stock_report.rs` and `stock_reports.rs`, `MealSlot.tsx` and `MealSlots.tsx`, `docs/guide/` and `docs/guides/`), for the file tree and "mark folder as viewed".
- **Conversation**: one comment, one submitted COMMENT review, an **open** thread on `src/units.rs` lines 19–20 with a suggestion block, and a **resolved** thread with a reply on `src/pantry/stock_report.rs` line 9. Seeded comments say "Fixture seed".
- **Linear**: the body mentions `BACK-293`, the "[Loupe test]" ticket, which the side column finds by identifier. No Linear URL, so the workspace stays out of a public repository.
- **Commits**: five, for the commit picker and the per-commit view.

## Checks

`.github/workflows/loupe-fixture.yml` lives only on the fixture branches and runs only on `pull_request` events whose base matches `loupe-fixture/**`. Each run gives `passes` (success, with a notice and a warning annotation), `fails` (failure, with an error annotation), `skipped` (`if: false`), and `status`, which posts the commit status `loupe-fixture/status` with a description. The fixture branches share no history with `main` and hold neither `ci.yml` nor `release.yml`, so neither CI nor Release ever runs on them. A run costs four jobs of a few seconds, and only when a fixture branch is pushed.

## Protection

The repository ruleset **Loupe test fixture** covers `refs/heads/loupe-fixture/**` with no bypass: no deletion, no force push, and no update. GitHub already refuses to merge a draft ("Pull Request is still a draft"), and a merge into a fixture branch is an update of that branch, which the ruleset refuses too (a push is refused with "Cannot update this protected ref"), so a pull request marked ready by mistake still cannot land. Even so, stop "Merge stack" at its confirmation. To change the fixture on purpose, set the ruleset to Disabled in Settings → Rules, push, and enable it again.

## What a test may write

GitHub never lets the author of a pull request approve it or request changes on it, and the fixture is opened by IsraelAraujo70, the account Loupe is signed in with on the crew's machines. **Approve and Request changes can only be tested with a mock or a second account**: against #186 they fail with "Can not approve your own pull request" and "Can not request changes on your own pull request". Comment reviews, conversation comments, threads, replies, suggestions, resolve and unresolve, viewed marks, and draft/ready all work for real.

Rules for writing:

1. Write only to #186 on GitHub and to BACK-293 on Linear. #187, #188, #189, and #190 are read-only.
2. Start every comment, reply, and review body a test writes with `🤖 Loupe test:`.
3. Never merge, close, reopen, retarget, or push to a fixture branch. "Merge stack" goes as far as the confirmation and no further.
4. Put back what you change in the same run: draft state, title, body, labels, reviewers, viewed marks, and the state of the two seeded threads.
5. When a test is done writing, run `scripts/loupe-fixture-reset.sh 186`.

## Cleaning up

`scripts/loupe-fixture-reset.sh [PR...]` (all five by default) deletes the comments that start with `🤖 Loupe test:`, discards your pending review, removes your viewed marks, converts a pull request back to draft, and reopens or resolves the two seeded threads. Submitted reviews cannot be deleted on GitHub; their comments go and an empty review stays in the timeline. The script refuses any pull request whose title is not a fixture's. Title, body, labels, and reviewers are not reset: put them back yourself.

## Live tests that use it

Run with `GITHUB_TOKEN=$(gh auth token) cargo test -p loupe-github --test <file> -- --ignored --nocapture` (on the VPS, build on Boat and run the test binary here). The numbers live in `crates/loupe-github/tests/fixture/mod.rs`.

| Test | Reads or writes |
|---|---|
| `live_pull_request::loads_the_review_fixture` | Reads #186: draft, branches, BACK-293, seeded threads, every kind of file. |
| `live_pull_request::reads_the_fixture_checks` | Reads #186's checks and commit status. |
| `live_pull_request::the_fixture_stack_chains_three_pull_requests` | Reads the stack graph from #187. |
| `live_pull_request::lists_every_commit_of_the_fixture_past_250` | Reads #190's 260 commits. |
| `live_rest::loads_the_diffs_github_leaves_out_of_the_large_fixture` | Reads #189 and loads one left-out diff. |
| `live_rest::polling_the_checks_of_an_unchanged_commit_answers_unchanged` | Polls #186's head checks. |
| `live_e2e_fixture` | **Writes** to #186 (`LOUPE_E2E=1`): a comment, two threads, a COMMENT review, a reply, a resolve, a viewed mark. |
| `live_viewed_batch` | **Writes** to #186 (`LOUPE_E2E=1`): toggles `README.md`'s viewed mark and restores it. |

`live_merge` still uses #1 and #16, which are closed and merged pull requests, not the fixture.
