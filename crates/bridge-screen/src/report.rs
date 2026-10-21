//! Opt-in progress reports (spec §10.6): a self-contained HTML snapshot — inline CSS, no
//! scripts, no outside URLs — written to the OS temp folder for the owner to publish.
//! Nothing here touches the network; publishing is the agent's Artifact tool, after the
//! owner confirms.

use std::fmt::Write as _;
use std::path::PathBuf;

use anyhow::Result;
use bridge_screen_core::decision;
use bridge_screen_core::store::Store;
use bridge_screen_core::verify;
use bridge_screen_core::view::{Glance, ProjectRow};

/// A written report and what it holds.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Report {
    pub path: PathBuf,
    /// Exactly what the page contains, for the owner to check before publishing.
    pub summary: String,
}

/// Decisions shown per report.
const MAX_DECISIONS: usize = 10;

fn esc(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

/// Repos whose owner said `publish_reports = never`.
fn private(store: &Store, repo_id: &str) -> Result<bool> {
    Ok(verify::settings(store.conn(), repo_id)?
        .publish_reports
        .as_deref()
        == Some("never"))
}

const STYLE: &str = "body{font:15px/1.5 system-ui,sans-serif;background:#1f1e1d;color:#ece9e3;max-width:52rem;margin:2rem auto;padding:0 1rem}
h1{font-size:1.2rem}h1 .m{color:#d97757}h2{font-size:1rem;margin-top:1.5rem}
.muted{color:#a39e94}.bad{color:#f07a72}.ok{color:#4eba65}.warn{color:#e5a83b}
.bar{display:flex;height:.5rem;border:1px solid #8a857c;border-radius:4px;overflow:hidden;margin:.2rem 0}
.done{background:#d97757}.claimed{background:repeating-linear-gradient(135deg,#d97757 0 3px,transparent 3px 6px)}
ul{padding-left:1.2rem}li{margin:.15rem 0}";

/// Write the report for `glance` (already narrowed to its scope) and describe it.
pub fn write(store: &Store, glance: &Glance, label: &str) -> Result<Report> {
    let mut left_out = Vec::new();
    let mut projects: Vec<&ProjectRow> = Vec::new();
    for p in &glance.projects {
        if private(store, &p.repo_id)? {
            left_out.push(p.name.clone());
        } else {
            projects.push(p);
        }
    }
    anyhow::ensure!(
        !projects.is_empty(),
        "refused: publish_reports is set to never for {} (bridge-screen config set publish-reports ask to change it)",
        left_out.join(", ")
    );
    let shown: Vec<&str> = projects.iter().map(|p| p.repo_id.as_str()).collect();
    let items: Vec<_> = glance
        .needs_you
        .iter()
        .filter(|i| shown.contains(&i.repo_id.as_str()))
        .collect();
    let mut html = format!(
        "<!doctype html><html lang=\"en\"><head><meta charset=\"utf-8\"><title>bridge-screen progress</title><style>{STYLE}</style></head><body>\n<h1><span class=\"m\">✱</span> bridge-screen progress <span class=\"muted\">as of {}</span></h1>\n",
        esc(&glance.as_of)
    );
    if items.is_empty() {
        html.push_str("<p class=\"ok\">● All clear: nothing needs the owner.</p>\n");
    } else {
        let _ = writeln!(
            html,
            "<h2 class=\"bad\">■ {} need the owner</h2><ul>",
            items.len()
        );
        for i in &items {
            let _ = writeln!(
                html,
                "<li>{} <span class=\"muted\">· {}</span></li>",
                esc(&i.title),
                esc(&i.repo_name)
            );
        }
        html.push_str("</ul>\n");
    }
    let mut task_titles: Vec<String> = Vec::new();
    let mut decisions_shown = 0;
    for p in &projects {
        decisions_shown += project_html(store, p, &mut html, &mut task_titles, decisions_shown)?;
    }
    html.push_str("</body></html>\n");
    let stamp = glance.as_of.replace([':', '.'], "-");
    let path = std::env::temp_dir().join(format!("bridge-screen-report-{label}-{stamp}.html"));
    std::fs::write(&path, &html)?;
    task_titles.sort();
    task_titles.dedup();
    let names: Vec<String> = projects.iter().map(|p| titles(p)).collect();
    let mut summary = format!(
        "Wrote {}. It is one self-contained page (inline styles, no scripts, no outside links) holding:\n- the time of the snapshot ({})\n- {} needs-you item(s){}\n- projects: {} (done% and claimed% bars, the forecast, tokens and cost, milestone hill states, live-session counts)\n- {decisions_shown} recent decision(s) (what and why){}",
        path.display(),
        glance.as_of,
        items.len(),
        if items.is_empty() {
            String::new()
        } else {
            format!(
                ": {}",
                items
                    .iter()
                    .map(|i| i.title.as_str())
                    .collect::<Vec<_>>()
                    .join("; ")
            )
        },
        names.join(", "),
        if task_titles.is_empty() {
            String::new()
        } else {
            format!(", naming tasks {}", task_titles.join("; "))
        }
    );
    if !left_out.is_empty() {
        let _ = write!(
            summary,
            "\nLeft out (publish_reports = never): {}.",
            left_out.join(", ")
        );
    }
    summary.push_str(
        "\nShow this summary to the owner. Publish it with the Artifact tool only after the owner confirms, then call report_published with the link.",
    );
    Ok(Report { path, summary })
}

/// Every title the page shows for a project: its name, goal and milestones.
fn titles(p: &ProjectRow) -> String {
    let mut parts: Vec<String> = p.goal.iter().map(|g| format!("goal {}", g.title)).collect();
    if !p.milestones.is_empty() {
        let ms: Vec<String> = p
            .milestones
            .iter()
            .map(|m| format!("{} {}", m.id, m.title))
            .collect();
        parts.push(format!("milestones {}", ms.join(", ")));
    }
    if parts.is_empty() {
        p.name.clone()
    } else {
        format!("{} ({})", p.name, parts.join("; "))
    }
}

/// One project's section; returns how many decisions it showed.
fn project_html(
    store: &Store,
    p: &ProjectRow,
    html: &mut String,
    task_titles: &mut Vec<String>,
    decisions_shown: usize,
) -> Result<usize> {
    let _ = writeln!(
        html,
        "<h2>{}{}</h2>\n<div class=\"bar\"><div class=\"done\" style=\"width:{}%\"></div><div class=\"claimed\" style=\"width:{}%\"></div></div>\n<p>{}% done · +{}% claimed · {} live sessions</p>",
        esc(&p.name),
        p.goal
            .as_ref()
            .map(|g| format!(" — {}", esc(&g.title)))
            .unwrap_or_default(),
        p.done_pct,
        p.claimed_pct,
        p.done_pct,
        p.claimed_pct,
        p.sessions.len()
    );
    if let Some(f) = &p.forecast {
        let _ = writeln!(
            html,
            "<p title=\"{}\">{} <span class=\"muted\">({})</span></p>",
            esc(&f.help),
            esc(&f.text),
            esc(if f.compact.is_some() {
                &f.help
            } else {
                &f.basis
            })
        );
    }
    if let Some(cost) = &p.cost {
        let _ = writeln!(html, "<p class=\"muted\">{}</p>", esc(&cost.summary));
    }
    if !p.milestones.is_empty() {
        html.push_str("<ul>");
        for m in &p.milestones {
            let hill = match (m.hill, crate::words::hill(m.hill, m.done_pct)) {
                (Some(h), Some((icon, word))) => {
                    let class = if word == "exploring" { "warn" } else { "ok" };
                    format!(
                        " <span class=\"{class}\" title=\"{}\">{icon} {word}</span>",
                        esc(crate::words::hill_tip(h))
                    )
                }
                _ => String::new(),
            };
            let stuck = if m.stuck {
                format!(
                    " <span class=\"warn\">stuck · {} sessions</span>",
                    m.stuck_sessions
                )
            } else {
                String::new()
            };
            let _ = write!(
                html,
                "<li>{} {}{hill} — {}%{stuck}</li>",
                esc(&m.id),
                esc(&m.title),
                m.done_pct
            );
        }
        html.push_str("</ul>\n");
    }
    let mut recent = decision::list(store.conn(), &p.repo_id, None, false)?;
    recent.reverse();
    recent.truncate(MAX_DECISIONS.saturating_sub(decisions_shown));
    if !recent.is_empty() {
        html.push_str("<h2>Recent decisions</h2><ul>");
        for d in &recent {
            let task = match &d.task_id {
                Some(t) => {
                    let title = bridge_screen_core::claim::task_title(store.conn(), &p.repo_id, t)?
                        .unwrap_or_default();
                    let shown = format!("{t} {title}").trim_end().to_string();
                    task_titles.push(shown.clone());
                    format!(" <span class=\"muted\">({})</span>", esc(&shown))
                }
                None => String::new(),
            };
            let _ = write!(html, "<li>{} — {}{task}</li>", esc(&d.what), esc(&d.why));
        }
        html.push_str("</ul>\n");
    }
    Ok(recent.len())
}
