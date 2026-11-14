//! What a run's graph is drawn from: the jobs of a workflow file, what each
//! one needs, and which of them fan out into a matrix or call another
//! workflow; and which job of the file each job of a run came from.
//!
//! The REST API lists a run's jobs by display name only, without the job's
//! key or its `needs`, so the file is read at the run's commit. Only the
//! `jobs` map is read, and only the keys the graph needs, so this is a small
//! reader of indented YAML rather than a YAML parser.

use serde::{Deserialize, Serialize};

/// The jobs of a workflow file, in file order.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkflowSpec {
    pub jobs: Vec<JobSpec>,
}

/// One entry of a workflow's `jobs` map.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct JobSpec {
    /// The job's key, such as `test_server`.
    pub key: String,
    /// Its `name`, which may hold `${{ }}` expressions.
    pub name: Option<String>,
    /// The keys of the jobs it needs.
    pub needs: Vec<String>,
    /// It has a `strategy.matrix`: it runs as several jobs.
    pub matrix: bool,
    /// It calls another workflow (`uses`): its jobs are named
    /// `<name> / <job of the called workflow>`.
    pub reusable: bool,
}

impl JobSpec {
    /// What GitHub names the job before expanding it.
    pub fn display_name(&self) -> &str {
        self.name.as_deref().unwrap_or(&self.key)
    }
}

/// Reads the `jobs` map of a workflow file. What it cannot read is left out:
/// a file it cannot make sense of gives no jobs, and the run is drawn
/// without edges.
pub fn parse_workflow(text: &str) -> WorkflowSpec {
    let lines: Vec<(usize, String)> = text
        .lines()
        .map(|raw| {
            let content = strip_comment(raw.trim_end());
            let indent = content.len() - content.trim_start().len();
            (indent, content.trim().to_owned())
        })
        .filter(|(_, content)| !content.is_empty())
        .collect();
    let Some(start) = lines.iter().position(|(indent, content)| {
        *indent == 0 && key_of(content).is_some_and(|(key, _)| key == "jobs")
    }) else {
        return WorkflowSpec::default();
    };
    let body: Vec<&(usize, String)> = lines[start + 1..]
        .iter()
        .take_while(|(indent, _)| *indent > 0)
        .collect();
    let Some(job_indent) = body.first().map(|(indent, _)| *indent) else {
        return WorkflowSpec::default();
    };
    let mut jobs: Vec<JobSpec> = Vec::new();
    let mut property_indent: Option<usize> = None;
    // The job property the deeper lines belong to.
    let mut property: Option<String> = None;
    // An open `needs: [` that runs over several lines.
    let mut open_list: Option<String> = None;
    for (indent, content) in body {
        let (indent, content) = (*indent, content.as_str());
        if let Some(list) = open_list.as_mut() {
            list.push(' ');
            list.push_str(content);
            if content.contains(']') {
                let list = open_list.take().unwrap_or_default();
                if let Some(job) = jobs.last_mut() {
                    job.needs = flow_list(&list);
                }
            }
            continue;
        }
        if indent == job_indent {
            let Some((key, _)) = key_of(content) else {
                continue;
            };
            jobs.push(JobSpec {
                key,
                name: None,
                needs: Vec::new(),
                matrix: false,
                reusable: false,
            });
            property_indent = None;
            property = None;
            continue;
        }
        let Some(job) = jobs.last_mut() else {
            continue;
        };
        let at = *property_indent.get_or_insert(indent);
        if indent == at {
            // `needs:` then `- job` at the key's own indent, which YAML allows.
            if property.as_deref() == Some("needs")
                && let Some(item) = content.strip_prefix("- ")
            {
                job.needs.push(unquote(item));
                continue;
            }
            let Some((key, value)) = key_of(content) else {
                property = None;
                continue;
            };
            match key.as_str() {
                "name" if !value.is_empty() => job.name = Some(unquote(&value)),
                "needs" if value.starts_with('[') => {
                    open_list = needs_list(job, &value);
                }
                "needs" if !value.is_empty() => job.needs = vec![unquote(&value)],
                "uses" => job.reusable = true,
                _ => {}
            }
            property = Some(key);
        } else if indent > at {
            match property.as_deref() {
                // `needs:` alone, then a block list or a flow list on the
                // lines under it.
                Some("needs") if content.starts_with('[') => {
                    open_list = needs_list(job, content);
                }
                Some("needs") => job.needs.extend(content.strip_prefix("- ").map(unquote)),
                Some("strategy") if key_of(content).is_some_and(|(key, _)| key == "matrix") => {
                    job.matrix = true;
                }
                _ => {}
            }
        }
    }
    WorkflowSpec { jobs }
}

