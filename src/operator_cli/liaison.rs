//! The agent-facing WRITE tool `simard liaison record-decision` (issue #4911,
//! Deliverable 1, design component C10).
//!
//! The `operator-liaison` recipe calls this to durably record what it decided
//! about one operator-group message: an optional plain-English reply to post
//! back, and/or an optional intervention directive. The thin deterministic rail
//! ([`crate::overseer::signal_liaison`]) later READs the typed
//! [`crate::stewardship::liaison_decision_store::LiaisonDecisionRecord`] back —
//! it never scrapes prose. All validation lives in the parser, which rejects a
//! contradictory invocation LOUDLY (usage error → exit code 2) rather than
//! writing a contradictory record.
//!
//! Large free text (`reply`, the directive `task_description`, the directive
//! context) always rides a FILE, never argv, so no invocation can hit `E2BIG`.

use std::path::PathBuf;

use crate::state_root::simard_state_root;
use crate::stewardship::liaison_decision_store::{Directive, LiaisonDecisionRecord, write_record};
use crate::stewardship::merge_verdict_store::validate_repo_slug;

/// Exit code: the decision cleared validation and was written.
const EXIT_RECORDED: i32 = 0;
/// Exit code: a required flag was missing or the invocation was contradictory.
const EXIT_USAGE: i32 = 2;
/// Exit code: the record could not be written (state-root / IO).
const EXIT_IO: i32 = 3;

const LIAISON_HELP: &str = "\
simard liaison record-decision — record the operator-liaison agent's decision.

Required:
  --group-id <ID>          the operator group the message belongs to
  --message-id <ID>        the message high-water-mark id being answered
  --run-token <TOKEN>      this run's opaque freshness token

At least one of a reply and a complete directive is required:
  --reply-path <FILE>      file whose contents are the plain-English reply

Directive flags are ALL-OR-NOTHING:
  --directive-recipe <NAME>        recipe to launch (e.g. default-workflow)
  --directive-task-path <FILE>     file whose contents are the task description
  --directive-repo <owner/name>    validated repo the recipe targets
  --directive-context-path <FILE>  ContextFile carrying the full operator context

Optional:
  --state-root <DIR>       override the durable state root (tests/fixtures)

Exit codes:
  0  recorded       the decision was written
  2  usage error    a flag was missing/malformed or the invocation contradictory
  3  io error       the record could not be written
";

/// The parsed directive half of a liaison decision. All four fields are
/// required together (the parser enforces all-or-nothing).
#[derive(Debug)]
pub(crate) struct DirectiveArgs {
    /// The recipe to launch (e.g. `"default-workflow"`).
    pub recipe: String,
    /// The task description, read from `--directive-task-path` (off argv).
    pub task_description: String,
    /// The validated `owner/name` repo the recipe targets.
    pub target_repo: String,
    /// The full operator context, read from `--directive-context-path` (off
    /// argv). Persisted so the rail can hand it to the launched recipe.
    pub context: String,
}

/// A parsed `simard liaison record-decision` invocation.
#[derive(Debug)]
pub(crate) struct LiaisonRecordDecisionArgs {
    pub group_id: String,
    pub message_id: String,
    pub run_token: String,
    /// Optional plain-English reply, read from `--reply-path` (off argv).
    pub reply: Option<String>,
    /// Optional intervention directive (all-or-nothing).
    pub directive: Option<DirectiveArgs>,
    /// Optional durable state-root override (tests/fixtures).
    pub state_root: Option<PathBuf>,
}

/// Take a flag's value: inline (`--flag=value`) or the next token
/// (`--flag value`). Errors if neither is available.
fn flag_value(
    flag: &str,
    inline: Option<String>,
    next: &mut dyn Iterator<Item = String>,
) -> Result<String, String> {
    match inline {
        Some(v) => Ok(v),
        None => next
            .next()
            .ok_or_else(|| format!("--{flag} requires a value")),
    }
}

/// Read a required file argument's contents (large payloads ride files, never
/// argv → no E2BIG).
fn read_file_arg(flag: &str, path: &str) -> Result<String, String> {
    std::fs::read_to_string(path).map_err(|e| format!("--{flag} {path}: could not read file: {e}"))
}

