//! Error marks in the code. When an error (say, "server not reachable") ends
//! an answer after Nemotron already changed files, the app writes one comment
//! line into the file it was working on, right behind the last line it
//! changed. The developer sees it in the diff, line by line; "Weitermachen"
//! removes it again and goes on.
//!
//! The app writes the line itself: with the server out of reach, Nemotron
//! cannot. And only lines of exactly this shape are ever removed, so no other
//! comment is touched.

use std::path::Path;

/// Starts the text of every mark; the system prompt tells Nemotron what it is.
pub const MARK: &str = "FADIS-FEHLER:";
/// Ends the text of every mark. Together with [`MARK`] it makes a line ours.
const TAIL: &str = "(von Fadis Nemotron eingefügt, „Weitermachen“ entfernt die Zeile)";

/// A comment in a file of this type: what comes before and after the text.
/// `None` for files that have no comments (JSON, plain text, pictures …):
/// nothing is written there.
pub fn comment_syntax(path: &Path) -> Option<(&'static str, &'static str)> {
    let name = path.file_name()?.to_str()?;
    let extension = path
        .extension()
        .and_then(|e| e.to_str())
        .map(str::to_ascii_lowercase);
    let slashes = ("//", "");
    let hash = ("#", "");
    Some(match extension.as_deref() {
        Some(
            "rs" | "js" | "mjs" | "cjs" | "jsx" | "ts" | "tsx" | "java" | "kt" | "kts" | "swift"
            | "go" | "c" | "cc" | "cpp" | "cxx" | "h" | "hpp" | "m" | "mm" | "cs" | "dart"
            | "scala" | "php" | "gradle" | "proto" | "zig" | "v" | "sol",
        ) => slashes,
        Some(
            "py" | "rb" | "sh" | "bash" | "zsh" | "fish" | "yml" | "yaml" | "toml" | "pl" | "r"
            | "ps1" | "tf" | "cmake" | "mk" | "nix" | "conf" | "properties" | "gitignore",
        ) => hash,
        Some("html" | "htm" | "xml" | "svg" | "md" | "markdown" | "vue" | "xhtml") => {
            ("<!--", "-->")
        }
        Some("css" | "scss" | "less") => ("/*", "*/"),
        Some("sql" | "lua" | "hs" | "elm") => ("--", ""),
        Some("tex" | "erl") => ("%", ""),
        Some("vim") => ("\"", ""),
        Some(_) => return None,
        None => match name {
            "Makefile" | "Dockerfile" | "Gemfile" | "Rakefile" | "Procfile" | ".zshrc"
            | ".bashrc" | ".env.example" => hash,
            _ => return None,
        },
    })
}

/// The mark for a file, without line ending; `None` if the type has no comments.
pub fn marker_line(path: &Path, reason: &str) -> Option<String> {
    let (open, close) = comment_syntax(path)?;
    // One short line that cannot end the comment early.
    let reason: String = reason
        .lines()
        .next()
        .unwrap_or("")
        .replace("*/", "* /")
        .replace("-->", "- ->")
        .chars()
        .take(90)
        .collect();
    let text = format!("{MARK} Auftrag unterbrochen – {reason} {TAIL}");
    Some(if close.is_empty() {
        format!("{open} {text}")
    } else {
        format!("{open} {text} {close}")
    })
}

/// How many lines of `after` stay above the mark: up to and including the
/// last line that differs from `before`. Without a `before` (a new file), or
/// when nothing differs, the mark goes to the end.
pub fn lines_above_mark(before: Option<&str>, after: &str) -> usize {
    let total = after.split_inclusive('\n').count();
    let Some(before) = before else {
        return total;
    };
    similar::TextDiff::from_lines(before, after)
        .ops()
        .iter()
        .filter(|op| op.tag() != similar::DiffTag::Equal)
        .map(|op| op.new_range().end)
        .max()
        .map_or(total, |end| end.min(total))
}

/// `content` with `mark` as a line of its own after `above` lines. The line
/// ending of the file is kept, and a last line without ending gets one first.
pub fn insert_mark(content: &str, above: usize, mark: &str) -> String {
    let eol = if content.contains("\r\n") {
        "\r\n"
    } else {
        "\n"
    };
    let mut out = String::with_capacity(content.len() + mark.len() + 2);
    let mut lines = content.split_inclusive('\n');
    for line in lines.by_ref().take(above) {
        out.push_str(line);
    }
    if !out.is_empty() && !out.ends_with('\n') {
        out.push_str(eol);
    }
    out.push_str(mark);
    out.push_str(eol);
    for line in lines {
        out.push_str(line);
    }
    out
}

/// Whether a line is one of our marks: a comment that starts with the start
/// of the text we write and ends with the end of it (and the closing of the
/// comment, where the file type has one). Nothing else matches, not even a
/// comment that quotes both parts.
pub fn is_mark(line: &str) -> bool {
    const OPENERS: [&str; 7] = ["//", "#", "<!--", "/*", "--", "%", "\""];
    let line = line.trim();
    let line = line
        .strip_suffix("-->")
        .or_else(|| line.strip_suffix("*/"))
        .unwrap_or(line)
        .trim_end();
    line.ends_with(TAIL)
        && OPENERS.iter().any(|opener| {
            line.strip_prefix(opener)
                .and_then(|rest| rest.strip_prefix(' '))
                .is_some_and(|rest| rest.starts_with(MARK))
        })
}

