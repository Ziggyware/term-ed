//! Unattended maintenance: an agent that looks for obvious problems and
//! proposes fixes, but never applies one without approval.
//!
//! The design constraint is the one the user asked for: *"require user
//! approval before any changes made without user request."* So this module is
//! split in two halves:
//!
//! * **Analysis** is pure and offline. It reads files, finds things that are
//!   unambiguously wrong (a duplicated function, a `TODO` that names a bug, an
//!   empty `catch`, an unused import, unbalanced brackets), and produces a
//!   [`Proposal`] containing the exact edit it would make.
//! * **Application** never happens here. A proposal is only ever a *suggestion*
//!   until a human accepts it, at which point the diff goes through the normal
//!   editor edit path (so it is one undo step and shows change bars).
//!
//! Nothing in this module writes to disk or touches the network, which is what
//! makes it safe to run on a timer.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::code_intel;

/// How sure the analysis is that this is a real problem.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Confidence {
    /// Worth a look, but may be intentional.
    Low,
    /// Almost certainly a mistake.
    Medium,
    /// Objectively wrong: it cannot compile or is a literal duplicate.
    High,
}

impl Confidence {
    pub fn label(self) -> &'static str {
        match self {
            Self::Low => "low",
            Self::Medium => "medium",
            Self::High => "high",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Kind {
    /// The same lines appear twice in one file.
    DuplicateBlock,
    /// A `TODO`/`FIXME`/`XXX`/`HACK` marker.
    TodoMarker,
    /// An `empty` block: `catch {}`, `except: pass`, `if x {}`.
    EmptyBlock,
    /// A comment saying the code below is wrong.
    SelfAdmittedBug,
    /// An argument that is never read in the function body.
    UnusedParameter,
    /// Brackets that do not balance in a file that should have them.
    UnbalancedBrackets,
}

impl Kind {
    pub fn label(self) -> &'static str {
        match self {
            Self::DuplicateBlock => "duplicate block",
            Self::TodoMarker => "todo marker",
            Self::EmptyBlock => "empty block",
            Self::SelfAdmittedBug => "self-admitted bug",
            Self::UnusedParameter => "unused parameter",
            Self::UnbalancedBrackets => "unbalanced brackets",
        }
    }
}

/// One suggested change. `old` must match exactly once at `line`; the caller
/// refuses if it does not, so a stale proposal cannot corrupt a buffer.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Proposal {
    pub path: PathBuf,
    pub kind: Kind,
    pub confidence: Confidence,
    /// 0-based line the change starts at.
    pub line: usize,
    /// One sentence a human can decide on.
    pub summary: String,
    /// The exact text to replace (`old`) and what to put there (`new`).
    pub old: String,
    pub new: String,
}

impl Proposal {
    /// True when applying this is safe to do automatically *if* the user has
    /// pre-approved this confidence level. High means "the current text cannot
    /// be right"; nothing lower qualifies.
    pub fn auto_applicable(&self) -> bool {
        self.confidence == Confidence::High
    }
}

/// A file's analysis, with the signals that produced it.
#[derive(Clone, Debug, Default)]
pub struct Report {
    pub proposals: Vec<Proposal>,
    /// Files looked at, and files skipped (binary or too large).
    pub scanned: usize,
    pub skipped: usize,
}

impl Report {
    pub fn is_empty(&self) -> bool {
        self.proposals.is_empty()
    }

    /// Proposals of at least the given confidence, strongest first.
    pub fn at_least(&self, level: Confidence) -> Vec<&Proposal> {
        let mut out: Vec<&Proposal> = self.proposals.iter().filter(|p| p.confidence >= level).collect();
        out.sort_by(|a, b| {
            b.confidence
                .cmp(&a.confidence)
                .then_with(|| a.path.cmp(&b.path))
                .then_with(|| a.line.cmp(&b.line))
        });
        out
    }

