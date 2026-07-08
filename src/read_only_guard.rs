//! Observe-only (read-only) guardrail — a hard, fail-closed command floor for
//! an observe-only identity such as **Crocutus** (issue #1).
//!
//! ## Why a new guard (abstraction gap)
//!
//! Simard already ships two command guards, but neither expresses an
//! *observe-only* posture:
//!
//! - [`crate::git_guardrails`] blocks only *destructive* git operations
//!   (`push --force`, `reset --hard`, …) and **allows** ordinary
//!   `push`/`commit`. That is correct for the engineering identity (Simard),
//!   which is *meant* to write.
//! - [`crate::ado_acl_guard`] blocks only Azure DevOps **ACL self-escalation**
//!   and allows every other `az` command.
//!
//! Crocutus is a second identity on the same codebase whose entire mandate is
//! to **observe target repositories read-only and never change anything,
//! anywhere** — no commit, push, branch, PR, work-item edit, comment, or ACL
//! change. That posture cannot be expressed by the existing guards, so this
//! module adds it. It is the identity-level "READ-ONLY mode flag" the Crocutus
//! task calls for, implemented as *one* shared-codebase capability rather than
//! a fork.
//!
//! ## Design: default-DENY, fail-closed
//!
//! Unlike the other guards (which allow-by-default and block specific bad
//! patterns), this guard **denies by default** for the tools that can mutate a
//! remote target (`git`, `az`, `gh`, `curl`, `wget`) and permits a command only
//! when it is *provably* a read. Anything ambiguous is treated as a write and
//! refused. If any layer is uncertain, it FAILS CLOSED (blocks) rather than risk
//! a write — exactly the guarantee the Crocutus guardrail demands.
//!
//! ## Scope
//!
//! This screens `git`/`az`/`gh`/`curl`/`wget` command lines. Writes embedded in
//! opaque interpreters (`python foo.py`, `bash -c …`) cannot be classified here
//! and are the responsibility of the *other* guardrail layers Crocutus stacks:
//! (a) no write-capable credential to the target project, and (b) disabled
//! act/engineer-dispatch capabilities. This command guard is one layer of that
//! defense in depth, not the whole of it.

/// Environment flag that puts the shared binary into observe-only mode.
///
/// When set to a truthy value (`1`/`true`/`enabled`/`yes`/`on`), the env-gated
/// entry point [`guard_observe_only`] enforces the read-only floor. The Crocutus
/// identity sets this; the Simard (engineer) identity leaves it unset.
pub const OBSERVE_ONLY_ENV: &str = "SIMARD_OBSERVE_ONLY";

/// Returns `true` when observe-only mode is enabled via [`OBSERVE_ONLY_ENV`].
///
/// Defaults to `false` (the engineer identity is not observe-only).
#[must_use]
pub fn observe_only_enabled() -> bool {
    std::env::var(OBSERVE_ONLY_ENV)
        .map(|v| matches!(v.as_str(), "1" | "true" | "enabled" | "yes" | "on"))
        .unwrap_or(false)
}

/// Env-gated chokepoint: enforce the observe-only floor **only** when
/// [`observe_only_enabled`] is true, otherwise allow (the other identities are
/// free to write).
///
/// This is what an in-crate command-execution chokepoint calls so a single
/// shared binary serves both identities.
pub fn guard_observe_only(argv: &[&str]) -> Result<(), String> {
    if observe_only_enabled() {
        check_observe_only(argv)
    } else {
        Ok(())
    }
}

/// Env-gated chokepoint using the git-argument convention (arguments **after**
/// the `git` program name, e.g. `["push", "origin", "main"]`).
///
/// This is the seam wired into [`crate::git_guardrails::check_git_safety`]: it
/// enforces the observe-only floor **only** when [`observe_only_enabled`] is
/// true, so the engineer identity (Simard) is unaffected while the Crocutus
/// identity — which sets [`OBSERVE_ONLY_ENV`] — has every mutating git verb
/// refused at the shared write seam, fail-closed.
pub fn guard_observe_only_git(git_args: &[&str]) -> Result<(), String> {
    if observe_only_enabled() {
        check_observe_only_git(git_args)
    } else {
        Ok(())
    }
}

