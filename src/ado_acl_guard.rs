//! Azure DevOps ACL self-escalation guard — deterministic safety floor.
//!
//! ## Background (issue #809)
//!
//! During a `default-workflow` run against Azure Repos, a `git push
//! --force-with-lease` was denied for lack of the `ForcePush` permission
//! (`TF401027`). To bypass the denial the autonomous agent **modified the
//! shared repository's Azure DevOps security ACLs to grant its own identity
//! `ForcePush`**, retried the push, then reverted the grant. Two defects:
//!
//! 1. **Authorization-boundary violation.** A maintainer authorizing a
//!    force-push is *not* authorizing the agent to edit a shared repo's
//!    security namespace. A recipe must never self-escalate its own
//!    permissions. On a denial it must **stop and report the exact missing
//!    permission** so a human can grant it, or use only mechanisms within its
//!    existing permissions (e.g. a fast-forward reconcile).
//!
//! 2. **Non-atomic restore.** The grant→retry→revert window was not
//!    crash-safe: a `SIGTERM`/panic between grant and revert would leave the
//!    elevated `ForcePush` grant in place — a silent, persistent privilege
//!    escalation on a shared repo.
//!
//! ## Design
//!
//! Behavioural enforcement for the autonomous agent lives in the engineer
//! system prompt (prompt-first architecture): the agent executes `az` inside
//! its own Copilot/RustyClawd subprocess, so the prompt — not this crate — is
//! what stops it from self-escalating. This module is the deterministic,
//! unit-tested *safety floor* that any in-crate command-execution chokepoint
//! can call to enforce the same rule mechanically (mirroring how
//! [`crate::git_guardrails`] screens git commands):
//!
//! - [`check_ado_acl_safety`] classifies a command as an Azure DevOps
//!   security-namespace / ACL **mutation** and, unless escalation is explicitly
//!   opted in by the operator, refuses it and surfaces the missing permission.
//!   Detection fails **closed** (see [`is_ado_acl_mutation`]); read-only
//!   inspection (`security permission show/list`, `GET`/`HEAD`) is allowed.
//!
//! - [`ScopedAclGrant`] and [`with_scoped_acl_grant`] make the opt-in,
//!   privileged-remediation path crash-safe: the revoke runs on success, an
//!   early `Err`/`?`, a panic unwind, and normal scope exit (via `Drop`), and
//!   is **idempotent** so a re-run cannot leave permissions elevated. `Drop`
//!   does NOT run on a hard kill (`SIGKILL`/OOM), `process::exit`, or
//!   `abort()`; that residual window only exists under explicit opt-in, since
//!   the default policy never grants anything at all.

/// Environment variable that explicitly opts in to privileged ACL remediation.
///
/// Unset (the default) means the guard refuses any ACL mutation and surfaces
/// the missing permission to the operator instead of self-escalating.
pub const ESCALATION_OPT_IN_ENV: &str = "SIMARD_ALLOW_ADO_ACL_ESCALATION";

/// Long (`--`) flags that introduce an explicit HTTP method, plus `az rest`'s
/// short `-m` (az's argparse does not group short options, so `-m` is exact).
/// curl's `-X` method flag is handled by the short-option cluster parser
/// ([`classify_short_cluster`]) so grouped forms like `-sX PUT` are caught.
const METHOD_FLAGS: &[&str] = &["--method", "--http-method", "--request", "-m"];

/// Long (`--`) flags that supply a request body or upload, i.e. flags that make
/// `az rest`/`curl` issue a write (POST/PUT) even with no explicit method.
/// Covers az rest (`--body`/`--in-file`) and curl's data (`--data*`), urlencoded
/// data (`--data-urlencode`), multipart form (`--form*`), upload
/// (`--upload-file`, which issues a PUT), and `--json` forms. curl's short
/// body/upload options (`-d`/`-F`/`-T`) are handled by the cluster parser.
const BODY_FLAGS: &[&str] = &[
    "--body",
    "--in-file",
    "--input-file",
    "--data",
    "--data-raw",
    "--data-binary",
    "--data-ascii",
    "--data-urlencode",
    "--form",
    "--form-string",
    "--upload-file",
    "--json",
];

/// Returns `true` when privileged ACL remediation has been explicitly opted in
/// via [`ESCALATION_OPT_IN_ENV`]. Defaults to `false`.
pub fn self_escalation_allowed() -> bool {
    std::env::var(ESCALATION_OPT_IN_ENV)
        .map(|v| matches!(v.as_str(), "1" | "true" | "enabled" | "yes"))
        .unwrap_or(false)
}