/// `content` without any of our marks, and how many were removed. Everything
/// else, line endings included, stays byte for byte.
pub fn remove_marks(content: &str) -> (String, usize) {
    let mut out = String::with_capacity(content.len());
    let mut removed = 0;
    for line in content.split_inclusive('\n') {
        if is_mark(line) {
            removed += 1;
        } else {
            out.push_str(line);
        }
    }
    (out, removed)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn marks_use_the_comment_of_the_file_and_skip_files_without_comments() {
        let mark = |name: &str| marker_line(Path::new(name), "Server nicht erreichbar");
        assert!(
            mark("src/lib.rs")
                .unwrap()
                .starts_with("// FADIS-FEHLER: Auftrag unterbrochen – Server nicht erreichbar ")
        );
        assert!(mark("tool.py").unwrap().starts_with("# FADIS-FEHLER:"));
        assert!(
            mark("index.html")
                .unwrap()
                .starts_with("<!-- FADIS-FEHLER:")
        );
        assert!(mark("index.html").unwrap().ends_with(" -->"));
        assert!(mark("style.css").unwrap().ends_with(" */"));
        assert!(mark("query.sql").unwrap().starts_with("-- FADIS-FEHLER:"));
        assert!(mark("Makefile").unwrap().starts_with("# "));
        for none in [
            "data.json",
            "notes.txt",
            "logo.png",
            "Cargo.lock",
            "LICENSE",
            "a.bin",
        ] {
            assert!(mark(none).is_none(), "{none}");
        }
    }

    #[test]
    fn the_reason_cannot_break_out_of_the_comment() {
        let css = marker_line(
            Path::new("a.css"),
            "so */ body { display:none } /*\nzweite Zeile",
        )
        .unwrap();
        assert_eq!(css.matches("*/").count(), 1, "{css}");
        assert!(!css.contains('\n'));
        let html = marker_line(Path::new("a.html"), "evil --> <script>x</script>").unwrap();
        assert_eq!(html.matches("-->").count(), 1, "{html}");
        let long = marker_line(Path::new("a.rs"), &"x".repeat(500)).unwrap();
        assert!(long.chars().count() < 220, "{}", long.chars().count());
    }

    #[test]
    fn the_mark_goes_behind_the_last_changed_line() {
        let before = "a\nb\nc\nd\ne\n";
        let after = "a\nB\nc\nd\nE\nf\n";
        // The last change ends at line 6 ("f" is new).
        assert_eq!(lines_above_mark(Some(before), after), 6);
        let after = "a\nB\nc\nd\ne\n";
        assert_eq!(lines_above_mark(Some(before), after), 2);
        // A new file: at the end. Nothing differs: at the end.
        assert_eq!(lines_above_mark(None, "x\ny\n"), 2);
        assert_eq!(lines_above_mark(Some("x\n"), "x\n"), 1);
        // Deleted lines at the end leave nothing to stand behind but the end.
        assert_eq!(lines_above_mark(Some("a\nb\nc\n"), "a\n"), 1);
    }

    #[test]
    fn inserting_keeps_the_line_endings_and_everything_else() {
        let mark = "// FADIS-FEHLER: x";
        assert_eq!(
            insert_mark("a\nb\nc\n", 2, mark),
            "a\nb\n// FADIS-FEHLER: x\nc\n"
        );
        assert_eq!(
            insert_mark("a\r\nb\r\n", 1, mark),
            "a\r\n// FADIS-FEHLER: x\r\nb\r\n"
        );
        // The last line has no ending: it gets one before the mark.
        assert_eq!(insert_mark("a\nb", 2, mark), "a\nb\n// FADIS-FEHLER: x\n");
        assert_eq!(insert_mark("", 0, mark), "// FADIS-FEHLER: x\n");
        assert_eq!(insert_mark("a\n", 0, mark), "// FADIS-FEHLER: x\na\n");
    }

    #[test]
    fn only_exactly_our_lines_are_removed() {
        let ours = marker_line(Path::new("x.rs"), "Server nicht erreichbar").unwrap();
        let theirs = [
            "// FADIS-FEHLER: das hat jemand von Hand geschrieben",
            "/// MARK = \"FADIS-FEHLER:\" und \"(von Fadis Nemotron eingefügt, „Weitermachen“ entfernt die Zeile)\"",
            "let s = \"FADIS-FEHLER: (von Fadis Nemotron eingefügt, „Weitermachen“ entfernt die Zeile)\";",
        ];
        let content = format!(
            "fn a() {{}}\r\n{}\r\n{ours}\r\nfn b() {{}}",
            theirs.join("\r\n")
        );
        let (cleaned, removed) = remove_marks(&content);
        assert_eq!(removed, 1, "only ours");
        assert!(cleaned.contains(theirs[0]) && cleaned.contains(theirs[2]));
        assert!(!cleaned.contains(&ours));
        assert!(cleaned.starts_with("fn a() {}\r\n") && cleaned.ends_with("fn b() {}"));
        assert_eq!(remove_marks("nichts\n").1, 0);
        assert_eq!(remove_marks("nichts\n").0, "nichts\n");
    }

    #[test]
    fn a_mark_that_was_inserted_is_found_and_removed_again() {
        for name in ["a.rs", "a.py", "a.html", "a.css", "a.sql"] {
            let mark = marker_line(Path::new(name), "timeout: receive response").unwrap();
            assert!(is_mark(&mark), "{name}: {mark}");
            let original = "eins\nzwei\ndrei\n";
            let marked = insert_mark(original, 2, &mark);
            assert!(marked.contains(&mark));
            assert_eq!(remove_marks(&marked), (original.to_owned(), 1), "{name}");
        }
    }
}