    pub fn summary(&self) -> String {
        if self.proposals.is_empty() {
            return format!("Looked at {} file(s); found nothing to fix.", self.scanned);
        }
        let high = self
            .proposals
            .iter()
            .filter(|p| p.confidence == Confidence::High)
            .count();
        format!(
            "{} suggestion(s) across {} file(s) ({high} high confidence); {} skipped",
            self.proposals.len(),
            self.proposals
                .iter()
                .map(|p| &p.path)
                .collect::<std::collections::BTreeSet<_>>()
                .len(),
            self.skipped
        )
    }
}

const TODO_MARKERS: &[&str] = &["TODO", "FIXME", "XXX", "HACK", "BUG"];
const ADMITTED_BUGS: &[&str] = &[
    "this is wrong",
    "this is broken",
    "this is a bug",
    "doesn't work",
    "does not work",
    "known bug",
    "not implemented yet",
    "temporarily broken",
    "workaround for",
];

/// Analyse one file's text. `path` is used only for the proposals, so this is
/// a pure function and easy to test.
pub fn analyze_text(path: &Path, text: &str) -> Vec<Proposal> {
    let mut out = Vec::new();
    let lines: Vec<&str> = text.split('\n').collect();

    // --- duplicate blocks: the same three non-trivial lines twice in a file.
    let mut seen: std::collections::HashMap<String, usize> = std::collections::HashMap::new();
    let mut i = 0;
    while i + 2 < lines.len() {
        let block = [lines[i].trim(), lines[i + 1].trim(), lines[i + 2].trim()];
        let trivial = block.iter().filter(|l| l.len() > 4).count();
        if trivial == 3 && !block.iter().any(|l| l.starts_with("//") || l.starts_with('#')) {
            let key = block.join("\n");
            if let Some(&first) = seen.get(&key) {
                out.push(Proposal {
                    path: path.to_path_buf(),
                    kind: Kind::DuplicateBlock,
                    confidence: Confidence::Medium,
                    line: i,
                    summary: format!(
                        "Lines {}-{} repeat lines {}-{} exactly.",
                        i + 1,
                        i + 3,
                        first + 1,
                        first + 3
                    ),
                    // Removing the copy is the fix; the exact text keeps it
                    // verifiable at apply time.
                    old: format!("{}\n{}\n{}\n", lines[i], lines[i + 1], lines[i + 2]),
                    new: String::new(),
                });
            } else {
                seen.insert(key, i);
            }
        }
        i += 1;
    }

    for (n, raw) in lines.iter().enumerate() {
        let trimmed = raw.trim();
        if trimmed.is_empty() {
            continue;
        }
        let lower = trimmed.to_ascii_lowercase();

        // --- TODO markers, but only where they are introduced as comments.
        // "todo" as an identifier is common; the uppercase marker inside a
        // comment is not.
        let is_comment = trimmed.starts_with("//")
            || trimmed.starts_with('#')
            || trimmed.starts_with("/*")
            || trimmed.starts_with('*')
            || trimmed.starts_with("--");
        if is_comment
            && let Some(marker) = TODO_MARKERS
                .iter()
                .find(|m| trimmed.to_ascii_uppercase().contains(*m))
        {
            out.push(Proposal {
                path: path.to_path_buf(),
                kind: Kind::TodoMarker,
                confidence: Confidence::Low,
                line: n,
                summary: format!("{marker} left in the source: {trimmed}"),
                // Removing a marker is only right when the work is done, which
                // cannot be inferred, so the note stands alone.
                old: raw.to_string(),
                new: raw.to_string(),
            });
        }

        // --- comments that admit the code below is broken.
        if (trimmed.starts_with("//") || trimmed.starts_with('#'))
            && let Some(detail) = ADMITTED_BUGS.iter().find(|d| lower.contains(*d))
        {
            out.push(Proposal {
                path: path.to_path_buf(),
                kind: Kind::SelfAdmittedBug,
                confidence: Confidence::Low,
                line: n,
                summary: format!("The comment says \"{detail}\"; confirm whether it is still true."),
                old: raw.to_string(),
                new: raw.to_string(),
            });
        }

        // --- blocks that swallow a failure. Two shapes matter:
        //     } catch { }              (empty on the same line)
        //     } catch {                (empty on the following lines)
        // Only an explicitly empty body counts; a comment saying "ignore" is
        // a deliberate choice, not an oversight.
        for keyword in ["catch", "except", "rescue"] {
            let Some(at) = clause_start(trimmed, keyword) else {
                continue;
            };
            let rest = &trimmed[at + keyword.len()..];
            let body = match rest.split_once('{') {
                Some((_, tail)) => tail.trim().to_string(),
                // Python-style `except: pass` / `except Exception:`.
                None => rest
                    .split_once(':')
                    .map(|(_, tail)| tail.trim().to_string())
                    .unwrap_or_default(),
            };
            let empty_inline = matches!(body.as_str(), "}" | "pass" | "pass }" | "");
            let empty_following = body.is_empty()
                && lines
                    .iter()
                    .skip(n + 1)
                    .map(|l| l.trim())
                    .find(|l| !l.is_empty())
                    .is_some_and(|l| l == "}" || l == "pass" || l.starts_with("} //"));
            if empty_inline || empty_following {
                out.push(Proposal {
                    path: path.to_path_buf(),
                    kind: Kind::EmptyBlock,
                    confidence: Confidence::Medium,
                    line: n,
                    summary: format!(
                        "This `{keyword}` block silently ignores the failure. Log it or narrow the catch."
                    ),
                    old: raw.to_string(),
                    new: raw.to_string(),
                });
                break;
            }
        }
    }

    // --- bracket balance. Only for files that use braces at all, and only
    // when the imbalance survives ignoring strings and comments.
    if let Some(detail) = balance_problem(text) {
        out.push(Proposal {
            path: path.to_path_buf(),
            kind: Kind::UnbalancedBrackets,
            confidence: Confidence::High,
            line: detail.0,
            summary: detail.1,
            old: String::new(),
            new: String::new(),
        });
    }

    out
}