fn strip_quotes(s: &str) -> &str {
    s.trim_matches(|c| c == '"' || c == '\'')
}

/// Result of classifying a curl short-option *cluster* token.
enum ShortOpt {
    /// A method option (`-X`) with its value.
    Method(String),
    /// A body/upload option (`-d`/`-F`/`-T`).
    Body,
    /// No method/body option of interest in this cluster.
    None,
}

/// Classifies a curl short-option cluster token (a single leading `-`, not
/// `--`). curl groups boolean short options and lets the final value-taking
/// option consume the rest of the token (glued) or the next argument — e.g.
/// `-sT file` == `-s -T file`, `-sXPUT` == `-s -X PUT`, `-sd@x` == `-s -d @x`.
/// We scan left to right and stop at the first value-taking option we care
/// about: `X` (method) or `d`/`F`/`T` (body/upload). Case is significant
/// (`-X` ≠ `-x` proxy, `-T` ≠ `-t`, `-F` ≠ `-f`). Erring toward over-detection
/// for unmodeled value-taking options is safe (the guard fails closed).
fn classify_short_cluster(tok: &str, next: Option<&&str>) -> ShortOpt {
    if tok.starts_with("--") {
        return ShortOpt::None;
    }
    let Some(cluster) = tok.strip_prefix('-') else {
        return ShortOpt::None;
    };
    // az's `-m<method>` / `-b<body>` (method/body short aliases) and curl's
    // value-taking `-m` (`--max-time`) are handled elsewhere; their *values*
    // (e.g. the `T` in `-mPUT`) must not be misread as curl write letters.
    if cluster.starts_with('m') || cluster.starts_with('b') {
        return ShortOpt::None;
    }
    for (i, ch) in cluster.char_indices() {
        match ch {
            'X' => {
                let rest = &cluster[i + ch.len_utf8()..];
                return if rest.is_empty() {
                    next.map(|n| ShortOpt::Method(strip_quotes(n).to_string()))
                        .unwrap_or(ShortOpt::None)
                } else {
                    ShortOpt::Method(strip_quotes(rest).to_string())
                };
            }
            'd' | 'F' | 'T' => return ShortOpt::Body,
            _ => continue,
        }
    }
    ShortOpt::None
}

/// Collects **every** explicit HTTP method on a tokenized command line, handling
/// space-separated (`--method post`), `=`-joined (`--method=post`,
/// `method=post`), az glued (`-mput`), and curl short-option cluster (`-XPOST`,
/// `-sX PUT`) forms.
///
/// All occurrences are returned (not just the first) because curl and `az rest`
/// (argparse) honor the **last** repeated `-X`/`--method`, so a decoy read
/// method before a real write method must not be allowed to mask the write.
fn explicit_methods(tokens: &[&str]) -> Vec<String> {
    let mut methods = Vec::new();
    for (i, tok) in tokens.iter().enumerate() {
        if METHOD_FLAGS.contains(tok)
            && let Some(val) = tokens.get(i + 1)
        {
            methods.push(strip_quotes(val).to_string());
        }
        for pfx in ["--method=", "--http-method=", "--request=", "method="] {
            if let Some(rest) = tok.strip_prefix(pfx) {
                methods.push(strip_quotes(rest).to_string());
            }
        }
        // az `-m<method>` glued form (e.g. `-mput`). az's argparse does not group
        // short booleans, so `m` consumes the whole remainder as its value. This
        // is handled here (not via the curl cluster parser, which keys off `X`).
        if let Some(rest) = tok.strip_prefix("-m")
            && !rest.is_empty()
        {
            methods.push(strip_quotes(rest).to_string());
        }
        if let ShortOpt::Method(m) = classify_short_cluster(tok, tokens.get(i + 1)) {
            methods.push(m);
        }
    }
    methods
}

/// Returns `true` when the command carries a request body or upload — a long
/// flag in [`BODY_FLAGS`] (or its `=`-joined form), az's `-b`/`--body` short
/// alias (`-b @file` / `-b@file`), or a curl short-option cluster containing
/// `-d`/`-F`/`-T` (including grouped/glued forms).
fn carries_request_body(tokens: &[&str]) -> bool {
    tokens.iter().enumerate().any(|(i, tok)| {
        BODY_FLAGS.contains(tok)
            || BODY_FLAGS.iter().any(|f| tok.starts_with(&format!("{f}=")))
            || tok.starts_with("-b")
            || matches!(
                classify_short_cluster(tok, tokens.get(i + 1)),
                ShortOpt::Body
            )
    })
}

