//! Pure parser for `git worktree list --porcelain` output.
//!
//! The porcelain format is line-oriented and stable across git versions
//! we care about (git ≥ 2.36). Each entry is a block of `KEY VALUE` lines
//! terminated by a blank line. The keys we use are:
//!
//! - `worktree <path>` — absolute path to the worktree
//! - `HEAD <sha>` — checked-out commit
//! - `branch <ref>` — checked-out branch (omitted for detached HEAD)
//! - `bare` — flag for the bare parent
//! - `detached` — flag for a worktree with no branch
//!
//! Unknown keys are ignored — git is allowed to add new lines without
//! breaking us.

use std::path::PathBuf;

/// Parsed entry from `git worktree list --porcelain`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorktreeEntry {
    /// Absolute filesystem path. Always present (parser drops entries
    /// that lack a `worktree` line — never observed in practice).
    pub path: PathBuf,
    /// Checked-out commit SHA, if reported. Absent for the bare parent.
    pub head: Option<String>,
    /// Branch ref name without the leading `refs/heads/`.
    /// `None` for detached HEAD or for the bare parent.
    pub branch: Option<String>,
    /// `true` for the bare parent entry. The worktrees themselves are
    /// never bare.
    pub is_bare: bool,
    /// `true` if the worktree is on a detached HEAD (no branch).
    pub is_detached: bool,
}

/// Parse the raw stdout of `git worktree list --porcelain` into a vec
/// of [`WorktreeEntry`]. Pure — no IO.
pub fn parse_worktree_list(input: &str) -> Vec<WorktreeEntry> {
    let mut entries = Vec::new();
    let mut cur: Option<WorktreeEntry> = None;

    let flush = |cur: &mut Option<WorktreeEntry>, entries: &mut Vec<WorktreeEntry>| {
        if let Some(entry) = cur.take()
            && !entry.path.as_os_str().is_empty()
        {
            entries.push(entry);
        }
    };

    for line in input.lines() {
        if line.is_empty() {
            flush(&mut cur, &mut entries);
            continue;
        }
        let entry = cur.get_or_insert_with(|| WorktreeEntry {
            path: PathBuf::new(),
            head: None,
            branch: None,
            is_bare: false,
            is_detached: false,
        });

        if let Some(rest) = line.strip_prefix("worktree ") {
            entry.path = PathBuf::from(rest);
        } else if let Some(rest) = line.strip_prefix("HEAD ") {
            entry.head = Some(rest.to_string());
        } else if let Some(rest) = line.strip_prefix("branch ") {
            // git emits `refs/heads/<name>` here; strip the prefix.
            let name = rest.strip_prefix("refs/heads/").unwrap_or(rest);
            entry.branch = Some(name.to_string());
        } else if line == "bare" {
            entry.is_bare = true;
        } else if line == "detached" {
            entry.is_detached = true;
        }
        // Unknown keys are silently ignored.
    }
    flush(&mut cur, &mut entries);
    entries
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_empty_input() {
        assert!(parse_worktree_list("").is_empty());
    }

    #[test]
    fn parses_single_branch_entry() {
        let raw = "\
worktree /tmp/wt1
HEAD abcdef0123456789
branch refs/heads/feat/x
";
        let parsed = parse_worktree_list(raw);
        assert_eq!(parsed.len(), 1);
        let e = &parsed[0];
        assert_eq!(e.path, PathBuf::from("/tmp/wt1"));
        assert_eq!(e.head.as_deref(), Some("abcdef0123456789"));
        assert_eq!(e.branch.as_deref(), Some("feat/x"));
        assert!(!e.is_bare);
        assert!(!e.is_detached);
    }

    #[test]
    fn parses_bare_parent_then_branch_worktree() {
        let raw = "\
worktree /home/u/repo
bare

worktree /home/u/repo/wt
HEAD 1234
branch refs/heads/feat/y
";
        let parsed = parse_worktree_list(raw);
        assert_eq!(parsed.len(), 2);
        assert!(parsed[0].is_bare);
        assert_eq!(parsed[0].path, PathBuf::from("/home/u/repo"));
        assert_eq!(parsed[1].branch.as_deref(), Some("feat/y"));
    }

    #[test]
    fn parses_detached_worktree() {
        let raw = "\
worktree /tmp/d
HEAD aaaa
detached
";
        let parsed = parse_worktree_list(raw);
        assert_eq!(parsed.len(), 1);
        assert!(parsed[0].is_detached);
        assert!(parsed[0].branch.is_none());
    }

    #[test]
    fn ignores_unknown_keys() {
        let raw = "\
worktree /tmp/u
HEAD ffff
locked some-reason
prunable-since 1234567890
branch refs/heads/x
";
        let parsed = parse_worktree_list(raw);
        assert_eq!(parsed.len(), 1);
        // We don't expose locked/prunable-since today; just confirm
        // the known fields parse cleanly past them.
        assert_eq!(parsed[0].branch.as_deref(), Some("x"));
    }

    #[test]
    fn drops_entries_without_worktree_line() {
        // git never emits this in practice, but the parser must not
        // synthesize a phantom entry from a stray HEAD block.
        let raw = "\
HEAD ffff
branch refs/heads/x
";
        let parsed = parse_worktree_list(raw);
        assert!(parsed.is_empty(), "got {parsed:?}");
    }

    #[test]
    fn accepts_branch_without_refs_heads_prefix() {
        // Defensive: future git versions or `--porcelain=v2` may emit a
        // bare branch name. Parser should accept either form.
        let raw = "\
worktree /tmp/x
branch feat/already-stripped
";
        let parsed = parse_worktree_list(raw);
        assert_eq!(parsed[0].branch.as_deref(), Some("feat/already-stripped"));
    }
}
