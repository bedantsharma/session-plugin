use std::fmt::Write as _;
use std::io::Read;
use std::path::Path;

use anyhow::{Context, Result};
use bridge_screen_core::config::Config;
use bridge_screen_core::handoff::LeftoverAction;
use bridge_screen_core::navigator::{self, Channel};
use bridge_screen_core::paths::Home;
use bridge_screen_core::plan::{EditContext, PlanOp, load_tree, plan_edit, render_text};
use bridge_screen_core::recovery::{self, Caller};
use bridge_screen_core::repo;
use bridge_screen_core::session::{self, Session};
use bridge_screen_core::squash;
use bridge_screen_core::store::{Actor, EventDraft, OWNER_SESSION, Store, append_in};
use bridge_screen_core::tangent::{self, Label, LabelRequest, LabelSource, NavState, OwnerAction};

use crate::cli::{Cli, Command, ConfigCommand, DriftAnswer, LabelArg, PlanCommand};

pub fn run(cli: Cli) -> Result<()> {
    match cli.command {
        Command::Install { dev, no_plugin } => install(dev, no_plugin),
        Command::Plan {
            action: PlanCommand::Show { dir, all },
        } => plan_show(&dir, all),
        Command::Plan {
            action:
                PlanCommand::Edit {
                    dir,
                    reason,
                    replan,
                    file,
                },
        } => plan_edit_cmd(&dir, reason, replan, file.as_deref()),
        Command::Drift {
            session,
            answer,
            parent,
            title,
            estimate,
        } => drift(&session, answer, parent.zip(title), estimate),
        Command::Label {
            session,
            unit_no,
            label,
        } => label_cmd(&session, unit_no, label),
        Command::Resolve { args, dir } => resolve(&dir, &args),
        Command::Squash { dir, onto, dry_run } => squash_cmd(&dir, onto, dry_run),
        Command::Rebuild => rebuild(),
        Command::Config { action: None, dir } => config(&dir),
        Command::Config {
            action: Some(ConfigCommand::ResetDrift),
            dir,
        } => reset_drift(&dir),
        Command::Mcp => crate::mcp::run(),
        Command::Status => {
            let home = Home::resolve()?;
            let mut store = open_store(&home)?;
            let cfg = Config::load(&home.config_path())?;
            // Sessions silent past the crash timeout are shown as crashed, not live.
            let mut repos: Vec<String> = session::list_live(store.conn(), None)?
                .into_iter()
                .map(|s| s.repo_id)
                .collect();
            repos.sort();
            repos.dedup();
            for repo_id in repos {
                recovery::scan_crashed(
                    &mut store,
                    &repo_id,
                    jiff::Timestamp::now(),
                    cfg.crash_timeout_min,
                )?;
            }
            print!("{}", crate::status::render(&store, &cfg)?);
            Ok(())
        }
        Command::Hook { .. } => unreachable!("main handles hooks before dispatching commands"),
    }
}

fn open_store(home: &Home) -> Result<Store> {
    home.ensure()?;
    Ok(Store::open(&home.db_path())?)
}

fn install(dev: bool, no_plugin: bool) -> Result<()> {
    let home = Home::resolve()?;
    open_store(&home)?;
    let created = Config::write_default_if_missing(&home.config_path())?;
    println!("bridge-screen home: {}", home.root().display());
    println!("database: {}", home.db_path().display());
    println!(
        "config: {} ({})",
        home.config_path().display(),
        if created { "created" } else { "kept existing" }
    );
    if no_plugin {
        return Ok(());
    }
    crate::install::register_plugin(dev)
}

fn plan_show(dir: &Path, all: bool) -> Result<()> {
    let repo = repo::identify(dir)?;
    let home = Home::resolve()?;
    let store = open_store(&home)?;
    store.register_repo(&repo)?;
    let nodes = load_tree(store.conn(), &repo.id, all)?;
    if nodes.is_empty() {
        println!(
            "No plan yet for {}. Add a goal with `bridge-screen plan edit`.",
            repo.name
        );
    } else {
        print!("{}", render_text(&nodes));
    }
    Ok(())
}

fn plan_edit_cmd(
    dir: &Path,
    reason: Option<String>,
    replan: bool,
    file: Option<&Path>,
) -> Result<()> {
    let repo = repo::identify(dir)?;
    let text = if let Some(path) = file {
        std::fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?
    } else {
        let mut buf = String::new();
        std::io::stdin()
            .read_to_string(&mut buf)
            .context("reading stdin")?;
        buf
    };
    let ops: Vec<PlanOp> =
        serde_json::from_str(&text).context("ops must be a JSON array of plan operations")?;
    let home = Home::resolve()?;
    let mut store = open_store(&home)?;
    store.register_repo(&repo)?;
    let ctx = EditContext {
        repo_id: repo.id,
        session_id: OWNER_SESSION.to_string(),
        actor: Actor::default(),
        reason,
        replan,
    };
    let outcome = plan_edit(&mut store, &ctx, &ops)?;
    println!("applied {} operation(s)", ops.len());
    for id in outcome.added_ids {
        println!("added {id}");
    }
    Ok(())
}

