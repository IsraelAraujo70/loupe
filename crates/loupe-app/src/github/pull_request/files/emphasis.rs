//! Word emphasis inside changed lines.
//!
//! gpui-component pairs the n-th removed line of a change with the n-th added
//! one and marks every word that differs once a pair shares a quarter of its
//! tokens, spaces and punctuation included, so unrelated lines light up
//! almost whole. The files tab turns that off (`with_inline_unit(None)`) and
//! pairs each removed line with the most similar added line soon after it,
//! marking words only when the two share at least half of them; any other
//! changed line keeps just its tone. The marks reach the component through
//! its syntax highlighter, the one hook that paints ranges inside a line. It
//! hands over one side of one hunk at a time, without saying which, so the
//! marks are found by that side's lines.

use std::collections::HashMap;
use std::hash::{DefaultHasher, Hash, Hasher};
use std::ops::Range;
use std::sync::{Arc, Mutex, OnceLock, PoisonError, RwLock};

use gpui_kit::component::highlighter::HighlightTheme;
use gpui_kit::{HighlightStyle, Hsla};
use loupe_github::diff::{Hunk, LineKind};
use similar::{ChangeTag, TextDiff};

use crate::github::highlight::highlight_hunk;

/// Paired lines must share at least this proportion of their words for the
/// words that differ to be marked. Punctuation counts only in a line without
/// words: two unrelated calls share most of theirs.
const MIN_SHARED: f32 = 0.5;
/// How many added lines after the last pair a removed line is compared with.
const LOOKAHEAD: usize = 8;
/// Longer lines, in bytes, are not compared: the component's own limit.
const MAX_LINE: usize = 1000;
/// The language of a file without a grammar. The component hands a file to
/// the highlighter only when its language is not `text`, and the highlighter
/// skips syntax for this one.
pub const PLAIN: &str = "plain";

/// The marks of the hunks the diff shows, replaced whenever they change.
pub type SharedEmphasis = Arc<RwLock<Arc<Emphasis>>>;

/// Syntax colors of one theme already worked out, by language and hunk side,
/// so a new highlighter (one for a new text to find) paints every file again
/// without parsing it again.
pub type SyntaxCache = Arc<Mutex<HashMap<u64, Arc<[(Range<usize>, HighlightStyle)]>>>>;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Side {
    Old,
    New,
}

impl Side {
    fn other(self) -> Self {
        match self {
            Side::Old => Side::New,
            Side::New => Side::Old,
        }
    }
}

/// The hunks the diff shows, indexed by the lines of each side on first use,
/// which happens on the background executor.
#[derive(Default)]
pub struct Emphasis {
    files: Vec<Arc<Vec<Hunk>>>,
    index: OnceLock<Index>,
}

/// The sides of every hunk, by the hash of their lines.
type Index = HashMap<u64, Vec<(Side, Arc<HunkMarks>)>>;

/// One hunk, with the marks of its lines worked out on first use.
struct HunkMarks {
    hunks: Arc<Vec<Hunk>>,
    ix: usize,
    marks: OnceLock<[Vec<Vec<Range<usize>>>; 2]>,
}

impl HunkMarks {
    fn lines(&self, side: Side) -> impl Iterator<Item = &str> {
        side_lines(&self.hunks[self.ix], side)
    }

    /// Byte ranges to mark in each line of `side`, in order.
    fn marks(&self, side: Side) -> &[Vec<Range<usize>>] {
        &self.marks.get_or_init(|| hunk_marks(&self.hunks[self.ix]))[side as usize]
    }
}

impl Emphasis {
    /// The marks of `files`, each the hunks of one file as the diff shows them.
    pub fn new(files: impl IntoIterator<Item = Arc<Vec<Hunk>>>) -> Self {
        Self {
            files: files.into_iter().collect(),
            index: OnceLock::new(),
        }
    }

