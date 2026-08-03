//! Pure parsers for commit-message metadata and the `.git-blame-ignore-revs` file.
//!
//! Zero I/O below the loader: everything else takes a `&str` and returns
//! plain data. All parsing is unit-tested.

use std::collections::HashSet;
use std::path::Path;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RawIdent {
    pub name: String,
    pub email: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Trailer {
    pub role: TrailerRole,
    pub ident: RawIdent,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrailerRole {
    Coauthor,
    Signoff,
}

impl TrailerRole {
    pub fn as_str(self) -> &'static str {
        match self {
            TrailerRole::Coauthor => "coauthor",
            TrailerRole::Signoff => "signoff",
        }
    }
}

/// Extract `Co-authored-by:` / `Signed-off-by:` trailers from a commit
/// message. Trailers must live in the message body (i.e. not on the subject
/// line); we scan every non-subject line to stay lenient about missing blank
/// separators.
pub fn parse_trailers(msg: &str) -> Vec<Trailer> {
    let mut out = Vec::new();
    for (i, raw) in msg.lines().enumerate() {
        if i == 0 {
            continue; // subject line
        }
        let line = raw.trim();
        let Some((key, value)) = line.split_once(':') else {
            continue;
        };
        let role = match key.trim().to_ascii_lowercase().as_str() {
            "co-authored-by" => TrailerRole::Coauthor,
            "signed-off-by" => TrailerRole::Signoff,
            _ => continue,
        };
        if let Some(ident) = parse_ident(value.trim()) {
            out.push(Trailer { role, ident });
        }
    }
    out
}

/// Parse `Full Name <email@example.com>` (or bare `<email>` / bare name).
/// Quoted names (`"Doe, John" <j@x>`) are stripped of outer quotes.
pub fn parse_ident(s: &str) -> Option<RawIdent> {
    let s = s.trim();
    if s.is_empty() {
        return None;
    }
    let (name_part, email_part) = if let Some(lt) = s.rfind('<') {
        let gt = s.rfind('>')?;
        if gt <= lt {
            return None;
        }
        let name = s[..lt].trim();
        let email = s[lt + 1..gt].trim();
        (name, email)
    } else {
        (s, "")
    };
    let name = name_part.trim_matches('"').trim().to_string();
    Some(RawIdent {
        name,
        email: email_part.to_string(),
    })
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConvCommit {
    pub msg_type: Option<String>,
    pub is_breaking: bool,
    pub is_revert: bool,
}

/// Parse Conventional Commit prefix from the subject line.
///
/// Handles `feat:`, `fix(scope):`, `feat!:`, `refactor(core)!:`,
/// `revert:` (also flags `is_revert = true` for `Revert "…"` git-generated
/// subjects). Malformed / unprefixed subjects return all-None.
pub fn parse_conv_commit(msg: &str) -> ConvCommit {
    let subject = msg.lines().next().unwrap_or("").trim();

    // Git's own "Revert ..." subject line.
    let is_revert_prefix = subject.starts_with("Revert \"") || subject.starts_with("Revert '");

    let (prefix, rest) = match subject.split_once(':') {
        Some(p) => p,
        None => {
            return ConvCommit {
                msg_type: None,
                is_breaking: false,
                is_revert: is_revert_prefix,
            };
        }
    };
    let _ = rest;

    let prefix = prefix.trim();
    // Strip trailing `!`.
    let (base, is_breaking) = match prefix.strip_suffix('!') {
        Some(b) => (b, true),
        None => (prefix, false),
    };
    // Strip trailing `(scope)`.
    let msg_type = match base.split_once('(') {
        Some((t, scope)) if scope.ends_with(')') => t.trim().to_string(),
        _ => base.trim().to_string(),
    };
    if msg_type.is_empty() || !msg_type.chars().all(|c| c.is_ascii_alphabetic()) {
        return ConvCommit {
            msg_type: None,
            is_breaking: false,
            is_revert: is_revert_prefix,
        };
    }
    let is_revert = is_revert_prefix || msg_type.eq_ignore_ascii_case("revert");
    ConvCommit {
        msg_type: Some(msg_type.to_ascii_lowercase()),
        is_breaking,
        is_revert,
    }
}

/// Load `.git-blame-ignore-revs` from `repo_root`. Missing file → empty set.
/// Blank lines and `#` comments are skipped. Non-hex tokens are silently
/// dropped (git itself is lenient here).
pub fn load_ignore_revs(repo_root: &Path) -> HashSet<String> {
    let path = repo_root.join(".git-blame-ignore-revs");
    let Ok(body) = std::fs::read_to_string(&path) else {
        return HashSet::new();
    };
    let mut out = HashSet::new();
    for line in body.lines() {
        let line = line.split('#').next().unwrap_or("").trim();
        if line.is_empty() {
            continue;
        }
        if line.chars().all(|c| c.is_ascii_hexdigit()) && line.len() >= 4 {
            out.insert(line.to_ascii_lowercase());
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn trailer_coauthor_basic() {
        let msg = "feat: thing\n\nCo-authored-by: Ada <ada@example.com>\n";
        let t = parse_trailers(msg);
        assert_eq!(t.len(), 1);
        assert_eq!(t[0].role, TrailerRole::Coauthor);
        assert_eq!(t[0].ident.name, "Ada");
        assert_eq!(t[0].ident.email, "ada@example.com");
    }

    #[test]
    fn trailer_signoff_and_coauthor() {
        let msg =
            "fix: bug\n\nCo-authored-by: \"Doe, John\" <j@x.org>\nSigned-off-by: Bob <bob@x>\n";
        let t = parse_trailers(msg);
        assert_eq!(t.len(), 2);
        assert_eq!(t[0].role, TrailerRole::Coauthor);
        assert_eq!(t[0].ident.name, "Doe, John");
        assert_eq!(t[1].role, TrailerRole::Signoff);
    }

    #[test]
    fn trailer_unicode() {
        let msg = "x\n\nCo-authored-by: Åsa Ölund <asa@ø.no>\n";
        let t = parse_trailers(msg);
        assert_eq!(t[0].ident.name, "Åsa Ölund");
        assert_eq!(t[0].ident.email, "asa@ø.no");
    }

    #[test]
    fn trailer_subject_line_ignored() {
        let msg = "Co-authored-by: Not <not@x>";
        assert!(parse_trailers(msg).is_empty());
    }

    #[test]
    fn conv_commit_basic() {
        let c = parse_conv_commit("feat: add thing");
        assert_eq!(c.msg_type.as_deref(), Some("feat"));
        assert!(!c.is_breaking);
        assert!(!c.is_revert);
    }

    #[test]
    fn conv_commit_breaking_with_scope() {
        let c = parse_conv_commit("refactor(core)!: break API");
        assert_eq!(c.msg_type.as_deref(), Some("refactor"));
        assert!(c.is_breaking);
    }

    #[test]
    fn conv_commit_fix_scope() {
        let c = parse_conv_commit("fix(auth): tighten check");
        assert_eq!(c.msg_type.as_deref(), Some("fix"));
    }

    #[test]
    fn conv_commit_revert_type() {
        let c = parse_conv_commit("revert: undo thing");
        assert_eq!(c.msg_type.as_deref(), Some("revert"));
        assert!(c.is_revert);
    }

    #[test]
    fn conv_commit_git_revert_subject() {
        let c = parse_conv_commit("Revert \"feat: bad thing\"");
        assert!(c.is_revert);
        assert!(c.msg_type.is_none());
    }

    #[test]
    fn conv_commit_malformed() {
        let c = parse_conv_commit("wip stuff");
        assert!(c.msg_type.is_none());
        assert!(!c.is_breaking);

        let c = parse_conv_commit("123: numeric prefix");
        assert!(c.msg_type.is_none());
    }

    #[test]
    fn ignore_revs_parses() {
        let td = tempfile::tempdir().unwrap();
        std::fs::write(
            td.path().join(".git-blame-ignore-revs"),
            "# comment\n\n\
             deadbeefdeadbeefdeadbeefdeadbeefdeadbeef\n\
             CAFEBABECAFEBABECAFEBABECAFEBABECAFEBABE  # inline\n\
             not-hex-ignored\n",
        )
        .unwrap();
        let s = load_ignore_revs(td.path());
        assert_eq!(s.len(), 2);
        assert!(s.contains("deadbeefdeadbeefdeadbeefdeadbeefdeadbeef"));
        assert!(s.contains("cafebabecafebabecafebabecafebabecafebabe"));
    }

    #[test]
    fn ignore_revs_missing_ok() {
        let td = tempfile::tempdir().unwrap();
        assert!(load_ignore_revs(td.path()).is_empty());
    }
}