fn resolve(dir: &Path, args: &[String]) -> Result<()> {
    let (action, ids) = args
        .split_last()
        .context("give leftover ids and an action")?;
    let action = LeftoverAction::parse(action).with_context(|| {
        format!("the last argument must be adopt, resume or undo, not {action}")
    })?;
    let repo = repo::identify(dir)?;
    let home = Home::resolve()?;
    let mut store = open_store(&home)?;
    store.register_repo(&repo)?;
    let res = recovery::resolve(
        &mut store,
        &repo.id,
        &home.snapshots_dir(&repo.id),
        ids,
        action,
        &Caller::Owner,
    )?;
    println!("{}", crate::mcp::resolution_text(&store, &repo.id, &res)?);
    Ok(())
}

fn squash_cmd(dir: &Path, onto: Option<String>, dry_run: bool) -> Result<()> {
    let repo = repo::identify(dir)?;
    let wt = bridge_screen_core::git::toplevel(dir)?;
    let home = Home::resolve()?;
    let mut store = open_store(&home)?;
    store.register_repo(&repo)?;
    let opts = squash::Options { onto, dry_run };
    match squash::squash(&wt, &opts, store.conn(), &repo.id)? {
        squash::Outcome::Refused(r) => anyhow::bail!("not squashing: {r} (history unchanged)"),
        squash::Outcome::DryRun(plan) => {
            println!(
                "Would squash {} commits on {} onto {} ({}), {} of them WIP:",
                plan.commits.len(),
                plan.branch,
                &plan.merge_base[..plan.merge_base.len().min(8)],
                plan.base,
                plan.wips
            );
            for c in &plan.commits {
                println!("  {} {}", &c.sha[..c.sha.len().min(8)], c.subject);
            }
            println!("Message:\n{}", plan.message);
            if let Some((u, _)) = &plan.upstream {
                println!("Then push to {u} with --force-with-lease.");
            }
            Ok(())
        }
        squash::Outcome::Squashed(done) => {
            store.append(EventDraft::new(
                &repo.id,
                OWNER_SESSION,
                "squashed",
                serde_json::to_value(done.event())?,
            ))?;
            let mut text = format!(
                "Squashed {} commits on {} into {}. Backup: {}.",
                done.old_shas.len(),
                done.branch,
                &done.new_sha[..done.new_sha.len().min(8)],
                done.backup_ref
            );
            match &done.push {
                Some(Ok(())) => text.push_str(" Pushed with --force-with-lease."),
                Some(Err(e)) => {
                    store.append(EventDraft::new(
                        &repo.id,
                        OWNER_SESSION,
                        "squash_failed",
                        serde_json::json!({"reason": format!("push failed: {e}")}),
                    ))?;
                    let _ = write!(text, " The push failed ({e}); the squash is local only.");
                }
                None => {}
            }
            println!("{text}");
            Ok(())
        }
    }
}

fn rebuild() -> Result<()> {
    let home = Home::resolve()?;
    let mut store = open_store(&home)?;
    let stats = store.rebuild()?;
    println!("rebuilt views from {} events", stats.events);
    Ok(())
}

fn config(dir: &Path) -> Result<()> {
    let home = Home::resolve()?;
    let cfg = Config::load(&home.config_path())?;
    print!("{}", cfg.to_toml()?);
    // Outside a repo there are no tuned values to show.
    if let Ok(repo) = repo::identify(dir) {
        let store = open_store(&home)?;
        if let Some(line) = tuned_line(&store, &repo.id, &repo.name, &cfg)? {
            println!("{line}");
        }
    }
    Ok(())
}

/// `# tuned for shop: drift_lower = 0.35, …`, if the repo has tuned values.
pub fn tuned_line(
    store: &Store,
    repo_id: &str,
    name: &str,
    cfg: &Config,
) -> Result<Option<String>> {
    let th = tangent::thresholds(store.conn(), repo_id, cfg)?;
    if th == tangent::Thresholds::from_config(cfg) {
        return Ok(None);
    }
    Ok(Some(format!(
        "# tuned for {name}: drift_lower = {}, drift_upper = {}, tolerance_pct = {}",
        th.lower, th.upper, th.tolerance_pct
    )))
}