/// Sets `job.needs` from a flow list that closes on this line, or returns
/// the open list to keep reading on the next lines.
fn needs_list(job: &mut JobSpec, value: &str) -> Option<String> {
    if value.contains(']') {
        job.needs = flow_list(value);
        None
    } else {
        Some(value.to_owned())
    }
}

/// `key: value` with the key unquoted and the value trimmed; `None` for a
/// line that is not a mapping entry (a list item, a bare scalar).
fn key_of(content: &str) -> Option<(String, String)> {
    if content.starts_with('-') {
        return None;
    }
    let (key, value) = match content.chars().next() {
        Some(quote @ ('"' | '\'')) => {
            let end = content[1..].find(quote)? + 1;
            let rest = content[end + 1..].trim_start().strip_prefix(':')?;
            (content[1..end].to_owned(), rest)
        }
        _ => {
            let colon = content
                .find(": ")
                .or_else(|| content.strip_suffix(':').map(str::len))?;
            (content[..colon].to_owned(), &content[colon + 1..])
        }
    };
    (!key.is_empty()).then(|| (key, value.trim().to_owned()))
}

fn unquote(value: &str) -> String {
    let value = value.trim();
    for quote in ['"', '\''] {
        if let Some(inner) = value
            .strip_prefix(quote)
            .and_then(|value| value.strip_suffix(quote))
        {
            return inner.to_owned();
        }
    }
    value.to_owned()
}

/// `[a, "b", c]` as its items.
fn flow_list(value: &str) -> Vec<String> {
    let inner = value
        .trim()
        .trim_start_matches('[')
        .split(']')
        .next()
        .unwrap_or_default();
    inner
        .split(',')
        .map(unquote)
        .filter(|item| !item.is_empty())
        .collect()
}

/// The line without a trailing `# comment` (a `#` at the start or after a
/// space, outside quotes).
fn strip_comment(line: &str) -> &str {
    let mut quote: Option<char> = None;
    let mut previous = ' ';
    for (ix, c) in line.char_indices() {
        match quote {
            Some(open) if c == open => quote = None,
            Some(_) => {}
            // A quote opens only where a scalar starts: `CLI's` is text.
            None if (c == '"' || c == '\'') && matches!(previous, ' ' | ':' | '[' | ',' | '-') => {
                quote = Some(c)
            }
            None if c == '#' && previous.is_whitespace() => return &line[..ix],
            None => {}
        }
        previous = c;
    }
    line
}

/// How well a job's name fits a job of the file.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
enum Fit {
    /// Through a `${{ }}` expression, which can stand for anything; the
    /// template with more literal text around its expressions fits better.
    Pattern(usize),
    /// Its name plus a matrix's `(values)` or a called workflow's `/ job`.
    Expanded,
    Exact,
}

/// For each job of a run, by name, the index in `spec.jobs` of the job it
/// ran from, or `None` when no job of the file fits. An exact name wins over
/// an expanded one (`Build (linux)`, `Deploy / verify`), which wins over one
/// that fits only through an expression; ties go to the file's order.
pub fn assign_jobs(spec: &WorkflowSpec, names: &[&str]) -> Vec<Option<usize>> {
    names
        .iter()
        .map(|name| {
            spec.jobs
                .iter()
                .enumerate()
                .filter_map(|(ix, job)| fit(job, name).map(|fit| (fit, ix)))
                .max_by(|(a, a_ix), (b, b_ix)| a.cmp(b).then(b_ix.cmp(a_ix)))
                .map(|(_, ix)| ix)
        })
        .collect()
}

/// What a job of a called workflow is called inside its caller's group:
/// `Deploy / verify` is `verify`.
pub fn called_job_name<'a>(job: &JobSpec, name: &'a str) -> &'a str {
    if job.reusable {
        name.split_once(" / ").map_or(name, |(_, inner)| inner)
    } else {
        name
    }
}

