//! What kind of change a file is, so a review reads the code first.
//!
//! Every changed file is code, a test, documentation, or low signal: a file
//! a reviewer skims or skips (a binary, a lockfile, generated or vendored
//! code, a minified bundle, a data file, or a new file of more than a
//! thousand lines). The files tab orders them code, tests, docs, and then the
//! low-signal group, and opens the low-signal files folded under a label that
//! says what they are and how large. A file with no lines to read joins
//! them, named for what changed: a mode change or an empty file.

use loupe_github::{ChangedFile, FileStatus};

/// A new file with more added lines than this is low signal.
pub const LARGE_NEW_FILE: u32 = 1_000;
/// An added line this long in a text asset means it was minified.
const MINIFIED_LINE: usize = 1_000;
/// How many of a file's first lines may carry a "generated" marker.
const HEADER_LINES: usize = 5;

/// What kind of change a file is, in the order a review reads them.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Signal {
    Code,
    Test,
    Docs,
    Low(LowSignal),
}

/// Why a file is low signal.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum LowSignal {
    Binary,
    /// A file GitHub lists as modified with no lines and no patch: only its
    /// mode changed, as a script made executable.
    ModeChange,
    /// A file added or deleted with no lines, as a `.gitkeep`.
    Empty,
    Lockfile,
    Generated,
    Vendored,
    Minified,
    Data,
    LargeNew,
}

impl Signal {
    pub fn is_low(self) -> bool {
        matches!(self, Signal::Low(_))
    }

    /// Where the kind sorts: code first, low signal last.
    pub fn rank(self) -> u8 {
        match self {
            Signal::Code => 0,
            Signal::Test => 1,
            Signal::Docs => 2,
            Signal::Low(_) => 3,
        }
    }
}

impl LowSignal {
    pub fn name(self) -> &'static str {
        match self {
            LowSignal::Binary => "Binary",
            LowSignal::ModeChange => "Mode change",
            LowSignal::Empty => "Empty file",
            LowSignal::Lockfile => "Lockfile",
            LowSignal::Generated => "Generated",
            LowSignal::Vendored => "Vendored",
            LowSignal::Minified => "Minified",
            LowSignal::Data => "Data",
            LowSignal::LargeNew => "Large new file",
        }
    }

    /// What the folded file's label says: the kind and the lines it
    /// changes, as "Generated · 2,001 lines". A binary file has no lines.
    pub fn label(self, file: &ChangedFile) -> String {
        let lines = file.additions + file.deletions;
        match self {
            LowSignal::Binary => "Binary file".to_owned(),
            LowSignal::ModeChange | LowSignal::Empty => self.name().to_owned(),
            _ if lines == 1 => format!("{} · 1 line", self.name()),
            _ => format!("{} · {} lines", self.name(), thousands(lines)),
        }
    }
}

/// `n` with a comma between each group of three digits: 78,029.
pub fn thousands(n: u32) -> String {
    let digits = n.to_string();
    let mut out = String::with_capacity(digits.len() + digits.len() / 3);
    for (ix, digit) in digits.chars().enumerate() {
        if ix > 0 && (digits.len() - ix).is_multiple_of(3) {
            out.push(',');
        }
        out.push(digit);
    }
    out
}

/// The kind of every file, in the same order.
pub fn classify_all(files: &[ChangedFile], attributes: &Attributes) -> Vec<Signal> {
    files
        .iter()
        .map(|file| classify(file, attributes))
        .collect()
}