fn reset_drift(dir: &Path) -> Result<()> {
    let repo = repo::identify(dir)?;
    let home = Home::resolve()?;
    let mut store = open_store(&home)?;
    store.register_repo(&repo)?;
    let cfg = Config::load(&home.config_path())?;
    let changes = store.write(|tx| bridge_screen_core::tuning::reset(tx, &repo.id, &cfg))?;
    if changes.is_empty() {
        println!("Drift tuning for {} is already at the defaults.", repo.name);
    } else {
        let parts: Vec<String> = changes
            .iter()
            .map(|c| format!("{} {} → {}", c.key, c.old, c.new))
            .collect();
        println!(
            "Reset drift tuning for {}: {}.",
            repo.name,
            parts.join(", ")
        );
    }
    Ok(())
}

/// The live session whose id starts with `prefix`.
fn find_session(store: &Store, prefix: &str) -> Result<Session> {
    let found: Vec<Session> = session::list_live(store.conn(), None)?
        .into_iter()
        .filter(|s| s.session_id.starts_with(prefix))
        .collect();
    match found.len() {
        0 => anyhow::bail!("no live session starts with {prefix}"),
        1 => Ok(found.into_iter().next().expect("one session")),
        _ => {
            let ids: Vec<&str> = found.iter().map(|s| s.session_id.as_str()).collect();
            anyhow::bail!("session prefix {prefix} is ambiguous: {}", ids.join(", "))
        }
    }
}

/// Re-check the session's tolerance band after an owner action (state events only).
fn recheck(store: &mut Store, config: &Config, session_id: &str) -> Result<()> {
    store.write(|tx| {
        let Some(s) = session::get(tx, session_id)? else {
            return Ok(());
        };
        let th = tangent::thresholds(tx, &s.repo_id, config)?;
        let step = navigator::nav_step(
            tx,
            session_id,
            Channel::ToolResult,
            &th,
            jiff::Timestamp::now(),
        )?;
        for draft in step.events {
            append_in(tx, draft)?;
        }
        Ok(())
    })?;
    Ok(())
}

fn drift(
    prefix: &str,
    answer: DriftAnswer,
    task: Option<(String, String)>,
    estimate: Option<u32>,
) -> Result<()> {
    let home = Home::resolve()?;
    let mut store = open_store(&home)?;
    let config = Config::load(&home.config_path())?;
    let s = find_session(&store, prefix)?;
    // An answer needs a question: the session is (or was last seen) outside tolerance.
    let th = tangent::thresholds(store.conn(), &s.repo_id, &config)?;
    let live = tangent::evaluate(store.conn(), &s, &th, jiff::Timestamp::now())?.state;
    if live == NavState::Green && s.nav_state == NavState::Green {
        anyhow::bail!(
            "{} is inside its tolerance band; there is no drift to answer",
            s.session_id
        );
    }
    let mut reply = format!(
        "Told {}: {}.",
        s.session_id,
        answer.as_str().replace('_', " ")
    );
    if answer == DriftAnswer::MakeTask {
        let (parent, title) = task.context("make-task needs --parent and --title")?;
        let out = tangent::promote_detour(
            &mut store,
            &s.session_id,
            &parent,
            &title,
            estimate,
            &Actor::default(),
        )?;
        reply = format!(
            "Detour {} is now task {}, claimed by {}.",
            out.detour.label(),
            out.task_id,
            s.session_id
        );
    }
    let data = OwnerAction {
        action: "drift".into(),
        answer: Some(answer.as_str().into()),
        target: Some(s.session_id.clone()),
    };
    store.append(EventDraft::new(
        &s.repo_id,
        OWNER_SESSION,
        "owner_action",
        serde_json::to_value(data)?,
    ))?;
    recheck(&mut store, &config, &s.session_id)?;
    println!("{reply}");
    Ok(())
}

fn label_cmd(prefix: &str, unit_no: i64, label: LabelArg) -> Result<()> {
    let home = Home::resolve()?;
    let mut store = open_store(&home)?;
    let config = Config::load(&home.config_path())?;
    let s = find_session(&store, prefix)?;
    let label = match label {
        LabelArg::OnTask => Label::OnTask,
        LabelArg::Detour => Label::Detour,
        LabelArg::Drift => Label::Drift,
    };
    let th = tangent::thresholds(store.conn(), &s.repo_id, &config)?;
    let req = LabelRequest {
        session_id: s.session_id.clone(),
        unit_no: Some(unit_no),
        label,
        source: LabelSource::Owner,
        why: None,
    };
    tangent::label_unit(&mut store, &req, &th, &Actor::default())?;
    recheck(&mut store, &config, &s.session_id)?;
    println!(
        "Labelled unit {unit_no} of {} as {}.",
        s.session_id,
        label.display()
    );
    Ok(())
}