    /// The side and line marks of the hunk side `text` is, as the component
    /// hands it over: that side's lines, each ending in a newline but maybe
    /// the last. Text that matches hunks marked differently gets none.
    fn find(&self, text: &str) -> Option<(Side, &[Vec<Range<usize>>])> {
        let lines = || {
            text.split_terminator('\n')
                .map(|line| line.strip_suffix('\r').unwrap_or(line))
        };
        let index = self.index.get_or_init(|| {
            let mut index = Index::new();
            for hunks in &self.files {
                for ix in 0..hunks.len() {
                    let hunk = Arc::new(HunkMarks {
                        hunks: hunks.clone(),
                        ix,
                        marks: OnceLock::new(),
                    });
                    for side in [Side::Old, Side::New] {
                        index
                            .entry(lines_hash(hunk.lines(side)))
                            .or_default()
                            .push((side, hunk.clone()));
                    }
                }
            }
            index
        });
        let mut found = index
            .get(&lines_hash(lines()))?
            .iter()
            .filter(|(side, hunk)| hunk.lines(*side).eq(lines()));
        let (side, hunk) = found.next()?;
        let other = side.other();
        if found.any(|(also, again)| also != side || !again.lines(other).eq(hunk.lines(other))) {
            return None;
        }
        Some((*side, hunk.marks(*side)))
    }
}

/// The diff's highlighter: syntax colors from `theme`, kept in `syntax`,
/// and `added` or `removed` behind the words that changed in a paired line.
pub fn highlighter(
    emphasis: SharedEmphasis,
    syntax: SyntaxCache,
    theme: Arc<HighlightTheme>,
    added: Hsla,
    removed: Hsla,
) -> impl Fn(&str, &str) -> Vec<(Range<usize>, HighlightStyle)> + Send + Sync + 'static {
    move |language, text| {
        let mut styles = if language == PLAIN {
            Vec::new()
        } else {
            let mut hasher = DefaultHasher::new();
            (language, text).hash(&mut hasher);
            let key = hasher.finish();
            let cached = syntax
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .get(&key)
                .cloned();
            let colors = cached.unwrap_or_else(|| {
                let colors: Arc<[_]> = highlight_hunk(language, text, &theme).into();
                syntax
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner)
                    .insert(key, colors.clone());
                colors
            });
            colors.to_vec()
        };
        let emphasis = emphasis
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .clone();
        if let Some((side, marks)) = emphasis.find(text) {
            let background_color = Some(match side {
                Side::Old => removed,
                Side::New => added,
            });
            let mut start = 0;
            for (line, ranges) in text.split_inclusive('\n').zip(marks) {
                styles.extend(ranges.iter().map(|range| {
                    (
                        start + range.start..start + range.end,
                        HighlightStyle {
                            background_color,
                            ..Default::default()
                        },
                    )
                }));
                start += line.len();
            }
        }
        styles
    }
}

fn side_lines(hunk: &Hunk, side: Side) -> impl Iterator<Item = &str> {
    let skipped = match side {
        Side::Old => LineKind::Added,
        Side::New => LineKind::Removed,
    };
    hunk.lines
        .iter()
        .filter(move |line| line.kind != skipped)
        .map(|line| line.text.as_str())
}

fn lines_hash<'a>(lines: impl Iterator<Item = &'a str>) -> u64 {
    let mut hasher = DefaultHasher::new();
    for line in lines {
        line.hash(&mut hasher);
    }
    hasher.finish()
}