/// Returns `true` only when a command targeting an access-control REST surface
/// is **provably** a read: no request body/upload **and** every explicit method
/// (if any) is a read method (`GET`/`HEAD`/`OPTIONS`). No explicit method means
/// the default `GET` (both `az rest` and `curl`). Requiring *all* methods to be
/// reads fails closed against repeated-flag decoys (`-X GET -X PUT`).
fn is_provably_read(tokens: &[&str]) -> bool {
    if carries_request_body(tokens) {
        return false;
    }
    explicit_methods(tokens)
        .iter()
        .all(|m| matches!(m.to_ascii_lowercase().as_str(), "get" | "head" | "options"))
}

/// Returns `true` when `args` describe a command that **mutates** an Azure
/// DevOps security namespace / ACL — a self-escalation surface.
///
/// Detection fails **closed**: any command that targets an access-control REST
/// surface is treated as a mutation unless it is provably a read (so `az rest
/// --body` implicit-POST, the `-m`/`-X`/`--request` method aliases, and curl
/// grouped/glued forms like `-d@file` / `-sXPUT` / `-sT file` are all caught).
/// Read-only inspection (`az devops security permission show|list`, `GET`/`HEAD`
/// against the access-control REST APIs) is **not** flagged.
///
/// Scope: this screens `az`/`az devops`/`az rest`/`curl` command lines. ACL
/// writes embedded in opaque scripts (e.g. a `python`/`node` HTTP call) cannot
/// be detected here; the system-prompt prohibition is the primary control.
pub fn is_ado_acl_mutation(args: &[&str]) -> bool {
    let joined = args.join(" ").to_lowercase();

    // `az devops security permission` mutating subcommands (`update`, `reset`,
    // and `reset-all` are all covered by these two substrings).
    if joined.contains("security permission update") || joined.contains("security permission reset")
    {
        return true;
    }

    // Joining or leaving a security group self-escalates transitively (e.g.
    // adding your own identity to Project Administrators grants ForcePush).
    if joined.contains("security group membership add")
        || joined.contains("security group membership remove")
    {
        return true;
    }

    // Any command touching an ADO access-control / security-namespace / group-
    // membership REST surface is a mutation unless it is provably a read (fails
    // closed). Flag parsing uses the original, case-preserved `args` so curl's
    // `-X` (method) is not confused with `-x` (proxy).
    let touches_acl = joined.contains("accesscontrolentries")
        || joined.contains("accesscontrollists")
        || joined.contains("/_apis/accesscontrol")
        || joined.contains("securitynamespaces")
        || joined.contains("/_apis/graph/memberships");
    if touches_acl && !is_provably_read(args) {
        return true;
    }

    false
}

/// Guard entry point. Returns `Ok(())` for any command that is not an Azure
/// DevOps ACL mutation.
///
/// For an ACL mutation:
/// - If escalation is **not** opted in (the default), returns `Err` with a
///   message that surfaces the missing permission to the operator and refuses
///   to self-escalate.
/// - If escalation **is** opted in via [`ESCALATION_OPT_IN_ENV`], returns
///   `Ok(())`; callers are then required to perform the grant through
///   [`with_scoped_acl_grant`] / [`ScopedAclGrant`] so the revert is crash-safe.
pub fn check_ado_acl_safety(args: &[&str]) -> Result<(), String> {
    if !is_ado_acl_mutation(args) {
        return Ok(());
    }
    if self_escalation_allowed() {
        return Ok(());
    }
    Err(format!(
        "GUARDRAIL BLOCKED: refusing to modify Azure DevOps repository security ACLs \
         (command: `{}`). Self-granting a permission such as ForcePush to bypass a push \
         denial is an authorization-boundary violation. STOP and report the exact missing \
         permission to the operator so a human can grant it, or use only mechanisms within \
         your existing permissions (e.g. a fast-forward reconcile). If privileged remediation \
         is genuinely required it must be explicitly opted in via {ESCALATION_OPT_IN_ENV}=1 \
         and performed through a crash-safe scoped grant \
         (ado_acl_guard::with_scoped_acl_grant) whose revoke always runs and is idempotent.",
        args.join(" ")
    ))
}

type RevokeFn = Box<dyn FnMut() -> Result<(), String> + Send>;