/// What kind of change `file` is. `attributes` are the repository's
/// `.gitattributes`, whose `linguist-generated` and `linguist-vendored`
/// decide whether a file is generated or vendored, either way, over the
/// guess from its path or first lines. Binaries, files with no lines, and
/// lockfiles are told before them, and a file they leave out can still be
/// minified, data, or a large new file.
pub fn classify(file: &ChangedFile, attributes: &Attributes) -> Signal {
    let path = file.path.as_str();
    let name = path.rsplit('/').next().unwrap_or(path);
    let lower = name.to_lowercase();
    let extension = extension(&lower);
    if BINARY.contains(&extension) {
        return Signal::Low(LowSignal::Binary);
    }
    let moved = matches!(file.status, FileStatus::Renamed | FileStatus::Copied);
    if file.patch.is_none() && file.additions + file.deletions == 0 && !moved {
        return Signal::Low(match file.status {
            FileStatus::Added | FileStatus::Removed => LowSignal::Empty,
            _ => LowSignal::ModeChange,
        });
    }
    if LOCKFILES.contains(&name) || extension == "lock" {
        return Signal::Low(LowSignal::Lockfile);
    }
    let marked = attributes.lookup(path);
    match marked.vendored {
        Some(true) => return Signal::Low(LowSignal::Vendored),
        None if vendored_path(path) => return Signal::Low(LowSignal::Vendored),
        _ => {}
    }
    match marked.generated {
        Some(true) => return Signal::Low(LowSignal::Generated),
        None if generated_path(path, &lower) || generated_header(file) => {
            return Signal::Low(LowSignal::Generated);
        }
        _ => {}
    }
    if minified(&lower, extension, file) {
        return Signal::Low(LowSignal::Minified);
    }
    if DATA.contains(&extension) {
        return Signal::Low(LowSignal::Data);
    }
    if file.status == FileStatus::Added && file.deletions == 0 && file.additions > LARGE_NEW_FILE {
        return Signal::Low(LowSignal::LargeNew);
    }
    if test_path(path, &lower) {
        return Signal::Test;
    }
    if docs_path(path, &lower, extension) {
        return Signal::Docs;
    }
    Signal::Code
}

/// The extension of a lowercase file name, without the dot; empty for none
/// and for a dotfile such as `.gitignore`.
fn extension(lower: &str) -> &str {
    match lower.rfind('.') {
        Some(dot) if dot > 0 => &lower[dot + 1..],
        _ => "",
    }
}

const BINARY: &[&str] = &[
    "png", "jpg", "jpeg", "gif", "bmp", "ico", "icns", "webp", "avif", "tif", "tiff", "psd",
    "heic", "ttf", "otf", "woff", "woff2", "eot", "mp3", "mp4", "m4a", "mov", "avi", "mkv", "webm",
    "wav", "ogg", "flac", "pdf", "zip", "gz", "tgz", "bz2", "xz", "zst", "7z", "rar", "jar", "war",
    "class", "so", "dylib", "dll", "exe", "bin", "o", "a", "wasm", "pyc", "db", "sqlite",
    "sqlite3", "parquet", "avro", "xls", "xlsx", "doc", "docx", "ppt", "pptx", "lockb", "keystore",
    "jks", "p12", "der",
];

const LOCKFILES: &[&str] = &[
    "Cargo.lock",
    "package-lock.json",
    "npm-shrinkwrap.json",
    "yarn.lock",
    "pnpm-lock.yaml",
    "bun.lock",
    "Gemfile.lock",
    "poetry.lock",
    "Pipfile.lock",
    "uv.lock",
    "pdm.lock",
    "composer.lock",
    "go.sum",
    "go.work.sum",
    "flake.lock",
    "mix.lock",
    "Podfile.lock",
    "Package.resolved",
    "pubspec.lock",
    "gradle.lockfile",
    "packages.lock.json",
    "deno.lock",
    "MODULE.bazel.lock",
];

const DATA: &[&str] = &[
    "csv", "tsv", "psv", "jsonl", "ndjson", "geojson", "topojson", "arff",
];

/// Folders whose files are someone else's code checked in.
const VENDORED: &[&str] = &["vendor", "node_modules", "bower_components", "Pods"];

fn segments(path: &str) -> impl Iterator<Item = &str> {
    let folders = path.rsplit_once('/').map_or("", |(folders, _)| folders);
    folders.split('/').filter(|segment| !segment.is_empty())
}

fn vendored_path(path: &str) -> bool {
    segments(path).any(|segment| VENDORED.contains(&segment))
}

/// File names and folders that code generators write, as GitHub Linguist
/// recognizes them.
fn generated_path(path: &str, lower: &str) -> bool {
    const SUFFIXES: &[&str] = &[
        ".pb.go",
        ".pb.cc",
        ".pb.h",
        ".pb.swift",
        "_pb2.py",
        "_pb2_grpc.py",
        ".g.dart",
        ".freezed.dart",
        ".designer.cs",
        ".snap",
        ".js.map",
        ".css.map",
    ];
    SUFFIXES.iter().any(|suffix| lower.ends_with(suffix))
        || lower.contains(".generated.")
        || segments(path).any(|segment| segment == "__generated__" || segment == "__snapshots__")
}

