//! Project pages end to end: the folder survives `rebuild`, the SessionStart briefing offers
//! and points at the pages (spec 2026-10-22).

mod common;

use std::collections::BTreeMap;
use std::path::Path;

use bridge_screen_core::pages::{self, Page, WriteMode};
use bridge_screen_core::paths::Home;
use bridge_screen_core::repo;
use common::Env;

/// Every file under `dir` with its bytes.
fn snapshot(dir: &Path) -> BTreeMap<String, Vec<u8>> {
    let mut out = BTreeMap::new();
    for entry in std::fs::read_dir(dir).unwrap() {
        let path = entry.unwrap().path();
        out.insert(
            path.file_name().unwrap().to_string_lossy().into_owned(),
            std::fs::read(&path).unwrap(),
        );
    }
    out
}

#[test]
fn rebuild_leaves_pages_alone() {
    let env = Env::new();
    let id = repo::identify(env.repo.path()).unwrap().id;
    let home = Home::at(env.home.path());
    pages::set_up(&home, &id, "yes").unwrap();
    pages::write(
        &home,
        &id,
        Page::DecisionLog,
        "## #1 something",
        WriteMode::Append,
        None,
    )
    .unwrap();
    let before = snapshot(&home.lead_dir(&id));
    assert!(before.len() >= 5);
    assert!(env.run(&["rebuild"], "").status.success());
    assert_eq!(snapshot(&home.lead_dir(&id)), before);
}