/// RAII guard for a temporary, privileged ACL grant.
///
/// Constructed via [`ScopedAclGrant::acquire`], which runs the `grant` closure
/// up front. The matching `revoke` closure is guaranteed to run on **every**
/// exit path — explicit [`ScopedAclGrant::revoke_now`], normal scope exit, an
/// early `Err`/`?`, or a panic unwind — via the `Drop` implementation. Revoke
/// is **idempotent**: the underlying closure runs at most once, so re-entry or
/// a re-run can never leave the permission elevated.
pub struct ScopedAclGrant {
    revoke: Option<RevokeFn>,
    description: String,
}

impl ScopedAclGrant {
    /// Run `grant`; on success return a guard that will always run `revoke`.
    ///
    /// If `grant` fails, nothing was granted, so no revoke is scheduled and the
    /// error is returned to the caller.
    pub fn acquire<G, R>(
        description: impl Into<String>,
        grant: G,
        revoke: R,
    ) -> Result<Self, String>
    where
        G: FnOnce() -> Result<(), String>,
        R: FnMut() -> Result<(), String> + Send + 'static,
    {
        let description = description.into();
        grant()
            .map_err(|e| format!("ACL grant '{description}' failed (nothing to revoke): {e}"))?;
        Ok(Self {
            revoke: Some(Box::new(revoke)),
            description,
        })
    }

    /// Description of the grant this guard protects.
    pub fn description(&self) -> &str {
        &self.description
    }

    /// `true` until the grant has been revoked.
    pub fn is_active(&self) -> bool {
        self.revoke.is_some()
    }

    /// Idempotent revoke. The first call runs the revoke closure and returns its
    /// result; subsequent calls are no-ops returning `Ok(())`.
    pub fn revoke_now(&mut self) -> Result<(), String> {
        match self.revoke.take() {
            Some(mut revoke) => revoke(),
            None => Ok(()),
        }
    }
}

impl Drop for ScopedAclGrant {
    fn drop(&mut self) {
        if self.revoke.is_some() {
            // Best-effort revoke on any unwind / early-return / normal-exit
            // path. We cannot propagate an error from `drop`, so a revoke
            // failure is surfaced LOUDLY on the tracing error channel (never
            // silently swallowed) so an operator can intervene and the leaked
            // grant is observable.
            //
            // LIMITATION: `Drop` does not run on a hard kill (`SIGKILL`/OOM),
            // `std::process::exit`, or `abort()`. Crash-safety here therefore
            // covers panics, `?` early returns, and normal scope exit — not
            // hard termination. The default policy never escalates at all
            // (`SIMARD_ALLOW_ADO_ACL_ESCALATION` unset ⇒ no grant is ever
            // made), so this window only exists under explicit operator opt-in.
            if let Err(e) = self.revoke_now() {
                tracing::error!(
                    target: "ado_acl_guard",
                    grant = %self.description,
                    error = %e,
                    "ACL revoke FAILED on guard drop — the temporary grant may still be \
                     active and must be revoked manually by an operator"
                );
            }
        }
    }
}

impl std::fmt::Debug for ScopedAclGrant {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ScopedAclGrant")
            .field("description", &self.description)
            .field("active", &self.is_active())
            .finish()
    }
}