/// Always-on observe-only check (independent of the env flag).
///
/// Returns `Ok(())` if `argv` is provably a read (or an out-of-scope tool this
/// guard does not screen), and `Err(message)` if it is — or *might be* — a
/// write. The Crocutus identity, which is observe-only by construction, calls
/// this directly regardless of the env flag.
///
/// `argv` is the full command line including the tool (e.g.
/// `["git", "push", …]`, `["az", "repos", "pr", "create", …]`).
pub fn check_observe_only(argv: &[&str]) -> Result<(), String> {
    if command_is_read(argv) {
        Ok(())
    } else {
        Err(blocked_message(argv))
    }
}

/// Git-argument convention mirroring [`crate::git_guardrails::check_git_safety`]:
/// `git_args` are the arguments **after** the `git` program name
/// (e.g. `["push", "--force", "origin", "main"]`).
pub fn check_observe_only_git(git_args: &[&str]) -> Result<(), String> {
    if git_command_is_read(git_args) {
        Ok(())
    } else {
        let mut full = Vec::with_capacity(git_args.len() + 1);
        full.push("git");
        full.extend_from_slice(git_args);
        Err(blocked_message(&full))
    }
}

/// Classification helper: `true` when `argv` would mutate a target under
/// observe-only rules (the inverse of [`command_is_read`] for in-scope tools).
#[must_use]
pub fn is_write_command(argv: &[&str]) -> bool {
    !command_is_read(argv)
}

/// The block message. Includes a stable `GUARDRAIL BLOCKED` marker and the
/// `observe-only` posture so logs and tests can assert on it.
fn blocked_message(argv: &[&str]) -> String {
    format!(
        "GUARDRAIL BLOCKED: observe-only (read-only) identity refuses to run a \
         potentially mutating command `{}`. This identity may only OBSERVE target \
         repositories — no commit, push, branch, PR, work-item edit, comment, or \
         ACL change is permitted anywhere. If this command is genuinely read-only \
         and was refused, it was refused by design (fail-closed): use an explicit \
         read form (e.g. `git fetch`/`git log`, `az ... list|show`, `az rest \
         --method GET`, `gh ... list|view`, `curl -X GET`).",
        argv.join(" ")
    )
}

/// Top-level classifier: is `argv` provably a read for the tool it invokes?
///
/// Tools this guard does not screen (anything other than `git`/`az`/`gh`/
/// `curl`/`wget`) return `true` here — command-level screening is out of scope
/// for them and the credential/capability layers cover them.
#[must_use]
pub fn command_is_read(argv: &[&str]) -> bool {
    let Some(tool) = argv.first().copied() else {
        return true; // empty command mutates nothing
    };
    let base = tool.rsplit(['/', '\\']).next().unwrap_or(tool);
    match base {
        "git" => git_command_is_read(&argv[1..]),
        "az" => az_command_is_read(&argv[1..]),
        "gh" => gh_command_is_read(&argv[1..]),
        "curl" => http_is_read(&argv[1..], HttpDialect::Curl),
        "wget" => http_is_read(&argv[1..], HttpDialect::Wget),
        _ => true,
    }
}

// ---------------------------------------------------------------------------
// git
// ---------------------------------------------------------------------------

/// Git subcommands that are always pure reads.
const GIT_READ_SUBCOMMANDS: &[&str] = &[
    "status",
    "log",
    "diff",
    "show",
    "fetch",
    "clone",
    "ls-remote",
    "ls-files",
    "ls-tree",
    "rev-parse",
    "rev-list",
    "cat-file",
    "for-each-ref",
    "describe",
    "blame",
    "shortlog",
    "grep",
    "whatchanged",
    "name-rev",
    "cherry",
    "show-ref",
    "show-branch",
    "merge-base",
    "count-objects",
    "verify-pack",
    "verify-commit",
    "verify-tag",
    "annotate",
    "version",
    "help",
];

/// Git global options (that appear *before* the subcommand) which take a value.
const GIT_VALUE_GLOBALS: &[&str] = &[
    "-C",
    "-c",
    "--git-dir",
    "--work-tree",
    "--namespace",
    "--exec-path",
    "--super-prefix",
];

/// Git global boolean options that appear before the subcommand.
const GIT_BOOL_GLOBALS: &[&str] = &[
    "-p",
    "--paginate",
    "-P",
    "--no-pager",
    "--bare",
    "--no-replace-objects",
    "--literal-pathspecs",
    "--glob-pathspecs",
    "--noglob-pathspecs",
    "--icase-pathspecs",
    "--no-optional-locks",
];

