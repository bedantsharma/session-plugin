use std::io::Read;
use std::path::Path;

use anyhow::{Context, Result};
use bridge_screen_core::config::Config;
use bridge_screen_core::navigator::{self, Channel};
use bridge_screen_core::paths::Home;
use bridge_screen_core::plan::{EditContext, PlanOp, load_tree, plan_edit, render_text};
use bridge_screen_core::repo;
use bridge_screen_core::session::{self, Session};
use bridge_screen_core::store::{Actor, EventDraft, OWNER_SESSION, Store, append_in};
use bridge_screen_core::tangent::{self, Label, LabelRequest, LabelSource, OwnerAction};

use crate::cli::{Cli, Command, DriftAnswer, LabelArg, PlanCommand};

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
        Command::Rebuild => rebuild(),
        Command::Config => config(),
        Command::Mcp => crate::mcp::run(),
        Command::Status => {
            let store = open_store(&Home::resolve()?)?;
            print!("{}", crate::status::render(&store)?);
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

fn rebuild() -> Result<()> {
    let home = Home::resolve()?;
    let mut store = open_store(&home)?;
    let stats = store.rebuild()?;
    println!("rebuilt views from {} events", stats.events);
    Ok(())
}

fn config() -> Result<()> {
    let home = Home::resolve()?;
    let cfg = Config::load(&home.config_path())?;
    print!("{}", cfg.to_toml()?);
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