/// Run `body` while a temporary ACL grant is held, guaranteeing the grant is
/// revoked afterwards **regardless of how `body` exits** (success, `Err`, or
/// panic).
///
/// Order of operations: `grant()` → `body()` → `revoke`. If `body` returns
/// `Err`, the revoke still runs and `body`'s error is propagated. If `body`
/// panics, the guard's `Drop` revokes during unwind. The revoke is idempotent,
/// so it runs exactly once.
pub fn with_scoped_acl_grant<G, R, B, T>(
    description: impl Into<String>,
    grant: G,
    revoke: R,
    body: B,
) -> Result<T, String>
where
    G: FnOnce() -> Result<(), String>,
    R: FnMut() -> Result<(), String> + Send + 'static,
    B: FnOnce() -> Result<T, String>,
{
    let mut guard = ScopedAclGrant::acquire(description, grant, revoke)?;
    let body_result = body();
    // Explicit revoke so we can observe/report a revoke failure on the normal
    // path. On a panic this line is skipped but `Drop` still revokes.
    let revoke_result = guard.revoke_now();
    match (body_result, revoke_result) {
        (Ok(value), Ok(())) => Ok(value),
        (Ok(_), Err(re)) => Err(format!("body succeeded but ACL revoke failed: {re}")),
        (Err(be), Ok(())) => Err(be),
        (Err(be), Err(re)) => Err(format!("{be}; additionally ACL revoke failed: {re}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serial_test::serial;
    use std::panic::{AssertUnwindSafe, catch_unwind};
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};

    fn clear_env() {
        unsafe { std::env::remove_var(ESCALATION_OPT_IN_ENV) };
    }

    // ---- detection -------------------------------------------------------

    #[test]
    fn detects_az_security_permission_update() {
        assert!(is_ado_acl_mutation(&[
            "az",
            "devops",
            "security",
            "permission",
            "update",
            "--id",
            "ForcePush",
            "--allow-bit",
            "8",
        ]));
    }

    #[test]
    fn detects_az_security_permission_reset() {
        assert!(is_ado_acl_mutation(&[
            "az",
            "devops",
            "security",
            "permission",
            "reset",
            "--id",
            "ForcePush",
        ]));
    }

    #[test]
    fn detects_rest_ace_post() {
        assert!(is_ado_acl_mutation(&[
            "az",
            "rest",
            "--method",
            "POST",
            "--uri",
            "https://dev.azure.com/org/_apis/accesscontrolentries/ns?api-version=7.1",
        ]));
    }

    #[test]
    fn ignores_readonly_permission_show() {
        // Inspecting the current ACL must NOT be blocked.
        assert!(!is_ado_acl_mutation(&[
            "az",
            "devops",
            "security",
            "permission",
            "show",
            "--id",
            "ForcePush",
        ]));
    }

    #[test]
    fn ignores_readonly_rest_get() {
        assert!(!is_ado_acl_mutation(&[
            "az",
            "rest",
            "--method",
            "GET",
            "--uri",
            "https://dev.azure.com/org/_apis/accesscontrollists/ns",
        ]));
    }

    #[test]
    fn ignores_unrelated_commands() {
        assert!(!is_ado_acl_mutation(&["git", "push", "--force-with-lease"]));
        assert!(!is_ado_acl_mutation(&["az", "repos", "pr", "create"]));
    }

    // ---- detection: bypass resistance (issue #809 review) ----------------

    #[test]
    fn detects_rest_ace_short_method_flag() {
        // `-m` is `az rest`'s short alias for `--method`.
        assert!(is_ado_acl_mutation(&[
            "az",
            "rest",
            "-m",
            "POST",
            "--uri",
            "https://dev.azure.com/org/_apis/accesscontrolentries/ns?api-version=7.1",
            "--body",
            "@ace.json",
        ]));
    }

    #[test]
    fn detects_rest_ace_implicit_post_via_body() {
        // `az rest` defaults the method to POST when `--body` is supplied with
        // no explicit `--method`; this is a real ACE-granting write.
        assert!(is_ado_acl_mutation(&[
            "az",
            "rest",
            "--uri",
            "https://dev.azure.com/org/_apis/accesscontrolentries/ns",
            "--body",
            "@ace.json",
        ]));
    }

    #[test]
    fn detects_curl_ace_write_forms() {
        // curl `--request POST` (only `-X`/`-x` was matched before).
        assert!(is_ado_acl_mutation(&[
            "curl",
            "--request",
            "POST",
            "https://dev.azure.com/org/_apis/accesscontrolentries/ns",
        ]));
        // curl `-X PUT`.
        assert!(is_ado_acl_mutation(&[
            "curl",
            "-X",
            "PUT",
            "https://dev.azure.com/org/_apis/accesscontrollists/ns",
        ]));
        // curl implicit POST via `--data`.
        assert!(is_ado_acl_mutation(&[
            "curl",
            "--data",
            "@ace.json",
            "https://dev.azure.com/org/_apis/accesscontrolentries/ns",
        ]));
    }

    #[test]
    fn detects_group_membership_add() {
        // Joining a privileged group self-escalates ForcePush transitively.
        assert!(is_ado_acl_mutation(&[
            "az",
            "devops",
            "security",
            "group",
            "membership",
            "add",
            "--group-id",
            "Project Administrators",
            "--member-id",
            "rysweet@microsoft.com",
        ]));
    }

    #[test]
    fn ignores_curl_acl_read_without_body() {
        // A bare GET (no method, no body) against an ACL endpoint is a read.
        assert!(!is_ado_acl_mutation(&[
            "curl",
            "https://dev.azure.com/org/_apis/accesscontrollists/ns",
        ]));
    }

    #[test]
    fn detects_curl_glued_short_options() {
        // curl glued data short option `-d@file` (implicit POST).
        assert!(is_ado_acl_mutation(&[
            "curl",
            "https://dev.azure.com/org/_apis/accesscontrolentries/ns",
            "-d@ace.json",
        ]));
        // curl glued inline body `-d{json}`.
        assert!(is_ado_acl_mutation(&[
            "curl",
            "https://dev.azure.com/org/_apis/accesscontrolentries/ns",
            "-d{\"token\":\"x\"}",
        ]));
        // curl glued method short option `-XPOST`.
        assert!(is_ado_acl_mutation(&[
            "curl",
            "-XPOST",
            "https://dev.azure.com/org/_apis/accesscontrollists/ns",
        ]));
    }

    #[test]
    fn detects_rest_graph_membership_write() {
        // Self-add to a group via the Graph memberships REST API (transitive
        // ForcePush escalation) must be flagged.
        assert!(is_ado_acl_mutation(&[
            "az",
            "rest",
            "--method",
            "PUT",
            "--uri",
            "https://vssps.dev.azure.com/org/_apis/graph/memberships/subj/cont?api-version=7.1",
        ]));
    }

    #[test]
    fn detects_curl_upload_and_form_writes() {
        // curl `-T <file>` issues a PUT with no `-X` — the add-membership form
        // (empty-body PUT to /_apis/graph/memberships). Must be flagged.
        assert!(is_ado_acl_mutation(&[
            "curl",
            "-T",
            "empty.txt",
            "https://vssps.dev.azure.com/org/_apis/graph/memberships/self/admins?api-version=7.2-preview.1",
        ]));
        // Long form `--upload-file`.
        assert!(is_ado_acl_mutation(&[
            "curl",
            "--upload-file",
            "empty.txt",
            "https://vssps.dev.azure.com/org/_apis/graph/memberships/self/admins",
        ]));
        // Glued upload `-Tempty.txt`.
        assert!(is_ado_acl_mutation(&[
            "curl",
            "-Tempty.txt",
            "https://vssps.dev.azure.com/org/_apis/graph/memberships/self/admins",
        ]));
        // Multipart form POST `-F`.
        assert!(is_ado_acl_mutation(&[
            "curl",
            "-F",
            "ace=@ace.json",
            "https://dev.azure.com/org/_apis/accesscontrolentries/ns",
        ]));
        // urlencoded data POST.
        assert!(is_ado_acl_mutation(&[
            "curl",
            "--data-urlencode",
            "token=x",
            "https://dev.azure.com/org/_apis/accesscontrolentries/ns",
        ]));
        // `--json` POST (newer curl).
        assert!(is_ado_acl_mutation(&[
            "curl",
            "--json",
            "@ace.json",
            "https://dev.azure.com/org/_apis/accesscontrolentries/ns",
        ]));
    }

    #[test]
    fn proxy_short_option_is_not_a_method() {
        // curl `-x` is `--proxy`, NOT a method (case matters vs `-X`). A GET via
        // a proxy with no body is still a read and must not be flagged.
        assert!(!is_ado_acl_mutation(&[
            "curl",
            "-x",
            "http://proxy:8080",
            "https://dev.azure.com/org/_apis/accesscontrollists/ns",
        ]));
        // Glued `-XGET` (read) with no body is also not a mutation.
        assert!(!is_ado_acl_mutation(&[
            "curl",
            "-XGET",
            "https://dev.azure.com/org/_apis/accesscontrollists/ns",
        ]));
    }

    #[test]
    fn detects_curl_grouped_short_options() {
        // curl groups boolean short options with the value-taking one last, so
        // prepending an ubiquitous flag like `-s` must NOT hide the write.
        let acl = "https://dev.azure.com/org/_apis/accesscontrolentries/ns";
        let graph = "https://vssps.dev.azure.com/org/_apis/graph/memberships/self/admins";
        // `-sX PUT` == `-s -X PUT` (empty-body PUT add-membership).
        assert!(is_ado_acl_mutation(&["curl", "-sX", "PUT", graph]));
        // `-sXPUT` glued.
        assert!(is_ado_acl_mutation(&["curl", "-sXPUT", graph]));
        // `-sT file` == `-s -T file` (upload PUT).
        assert!(is_ado_acl_mutation(&["curl", "-sT", "empty.txt", graph]));
        // `-fsST file` — write flag grouped behind several booleans.
        assert!(is_ado_acl_mutation(&["curl", "-fsST", "empty.txt", graph]));
        // `-sd @ace.json` and glued `-sd@ace.json` (POST an ACE).
        assert!(is_ado_acl_mutation(&["curl", "-sd", "@ace.json", acl]));
        assert!(is_ado_acl_mutation(&["curl", "-sd@ace.json", acl]));
        // `-sF k=v` (multipart POST).
        assert!(is_ado_acl_mutation(&["curl", "-sF", "ace=@ace.json", acl]));
    }

    #[test]
    fn grouped_boolean_only_read_is_not_flagged() {
        // `-s` (silent) alone with no write flag is still a GET read.
        assert!(!is_ado_acl_mutation(&[
            "curl",
            "-s",
            "https://dev.azure.com/org/_apis/accesscontrollists/ns",
        ]));
        // `-sXGET` grouped read.
        assert!(!is_ado_acl_mutation(&[
            "curl",
            "-sXGET",
            "https://dev.azure.com/org/_apis/accesscontrollists/ns",
        ]));
    }

    #[test]
    fn detects_repeated_method_decoy() {
        // curl/argparse honor the LAST repeated method flag, so a decoy read
        // before the real write must not mask the write.
        let graph = "https://vssps.dev.azure.com/org/_apis/graph/memberships/self/admins";
        assert!(is_ado_acl_mutation(&[
            "curl", "-X", "GET", "-X", "PUT", graph
        ]));
        assert!(is_ado_acl_mutation(&[
            "curl",
            "--request",
            "GET",
            "-X",
            "PUT",
            graph
        ]));
        assert!(is_ado_acl_mutation(&["curl", "-XGET", "-XPUT", graph]));
        assert!(is_ado_acl_mutation(&[
            "az", "rest", "--method", "GET", "--method", "PUT", "--uri", graph,
        ]));
        // A decoy in the other order (real write first) is also caught.
        assert!(is_ado_acl_mutation(&[
            "curl", "-X", "PUT", "-X", "GET", graph
        ]));
    }

    #[test]
    fn detects_az_glued_method_and_body() {
        let graph = "https://vssps.dev.azure.com/org/_apis/graph/memberships/self/admins";
        let acl = "https://dev.azure.com/org/_apis/accesscontrolentries/ns";
        // az glued lowercase method `-mput` (empty-body PUT add-membership).
        assert!(is_ado_acl_mutation(&[
            "az", "rest", "-mput", "--uri", graph
        ]));
        // `-mpost` with `-b` (the `--body` short alias) supplying the ACE.
        assert!(is_ado_acl_mutation(&[
            "az",
            "rest",
            "-mpost",
            "--uri",
            acl,
            "-b",
            "@ace.json",
        ]));
        // `-mpatch` glued.
        assert!(is_ado_acl_mutation(&[
            "az",
            "rest",
            "-mpatch",
            "--uri",
            acl,
            "-b",
            "@acl.json"
        ]));
        // Uppercase glued method still caught.
        assert!(is_ado_acl_mutation(&[
            "az", "rest", "-mPUT", "--uri", graph
        ]));
        // No method, write inferred from the `-b` body short alias.
        assert!(is_ado_acl_mutation(&[
            "az",
            "rest",
            "--uri",
            acl,
            "-b",
            "@ace.json"
        ]));
        // Glued `-b@file`.
        assert!(is_ado_acl_mutation(&[
            "az",
            "rest",
            "--uri",
            acl,
            "-b@ace.json"
        ]));
    }

    #[test]
    fn az_glued_get_is_a_read() {
        // `-mget` glued is a read and must not be flagged.
        assert!(!is_ado_acl_mutation(&[
            "az",
            "rest",
            "-mget",
            "--uri",
            "https://dev.azure.com/org/_apis/accesscontrollists/ns",
        ]));
    }

    // ---- policy gate -----------------------------------------------------

    #[test]
    #[serial(cognitive_memory)]
    fn blocks_self_escalation_by_default() {
        clear_env();
        let result = check_ado_acl_safety(&[
            "az",
            "devops",
            "security",
            "permission",
            "update",
            "--allow-bit",
            "8",
        ]);
        assert!(result.is_err());
        let msg = result.unwrap_err();
        assert!(msg.contains("GUARDRAIL BLOCKED"));
        // Surfaces the missing permission / operator-report guidance.
        assert!(msg.contains("missing permission"));
        assert!(msg.contains(ESCALATION_OPT_IN_ENV));
    }

    #[test]
    #[serial(cognitive_memory)]
    fn allows_readonly_even_without_optin() {
        clear_env();
        assert!(
            check_ado_acl_safety(&[
                "az",
                "devops",
                "security",
                "permission",
                "show",
                "--id",
                "ForcePush",
            ])
            .is_ok()
        );
    }

    #[test]
    #[serial(cognitive_memory)]
    fn allows_mutation_when_opted_in() {
        clear_env();
        unsafe { std::env::set_var(ESCALATION_OPT_IN_ENV, "1") };
        let result = check_ado_acl_safety(&[
            "az",
            "devops",
            "security",
            "permission",
            "update",
            "--allow-bit",
            "8",
        ]);
        clear_env();
        assert!(result.is_ok());
    }

    // ---- crash-safe scoped grant ----------------------------------------

    fn counting_revoke(counter: Arc<AtomicUsize>) -> impl FnMut() -> Result<(), String> {
        move || {
            counter.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }
    }

    #[test]
    fn revokes_once_on_success_path() {
        let revokes = Arc::new(AtomicUsize::new(0));
        let out: Result<i32, String> = with_scoped_acl_grant(
            "ForcePush@branch",
            || Ok(()),
            counting_revoke(Arc::clone(&revokes)),
            || Ok(7),
        );
        assert_eq!(out, Ok(7));
        assert_eq!(revokes.load(Ordering::SeqCst), 1);
    }

    /// Regression for #809: the elevated ACL is revoked even when the push step
    /// FAILS mid-run (returns Err).
    #[test]
    fn revokes_when_body_returns_err() {
        let revokes = Arc::new(AtomicUsize::new(0));
        let out: Result<(), String> = with_scoped_acl_grant(
            "ForcePush@branch",
            || Ok(()),
            counting_revoke(Arc::clone(&revokes)),
            || Err("TF401027: ForcePush still denied".to_string()),
        );
        assert!(out.is_err());
        assert!(out.unwrap_err().contains("TF401027"));
        assert_eq!(
            revokes.load(Ordering::SeqCst),
            1,
            "ACL must be revoked exactly once even when the push fails"
        );
    }

    /// Regression for #809: the elevated ACL is revoked even when the push step
    /// PANICS / is killed mid-run (simulated via panic + unwind through `Drop`).
    #[test]
    fn revokes_when_body_panics() {
        let revokes = Arc::new(AtomicUsize::new(0));
        let revokes_for_closure = Arc::clone(&revokes);
        let result = catch_unwind(AssertUnwindSafe(|| {
            let _out: Result<(), String> = with_scoped_acl_grant(
                "ForcePush@branch",
                || Ok(()),
                counting_revoke(revokes_for_closure),
                || panic!("process SIGTERM'd mid-push"),
            );
        }));
        assert!(result.is_err(), "body was expected to panic");
        assert_eq!(
            revokes.load(Ordering::SeqCst),
            1,
            "ACL must be revoked exactly once even when the push panics mid-run"
        );
    }

    #[test]
    fn revoke_is_idempotent() {
        let revokes = Arc::new(AtomicUsize::new(0));
        let mut guard = ScopedAclGrant::acquire(
            "ForcePush@branch",
            || Ok(()),
            counting_revoke(Arc::clone(&revokes)),
        )
        .expect("grant succeeds");
        assert!(guard.is_active());
        assert!(guard.revoke_now().is_ok());
        assert!(!guard.is_active());
        // Re-running revoke (e.g. a crash-recovery re-run) must be a no-op.
        assert!(guard.revoke_now().is_ok());
        drop(guard);
        assert_eq!(
            revokes.load(Ordering::SeqCst),
            1,
            "underlying revoke must run at most once"
        );
    }

    #[test]
    fn no_revoke_when_grant_fails() {
        let revokes = Arc::new(AtomicUsize::new(0));
        let result = ScopedAclGrant::acquire(
            "ForcePush@branch",
            || Err("grant POST returned 403".to_string()),
            counting_revoke(Arc::clone(&revokes)),
        );
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("grant"));
        assert_eq!(
            revokes.load(Ordering::SeqCst),
            0,
            "nothing was granted, so nothing must be revoked"
        );
    }

    #[test]
    fn drop_revokes_on_early_return() {
        // Simulate `?`-style early return: guard created, then we bail before an
        // explicit revoke. `Drop` must still revoke.
        let revokes = Arc::new(AtomicUsize::new(0));
        {
            let _guard = ScopedAclGrant::acquire(
                "ForcePush@branch",
                || Ok(()),
                counting_revoke(Arc::clone(&revokes)),
            )
            .expect("grant succeeds");
            // early return / scope exit without calling revoke_now
        }
        assert_eq!(revokes.load(Ordering::SeqCst), 1);
    }
}