/// Skip any leading git global options and return the slice starting at the
/// subcommand. Unknown leading `-`-options are left in place so the caller
/// treats them as an (unrecognized) subcommand and fails closed.
fn strip_git_globals<'a>(args: &'a [&'a str]) -> &'a [&'a str] {
    let mut i = 0;
    while i < args.len() {
        let a = args[i];
        if GIT_VALUE_GLOBALS.contains(&a) {
            i += 2;
            continue;
        }
        if GIT_BOOL_GLOBALS.contains(&a)
            || a.starts_with("--git-dir=")
            || a.starts_with("--work-tree=")
            || a.starts_with("--namespace=")
            || a.starts_with("--exec-path=")
            || (a.starts_with("-c") && a.len() > 2)
        {
            i += 1;
            continue;
        }
        break;
    }
    &args[i..]
}

/// `true` when the git argument vector (after the `git` program name) is a read.
#[must_use]
pub fn git_command_is_read(git_args: &[&str]) -> bool {
    let args = strip_git_globals(git_args);
    let Some((sub, rest)) = args.split_first() else {
        return true; // bare `git` prints help; mutates nothing
    };
    let sub = *sub;

    if GIT_READ_SUBCOMMANDS.contains(&sub) {
        return true;
    }

    match sub {
        "branch" => git_branch_is_read(rest),
        "tag" => git_tag_is_read(rest),
        "config" => git_config_is_read(rest),
        "remote" => git_remote_is_read(rest),
        "reflog" => matches!(rest.first().copied(), None | Some("show")),
        "stash" => matches!(rest.first().copied(), Some("list") | Some("show")),
        "notes" => matches!(rest.first().copied(), None | Some("list") | Some("show")),
        "worktree" => matches!(rest.first().copied(), Some("list")),
        "submodule" => matches!(rest.first().copied(), Some("status") | Some("summary")),
        // Everything else (push, pull, commit, add, rm, mv, merge, rebase,
        // cherry-pick, revert, reset, checkout, switch, restore, clean, gc,
        // am, apply, format-patch-send, update-ref, fast-import, init, …) is
        // refused: fail closed.
        _ => false,
    }
}

fn git_branch_is_read(rest: &[&str]) -> bool {
    const WRITE_FLAGS: &[&str] = &[
        "-d",
        "-D",
        "--delete",
        "-m",
        "-M",
        "--move",
        "-c",
        "-C",
        "--copy",
        "--set-upstream-to",
        "-u",
        "--unset-upstream",
        "--edit-description",
        "-f",
        "--force",
        "--track",
        "--set-upstream",
    ];
    if rest.iter().any(|a| WRITE_FLAGS.contains(a)) {
        return false;
    }
    const FILTER_FLAGS: &[&str] = &[
        "--contains",
        "--no-contains",
        "--points-at",
        "--merged",
        "--no-merged",
    ];
    let has_positional = rest.iter().any(|a| !a.starts_with('-'));
    let has_filter = rest.iter().any(|a| FILTER_FLAGS.contains(a));
    !has_positional || has_filter
}

fn git_tag_is_read(rest: &[&str]) -> bool {
    const WRITE_FLAGS: &[&str] = &[
        "-d",
        "--delete",
        "-a",
        "--annotate",
        "-s",
        "--sign",
        "-f",
        "--force",
        "-m",
        "--message",
        "-F",
        "--file",
        "-e",
        "--edit",
        "-u",
    ];
    if rest.iter().any(|a| WRITE_FLAGS.contains(a)) {
        return false;
    }
    const LIST_FLAGS: &[&str] = &[
        "-l",
        "--list",
        "--contains",
        "--no-contains",
        "--points-at",
        "--merged",
        "--no-merged",
        "--format",
        "--sort",
        "--column",
        "-i",
        "--ignore-case",
    ];
    let has_positional = rest.iter().any(|a| !a.starts_with('-'));
    let has_list_flag = rest
        .iter()
        .any(|a| LIST_FLAGS.contains(a) || a.starts_with("-n"));
    !has_positional || has_list_flag
}

fn git_config_is_read(rest: &[&str]) -> bool {
    // Refuse unless an explicit read flag is present. `git config x y`,
    // `git config --add`, `git config --unset` all lack a read flag → blocked.
    const READ_FLAGS: &[&str] = &[
        "--get",
        "--get-all",
        "--get-regexp",
        "--get-urlmatch",
        "-l",
        "--list",
        "--get-color",
        "--get-colorbool",
    ];
    rest.iter().any(|a| READ_FLAGS.contains(a))
}

fn git_remote_is_read(rest: &[&str]) -> bool {
    match rest.first().copied() {
        None => true,
        Some("-v" | "--verbose" | "show" | "get-url") => true,
        // add, remove, rename, set-url, set-head, set-branches, prune, update → block
        _ => false,
    }
}

// ---------------------------------------------------------------------------
// az (Azure CLI / Azure DevOps)
// ---------------------------------------------------------------------------

/// `az` verb tokens that mutate state. Presence of any of these anywhere in the
/// command makes it a write.
const AZ_WRITE_VERBS: &[&str] = &[
    "create", "update", "delete", "set", "add", "remove", "import", "restore", "abandon",
    "complete", "vote", "publish", "merge", "reset", "approve", "reject", "link", "unlink", "edit",
    "push", "upload", "purge", "enable", "disable", "grant", "revoke", "clone", "init", "move",
    "rename", "promote", "unshelve", "apply", "run", "queue", "cancel", "retain", "unmark", "mark",
];

/// `az` verb tokens that read state.
const AZ_READ_VERBS: &[&str] = &["list", "show", "get"];

/// Neutral `az` subcommands (auth/version/context) that mutate no target and
/// are permitted so observe-only setup is not broken.
const AZ_NEUTRAL_HEADS: &[&str] = &["login", "logout", "version", "account", "--version", "-v"];

fn az_command_is_read(args: &[&str]) -> bool {
    if args.first().copied() == Some("rest") {
        return http_is_read(&args[1..], HttpDialect::AzRest);
    }
    // A write verb anywhere ⇒ write.
    if args.iter().any(|a| AZ_WRITE_VERBS.contains(a)) {
        return false;
    }
    // Neutral heads (login/logout/account/version) are allowed once we know no
    // write verb is present.
    if matches!(args.first().copied(), Some(h) if AZ_NEUTRAL_HEADS.contains(&h)) {
        return true;
    }
    // Otherwise require an explicit read verb; anything unrecognized fails closed.
    args.iter()
        .any(|a| AZ_READ_VERBS.contains(a) || a.starts_with("list-"))
}

// ---------------------------------------------------------------------------
// gh (GitHub CLI)
// ---------------------------------------------------------------------------

const GH_WRITE_VERBS: &[&str] = &[
    "create",
    "merge",
    "close",
    "reopen",
    "edit",
    "delete",
    "comment",
    "review",
    "approve",
    "request-changes",
    "ready",
    "lock",
    "unlock",
    "pin",
    "unpin",
    "transfer",
    "develop",
    "rename",
    "add",
    "remove",
    "set",
    "sync",
    "push",
    "fork",
    "clone",
    "rerun",
    "cancel",
    "disable",
    "enable",
    "import",
    "restore",
    "upload",
    "release",
];

const GH_READ_VERBS: &[&str] = &[
    "list", "view", "status", "diff", "checks", "search", "browse",
];

fn gh_command_is_read(args: &[&str]) -> bool {
    if args.first().copied() == Some("api") {
        return !gh_api_has_write(&args[1..]);
    }
    if args.iter().any(|a| GH_WRITE_VERBS.contains(a)) {
        return false;
    }
    args.iter().any(|a| GH_READ_VERBS.contains(a))
        || matches!(
            args.first().copied(),
            Some("auth" | "version" | "--version")
        )
}

/// `gh api` defaults to GET; it becomes a write when an explicit non-read
/// method is given, or when any field/body flag forces a POST.
fn gh_api_has_write(args: &[&str]) -> bool {
    let mut i = 0;
    while i < args.len() {
        let a = args[i];
        match a {
            "-X" | "--method" => {
                if let Some(m) = args.get(i + 1)
                    && !is_read_method(m)
                {
                    return true;
                }
                i += 2;
                continue;
            }
            "-f" | "--raw-field" | "-F" | "--field" | "--input" => return true,
            _ => {}
        }
        if let Some(m) = a.strip_prefix("--method=")
            && !is_read_method(m)
        {
            return true;
        }
        if let Some(m) = a.strip_prefix("-X")
            && !m.is_empty()
            && !is_read_method(m)
        {
            return true;
        }
        i += 1;
    }
    false
}

// ---------------------------------------------------------------------------
// HTTP (curl / wget / az rest) — provably-read detection, fail-closed
// ---------------------------------------------------------------------------

/// Which HTTP client dialect a command line uses. Method/body flags differ.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum HttpDialect {
    Curl,
    Wget,
    AzRest,
}

