use std::fmt::Write as _;

use super::parse::{MAX_TEXT_CHARS, parse};

const SUPERPOWERS: &str = r"# Login Flow Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: use superpowers:executing-plans.

**Goal:** Users can sign in with email
and a password.

**Architecture:** A form and an endpoint.

---

### Task 1: Login endpoint

**Files:**
- Create: `src/api/login.rs`
- Modify: `src/api/mod.rs:12-40`
- Test: `tests/login.rs`

- [x] **Step 1: Write the failing test**

```rust
- [ ] not a step
```

- [ ] **Step 2: Run it to see it fail**
- [ ] **Step 3: Implement**

### Task 2: Login form

**Files:**
- Create: `web/Login.svelte`

- [ ] **Step 1: Render the form**
";

#[test]
fn parses_superpowers_template() {
    let p = parse(SUPERPOWERS);
    assert_eq!(p.title.as_deref(), Some("Login Flow"));
    assert_eq!(
        p.goal.as_deref(),
        Some("Users can sign in with email and a password.")
    );
    assert_eq!(p.tasks.len(), 2);
    let t1 = &p.tasks[0];
    assert_eq!((t1.number, t1.title.as_str()), (1, "Login endpoint"));
    assert_eq!(
        t1.files,
        vec!["src/api/login.rs", "src/api/mod.rs", "tests/login.rs"]
    );
    let steps: Vec<(u32, &str, bool)> = t1
        .steps
        .iter()
        .map(|s| (s.number, s.text.as_str(), s.checked))
        .collect();
    assert_eq!(
        steps,
        vec![
            (1, "Write the failing test", true),
            (2, "Run it to see it fail", false),
            (3, "Implement", false),
        ]
    );
    assert_eq!(p.tasks[1].files, vec!["web/Login.svelte"]);
    assert_eq!(p.tasks[1].steps.len(), 1);
    assert_eq!(p.warnings, Vec::<String>::new());
}

#[test]
fn parses_this_repos_plan_style() {
    let text = "# Sub-project 7: Polish — Implementation Plan\n\n\
                **Goal:** Fewer false alarms.\n\n\
                ### Task 1: Words\n\n\
                - [ ] Failing tests:\n  - `a_test`: something\n  - [ ] nested is not a step\n\
                - [x] Implement; commit `core: words`.\n\n\
                ### Task 2 — Screens\n\n- [ ] Build it\n";
    let p = parse(text);
    assert_eq!(p.title.as_deref(), Some("Sub-project 7: Polish"));
    assert_eq!(p.goal.as_deref(), Some("Fewer false alarms."));
    assert_eq!(p.tasks.len(), 2);
    assert_eq!(p.tasks[0].files, Vec::<String>::new());
    let steps: Vec<(u32, &str, bool)> = p.tasks[0]
        .steps
        .iter()
        .map(|s| (s.number, s.text.as_str(), s.checked))
        .collect();
    assert_eq!(
        steps,
        vec![
            (1, "Failing tests:", false),
            (2, "Implement; commit `core: words`.", true)
        ]
    );
    assert_eq!(p.tasks[1].title, "Screens");
}

#[test]
fn fenced_checkboxes_are_not_steps() {
    let text = "### Task 1: A\n\n```\n- [ ] fake\n### Task 2: fake\n```\n~~~\n- [x] also fake\n~~~\n- [ ] real\n";
    let p = parse(text);
    assert_eq!(p.tasks.len(), 1);
    assert_eq!(p.tasks[0].steps.len(), 1);
    assert_eq!(p.tasks[0].steps[0].text, "real");
}

#[test]
fn odd_plan_files_are_skipped() {
    let empty = parse("");
    assert_eq!(empty.tasks.len(), 0);
    assert_ne!(empty.warnings, Vec::<String>::new());

    let none = parse("# Notes\n\nJust some text.\n- [ ] a stray box\n");
    assert_eq!(none.tasks.len(), 0);
    assert_ne!(none.warnings, Vec::<String>::new());

    let dup = parse(
        "### Task 1: First\n- [ ] a\n### Task 1: Again\n- [ ] b\n### Task 0: zero\n### Task 2:\n",
    );
    assert_eq!(dup.tasks.len(), 1);
    assert_eq!(dup.tasks[0].title, "First");
    assert_eq!(dup.tasks[0].steps.len(), 1);
    assert_eq!(dup.warnings.len(), 3, "{:?}", dup.warnings);

    let long_step = format!("### Task 1: T\n- [ ] {}\n", "x".repeat(1000));
    assert_eq!(
        parse(&long_step).tasks[0].steps[0].text.chars().count(),
        MAX_TEXT_CHARS
    );

    let mut big = String::with_capacity(5 << 20);
    let mut n = 0;
    while big.len() < 5 << 20 {
        n += 1;
        let _ = write!(
            big,
            "### Task {n}: Task number {n}\n- [ ] step one\n- [x] step two\n\n"
        );
    }
    let p = parse(&big);
    assert_eq!(p.tasks.len(), n);

    // Lossy text from a binary file never panics.
    let binary = String::from_utf8_lossy(&[0xff, 0xfe, 0, 1, 2, b'#', b' ', 0xc3]).to_string();
    assert_eq!(parse(&binary).tasks.len(), 0);
}
