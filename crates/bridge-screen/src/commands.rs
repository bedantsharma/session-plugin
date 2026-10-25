use std::fmt::Write as _;
use std::io::Read;
use std::path::Path;

use anyhow::{Context, Result};
use bridge_screen_core::config::Config;
use bridge_screen_core::handoff::LeftoverAction;
use bridge_screen_core::owner::{self, OwnerAction};
use bridge_screen_core::paths::Home;
use bridge_screen_core::plan::{EditContext, PlanOp, load_tree, plan_edit, render_annotated};
use bridge_screen_core::recovery;
use bridge_screen_core::repo;
use bridge_screen_core::session;
use bridge_screen_core::squash;
use bridge_screen_core::store::{Actor, EventDraft, OWNER_SESSION, Store};
use bridge_screen_core::tangent::{self, Label};
use bridge_screen_core::verify;

use crate::cli::{Cli, Command, ConfigCommand, DriftAnswer, LabelArg, PlanCommand, SettingArg};

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
            reason,
        } => drift(&session, answer, parent.zip(title), estimate, reason),
        Command::Label {
            session,
            unit_no,
            label,
        } => label_cmd(&session, unit_no, label),
        Command::Resolve { args, dir } => resolve(&dir, &args),
        Command::Squash { dir, onto, dry_run } => squash_cmd(&dir, onto, dry_run),
        Command::Verify {
            task,
            broken,
            withdraw,
            level,
            dir,
        } => verify_cmd(&dir, &task, broken.as_deref(), withdraw, level),
        Command::Decisions { dir, task, all } => decisions_cmd(&dir, task.as_deref(), all),
        Command::Usage {
            dir,
            session,
            task,
            by,
            catch_up,
        } => crate::usage::run(&dir, session.as_deref(), task.as_deref(), by, catch_up),
        Command::Forecast { dir } => crate::usage::forecast(&dir),
        Command::Import {
            file,
            into,
            dry_run,
            fresh,
            done,
            dir,
        } => {
            if fresh || !done.is_empty() {
                crate::onboard::import_scope(&dir, fresh, &done)
            } else {
                crate::onboard::import(&dir, file.as_deref(), into.as_deref(), dry_run)
            }
        }
        Command::Init { goal, dir } => crate::onboard::init(&dir, goal.as_deref()),
        Command::Wait { action, dir } => crate::onboard::wait(&dir, action),
        Command::Pages { action, dir } => crate::onboard::pages(&dir, &action),
        Command::Rebuild => rebuild(),
        Command::Config { action: None, dir } => config(&dir),
        Command::Config {
            action: Some(ConfigCommand::ResetDrift),
            dir,
        } => reset_drift(&dir),
        Command::Config {
            action: Some(ConfigCommand::Set { key, value }),
            dir,
        } => set_setting(&dir, key, Some(&value)),
        Command::Config {
            action: Some(ConfigCommand::Unset { key }),
            dir,
        } => set_setting(&dir, key, None),
        Command::Mcp => crate::mcp::run(),
        Command::Ui { port, no_open } => crate::ui::run(port, no_open),
        Command::Status => print_status(),
        Command::Watch => crate::watch::run(),
        Command::Hook { .. } => unreachable!("main handles hooks before dispatching commands"),
    }
}

/// `bridge-screen status` (also what `watch` prints when stdout isn't a terminal).
pub fn print_status() -> Result<()> {
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
    let glance = bridge_screen_core::view::glance(store.conn(), &cfg, jiff::Timestamp::now())?;
    let opts = crate::status::Options {
        color: crate::status::stdout_color(),
        dashboard: crate::ui::running(&home).map(|l| l.address()),
    };
    print!("{}", crate::status::render(&glance, &opts));
    Ok(())
}