/// Marks for each line of each side of `hunk`: the changed words of every
/// removed line paired with an added one in the same run of changes.
fn hunk_marks(hunk: &Hunk) -> [Vec<Vec<Range<usize>>>; 2] {
    let mut marks =
        [Side::Old, Side::New].map(|side| vec![Vec::new(); side_lines(hunk, side).count()]);
    let (mut removed, mut added) = (Vec::new(), Vec::new());
    let (mut old_ix, mut new_ix) = (0, 0);
    for line in &hunk.lines {
        match line.kind {
            LineKind::Context => {
                pair_run(&removed, &added, &mut marks);
                removed.clear();
                added.clear();
                old_ix += 1;
                new_ix += 1;
            }
            LineKind::Removed => {
                removed.push((old_ix, line.text.as_str()));
                old_ix += 1;
            }
            LineKind::Added => {
                added.push((new_ix, line.text.as_str()));
                new_ix += 1;
            }
        }
    }
    pair_run(&removed, &added, &mut marks);
    marks
}

/// Pairs each removed line with the most similar of the next added lines
/// after the last pair, keeping both in order.
fn pair_run(
    removed: &[(usize, &str)],
    added: &[(usize, &str)],
    marks: &mut [Vec<Vec<Range<usize>>>; 2],
) {
    let mut next = 0;
    for &(old_ix, old) in removed {
        let best = added
            .iter()
            .enumerate()
            .skip(next)
            .take(LOOKAHEAD)
            .filter_map(|(at, &(new_ix, new))| {
                changed_words(old, new).map(|words| (at, new_ix, words))
            })
            .max_by(|a, b| a.2.shared.total_cmp(&b.2.shared).then(b.0.cmp(&a.0)));
        if let Some((at, new_ix, words)) = best {
            marks[Side::Old as usize][old_ix] = words.old;
            marks[Side::New as usize][new_ix] = words.new;
            next = at + 1;
        }
    }
}

struct Words {
    /// The proportion of words the two lines share.
    shared: f32,
    old: Vec<Range<usize>>,
    new: Vec<Range<usize>>,
}

/// The words that differ between two lines, as byte ranges on each side,
/// with changed words separated only by spaces marked as one run. `None`
/// when the lines share too few words to be the same line edited.
fn changed_words(old: &str, new: &str) -> Option<Words> {
    if old == new || old.len() > MAX_LINE || new.len() > MAX_LINE {
        return None;
    }
    let diff = TextDiff::from_unicode_words(old, new);
    // Shared and total tokens, counting words and, apart, all but spaces.
    let (mut words_shared, mut words_total) = (0, 0);
    let (mut tokens_shared, mut tokens_total) = (0, 0);
    let mut words = Words {
        shared: 0.,
        old: Vec::new(),
        new: Vec::new(),
    };
    let (mut old_at, mut new_at) = (0, 0);
    for change in diff.iter_all_changes() {
        let value = change.value();
        let len = value.len();
        let token = usize::from(!value.trim().is_empty());
        let word = usize::from(value.chars().any(char::is_alphanumeric));
        match change.tag() {
            ChangeTag::Equal => {
                words_shared += 2 * word;
                words_total += 2 * word;
                tokens_shared += 2 * token;
                tokens_total += 2 * token;
                old_at += len;
                new_at += len;
            }
            ChangeTag::Delete => {
                words_total += word;
                tokens_total += token;
                if token > 0 {
                    push_run(&mut words.old, old_at..old_at + len, old);
                }
                old_at += len;
            }
            ChangeTag::Insert => {
                words_total += word;
                tokens_total += token;
                if token > 0 {
                    push_run(&mut words.new, new_at..new_at + len, new);
                }
                new_at += len;
            }
        }
    }
    let (shared, total) = if words_total > 0 {
        (words_shared, words_total)
    } else {
        (tokens_shared, tokens_total)
    };
    words.shared = shared as f32 / total.max(1) as f32;
    (total == 0 || words.shared >= MIN_SHARED).then_some(words)
}

/// Extends the last run over `range` when only spaces lie between them.
fn push_run(runs: &mut Vec<Range<usize>>, range: Range<usize>, text: &str) {
    if let Some(last) = runs.last_mut()
        && text[last.end..range.start].trim().is_empty()
    {
        last.end = range.end;
        return;
    }
    runs.push(range);
}