/// Whether the file's first lines, when its patch shows them, say a tool
/// wrote it: `@generated`, Go's "Code generated … DO NOT EDIT.", or
/// "auto-generated".
fn generated_header(file: &ChangedFile) -> bool {
    head_lines(file).any(|line| {
        let lower = line.to_lowercase();
        line.contains("@generated")
            || line.contains("DO NOT EDIT")
            || lower.contains("auto-generated")
            || lower.contains("autogenerated")
            || lower.contains("automatically generated")
    })
}

/// The first lines of the file's new side, when its patch starts at the top
/// of the file; nothing otherwise.
fn head_lines(file: &ChangedFile) -> impl Iterator<Item = &str> {
    let patch = file.patch.as_deref().unwrap_or("");
    let mut lines = patch.lines();
    let starts_at_top = lines
        .next()
        .and_then(|header| header.split(" +").nth(1))
        .and_then(|new| new.split([',', ' ']).next())
        .is_some_and(|start| start == "1");
    lines
        .filter(move |_| starts_at_top)
        .filter(|line| !line.starts_with('-') && !line.starts_with('\\'))
        .take(HEADER_LINES)
        .map(|line| line.get(1..).unwrap_or(""))
}

/// A `.min.` name, or a script, a style sheet, or markup with an added line
/// too long for a person to have written.
fn minified(lower: &str, extension: &str, file: &ChangedFile) -> bool {
    if lower.contains(".min.") {
        return true;
    }
    const ASSETS: &[&str] = &["js", "mjs", "cjs", "css", "html", "svg", "json"];
    ASSETS.contains(&extension)
        && file.patch.as_deref().is_some_and(|patch| {
            patch
                .lines()
                .any(|line| line.starts_with('+') && line.len() > MINIFIED_LINE)
        })
}

fn test_path(path: &str, lower: &str) -> bool {
    const FOLDERS: &[&str] = &[
        "test",
        "tests",
        "__tests__",
        "spec",
        "specs",
        "e2e",
        "testdata",
        "fixtures",
        "__mocks__",
    ];
    if segments(path).any(|segment| FOLDERS.contains(&segment.to_lowercase().as_str())) {
        return true;
    }
    let stem = lower.rsplit_once('.').map_or(lower, |(stem, _)| stem);
    let name = path.rsplit('/').next().unwrap_or(path);
    let name_stem = name.rsplit_once('.').map_or(name, |(stem, _)| stem);
    lower.contains(".test.")
        || lower.contains(".spec.")
        || stem.ends_with("_test")
        || stem.ends_with("_spec")
        || stem.starts_with("test_")
        || lower == "conftest.py"
        || name_stem.ends_with("Test")
        || name_stem.ends_with("Tests")
}

fn docs_path(path: &str, lower: &str, extension: &str) -> bool {
    const EXTENSIONS: &[&str] = &["md", "mdx", "markdown", "rst", "adoc", "asciidoc"];
    const NAMES: &[&str] = &[
        "readme",
        "changelog",
        "changes",
        "contributing",
        "license",
        "licence",
        "notice",
        "authors",
        "code_of_conduct",
        "security",
    ];
    let stem = lower.split('.').next().unwrap_or(lower);
    let in_docs = segments(path)
        .next()
        .is_some_and(|top| matches!(top.to_lowercase().as_str(), "docs" | "doc"));
    EXTENSIONS.contains(&extension) || NAMES.contains(&stem) || in_docs
}

/// The `linguist-generated` and `linguist-vendored` lines of a repository's
/// root `.gitattributes`, in file order.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Attributes {
    rules: Vec<Rule>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct Rule {
    pattern: String,
    generated: Option<bool>,
    vendored: Option<bool>,
}

/// What `.gitattributes` says of one path; `None` where it says nothing.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Marked {
    pub generated: Option<bool>,
    pub vendored: Option<bool>,
}

