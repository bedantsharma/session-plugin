use std::io::Read;
use std::path::Path;

use anyhow::{Context, Result};
use bridge_screen_core::config::Config;
use bridge_screen_core::paths::Home;
use bridge_screen_core::plan::{EditContext, PlanOp, load_tree, plan_edit, render_text};
use bridge_screen_core::repo;
use bridge_screen_core::store::{Actor, OWNER_SESSION, Store};

use crate::cli::{Cli, Command, PlanCommand};

pub fn run(cli: Cli) -> Result<()> {
    match cli.command {
        Command::Install => install(),
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
        Command::Rebuild => rebuild(),
        Command::Config => config(),
    }
}

fn open_store(home: &Home) -> Result<Store> {
    home.ensure()?;
    Ok(Store::open(&home.db_path())?)
}

fn install() -> Result<()> {
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
    println!("Claude Code plugin registration arrives in a later release.");
    Ok(())
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