/// Parse + validate `liaison record-decision` argv into typed
/// [`LiaisonRecordDecisionArgs`]. All enforcement lives here — a partial
/// directive, an empty decision (neither reply nor directive), and an invalid
/// `--directive-repo` slug all fail LOUDLY rather than writing a bad record.
pub(crate) fn parse_liaison_record_decision_args(
    args: Vec<String>,
) -> Result<LiaisonRecordDecisionArgs, String> {
    let mut group_id: Option<String> = None;
    let mut message_id: Option<String> = None;
    let mut run_token: Option<String> = None;
    let mut reply_path: Option<String> = None;
    let mut directive_recipe: Option<String> = None;
    let mut directive_task_path: Option<String> = None;
    let mut directive_repo: Option<String> = None;
    let mut directive_context_path: Option<String> = None;
    let mut state_root: Option<PathBuf> = None;

    let mut iter = args.into_iter();
    while let Some(arg) = iter.next() {
        let Some(rest) = arg.strip_prefix("--") else {
            return Err(format!("unexpected positional argument {arg:?}"));
        };
        let (key, inline) = match rest.split_once('=') {
            Some((k, v)) => (k.to_string(), Some(v.to_string())),
            None => (rest.to_string(), None),
        };
        match key.as_str() {
            "group-id" => group_id = Some(flag_value("group-id", inline, &mut iter)?),
            "message-id" => message_id = Some(flag_value("message-id", inline, &mut iter)?),
            "run-token" => run_token = Some(flag_value("run-token", inline, &mut iter)?),
            "reply-path" => reply_path = Some(flag_value("reply-path", inline, &mut iter)?),
            "directive-recipe" => {
                directive_recipe = Some(flag_value("directive-recipe", inline, &mut iter)?)
            }
            "directive-task-path" => {
                directive_task_path = Some(flag_value("directive-task-path", inline, &mut iter)?)
            }
            "directive-repo" => {
                directive_repo = Some(flag_value("directive-repo", inline, &mut iter)?)
            }
            "directive-context-path" => {
                directive_context_path =
                    Some(flag_value("directive-context-path", inline, &mut iter)?)
            }
            "state-root" => {
                state_root = Some(PathBuf::from(flag_value("state-root", inline, &mut iter)?))
            }
            other => return Err(format!("unknown flag --{other}")),
        }
    }

    let group_id = group_id.ok_or_else(|| "missing required --group-id".to_string())?;
    if group_id.trim().is_empty() {
        return Err("--group-id must be non-empty".to_string());
    }
    let message_id = message_id.ok_or_else(|| "missing required --message-id".to_string())?;
    if message_id.trim().is_empty() {
        return Err("--message-id must be non-empty".to_string());
    }
    let run_token = run_token.ok_or_else(|| "missing required --run-token".to_string())?;
    if run_token.trim().is_empty() {
        return Err("--run-token must be non-empty".to_string());
    }

    let reply = match reply_path {
        Some(p) => Some(read_file_arg("reply-path", &p)?),
        None => None,
    };

    // The four directive flags are ALL-OR-NOTHING.
    let directive_present = directive_recipe.is_some()
        || directive_task_path.is_some()
        || directive_repo.is_some()
        || directive_context_path.is_some();
    let directive_complete = directive_recipe.is_some()
        && directive_task_path.is_some()
        && directive_repo.is_some()
        && directive_context_path.is_some();
    if directive_present && !directive_complete {
        return Err(
            "a directive is all-or-nothing: --directive-recipe, --directive-task-path, \
             --directive-repo and --directive-context-path must all be present together"
                .to_string(),
        );
    }

    let directive = if directive_complete {
        let repo = directive_repo.expect("checked present");
        // Reuse the store's traversal-safe slug guard so a malformed/unsafe repo
        // is rejected at parse time, before any path is derived.
        validate_repo_slug(&repo)?;
        let recipe = directive_recipe.expect("checked present");
        if recipe.trim().is_empty() {
            return Err("--directive-recipe must be non-empty".to_string());
        }
        let task_description = read_file_arg(
            "directive-task-path",
            &directive_task_path.expect("present"),
        )?;
        let context = read_file_arg(
            "directive-context-path",
            &directive_context_path.expect("present"),
        )?;
        Some(DirectiveArgs {
            recipe,
            task_description,
            target_repo: repo,
            context,
        })
    } else {
        None
    };

    // A decision with neither a reply nor a directive is a no-op — reject it.
    if reply.is_none() && directive.is_none() {
        return Err(
            "a liaison decision must carry a reply (--reply-path) and/or a complete directive"
                .to_string(),
        );
    }

    Ok(LiaisonRecordDecisionArgs {
        group_id,
        message_id,
        run_token,
        reply,
        directive,
        state_root,
    })
}

