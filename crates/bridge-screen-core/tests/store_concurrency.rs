use std::collections::BTreeSet;
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