/// Stands for any text in a name shape, like an expression does.
const ANY: &str = "${{}}";

fn fit(job: &JobSpec, name: &str) -> Option<Fit> {
    let template = job.display_name();
    if name == template {
        return Some(Fit::Exact);
    }
    let has_expression = template.contains("${{");
    let mut shapes = vec![template.to_owned()];
    if job.matrix {
        shapes.push(format!("{template} ({ANY})"));
    }
    if job.reusable {
        let callers = shapes.clone();
        shapes.extend(
            callers
                .into_iter()
                .map(|caller| format!("{caller} / {ANY}")),
        );
    }
    let shape = shapes.iter().position(|shape| glob(shape, name))?;
    Some(if has_expression {
        Fit::Pattern(literal_length(template))
    } else if shape == 0 {
        Fit::Exact
    } else {
        Fit::Expanded
    })
}

/// The length of `template` without its `${{ … }}` expressions.
fn literal_length(template: &str) -> usize {
    let mut length = 0;
    let mut rest = template;
    while let Some(open) = rest.find("${{") {
        length += open;
        rest = rest[open..]
            .find("}}")
            .map_or("", |close| &rest[open + close + 2..]);
    }
    length + rest.len()
}

/// Whether `name` fits `template`, where each `${{ … }}` stands for any text.
fn glob(template: &str, name: &str) -> bool {
    let mut parts: Vec<&str> = Vec::new();
    let mut rest = template;
    while let Some(open) = rest.find("${{") {
        parts.push(&rest[..open]);
        rest = match rest[open..].find("}}") {
            Some(close) => &rest[open + close + 2..],
            None => "",
        };
    }
    parts.push(rest);
    if parts.len() == 1 {
        return template == name;
    }
    let (first, last) = (parts[0], parts[parts.len() - 1]);
    if name.len() < first.len() + last.len() || !name.starts_with(first) || !name.ends_with(last) {
        return false;
    }
    let mut middle = &name[first.len()..name.len() - last.len()];
    for part in &parts[1..parts.len() - 1] {
        match middle.find(part) {
            Some(at) => middle = &middle[at + part.len()..],
            None => return false,
        }
    }
    true
}

#[cfg(test)]
mod tests {
    use super::{WorkflowSpec, assign_jobs, called_job_name, parse_workflow};

    const RELEASE: &str = r#"
name: Release
on:
  push:
    branches: [main]
jobs:
  # The first job.
  version:
    runs-on: ubuntu-24.04
    outputs:
      version: ${{ steps.version.outputs.version }}
    steps:
      - uses: actions/checkout@v4
      - run: |
          needs: not-a-key
          name: not-a-name
  linux:
    needs: version
    runs-on: ubuntu-24.04
  "macos":
    name: "Build macOS" # signed
    needs: [ version ]
  test_server:
    name: Release server tests ${{ matrix.shard }}
    needs: [linux,
      macos]
    strategy:
      fail-fast: false
      matrix:
        shard: [1, 2, 3]
  desktop:
    name: Desktop Linux arm64
    needs:
      - linux
      - 'macos'
    uses: ./.github/workflows/release-desktop.yml
  publish:
    if: github.event_name != 'pull_request'
    needs: [version, linux, macos, test_server, desktop]
    strategy:
      max-parallel: 1
permissions:
  contents: read
"#;

    fn spec() -> WorkflowSpec {
        parse_workflow(RELEASE)
    }

    #[test]
    fn reads_each_job_with_its_needs() {
        let spec = spec();
        let keys: Vec<&str> = spec.jobs.iter().map(|job| job.key.as_str()).collect();
        assert_eq!(
            keys,
            [
                "version",
                "linux",
                "macos",
                "test_server",
                "desktop",
                "publish"
            ]
        );
        let needs: Vec<Vec<&str>> = spec
            .jobs
            .iter()
            .map(|job| job.needs.iter().map(String::as_str).collect())
            .collect();
        assert_eq!(
            needs,
            [
                vec![],
                vec!["version"],
                vec!["version"],
                vec!["linux", "macos"],
                vec!["linux", "macos"],
                vec!["version", "linux", "macos", "test_server", "desktop"],
            ]
        );
    }

