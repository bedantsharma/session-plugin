use std::collections::BTreeSet;
use std::sync::{Arc, Barrier};
use std::thread;

use bridge_screen_core::store::{EventDraft, Store, read_all};
use serde_json::json;

#[test]
fn concurrent_appends_keep_every_event_and_unique_seq() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("db.sqlite");
    Store::open(&path).unwrap();

    let handles: Vec<_> = (0..4)
        .map(|t| {
            let path = path.clone();
            thread::spawn(move || {
                let mut store = Store::open(&path).unwrap();
                for i in 0..50 {
                    store
                        .append(EventDraft::new(
                            "r",
                            "shared",
                            "note",
                            json!({"t": t, "i": i}),
                        ))
                        .unwrap();
                }
            })
        })
        .collect();
    for h in handles {
        h.join().unwrap();
    }

    let store = Store::open(&path).unwrap();
    let events = read_all(store.conn(), None).unwrap();
    assert_eq!(events.len(), 200);
    let seqs: BTreeSet<i64> = events.iter().map(|e| e.seq).collect();
    assert_eq!(seqs, (1..=200).collect());
}

#[test]
fn concurrent_first_open_of_a_fresh_database_succeeds() {
    // Many hooks can start at once on a machine that has never run bridge-screen:
    // every one must open (and migrate) the new database without a "locked" error.
    for _ in 0..10 {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("db.sqlite");
        let barrier = Arc::new(Barrier::new(8));
        let handles: Vec<_> = (0..8)
            .map(|t| {
                let path = path.clone();
                let barrier = Arc::clone(&barrier);
                thread::spawn(move || {
                    barrier.wait();
                    let mut store = Store::open(&path)?;
                    store.append(EventDraft::new("r", format!("s{t}"), "note", json!({})))?;
                    Ok::<_, bridge_screen_core::error::Error>(())
                })
            })
            .collect();
        for h in handles {
            h.join().unwrap().unwrap();
        }
        let store = Store::open(&path).unwrap();
        assert_eq!(read_all(store.conn(), None).unwrap().len(), 8);
        let mode: String = store
            .conn()
            .query_row("PRAGMA journal_mode", [], |r| r.get(0))
            .unwrap();
        assert_eq!(mode, "wal");
    }
}
