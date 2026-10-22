use std::sync::Arc;

use super::{Page, PagesState, WriteMode, decline, read, set_up, state, status, write};
use crate::paths::Home;

const ID: &str = "0123456789abcdef";

fn home() -> (tempfile::TempDir, Home) {
    let dir = tempfile::tempdir().unwrap();
    let home = Home::at(dir.path().join("home"));
    (dir, home)
}

const ALL: [Page; 5] = [
    Page::Intent,
    Page::ProjectMap,
    Page::CurrentState,
    Page::DecisionLog,
    Page::Glossary,
];

#[test]
fn set_up_creates_five_pages_once() {
    let (_d, home) = home();
    let out = set_up(&home, ID, "yes please").unwrap();
    assert_eq!(out.created.len(), 5);
    assert_eq!(out.kept.len(), 0);
    assert_eq!(state(&home, ID), PagesState::SetUp);
    for p in ALL {
        assert!(home.lead_dir(ID).join(p.file_name()).is_file());
    }
    write(
        &home,
        ID,
        Page::CurrentState,
        "my edit",
        WriteMode::Replace,
        None,
    )
    .unwrap();
    let again = set_up(&home, ID, "yes").unwrap();
    assert_eq!(again.kept, ALL.to_vec());
    assert_eq!(again.created.len(), 0);
    assert!(
        read(&home, ID, Page::CurrentState)
            .unwrap()
            .unwrap()
            .contains("my edit")
    );
}

#[test]
fn decline_stops_everything() {
    let (_d, home) = home();
    decline(&home, ID, "no thanks").unwrap();
    assert_eq!(state(&home, ID), PagesState::Declined);
    for p in ALL {
        assert!(!home.lead_dir(ID).join(p.file_name()).exists());
    }
    set_up(&home, ID, "actually yes").unwrap();
    assert_eq!(state(&home, ID), PagesState::SetUp);
}

#[test]
fn blank_owner_words_refused() {
    let (_d, home) = home();
    assert!(set_up(&home, ID, " ").is_err());
    assert!(decline(&home, ID, "").is_err());
    assert!(write(&home, ID, Page::Intent, "x", WriteMode::Replace, None).is_err());
    assert!(write(&home, ID, Page::Intent, "x", WriteMode::Replace, Some("  ")).is_err());
    assert!(!home.lead_dir(ID).join("intent.md").exists());
}

#[test]
fn page_name_is_a_fixed_list() {
    for bad in ["../x", "intent.md", "INTENT", "", "/etc/passwd", " intent"] {
        assert!(Page::parse(bad).is_none(), "{bad:?}");
    }
    assert_eq!(Page::parse("project-map"), Some(Page::ProjectMap));
    for p in ALL {
        assert_eq!(Page::parse(p.name()), Some(p));
    }
}

#[test]
fn intent_needs_owner_words() {
    let (_d, home) = home();
    set_up(&home, ID, "yes").unwrap();
    assert!(
        write(
            &home,
            ID,
            Page::Intent,
            "# Intent",
            WriteMode::Replace,
            None
        )
        .is_err()
    );
    write(
        &home,
        ID,
        Page::Intent,
        "# Intent",
        WriteMode::Replace,
        Some("yes, that is the purpose"),
    )
    .unwrap();
    assert_eq!(read(&home, ID, Page::Intent).unwrap().unwrap(), "# Intent");
}

#[test]
fn decision_log_is_append_only() {
    let (_d, home) = home();
    set_up(&home, ID, "yes").unwrap();
    let err = write(
        &home,
        ID,
        Page::DecisionLog,
        "wipe",
        WriteMode::Replace,
        None,
    )
    .unwrap_err();
    assert!(err.to_string().contains("add to"), "{err}");
    write(
        &home,
        ID,
        Page::DecisionLog,
        "first entry",
        WriteMode::Append,
        None,
    )
    .unwrap();
    write(
        &home,
        ID,
        Page::DecisionLog,
        "second entry",
        WriteMode::Append,
        None,
    )
    .unwrap();
    let text = read(&home, ID, Page::DecisionLog).unwrap().unwrap();
    let a = text.find("first entry").unwrap();
    let b = text.find("second entry").unwrap();
    assert!(a < b);
    assert!(
        text.starts_with("<!-- What this page is for"),
        "old text kept"
    );
}