impl Attributes {
    pub fn parse(text: &str) -> Self {
        let rules = text
            .lines()
            .filter_map(|line| {
                let line = line.trim();
                if line.is_empty() || line.starts_with('#') {
                    return None;
                }
                let mut words = line.split_whitespace();
                let pattern = words.next()?.trim_start_matches('/').to_owned();
                let mut rule = Rule {
                    pattern,
                    generated: None,
                    vendored: None,
                };
                for word in words {
                    let (negated, word) = match word.strip_prefix(['-', '!']) {
                        Some(rest) => (true, rest),
                        None => (false, word),
                    };
                    let (key, value) = match word.split_once('=') {
                        Some((key, value)) => (key, !matches!(value, "false" | "0")),
                        None => (word, !negated),
                    };
                    match key {
                        "linguist-generated" => rule.generated = Some(value),
                        "linguist-vendored" => rule.vendored = Some(value),
                        _ => {}
                    }
                }
                (rule.generated.is_some() || rule.vendored.is_some()).then_some(rule)
            })
            .collect();
        Attributes { rules }
    }

    /// What the last matching line says of each attribute, as Git reads it.
    pub fn lookup(&self, path: &str) -> Marked {
        let mut marked = Marked::default();
        for rule in self
            .rules
            .iter()
            .filter(|rule| matches(&rule.pattern, path))
        {
            marked.generated = rule.generated.or(marked.generated);
            marked.vendored = rule.vendored.or(marked.vendored);
        }
        marked
    }
}

/// Whether a `.gitattributes` pattern matches `path`. A pattern with no
/// slash matches the file name at any depth; one with a slash matches from
/// the root, where `**` crosses folders and `*` and `?` do not. A pattern
/// that names a folder (`dist/**`) matches everything under it.
fn matches(pattern: &str, path: &str) -> bool {
    if !pattern.contains('/') {
        let name = path.rsplit('/').next().unwrap_or(path);
        return glob(pattern.as_bytes(), name.as_bytes());
    }
    glob(pattern.as_bytes(), path.as_bytes())
}

fn glob(pattern: &[u8], text: &[u8]) -> bool {
    match pattern {
        [] => text.is_empty(),
        [b'*', b'*', b'/', rest @ ..] => {
            glob(rest, text)
                || text
                    .iter()
                    .enumerate()
                    .any(|(ix, &byte)| byte == b'/' && glob(rest, &text[ix + 1..]))
        }
        [b'*', b'*', rest @ ..] => (0..=text.len()).any(|ix| glob(rest, &text[ix..])),
        [b'*', rest @ ..] => {
            let segment = text
                .iter()
                .position(|&byte| byte == b'/')
                .unwrap_or(text.len());
            (0..=segment).any(|ix| glob(rest, &text[ix..]))
        }
        [b'?', rest @ ..] => {
            text.first().is_some_and(|&byte| byte != b'/') && glob(rest, &text[1..])
        }
        [byte, rest @ ..] => text.first() == Some(byte) && glob(rest, &text[1..]),
    }
}

#[cfg(test)]
mod tests {
    use loupe_github::{ChangedFile, FileStatus, ViewedState};

    use super::{Attributes, LowSignal, Marked, Signal, classify, thousands};

    fn file(path: &str, status: FileStatus, additions: u32, patch: Option<&str>) -> ChangedFile {
        ChangedFile {
            path: path.to_owned(),
            previous_path: None,
            status,
            additions,
            deletions: 0,
            patch: patch.map(str::to_owned),
            viewed: ViewedState::Unviewed,
        }
    }

    fn kind(path: &str) -> Signal {
        classify(
            &file(
                path,
                FileStatus::Modified,
                3,
                Some("@@ -10,3 +10,3 @@\n a\n-b\n+c"),
            ),
            &Attributes::default(),
        )
    }

    fn low(kind: LowSignal) -> Signal {
        Signal::Low(kind)
    }

    #[test]
    fn code_tests_and_docs_sort_in_that_order_before_low_signal() {
        assert_eq!(kind("src/recipe/scale.rs"), Signal::Code);
        assert_eq!(kind("Cargo.toml"), Signal::Code);
        assert_eq!(kind("scripts/seed.sh"), Signal::Code);
        assert_eq!(kind("requirements.txt"), Signal::Code);
        assert_eq!(
            kind("web/src/components/IngredientList.test.tsx"),
            Signal::Test
        );
        assert_eq!(kind("web/src/api/client.spec.ts"), Signal::Test);
        assert_eq!(kind("crates/app/tests/live.rs"), Signal::Test);
        assert_eq!(kind("pkg/http/client_test.go"), Signal::Test);
        assert_eq!(kind("tests/test_scale.py"), Signal::Test);
        assert_eq!(kind("src/test/java/ScaleTest.java"), Signal::Test);
        assert_eq!(kind("Sources/AppTests/ScaleTests.swift"), Signal::Test);
        assert_eq!(kind("README.md"), Signal::Docs);
        assert_eq!(kind("docs/guide/quickstart.md"), Signal::Docs);
        assert_eq!(kind("LICENSE"), Signal::Docs);
        assert_eq!(kind("CHANGELOG"), Signal::Docs);
        assert!(Signal::Code < Signal::Test && Signal::Test < Signal::Docs);
        assert!(Signal::Docs < low(LowSignal::Binary));
        assert_eq!(low(LowSignal::Data).rank(), 3);
    }