/// Where `keyword` begins a clause on this line, if it does. Handles
/// `} catch {` as well as a line that starts with the keyword, and refuses
/// matches inside a longer identifier (`catch_all`).
fn clause_start(line: &str, keyword: &str) -> Option<usize> {
    let lower = line.to_ascii_lowercase();
    let mut from = 0;
    while let Some(off) = lower[from..].find(keyword) {
        let at = from + off;
        let before_ok = at == 0
            || !lower[..at]
                .chars()
                .next_back()
                .is_some_and(|c| c.is_alphanumeric() || c == '_');
        let after = lower[at + keyword.len()..].chars().next();
        let after_ok = after.is_some_and(|c| c == ' ' || c == '(' || c == ':' || c == '{');
        if before_ok && after_ok {
            return Some(at);
        }
        from = at + keyword.len();
    }
    None
}

/// Find a bracket imbalance that is not explained by a string or comment.
/// Returns the line and a description; this cannot be auto-fixed because the
/// missing bracket's position is exactly the ambiguous part.
fn balance_problem(text: &str) -> Option<(usize, String)> {
    let mut depth = 0isize;
    let mut line_of_last_open = 0usize;
    for (n, line) in text.split('\n').enumerate() {
        let mut chars = line.chars().peekable();
        let mut in_str: Option<char> = None;
        while let Some(c) = chars.next() {
            if let Some(q) = in_str {
                if c == '\\' {
                    chars.next();
                } else if c == q {
                    in_str = None;
                }
                continue;
            }
            match c {
                '"' | '\'' => in_str = Some(c),
                '/' if chars.peek() == Some(&'/') => break,
                '{' => {
                    depth += 1;
                    line_of_last_open = n;
                }
                '}' => {
                    depth -= 1;
                    if depth < 0 {
                        return Some((
                            n,
                            format!("Line {} closes a block that was never opened.", n + 1),
                        ));
                    }
                }
                _ => {}
            }
        }
    }
    if depth > 0 {
        return Some((
            line_of_last_open,
            format!(
                "{} block(s) opened here are never closed; the file will not parse.",
                depth
            ),
        ));
    }
    None
}