/// Run `simard liaison record-decision`, returning the process exit code. Emits
/// `[simard]`-prefixed diagnostics to stderr.
fn run_record_decision(args: Vec<String>) -> i32 {
    let parsed = match parse_liaison_record_decision_args(args) {
        Ok(p) => p,
        Err(e) => {
            eprintln!("[simard] liaison record-decision: usage error: {e}");
            return EXIT_USAGE;
        }
    };

    let state_root = parsed.state_root.clone().unwrap_or_else(simard_state_root);
    let directive = parsed.directive.as_ref().map(|d| Directive {
        recipe: d.recipe.clone(),
        task_description: d.task_description.clone(),
        target_repo: d.target_repo.clone(),
        context_path: stage_context_file(
            &state_root,
            &parsed.group_id,
            &parsed.message_id,
            &d.context,
        )
        .unwrap_or_default(),
    });

    // If a directive was requested but staging its ContextFile failed, refuse to
    // record a directive that points at nothing.
    if parsed.directive.is_some()
        && directive.as_ref().map(|d| d.context_path.is_empty()) == Some(true)
    {
        eprintln!("[simard] liaison record-decision: could not stage the directive context file");
        return EXIT_IO;
    }

    let record = LiaisonDecisionRecord::new(
        &parsed.group_id,
        &parsed.message_id,
        &parsed.run_token,
        parsed.reply.clone(),
        directive,
    );
    match write_record(&state_root, &record) {
        Ok(()) => {
            eprintln!(
                "[simard] liaison record-decision: recorded decision for message {} (token {}).",
                parsed.message_id, parsed.run_token
            );
            EXIT_RECORDED
        }
        Err(e) => {
            eprintln!("[simard] liaison record-decision: could not write record: {e}");
            EXIT_IO
        }
    }
}

/// Persist the directive's operator context to a durable ContextFile under the
/// state root and return its absolute path, so the payload never touches argv.
fn stage_context_file(
    state_root: &std::path::Path,
    group_id: &str,
    message_id: &str,
    context: &str,
) -> Result<String, String> {
    use crate::stewardship::record_io::{atomic_write_0600, sha256_hex};

    let group_seg = sha256_hex(group_id.as_bytes());
    // Keep the message id path-safe by hashing it too (it is validated elsewhere
    // but this staging path is defensive).
    let msg_seg = sha256_hex(message_id.as_bytes());

    let path = state_root
        .join("liaison_directive_context")
        .join(group_seg)
        .join(format!("{msg_seg}.txt"));
    atomic_write_0600(&path, context.as_bytes())?;
    Ok(path.to_string_lossy().into_owned())
}

/// Dispatch `simard liaison <subcommand>`. Currently only `record-decision`.
pub(crate) fn dispatch_liaison_command(
    mut args: impl Iterator<Item = String>,
) -> Result<(), Box<dyn std::error::Error>> {
    let subcommand = match args.next() {
        Some(s) => s,
        None => {
            print!("{LIAISON_HELP}");
            return Ok(());
        }
    };
    match subcommand.as_str() {
        "--help" | "-h" | "help" => {
            print!("{LIAISON_HELP}");
            Ok(())
        }
        "record-decision" => {
            let argv: Vec<String> = args.collect();
            if argv
                .iter()
                .any(|a| a == "--help" || a == "-h" || a == "help")
            {
                print!("{LIAISON_HELP}");
                return Ok(());
            }
            std::process::exit(run_record_decision(argv));
        }
        other => Err(format!("unsupported command 'liaison {other}'").into()),
    }
}
