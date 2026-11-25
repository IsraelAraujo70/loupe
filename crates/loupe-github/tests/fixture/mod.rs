//! The Loupe test fixture: permanent draft pull requests of this repository,
//! described in docs/TEST_FIXTURE.md. Each test binary uses part of it.
#![allow(dead_code)]

use loupe_github::{PrRef, RepoRef};

/// The bottom of the stack, with the rich diff, the seeded threads, and the
/// checks. The one pull request a test may write to.
pub const STACK_1: u64 = 186;
pub const STACK_2: u64 = 187;
pub const STACK_3: u64 = 188;
/// GitHub leaves most of its files' diffs out of the file list.
pub const LARGE_DIFF: u64 = 189;
/// More commits than the 250 GitHub lists for a pull request.
pub const MANY_COMMITS: u64 = 190;
pub const MANY_COMMITS_TOTAL: u32 = 260;

/// The branch under the stack and the two separate pull requests.
pub const BASE_BRANCH: &str = "loupe-fixture/base";
pub const STACK_1_BRANCH: &str = "loupe-fixture/stack-1";

/// Every comment a test writes starts with this, so
/// `scripts/loupe-fixture-reset.sh` can find and delete it.
pub const WRITE_PREFIX: &str = "🤖 Loupe test:";

pub fn pr(number: u64) -> PrRef {
    PrRef::new(RepoRef::new("IsraelAraujo70", "loupe"), number)
}