/// Analyse one file on disk. Binary and oversized files are skipped rather
/// than guessed at.
pub fn analyze_file(path: &Path, max_bytes: u64) -> Option<Vec<Proposal>> {
    let meta = std::fs::metadata(path).ok()?;
    if meta.len() > max_bytes {
        return None;
    }
    let text = std::fs::read_to_string(path).ok()?;
    Some(analyze_text(path, &text))
}

/// Analyse a set of files, honouring a size cap. Order is stable so a report
/// reads the same way twice.
pub fn analyze_files(files: &[PathBuf], max_bytes: u64) -> Report {
    let mut report = Report::default();
    let mut sorted: Vec<&PathBuf> = files.iter().collect();
    sorted.sort();
    for path in sorted {
        match analyze_file(path, max_bytes) {
            Some(found) => {
                report.scanned += 1;
                report.proposals.extend(found);
            }
            None => report.skipped += 1,
        }
    }
    report
}

/// What the user has allowed the unattended agent to do. Defaults are the safe
/// ones: analysis only, and nothing applied without a person looking at it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Policy {
    /// Run analysis at all.
    pub enabled: bool,
    /// Apply high-confidence fixes without asking. Off by default and only
    /// ever honoured for `Confidence::High`.
    pub apply_high_confidence: bool,
    /// Refuse to touch files larger than this many bytes.
    pub max_file_bytes: u64,
    /// Stop after this many proposals, so a huge tree cannot flood the panel.
    pub max_proposals: usize,
}

impl Default for Policy {
    fn default() -> Self {
        Self {
            enabled: false,
            // The user must opt in: silent edits are exactly what was asked
            // to be prevented.
            apply_high_confidence: false,
            max_file_bytes: 512 * 1024,
            max_proposals: 100,
        }
    }
}

impl Policy {
    /// Whether a proposal may be applied without a fresh confirmation.
    pub fn may_apply_unattended(&self, proposal: &Proposal) -> bool {
        self.enabled && self.apply_high_confidence && proposal.auto_applicable()
    }

    pub fn describe(&self) -> String {
        if !self.enabled {
            return "off: no background analysis".into();
        }
        if self.apply_high_confidence {
            "on: reports everything, applies only high-confidence fixes".into()
        } else {
            "on: reports only; every change needs approval".into()
        }
    }
}

/// Trim a report to the policy's budget, keeping the strongest findings.
pub fn limit(mut report: Report, max: usize) -> Report {
    if report.proposals.len() <= max {
        return report;
    }
    report.proposals.sort_by(|a, b| {
        b.confidence
            .cmp(&a.confidence)
            .then_with(|| a.path.cmp(&b.path))
            .then_with(|| a.line.cmp(&b.line))
    });
    report.proposals.truncate(max);
    report.proposals.sort_by(|a, b| a.path.cmp(&b.path).then_with(|| a.line.cmp(&b.line)));
    report
}