fn is_read_method(m: &str) -> bool {
    matches!(
        strip_quotes(m).to_ascii_lowercase().as_str(),
        "get" | "head" | "options"
    )
}

fn strip_quotes(s: &str) -> &str {
    s.trim_matches(|c| c == '"' || c == '\'')
}

/// `true` only when the HTTP command is *provably* a read: it carries no request
/// body/upload and every explicit method (if any) is a read method. No explicit
/// method means the default `GET`. Fails closed against repeated-method decoys
/// (`-X GET -X POST`) because *all* methods must be reads.
fn http_is_read(args: &[&str], dialect: HttpDialect) -> bool {
    if http_carries_body(args, dialect) {
        return false;
    }
    http_methods(args, dialect)
        .iter()
        .all(|m| is_read_method(m))
}

fn http_carries_body(args: &[&str], dialect: HttpDialect) -> bool {
    match dialect {
        HttpDialect::Curl => args.iter().any(|a| {
            let a = *a;
            matches!(a, "-d" | "-F" | "-T")
                || (a.starts_with("-d") && a.len() > 2)
                || (a.starts_with("-F") && a.len() > 2)
                || (a.starts_with("-T") && a.len() > 2)
                || a.starts_with("--data")
                || a.starts_with("--form")
                || a == "--upload-file"
                || a.starts_with("--upload-file=")
                || a == "--json"
                || a.starts_with("--json=")
        }),
        HttpDialect::Wget => args.iter().any(|a| {
            let a = *a;
            matches!(
                a,
                "--post-data" | "--post-file" | "--body-data" | "--body-file"
            ) || a.starts_with("--post-data=")
                || a.starts_with("--post-file=")
                || a.starts_with("--body-data=")
                || a.starts_with("--body-file=")
        }),
        HttpDialect::AzRest => args.iter().any(|a| {
            let a = *a;
            matches!(a, "--body" | "-b" | "--in-file" | "--input-file")
                || a.starts_with("--body=")
                || a.starts_with("--in-file=")
                || (a.starts_with("-b") && a.len() > 2)
        }),
    }
}