#[allow(clippy::single_range_in_vec_init)]
#[cfg(test)]
mod tests {
    use std::ops::Range;
    use std::sync::{Arc, RwLock};

    use gpui_kit::component::highlighter::HighlightTheme;
    use gpui_kit::{Hsla, green, red};
    use loupe_github::diff::parse_patch;

    use super::{Emphasis, PLAIN, SyntaxCache, changed_words, highlighter};

    const EDIT: &str = "@@ -1,3 +1,4 @@\n fn f() {\n-    let busy = crate::github::guide_service::is_generating(cx, &detail.pr);\n+    use crate::github::guide_service::{self, GuideScope};\n+    let busy = guide_service::is_generating(cx, &detail.pr);\n }\n";
    const OLD: &str = "fn f() {\n    let busy = crate::github::guide_service::is_generating(cx, &detail.pr);\n}\n";
    const NEW: &str = "fn f() {\n    use crate::github::guide_service::{self, GuideScope};\n    let busy = guide_service::is_generating(cx, &detail.pr);\n}\n";

    /// The ranges the highlighter marks in one side of a hunk, with their
    /// color, for diffs made of `patches`.
    fn marked(patches: &[&str], text: &str) -> Vec<(Range<usize>, Hsla)> {
        let files = patches
            .iter()
            .map(|patch| Arc::new(parse_patch(patch).unwrap()));
        let emphasis = Arc::new(RwLock::new(Arc::new(Emphasis::new(files))));
        let highlight = highlighter(
            emphasis,
            SyntaxCache::default(),
            HighlightTheme::default_dark(),
            green(),
            red(),
        );
        highlight(PLAIN, text)
            .into_iter()
            .map(|(range, style)| (range, style.background_color.unwrap()))
            .collect()
    }

    #[test]
    fn only_the_edited_words_are_marked() {
        let words = changed_words(
            "let busy = crate::github::guide_service::is_generating(cx, &detail.pr);",
            "let busy = guide_service::is_generating(cx, &detail.pr);",
        )
        .unwrap();
        assert_eq!(words.old, [11..26]);
        assert!(words.new.is_empty());
        let words =
            changed_words("let total = price * count;", "let total = cost * quantity;").unwrap();
        assert_eq!(
            (words.old, words.new),
            (vec![12..17, 20..25], vec![12..16, 19..27])
        );
        let words = changed_words("foo(a, b)", "foo(a, b, c)").unwrap();
        assert_eq!(words.new, [8..11]);
    }

    #[test]
    fn lines_that_share_few_words_are_not_marked() {
        // Most of their tokens are the same punctuation and path.
        assert!(
            changed_words(
                "use crate::github::guide_service::{self, GuideScope};",
                "let busy = crate::github::guide_service::is_generating(cx, &detail.pr);",
            )
            .is_none()
        );
        assert!(changed_words("let tooltip = if busy {", "} else if fresh {").is_none());
        assert!(changed_words("}", "});").is_some());
    }

    #[test]
    fn a_removed_line_pairs_with_its_edit_rather_than_the_line_beside_it() {
        // `crate::github::` on the second line of the old side.
        assert_eq!(marked(&[EDIT], OLD), [(24..39, red())]);
        // The added `use` line is new, and the edit only removed words.
        assert!(marked(&[EDIT], NEW).is_empty());
        let crlf = OLD.replace('\n', "\r\n");
        assert_eq!(marked(&[EDIT], &crlf), [(25..40, red())]);
    }

    #[test]
    fn a_hunk_side_marked_two_ways_gets_no_marks() {
        let other = EDIT.replace("+    let busy = guide_service", "+    let busy = service");
        assert!(marked(&[EDIT, &other], OLD).is_empty());
        // The same edit in two files marks both.
        assert_eq!(marked(&[EDIT, EDIT], OLD), [(24..39, red())]);
        assert!(marked(&[EDIT], "fn g() {\n}\n").is_empty());
    }
}