#[test]
fn concurrent_appends_both_land() {
    let (_d, home) = home();
    let home = Arc::new(home);
    let handles: Vec<_> = (0..8)
        .map(|i| {
            let home = Arc::clone(&home);
            std::thread::spawn(move || {
                write(
                    &home,
                    ID,
                    Page::DecisionLog,
                    &format!("entry-number-{i}"),
                    WriteMode::Append,
                    None,
                )
                .unwrap();
            })
        })
        .collect();
    for h in handles {
        h.join().unwrap();
    }
    let text = read(&home, ID, Page::DecisionLog).unwrap().unwrap();
    for i in 0..8 {
        assert!(text.contains(&format!("entry-number-{i}")), "lost {i}");
    }
}

#[test]
fn write_keeps_previous_version() {
    let (_d, home) = home();
    set_up(&home, ID, "yes").unwrap();
    write(&home, ID, Page::Glossary, "one", WriteMode::Replace, None).unwrap();
    write(&home, ID, Page::Glossary, "two", WriteMode::Replace, None).unwrap();
    let prev = home.lead_dir(ID).join("glossary.md.prev");
    assert_eq!(std::fs::read_to_string(prev).unwrap(), "one");
    assert_eq!(read(&home, ID, Page::Glossary).unwrap().unwrap(), "two");
}

#[test]
fn oversize_write_refused() {
    let (_d, home) = home();
    set_up(&home, ID, "yes").unwrap();
    write(&home, ID, Page::Glossary, "small", WriteMode::Replace, None).unwrap();
    let big = "x".repeat(64 * 1024 + 1);
    assert!(write(&home, ID, Page::Glossary, &big, WriteMode::Replace, None).is_err());
    assert_eq!(read(&home, ID, Page::Glossary).unwrap().unwrap(), "small");
    let max = "x".repeat(64 * 1024);
    write(&home, ID, Page::Glossary, &max, WriteMode::Replace, None).unwrap();
}

#[test]
fn partial_set_up_is_completed() {
    let (_d, home) = home();
    set_up(&home, ID, "yes").unwrap();
    let dir = home.lead_dir(ID);
    std::fs::remove_file(dir.join("glossary.md")).unwrap();
    std::fs::remove_file(dir.join("project-map.md")).unwrap();
    assert_ne!(state(&home, ID), PagesState::SetUp);
    let out = set_up(&home, ID, "yes").unwrap();
    assert_eq!(out.created, vec![Page::ProjectMap, Page::Glossary]);
    assert_eq!(out.kept.len(), 3);
    assert_eq!(state(&home, ID), PagesState::SetUp);
}

#[test]
fn status_lists_all_pages_with_dates() {
    let (_d, home) = home();
    assert!(status(&home, ID).iter().all(|i| i.updated.is_none()));
    set_up(&home, ID, "yes").unwrap();
    let s = status(&home, ID);
    assert_eq!(s.len(), 5);
    assert!(s.iter().all(|i| i.updated.is_some() && i.bytes > 0));
}

#[test]
fn offers_are_counted_from_the_marker() {
    let (_d, home) = home();
    assert_eq!(state(&home, ID), PagesState::NotSetUp { offers_made: 0 });
    std::fs::create_dir_all(home.lead_dir(ID)).unwrap();
    std::fs::write(home.lead_dir(ID).join(".offered"), "2").unwrap();
    assert_eq!(state(&home, ID), PagesState::NotSetUp { offers_made: 2 });
    set_up(&home, ID, "yes").unwrap();
    assert!(!home.lead_dir(ID).join(".offered").exists());
}

#[test]
fn decline_after_set_up_is_refused() {
    let (_d, home) = home();
    set_up(&home, ID, "yes").unwrap();
    assert!(decline(&home, ID, "no").is_err());
    assert_eq!(state(&home, ID), PagesState::SetUp);
}

#[test]
fn briefing_note_offers_three_times_then_stops() {
    use super::briefing_note;
    let (_d, home) = home();
    for n in 1..=3 {
        let note = briefing_note(&home, ID, 10, false).unwrap().unwrap();
        assert_eq!(note.lines().count(), 2);
        assert!(note.contains("lead_pages"));
        assert_eq!(state(&home, ID), PagesState::NotSetUp { offers_made: n });
    }
    assert_eq!(briefing_note(&home, ID, 10, false).unwrap(), None);
}