    #[test]
    fn reads_names_matrices_and_called_workflows() {
        let spec = spec();
        assert_eq!(spec.jobs[0].name, None);
        assert_eq!(spec.jobs[2].name.as_deref(), Some("Build macOS"));
        assert_eq!(
            spec.jobs[3].name.as_deref(),
            Some("Release server tests ${{ matrix.shard }}")
        );
        let matrices: Vec<bool> = spec.jobs.iter().map(|job| job.matrix).collect();
        assert_eq!(matrices, [false, false, false, true, false, false]);
        let called: Vec<bool> = spec.jobs.iter().map(|job| job.reusable).collect();
        assert_eq!(called, [false, false, false, false, true, false]);
    }

    #[test]
    fn a_flow_list_can_start_under_needs() {
        // pingdotgg/t3code's release.yml writes its fan-in like this.
        let spec = parse_workflow(
            "jobs:\n  release:\n    name: Publish GitHub Release\n    needs:\n      [\n        preflight,\n        test_server,\n      ]\n    if: always()\n  finalize:\n    needs: [release]\n",
        );
        assert_eq!(spec.jobs[0].needs, ["preflight", "test_server"]);
        assert_eq!(spec.jobs[1].needs, ["release"]);
    }

    #[test]
    fn a_block_list_can_sit_at_the_key_indent() {
        let spec = parse_workflow(
            "jobs:\n  test:\n    runs-on: x\n  deploy:\n    needs:\n    - test\n    - 'lint'\n    runs-on: x\n",
        );
        assert_eq!(spec.jobs[1].needs, ["test", "lint"]);
    }

    #[test]
    fn an_apostrophe_in_a_plain_name_is_text_and_the_comment_goes() {
        let spec = parse_workflow("jobs:\n  check:\n    name: Check the CLI's output  # flaky\n");
        assert_eq!(spec.jobs[0].name.as_deref(), Some("Check the CLI's output"));
    }

    #[test]
    fn the_more_specific_template_wins_over_a_bare_expression() {
        let spec = parse_workflow(
            "jobs:\n  build:\n    name: ${{ matrix.target }}\n    strategy:\n      matrix:\n        target: [linux]\n  test:\n    name: Test ${{ matrix.os }}\n    needs: build\n    strategy:\n      matrix:\n        os: [a]\n",
        );
        assert_eq!(assign_jobs(&spec, &["linux", "Test a"]), [Some(0), Some(1)]);
    }

    #[test]
    fn a_file_without_jobs_has_none() {
        assert!(parse_workflow("name: x\non: push\n").jobs.is_empty());
        assert!(parse_workflow("").jobs.is_empty());
    }

    #[test]
    fn each_job_of_a_run_goes_back_to_its_job_in_the_file() {
        let spec = spec();
        let names = [
            "version",
            "linux",
            "Build macOS",
            "Release server tests 1",
            "Release server tests 2",
            "Release server tests ${{ matrix.shard }}",
            "Desktop Linux arm64 / Build Linux arm64",
            "Desktop Linux arm64 / Verify",
            "Desktop Linux arm64",
            "publish",
            "something else",
        ];
        let keys: Vec<Option<&str>> = assign_jobs(&spec, &names)
            .into_iter()
            .map(|ix| ix.map(|ix| spec.jobs[ix].key.as_str()))
            .collect();
        assert_eq!(
            keys,
            [
                Some("version"),
                Some("linux"),
                Some("macos"),
                Some("test_server"),
                Some("test_server"),
                Some("test_server"),
                Some("desktop"),
                Some("desktop"),
                Some("desktop"),
                Some("publish"),
                None,
            ]
        );
        assert_eq!(
            called_job_name(&spec.jobs[4], "Desktop Linux arm64 / Verify"),
            "Verify"
        );
        assert_eq!(
            called_job_name(&spec.jobs[3], "Release server tests 1"),
            "Release server tests 1"
        );
    }

    #[test]
    fn a_matrix_without_a_name_adds_its_values_and_an_exact_name_wins() {
        let spec = parse_workflow(
            "jobs:\n  build:\n    strategy:\n      matrix:\n        os: [a, b]\n  build (a):\n    needs: build\n",
        );
        let assigned = assign_jobs(&spec, &["build (b)", "build (a)", "build"]);
        assert_eq!(assigned, [Some(0), Some(1), Some(0)]);
    }
}