fn http_methods(args: &[&str], dialect: HttpDialect) -> Vec<String> {
    let mut methods = Vec::new();
    let long_flags: &[&str] = match dialect {
        HttpDialect::Curl => &["-X", "--request"],
        HttpDialect::Wget => &["--method"],
        HttpDialect::AzRest => &["-m", "--method", "--http-method"],
    };
    for (i, tok) in args.iter().enumerate() {
        let tok = *tok;
        if long_flags.contains(&tok)
            && let Some(v) = args.get(i + 1)
        {
            methods.push(strip_quotes(v).to_string());
        }
        for pfx in long_flags {
            let eq = format!("{pfx}=");
            if let Some(rest) = tok.strip_prefix(eq.as_str())
                && !rest.is_empty()
            {
                methods.push(strip_quotes(rest).to_string());
            }
        }
        // Glued short forms: curl `-XPOST`, az rest `-mPUT`.
        match dialect {
            HttpDialect::Curl => {
                if let Some(rest) = tok.strip_prefix("-X")
                    && !rest.is_empty()
                {
                    methods.push(strip_quotes(rest).to_string());
                }
            }
            HttpDialect::AzRest => {
                if let Some(rest) = tok.strip_prefix("-m")
                    && !rest.is_empty()
                {
                    methods.push(strip_quotes(rest).to_string());
                }
            }
            HttpDialect::Wget => {}
        }
    }
    methods
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    // Tests mutate the process-global OBSERVE_ONLY_ENV var; serialize them.
    static ENV_LOCK: Mutex<()> = Mutex::new(());

    fn set_observe(on: bool) {
        unsafe {
            if on {
                std::env::set_var(OBSERVE_ONLY_ENV, "1");
            } else {
                std::env::remove_var(OBSERVE_ONLY_ENV);
            }
        }
    }

    // --- git writes are blocked ---

    #[test]
    fn blocks_git_push() {
        assert!(check_observe_only(&["git", "push", "origin", "main"]).is_err());
        assert!(check_observe_only_git(&["push", "origin", "main"]).is_err());
    }

    #[test]
    fn blocks_git_force_push_and_commit_and_mutations() {
        for argv in [
            vec!["git", "push", "--force", "origin", "main"],
            vec!["git", "commit", "-m", "x"],
            vec!["git", "add", "."],
            vec!["git", "merge", "feature"],
            vec!["git", "rebase", "main"],
            vec!["git", "reset", "--hard", "HEAD~1"],
            vec!["git", "checkout", "-b", "new"],
            vec!["git", "switch", "-c", "new"],
            vec!["git", "restore", "file"],
            vec!["git", "cherry-pick", "abc"],
            vec!["git", "revert", "abc"],
            vec!["git", "clean", "-fdx"],
            vec!["git", "tag", "v1.0"],
            vec!["git", "tag", "-d", "v1.0"],
            vec!["git", "branch", "newbranch"],
            vec!["git", "branch", "-D", "main"],
            vec!["git", "config", "user.name", "x"],
            vec!["git", "remote", "add", "origin", "url"],
            vec!["git", "remote", "set-url", "origin", "url"],
            vec!["git", "pull"],
            vec!["git", "am", "patch"],
            vec!["git", "apply", "patch"],
            vec!["git", "update-ref", "refs/heads/x", "abc"],
            vec!["git", "init"],
        ] {
            assert!(
                check_observe_only(&argv).is_err(),
                "expected BLOCK for {argv:?}"
            );
        }
    }

    #[test]
    fn blocks_git_push_with_leading_global_options() {
        assert!(check_observe_only(&["git", "-C", "/repo", "push", "origin", "main"]).is_err());
        assert!(
            check_observe_only(&["git", "-c", "user.name=x", "push", "origin", "main"]).is_err()
        );
    }

    // --- git reads are allowed ---

    #[test]
    fn allows_git_reads() {
        for argv in [
            vec!["git", "fetch", "origin"],
            vec!["git", "clone", "https://example/repo.git"],
            vec!["git", "log", "--oneline"],
            vec!["git", "status"],
            vec!["git", "diff", "HEAD~1"],
            vec!["git", "show", "HEAD"],
            vec!["git", "ls-remote", "origin"],
            vec!["git", "for-each-ref"],
            vec!["git", "branch"],
            vec!["git", "branch", "-a"],
            vec!["git", "branch", "--contains", "HEAD"],
            vec!["git", "tag"],
            vec!["git", "tag", "-l", "v*"],
            vec!["git", "config", "--get", "user.name"],
            vec!["git", "config", "--list"],
            vec!["git", "remote", "-v"],
            vec!["git", "remote", "show", "origin"],
            vec!["git", "rev-parse", "HEAD"],
            vec!["git", "-C", "/repo", "log"],
        ] {
            assert!(
                check_observe_only(&argv).is_ok(),
                "expected ALLOW for {argv:?}"
            );
        }
    }

    // --- az ---

    #[test]
    fn blocks_az_writes() {
        for argv in [
            vec!["az", "repos", "pr", "create", "--title", "x"],
            vec!["az", "boards", "work-item", "update", "--id", "1"],
            vec!["az", "boards", "work-item", "create"],
            vec!["az", "repos", "import", "create"],
            vec!["az", "pipelines", "run", "--name", "ci"],
            vec!["az", "repos", "policy", "create"],
            vec!["az", "devops", "security", "permission", "update"],
            vec!["az", "repos", "ref", "delete"],
        ] {
            assert!(
                check_observe_only(&argv).is_err(),
                "expected BLOCK for {argv:?}"
            );
        }
    }

    #[test]
    fn allows_az_reads() {
        for argv in [
            vec!["az", "repos", "list"],
            vec!["az", "repos", "pr", "list"],
            vec!["az", "boards", "work-item", "show", "--id", "1"],
            vec!["az", "repos", "ref", "list"],
            vec!["az", "account", "show"],
            vec!["az", "rest", "--method", "GET", "--url", "https://x"],
            vec!["az", "rest", "--url", "https://x"],
        ] {
            assert!(
                check_observe_only(&argv).is_ok(),
                "expected ALLOW for {argv:?}"
            );
        }
    }

    #[test]
    fn blocks_az_rest_writes() {
        assert!(check_observe_only(&["az", "rest", "--method", "POST", "--url", "u"]).is_err());
        assert!(
            check_observe_only(&["az", "rest", "-m", "put", "--url", "u", "--body", "@b"]).is_err()
        );
        assert!(check_observe_only(&["az", "rest", "--url", "u", "--body", "@b"]).is_err());
        assert!(check_observe_only(&["az", "rest", "-mPATCH", "--url", "u"]).is_err());
    }

    #[test]
    fn az_unrecognized_command_fails_closed() {
        // No read verb, no neutral head, no write verb → fail closed (block).
        assert!(check_observe_only(&["az", "repos", "pr"]).is_err());
    }

    // --- gh ---

    #[test]
    fn blocks_gh_writes() {
        for argv in [
            vec!["gh", "pr", "create"],
            vec!["gh", "pr", "merge", "1"],
            vec!["gh", "issue", "create"],
            vec!["gh", "issue", "comment", "1"],
            vec!["gh", "release", "create", "v1"],
            vec!["gh", "api", "-X", "POST", "/repos/x"],
            vec!["gh", "api", "--method", "DELETE", "/repos/x"],
            vec!["gh", "api", "-f", "name=x", "/repos/x"],
            vec!["gh", "api", "-XPATCH", "/repos/x"],
        ] {
            assert!(
                check_observe_only(&argv).is_err(),
                "expected BLOCK for {argv:?}"
            );
        }
    }

    #[test]
    fn allows_gh_reads() {
        for argv in [
            vec!["gh", "pr", "list"],
            vec!["gh", "pr", "view", "1"],
            vec!["gh", "issue", "list"],
            vec!["gh", "api", "/repos/x"],
            vec!["gh", "api", "-X", "GET", "/repos/x"],
        ] {
            assert!(
                check_observe_only(&argv).is_ok(),
                "expected ALLOW for {argv:?}"
            );
        }
    }

    // --- curl / wget ---

    #[test]
    fn blocks_http_writes() {
        for argv in [
            vec!["curl", "-X", "POST", "https://x"],
            vec!["curl", "-XPUT", "https://x"],
            vec!["curl", "-d", "@body", "https://x"],
            vec!["curl", "--data", "a=b", "https://x"],
            vec!["curl", "-T", "file", "https://x"],
            vec!["curl", "-X", "GET", "-X", "POST", "https://x"],
            vec!["wget", "--post-data", "a=b", "https://x"],
            vec!["wget", "--method=PUT", "https://x"],
        ] {
            assert!(
                check_observe_only(&argv).is_err(),
                "expected BLOCK for {argv:?}"
            );
        }
    }

    #[test]
    fn allows_http_reads() {
        for argv in [
            vec!["curl", "https://x"],
            vec!["curl", "-X", "GET", "https://x"],
            vec!["curl", "-sSL", "https://x"],
            vec!["curl", "-m", "5", "https://x"], // -m is max-time for curl, not a method
            vec!["wget", "https://x"],
            vec!["wget", "-O", "out", "https://x"],
        ] {
            assert!(
                check_observe_only(&argv).is_ok(),
                "expected ALLOW for {argv:?}"
            );
        }
    }

    // --- out of scope tools ---

    #[test]
    fn unknown_tools_are_out_of_scope() {
        // Command-level screening does not classify opaque interpreters; other
        // guardrail layers (no write credential, disabled capabilities) cover
        // them. This guard neither blocks nor claims to screen them.
        assert!(check_observe_only(&["python", "analyze.py"]).is_ok());
        assert!(check_observe_only(&["bash", "-c", "echo hi"]).is_ok());
        assert!(check_observe_only(&[]).is_ok());
    }

    // --- env gate ---

    #[serial_test::serial(cognitive_memory)]
    #[test]
    fn env_gate_only_enforces_when_enabled() {
        let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        set_observe(false);
        // Not observe-only: even a push is allowed by the env-gated entry point.
        assert!(guard_observe_only(&["git", "push", "origin", "main"]).is_ok());
        set_observe(true);
        assert!(guard_observe_only(&["git", "push", "origin", "main"]).is_err());
        // The always-on check ignores the env flag entirely.
        set_observe(false);
        assert!(check_observe_only(&["git", "push", "origin", "main"]).is_err());
        set_observe(false);
    }

    #[test]
    fn block_message_is_stable_and_marks_observe_only() {
        let err = check_observe_only(&["git", "push"]).unwrap_err();
        assert!(err.contains("GUARDRAIL BLOCKED"));
        assert!(err.contains("observe-only"));
    }

    #[test]
    fn is_write_command_matches_check() {
        assert!(is_write_command(&["git", "push"]));
        assert!(!is_write_command(&["git", "fetch"]));
    }
}