#[test]
fn offer_dropped_when_briefing_full() {
    use super::briefing_note;
    let (_d, home) = home();
    assert_eq!(briefing_note(&home, ID, 1, false).unwrap(), None);
    assert_eq!(briefing_note(&home, ID, 9, true).unwrap(), None);
    assert!(!home.lead_dir(ID).exists(), "nothing written");
    set_up(&home, ID, "yes").unwrap();
    assert_eq!(briefing_note(&home, ID, 0, false).unwrap(), None);
    let line = briefing_note(&home, ID, 1, true).unwrap().unwrap();
    assert_eq!(line.lines().count(), 1);
    assert!(line.starts_with("Project pages:"));
    assert!(line.contains(&jiff::Timestamp::now().to_string()[..10]));
}

#[test]
fn briefing_note_silent_after_decline() {
    use super::briefing_note;
    let (_d, home) = home();
    decline(&home, ID, "no").unwrap();
    assert_eq!(briefing_note(&home, ID, 10, false).unwrap(), None);
}

#[test]
fn briefing_note_errors_on_broken_folder() {
    use super::briefing_note;
    let (_d, home) = home();
    std::fs::create_dir_all(home.lead_dir(ID).parent().unwrap()).unwrap();
    std::fs::write(home.lead_dir(ID), "i am a file").unwrap();
    assert!(briefing_note(&home, ID, 10, false).is_err());
}

#[test]
fn view_has_five_plain_titled_pages_after_set_up() {
    let (_d, home) = home();
    let v = super::view(&home, ID).unwrap();
    assert_eq!((v.state, v.pages.len()), ("not_set_up", 0));
    set_up(&home, ID, "yes").unwrap();
    let v = super::view(&home, ID).unwrap();
    assert_eq!(v.state, "set_up");
    let titles: Vec<_> = v.pages.iter().map(|p| p.title).collect();
    assert_eq!(
        titles,
        [
            "What this project is for",
            "How it fits together",
            "Where things stand",
            "Why we chose what we chose",
            "Words we use"
        ]
    );
    assert!(
        v.pages
            .iter()
            .all(|p| p.updated.is_some() && !p.text.is_empty())
    );
}

#[test]
fn view_declined_and_odd_ids() {
    let (_d, home) = home();
    decline(&home, ID, "no").unwrap();
    assert_eq!(super::view(&home, ID).unwrap().state, "declined");
    for bad in ["", "../x", "a/b", "..", "a b"] {
        assert!(super::view(&home, bad).is_err(), "{bad:?}");
    }
}

#[cfg(unix)]
#[test]
fn planted_links_in_the_folder_are_never_followed() {
    use std::os::unix::fs::symlink;
    let (d, home) = home();
    set_up(&home, ID, "yes").unwrap();
    let dir = home.lead_dir(ID);
    let outside = d.path().join("outside.txt");
    std::fs::write(&outside, "secret").unwrap();
    // A link where a temp file would be written: the write must not go through it.
    for name in [
        "glossary.md.tmp",
        "glossary.md.prev.tmp",
        ".glossary.md.tmp",
    ] {
        symlink(&outside, dir.join(name)).unwrap();
    }
    write(&home, ID, Page::Glossary, "one", WriteMode::Replace, None).unwrap();
    write(&home, ID, Page::Glossary, "two", WriteMode::Replace, None).unwrap();
    assert_eq!(std::fs::read_to_string(&outside).unwrap(), "secret");
    // A page that is a link to some other file is not read (nothing leaks to the agent).
    std::fs::remove_file(dir.join("project-map.md")).unwrap();
    symlink(&outside, dir.join("project-map.md")).unwrap();
    assert!(read(&home, ID, Page::ProjectMap).is_err());
    assert!(
        write(
            &home,
            ID,
            Page::ProjectMap,
            "mine",
            WriteMode::Replace,
            None
        )
        .is_ok(),
        "writing replaces the link, never the file behind it"
    );
    assert_eq!(std::fs::read_to_string(&outside).unwrap(), "secret");
    assert_eq!(read(&home, ID, Page::ProjectMap).unwrap().unwrap(), "mine");
}