    #[test]
    fn binaries_lockfiles_and_data_are_low_signal_by_name() {
        assert_eq!(kind("assets/badge.png"), low(LowSignal::Binary));
        assert_eq!(kind("assets/Icon.PNG"), low(LowSignal::Binary));
        assert_eq!(kind("fonts/Inter.woff2"), low(LowSignal::Binary));
        assert_eq!(kind("Cargo.lock"), low(LowSignal::Lockfile));
        assert_eq!(kind("web/pnpm-lock.yaml"), low(LowSignal::Lockfile));
        assert_eq!(kind("web/package-lock.json"), low(LowSignal::Lockfile));
        assert_eq!(kind("go.sum"), low(LowSignal::Lockfile));
        assert_eq!(kind("deps/custom.lock"), low(LowSignal::Lockfile));
        assert_eq!(kind("data/nutrition/part-01.csv"), low(LowSignal::Data));
        assert_eq!(kind("events.ndjson"), low(LowSignal::Data));
        let attributes = Attributes::default();
        let bare =
            |path: &str, status: FileStatus| classify(&file(path, status, 0, None), &attributes);
        assert_eq!(
            bare("scripts/seed.sh", FileStatus::Modified),
            low(LowSignal::ModeChange)
        );
        assert_eq!(
            bare("web/.gitkeep", FileStatus::Added),
            low(LowSignal::Empty)
        );
        assert_eq!(
            bare("tools/blob", FileStatus::Removed),
            low(LowSignal::Empty)
        );
        // A rename with no edits keeps its place beside the code.
        assert_eq!(
            bare("src/recipe/parser.rs", FileStatus::Renamed),
            Signal::Code
        );
        assert_eq!(
            LowSignal::ModeChange.label(&file("a.sh", FileStatus::Modified, 0, None)),
            "Mode change"
        );
        assert_eq!(
            LowSignal::Empty.label(&file("web/.gitkeep", FileStatus::Added, 0, None)),
            "Empty file"
        );
        // A package manifest beside its lockfile is code.
        assert_eq!(kind("web/package.json"), Signal::Code);
    }

    #[test]
    fn generated_code_is_told_by_its_name_or_its_first_lines() {
        assert_eq!(kind("api/v1/user.pb.go"), low(LowSignal::Generated));
        assert_eq!(kind("proto/user_pb2.py"), low(LowSignal::Generated));
        assert_eq!(
            kind("src/__generated__/schema.ts"),
            low(LowSignal::Generated)
        );
        assert_eq!(
            kind("src/__snapshots__/card.test.tsx.snap"),
            low(LowSignal::Generated)
        );
        assert_eq!(kind("dist/app.js.map"), low(LowSignal::Generated));
        let attributes = Attributes::default();
        let headed = |patch: &str| {
            classify(
                &file("src/units_table.rs", FileStatus::Modified, 2, Some(patch)),
                &attributes,
            )
        };
        assert_eq!(
            headed("@@ -1,3 +1,3 @@\n // @generated by build.rs\n-a\n+b"),
            low(LowSignal::Generated)
        );
        assert_eq!(
            headed(
                "@@ -0,0 +1,2 @@\n+// Code generated by protoc-gen-go. DO NOT EDIT.\n+package user"
            ),
            low(LowSignal::Generated)
        );
        assert_eq!(
            headed("@@ -1 +1,2 @@\n-x\n+# This file is auto-generated\n+y"),
            low(LowSignal::Generated)
        );
        // The marker counts only at the top of the file.
        assert_eq!(
            headed("@@ -40,3 +40,3 @@\n // @generated\n-a\n+b"),
            Signal::Code
        );
        assert_eq!(
            headed("@@ -1,9 +1,9 @@\n a\n b\n c\n d\n e\n f\n+// @generated"),
            Signal::Code
        );
    }