pub fn open_store(home: &Home) -> Result<Store> {
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
        let marks = verify::plan_marks(store.conn(), &repo.id)?;
        print!("{}", render_annotated(&nodes, &marks));
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
    owner_cmd(
        &home,
        &mut store,
        None,
        &OwnerAction::Resolve {
            repo: Some(repo.id),
            leftover_ids: ids.to_vec(),
            action,
        },
    )
}

fn squash_cmd(dir: &Path, onto: Option<String>, dry_run: bool) -> Result<()> {
    let repo = repo::identify(dir)?;
    let wt = bridge_screen_core::git::toplevel(dir)?;
    let home = Home::resolve()?;
    let mut store = open_store(&home)?;
    store.register_repo(&repo)?;
    let opts = squash::Options { onto, dry_run };
    if !dry_run {
        // Changes inside the range can't be told apart once it is one commit: drop what
        // they made stale first (a failure here never stops the squash).
        let _ = verify::scan_stale(&mut store, &repo.id, OWNER_SESSION, &wt);
    }
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
    let settings = Config {
        prices: std::collections::BTreeMap::new(),
        ..cfg.clone()
    };
    print!("{}", settings.to_toml()?);
    print!("{}", price_table(&cfg));
    // Outside a repo there are no tuned values to show.
    if let Ok(repo) = repo::identify(dir) {
        let store = open_store(&home)?;
        if let Some(line) = tuned_line(&store, &repo.id, &repo.name, &cfg)? {
            println!("{line}");
        }
        let set = verify::settings(store.conn(), &repo.id)?;
        if let Some(cmd) = set.test_command {
            println!("# test command for {}: {cmd}", repo.name);
        }
        if let Some(v) = set.publish_reports {
            println!("# publish reports for {}: {v}", repo.name);
        }
        if let Some(secs) = set.test_timeout_s {
            println!("# test timeout for {}: {secs} s", repo.name);
        }
    }
    Ok(())
}