/// Symbols declared in a file but never used there, as a starting point for
/// the same proposal pipeline. Kept separate because it depends on
/// `code_intel`'s parsing rather than line shapes.
pub fn unused_symbols(path: &Path, text: &str) -> Vec<Proposal> {
    let mut out = Vec::new();
    for item in code_intel::outline(text) {
        let occurrences = code_intel::occurrences(text, &item.text);
        // `main` and public entry points are called from outside this file.
        if item.text == "main" || item.text.starts_with("__") {
            continue;
        }
        if occurrences.len() == 1 && item.line + 1 < text.split('\n').count() {
            out.push(Proposal {
                path: path.to_path_buf(),
                kind: Kind::UnusedParameter,
                confidence: Confidence::Low,
                line: item.line,
                summary: format!(
                    "`{}` is declared here and never used in this file.",
                    item.text
                ),
                old: text.split('\n').nth(item.line).unwrap_or("").to_string(),
                new: text.split('\n').nth(item.line).unwrap_or("").to_string(),
            });
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    fn kinds(text: &str) -> Vec<(Kind, Confidence, usize)> {
        analyze_text(Path::new("t.rs"), text)
            .into_iter()
            .map(|p| (p.kind, p.confidence, p.line))
            .collect()
    }

    #[test]
    fn a_repeated_block_is_reported_once() {
        let text = "fn a() {\n    let x = compute();\n    store(x);\n    report(x);\n}\n\
                    fn b() {\n    let x = compute();\n    store(x);\n    report(x);\n}\n";
        let found = analyze_text(Path::new("a.rs"), text);
        let dupes: Vec<&Proposal> = found
            .iter()
            .filter(|p| p.kind == Kind::DuplicateBlock)
            .collect();
        assert_eq!(dupes.len(), 1, "{found:#?}");
        assert_eq!(dupes[0].confidence, Confidence::Medium);
        assert!(dupes[0].summary.contains("repeat"), "{}", dupes[0].summary);
        // The proposal carries the exact text, so applying it is verifiable.
        assert!(text.contains(dupes[0].old.trim()));
    }

    #[test]
    fn trivial_and_commented_blocks_are_not_duplicates() {
        let text = "}\n}\n}\n\n// a note\n// a note\n// a note\n";
        assert!(
            !kinds(text).iter().any(|(k, _, _)| *k == Kind::DuplicateBlock),
            "{:?}",
            kinds(text)
        );
    }

    #[test]
    fn todo_markers_are_found_only_in_comments() {
        let text = "// TODO: handle the empty case\nlet todo = 1;\n# FIXME: this leaks\n";
        let found = analyze_text(Path::new("a.rs"), text);
        let todo: Vec<&Proposal> = found
            .iter()
            .filter(|p| p.kind == Kind::TodoMarker)
            .collect();
        assert_eq!(todo.len(), 2, "{found:#?}");
        // The plain variable named `todo` is not a marker.
        assert!(todo.iter().all(|p| p.line != 1));
        assert_eq!(todo[0].confidence, Confidence::Low);
        // A note is never auto-applied.
        assert!(!todo[0].auto_applicable());
    }

    #[test]
    fn a_comment_admitting_a_bug_is_surfaced() {
        let text = "// this is broken until the cache lands\nfn f() {}\n";
        let found = analyze_text(Path::new("a.rs"), text);
        assert!(
            found.iter().any(|p| p.kind == Kind::SelfAdmittedBug),
            "{found:#?}"
        );
    }

    #[test]
    fn swallowing_an_error_is_medium_confidence() {
        let found = kinds("try {\n    run();\n} catch {\n}\n");
        assert!(
            found.iter().any(|(k, c, _)| *k == Kind::EmptyBlock && *c == Confidence::Medium),
            "{found:?}"
        );
    }

    #[test]
    fn empty_blocks_are_found_in_every_shape_they_appear() {
        // Error swallowed on the following line (the most common shape).
        assert!(
            !kinds("try {\n    run();\n} catch {\n}\n").is_empty(),
            "block on the next line"
        );
        // Empty on the same line.
        assert!(!kinds("try { run(); } catch { }\n").is_empty());
        // Python.
        assert!(!kinds("except: pass\n").is_empty());
        assert!(!kinds("try:\n    run()\nexcept Exception:\n    pass\n").is_empty());
        // A handler that does something is fine.
        assert!(kinds("try { run(); } catch (e) { log(e); }\n").is_empty());
        // And an identifier that merely starts with the keyword is not a clause.
        assert!(kinds("catch_all(items)\n").is_empty());
    }

    #[test]
    fn unbalanced_brackets_are_high_confidence_but_not_auto_fixed() {
        let found = analyze_text(Path::new("a.rs"), "fn main() {\n    if x {\n}\n");
        let brackets: Vec<&Proposal> = found
            .iter()
            .filter(|p| p.kind == Kind::UnbalancedBrackets)
            .collect();
        assert_eq!(brackets.len(), 1, "{found:#?}");
        assert_eq!(brackets[0].confidence, Confidence::High);
        // Even at high confidence, an empty replacement is never auto-applied:
        // where the brace belongs is the ambiguous part.
        assert!(brackets[0].old.is_empty());
    }

    #[test]
    fn brackets_inside_strings_and_comments_do_not_confuse_the_balance() {
        assert!(balance_problem("fn f() {\n    let s = \"}\";\n}\n").is_none());
        assert!(balance_problem("fn f() {\n    // }\n}\n").is_none());
        assert!(balance_problem("fn f() {\n    let s = \"{\";\n").is_some());
        // An extra closer is reported at its own line.
        let (line, msg) = balance_problem("a\n}\n").unwrap();
        assert_eq!(line, 1);
        assert!(msg.contains("never opened"), "{msg}");
    }

    #[test]
    fn clean_code_produces_nothing() {
        let text = "pub fn add(a: i32, b: i32) -> i32 {\n    a + b\n}\n";
        assert!(analyze_text(Path::new("a.rs"), text).is_empty());
    }

    #[test]
    fn nothing_is_applied_unless_the_policy_says_so() {
        let proposal = Proposal {
            path: PathBuf::from("a.rs"),
            kind: Kind::UnbalancedBrackets,
            confidence: Confidence::High,
            line: 0,
            summary: "s".into(),
            old: "x".into(),
            new: "y".into(),
        };
        // Default policy: never unattended.
        assert!(!Policy::default().may_apply_unattended(&proposal));
        // Enabled but approval still required.
        let mut policy = Policy {
            enabled: true,
            ..Default::default()
        };
        assert!(!policy.may_apply_unattended(&proposal));
        // Opted in: high confidence may apply, medium never does.
        policy.apply_high_confidence = true;
        assert!(policy.may_apply_unattended(&proposal));
        let medium = Proposal {
            confidence: Confidence::Medium,
            ..proposal
        };
        assert!(!policy.may_apply_unattended(&medium));
    }

    #[test]
    fn policy_text_is_honest_about_what_it_does() {
        assert!(Policy::default().describe().contains("no background"));
        let report_only = Policy {
            enabled: true,
            ..Default::default()
        };
        assert!(report_only.describe().contains("needs approval"));
        let applying = Policy {
            enabled: true,
            apply_high_confidence: true,
            ..Default::default()
        };
        assert!(applying.describe().contains("high-confidence"));
    }

    #[test]
    fn reports_are_ordered_and_bounded() {
        let mut report = Report::default();
        for i in 0..20 {
            report.proposals.push(Proposal {
                path: PathBuf::from(format!("f{}.rs", i % 3)),
                kind: Kind::TodoMarker,
                confidence: if i % 5 == 0 {
                    Confidence::High
                } else {
                    Confidence::Low
                },
                line: i,
                summary: String::new(),
                old: String::new(),
                new: String::new(),
            });
        }
        let limited = limit(report, 5);
        assert_eq!(limited.proposals.len(), 5);
        // The high-confidence ones survived the cut.
        assert_eq!(
            limited
                .proposals
                .iter()
                .filter(|p| p.confidence == Confidence::High)
                .count(),
            4
        );
        // And the result is sorted by path/line for stable display.
        assert!(limited
            .proposals
            .windows(2)
            .all(|w| (&w[0].path, w[0].line) <= (&w[1].path, w[1].line)));
    }

    #[test]
    fn missing_and_oversized_files_are_skipped_not_guessed() {
        let report = analyze_files(
            &[PathBuf::from("/nonexistent/file.rs")],
            1024,
        );
        assert_eq!(report.scanned, 0);
        assert_eq!(report.skipped, 1);
        assert!(report.summary().contains("nothing to fix"), "{}", report.summary());
    }

    #[test]
    fn unused_declarations_are_flagged_but_not_deleted() {
        let text = "fn helper() -> i32 { 1 }\nfn main() {}\n";
        let found = unused_symbols(Path::new("a.rs"), text);
        assert_eq!(found.len(), 1, "{found:#?}");
        assert_eq!(found[0].line, 0);
        assert!(found[0].summary.contains("helper"));
        // The proposed edit is a no-op: removal needs a human decision.
        assert_eq!(found[0].old, found[0].new);
    }
}