    #[test]
    fn gitattributes_mark_generated_and_vendored_files_either_way() {
        let attributes = Attributes::parse(
            "# Linguist\n\
             src/generated/** linguist-generated\n\
             *.gen.ts linguist-generated=true\n\
             /third_party/** linguist-vendored\n\
             vendor/keep/** -linguist-vendored\n\
             src/generated/hand_written.rs linguist-generated=false\n\
             *.rs text eol=lf\n",
        );
        let kind = |path: &str| classify(&file(path, FileStatus::Modified, 7, None), &attributes);
        assert_eq!(
            kind("src/generated/units_table.rs"),
            low(LowSignal::Generated)
        );
        assert_eq!(
            kind("src/generated/deep/more.rs"),
            low(LowSignal::Generated)
        );
        assert_eq!(kind("web/src/api/types.gen.ts"), low(LowSignal::Generated));
        assert_eq!(kind("third_party/zlib/inflate.c"), low(LowSignal::Vendored));
        assert_eq!(kind("vendor/github.com/x/y.go"), low(LowSignal::Vendored));
        assert_eq!(kind("vendor/keep/patched.go"), Signal::Code);
        assert_eq!(kind("src/generated/hand_written.rs"), Signal::Code);
        assert_eq!(kind("src/units.rs"), Signal::Code);
        assert_eq!(
            attributes.lookup("src/lib.rs"),
            Marked {
                generated: None,
                vendored: None
            }
        );
        // A file the attributes call generated is generated whatever its
        // name says otherwise, and one they clear is not, even with a header.
        let cleared = Attributes::parse("*.pb.go -linguist-generated");
        assert_eq!(
            classify(
                &file("api/user.pb.go", FileStatus::Modified, 1, None),
                &cleared
            ),
            Signal::Code
        );
    }

    #[test]
    fn minified_and_large_new_files_are_low_signal() {
        let attributes = Attributes::default();
        assert_eq!(kind("web/vendor.min.js"), low(LowSignal::Minified));
        assert_eq!(kind("web/vendor/jquery.min.js"), low(LowSignal::Vendored));
        assert_eq!(kind("static/app.min.css"), low(LowSignal::Minified));
        let long = format!("@@ -0,0 +1 @@\n+{}", "a=1;".repeat(400));
        let bundle = file("static/bundle.js", FileStatus::Added, 1, Some(&long));
        assert_eq!(classify(&bundle, &attributes), low(LowSignal::Minified));
        // A long line in source code is not minification.
        let source = file("src/table.rs", FileStatus::Added, 1, Some(&long));
        assert_eq!(classify(&source, &attributes), Signal::Code);
        let new = |additions: u32| {
            classify(
                &file("src/big.rs", FileStatus::Added, additions, None),
                &attributes,
            )
        };
        assert_eq!(new(1_001), low(LowSignal::LargeNew));
        assert_eq!(new(1_000), Signal::Code);
        let mut grown = file("src/big.rs", FileStatus::Modified, 5_000, None);
        assert_eq!(classify(&grown, &attributes), Signal::Code);
        grown.status = FileStatus::Added;
        grown.deletions = 1;
        assert_eq!(classify(&grown, &attributes), Signal::Code);
    }

    #[test]
    fn labels_name_the_kind_and_the_size() {
        let csv = file("data/part-01.csv", FileStatus::Added, 2_001, None);
        assert_eq!(LowSignal::Generated.label(&csv), "Generated · 2,001 lines");
        assert_eq!(LowSignal::Data.label(&csv), "Data · 2,001 lines");
        let png = file("assets/badge.png", FileStatus::Added, 0, None);
        assert_eq!(LowSignal::Binary.label(&png), "Binary file");
        let one = file("Cargo.lock", FileStatus::Modified, 1, None);
        assert_eq!(LowSignal::Lockfile.label(&one), "Lockfile · 1 line");
        assert_eq!(thousands(0), "0");
        assert_eq!(thousands(999), "999");
        assert_eq!(thousands(1_000), "1,000");
        assert_eq!(thousands(78_029), "78,029");
        assert_eq!(thousands(1_234_567), "1,234,567");
    }
}