/// The prices in use (USD per million tokens), each marked as a default or the owner's.
fn price_table(cfg: &Config) -> String {
    let mut out = String::from(
        "# prices in use, USD per million tokens (input / output / cache read / cache write 5m / 1h):\n",
    );
    for (model, p) in &cfg.prices {
        let _ = writeln!(
            out,
            "#   {model}: {} / {} / {} / {} / {} ({})",
            p.input,
            p.output,
            p.cache_read,
            p.cache_write_5m,
            p.cache_write_1h,
            cfg.price_source(model).unwrap_or("yours")
        );
    }
    out.push_str("# models not listed are unpriced: their cost shows as unknown\n");
    out
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

fn set_setting(dir: &Path, key: SettingArg, value: Option<&str>) -> Result<()> {
    let repo = repo::identify(dir)?;
    let home = Home::resolve()?;
    let mut store = open_store(&home)?;
    store.register_repo(&repo)?;
    let new = match (key, value) {
        (_, None) => None,
        (SettingArg::TestCommand, Some(cmd)) => {
            let cmd = cmd.trim();
            anyhow::ensure!(!cmd.is_empty(), "the test command is empty");
            Some(serde_json::Value::from(cmd))
        }
        (SettingArg::PublishReports, Some(v)) => {
            let v = v.trim();
            anyhow::ensure!(
                v == "never" || v == "ask",
                "publish-reports takes never or ask, not {v}"
            );
            Some(serde_json::Value::from(v))
        }
        (SettingArg::TestTimeout, Some(secs)) => {
            let secs: u64 = secs
                .trim()
                .parse()
                .ok()
                .filter(|s| (1..=86_400).contains(s))
                .with_context(|| format!("test-timeout takes seconds (1–86400), not {secs}"))?;
            Some(serde_json::Value::from(secs))
        }
    };
    let what = key.key().replace('_', " ");
    let shown = new.as_ref().map(|v| match v {
        serde_json::Value::String(s) => s.clone(),
        other => other.to_string(),
    });
    if verify::change_setting(&mut store, &repo.id, key.key(), new)? {
        match shown {
            Some(v) => println!("Set {what} for {}: {v}", repo.name),
            None => println!("Unset {what} for {}.", repo.name),
        }
    } else {
        println!("The {what} for {} is unchanged.", repo.name);
    }
    Ok(())
}

fn decisions_cmd(dir: &Path, task: Option<&str>, all: bool) -> Result<()> {
    let repo = repo::identify(dir)?;
    let home = Home::resolve()?;
    let store = open_store(&home)?;
    let list = bridge_screen_core::decision::list(store.conn(), &repo.id, task, all)?;
    if list.is_empty() {
        println!("No decisions logged for {} yet.", repo.name);
    }
    for d in &list {
        println!("{}", bridge_screen_core::decision::line(d));
    }
    Ok(())
}

fn verify_cmd(
    dir: &Path,
    task: &str,
    broken: Option<&str>,
    withdraw: bool,
    level: i64,
) -> Result<()> {
    let repo = repo::identify(dir)?;
    let home = Home::resolve()?;
    let mut store = open_store(&home)?;
    store.register_repo(&repo)?;
    let action = match broken {
        _ if withdraw => OwnerAction::Withdraw {
            repo: repo.id,
            task: task.to_string(),
            verification_id: None,
        },
        Some(note) => OwnerAction::Broken {
            repo: repo.id,
            task: task.to_string(),
            note: note.to_string(),
        },
        None => OwnerAction::Verify {
            repo: repo.id,
            task: task.to_string(),
            level,
        },
    };
    owner_cmd(&home, &mut store, Some(dir), &action)
}

fn reset_drift(dir: &Path) -> Result<()> {
    let repo = repo::identify(dir)?;
    let home = Home::resolve()?;
    let mut store = open_store(&home)?;
    store.register_repo(&repo)?;
    let action = OwnerAction::ResetDrift { repo: repo.id };
    match owner_cmd(&home, &mut store, None, &action) {
        // Already at the defaults: nothing to do, which is fine here.
        Err(e) => match e.downcast_ref::<bridge_screen_core::error::Error>() {
            Some(bridge_screen_core::error::Error::Stale(msg)) => {
                println!("{msg}");
                Ok(())
            }
            _ => Err(e),
        },
        ok => ok,
    }
}

/// Run one owner action (the same path the dashboard, `watch` and the card take) and
/// print its reply.
pub fn owner_cmd(
    home: &Home,
    store: &mut Store,
    dir: Option<&Path>,
    action: &OwnerAction,
) -> Result<()> {
    let cfg = Config::load(&home.config_path())?;
    let ctx = owner::ActCtx {
        home,
        cfg: &cfg,
        now: jiff::Timestamp::now(),
        dir,
    };
    let reply = owner::act(store, &ctx, action)?;
    println!("{reply}");
    Ok(())
}

fn drift(
    prefix: &str,
    answer: DriftAnswer,
    task: Option<(String, String)>,
    estimate: Option<u32>,
    reason: Option<String>,
) -> Result<()> {
    let home = Home::resolve()?;
    let mut store = open_store(&home)?;
    let (parent, title) = task.unzip();
    let action = OwnerAction::Drift {
        session: prefix.to_string(),
        answer: answer.into(),
        parent,
        title,
        estimate,
        reason,
    };
    owner_cmd(&home, &mut store, None, &action)
}

fn label_cmd(prefix: &str, unit_no: i64, label: LabelArg) -> Result<()> {
    let home = Home::resolve()?;
    let mut store = open_store(&home)?;
    let label = match label {
        LabelArg::OnTask => Label::OnTask,
        LabelArg::Detour => Label::Detour,
        LabelArg::Drift => Label::Drift,
        LabelArg::Requested => Label::Requested,
    };
    let action = OwnerAction::Label {
        session: prefix.to_string(),
        unit_no,
        label,
    };
    owner_cmd(&home, &mut store, None, &action)
}
