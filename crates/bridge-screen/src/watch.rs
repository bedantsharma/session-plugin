//! `bridge-screen watch`: the glance as a live full-terminal view (spec §10.3).
//!
//! It renders `view::glance` like every other screen, refreshes every second (re-reading
//! only when the event log moved, or every 5 s for live times), and acts through
//! `owner::act`, so a key press records what a dashboard click records. Every state has a
//! word as well as a colour; emphasis is bold/reverse, which survives `NO_COLOR`.

use std::fmt::Write as _;
use std::io::IsTerminal as _;
use std::time::{Duration, Instant};

use anyhow::Result;
use bridge_screen_core::config::Config;
use bridge_screen_core::owner::{self, OwnerAction};
use bridge_screen_core::paths::Home;
use bridge_screen_core::store::Store;
use bridge_screen_core::view::{self, Glance, Light, NeedsKind, NeedsYou, SessionRow};
use ratatui::Frame;
use ratatui::Terminal;
use ratatui::backend::Backend;
use ratatui::crossterm::event::{self, Event, KeyCode, KeyEvent, KeyEventKind};
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Paragraph, Wrap};

/// Below this width every item is one short line.
pub const NARROW: u16 = 80;

/// What `watch` needs from the store: the glance, details and owner actions. Tests use
/// a fake.
pub trait Actor {
    fn glance(&mut self) -> Option<Glance>;
    /// The event log's cursor, to skip re-reading when nothing moved.
    fn cursor(&mut self) -> Option<i64>;
    fn detail(&mut self, target: &Target) -> Vec<String>;
    fn act(&mut self, action: &OwnerAction) -> Result<String, String>;
}

/// What a row is about.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Target {
    Item(String),
    Project(String),
    Session(String),
}

/// A field being typed for an action.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Field {
    name: String,
    label: String,
    required: bool,
    value: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum Picker {
    /// Choose an action with 1–9. Items are held by id: a refresh may reorder the queue.
    Choose { item: String },
    /// Type the action's inputs, one prompt at a time.
    Input {
        item: String,
        action: usize,
        fields: Vec<Field>,
        at: usize,
    },
}

/// The watch screen's state.
pub struct App {
    pub glance: Glance,
    pub color: bool,
    pub selected: usize,
    pub detail: Option<(usize, Vec<String>)>,
    picker: Option<Picker>,
    pub help: bool,
    pub message: Option<String>,
    pub quit: bool,
}

impl App {
    pub fn new(glance: Glance, color: bool) -> Self {
        Self {
            glance,
            color,
            selected: 0,
            detail: None,
            picker: None,
            help: false,
            message: None,
            quit: false,
        }
    }

    fn sessions(&self) -> Vec<&SessionRow> {
        self.glance
            .projects
            .iter()
            .flat_map(|p| &p.sessions)
            .collect()
    }

    /// Every selectable row, in screen order.
    pub fn rows(&self) -> Vec<Target> {
        let mut out: Vec<Target> = self
            .glance
            .needs_you
            .iter()
            .map(|i| Target::Item(i.id.clone()))
            .collect();
        out.extend(
            self.glance
                .projects
                .iter()
                .map(|p| Target::Project(p.repo_id.clone())),
        );
        out.extend(
            self.sessions()
                .iter()
                .map(|s| Target::Session(s.session_id.clone())),
        );
        out
    }

    /// Swap in a new glance, keeping the selection on the same row when it still exists.
    pub fn set_glance(&mut self, glance: Glance) {
        let current = self.rows().get(self.selected).cloned();
        self.glance = glance;
        let rows = self.rows();
        self.selected = current
            .and_then(|c| rows.iter().position(|r| *r == c))
            .unwrap_or(self.selected.min(rows.len().saturating_sub(1)));
        if let Some(Picker::Choose { item } | Picker::Input { item, .. }) = &self.picker
            && self.item_pos(item).is_none()
        {
            self.picker = None;
            self.message = Some("already resolved / no longer needed".into());
        }
    }

    /// Where the needs-you item `id` is now.
    fn item_pos(&self, id: &str) -> Option<usize> {
        self.glance.needs_you.iter().position(|i| i.id == id)
    }

    fn item_index(&self) -> Option<usize> {
        match self.rows().get(self.selected)? {
            Target::Item(id) => self.glance.needs_you.iter().position(|i| &i.id == id),
            _ => None,
        }
    }

    /// Handle one key; the actor runs actions and loads details.
    pub fn on_key(&mut self, key: KeyEvent, actor: &mut dyn Actor) {
        if key.kind != KeyEventKind::Press {
            return;
        }
        if let Some(picker) = self.picker.clone() {
            self.on_picker_key(picker, key.code, actor);
            return;
        }
        let rows = self.rows().len();
        match key.code {
            KeyCode::Char('q') => self.quit = true,
            KeyCode::Char('?') => self.help = !self.help,
            KeyCode::Esc => {
                self.help = false;
                self.detail = None;
            }
            KeyCode::Down | KeyCode::Char('j') if rows > 0 => {
                self.selected = (self.selected + 1).min(rows - 1);
            }
            KeyCode::Up | KeyCode::Char('k') => self.selected = self.selected.saturating_sub(1),
            KeyCode::Enter => {
                if self
                    .detail
                    .as_ref()
                    .is_some_and(|(i, _)| *i == self.selected)
                {
                    self.detail = None;
                } else if let Some(target) = self.rows().get(self.selected) {
                    self.detail = Some((self.selected, actor.detail(target)));
                }
            }
            KeyCode::Char('a') => match self.item_index() {
                Some(i) if !self.glance.needs_you[i].actions.is_empty() => {
                    self.picker = Some(Picker::Choose {
                        item: self.glance.needs_you[i].id.clone(),
                    });
                }
                Some(i) => {
                    let item = &self.glance.needs_you[i];
                    self.message = Some(match &item.respond_in {
                        Some(r) => format!(
                            "answer in S {}{}",
                            r.session_short,
                            r.worktree
                                .as_deref()
                                .map(|w| format!(" · {w}"))
                                .unwrap_or_default()
                        ),
                        None => "nothing to do here".into(),
                    });
                }
                None => self.message = Some("select a needs-you item to act on it".into()),
            },
            _ => {}
        }
    }

    fn on_picker_key(&mut self, picker: Picker, code: KeyCode, actor: &mut dyn Actor) {
        if code == KeyCode::Esc {
            self.picker = None;
            return;
        }
        let (Picker::Choose { item: id } | Picker::Input { item: id, .. }) = &picker;
        let Some(item) = self.item_pos(id) else {
            self.picker = None;
            self.message = Some("already resolved / no longer needed".into());
            return;
        };
        match picker {
            Picker::Choose { item: id } => {
                let KeyCode::Char(c) = code else { return };
                let Some(n) = c.to_digit(10).and_then(|d| usize::try_from(d).ok()) else {
                    return;
                };
                let Some(spec) = self.glance.needs_you[item].actions.get(n.wrapping_sub(1)) else {
                    return;
                };
                let fields: Vec<Field> = spec
                    .inputs
                    .iter()
                    .map(|i| Field {
                        name: i.name.clone(),
                        label: i.label.clone(),
                        required: i.required,
                        value: prefill(&spec.action, &i.name),
                    })
                    .collect();
                if fields.is_empty() {
                    self.picker = None;
                    self.run(item, n - 1, &[], actor);
                } else {
                    self.picker = Some(Picker::Input {
                        item: id,
                        action: n - 1,
                        fields,
                        at: 0,
                    });
                }
            }
            Picker::Input {
                item: id,
                action,
                mut fields,
                at,
            } => {
                match code {
                    KeyCode::Char(c) => fields[at].value.push(c),
                    KeyCode::Backspace => {
                        fields[at].value.pop();
                    }
                    KeyCode::Enter => {
                        if fields[at].required && fields[at].value.trim().is_empty() {
                            return;
                        }
                        if at + 1 == fields.len() {
                            self.picker = None;
                            self.run(item, action, &fields, actor);
                            return;
                        }
                        self.picker = Some(Picker::Input {
                            item: id,
                            action,
                            fields,
                            at: at + 1,
                        });
                        return;
                    }
                    _ => return,
                }
                self.picker = Some(Picker::Input {
                    item: id,
                    action,
                    fields,
                    at,
                });
            }
        }
    }

    /// Send the item's action with the typed fields (empty optional ones left out).
    fn run(&mut self, item: usize, action: usize, fields: &[Field], actor: &mut dyn Actor) {
        let spec = &self.glance.needs_you[item].actions[action];
        let mut value = serde_json::to_value(&spec.action).unwrap_or_default();
        for f in fields {
            let v = f.value.trim();
            if !v.is_empty() {
                value[&f.name] = serde_json::Value::from(v);
            }
        }
        self.message = Some(match serde_json::from_value::<OwnerAction>(value) {
            Ok(a) => match actor.act(&a) {
                Ok(reply) => reply,
                Err(e) => e,
            },
            Err(e) => format!("not an action: {e}"),
        });
        if let Some(g) = actor.glance() {
            self.set_glance(g);
        }
    }
}

/// An input's starting text: what the server already filled in (e.g. the parent).
fn prefill(action: &OwnerAction, name: &str) -> String {
    serde_json::to_value(action)
        .ok()
        .and_then(|v| v[name].as_str().map(str::to_string))
        .unwrap_or_default()
}

/// §10.4 roles as terminal colours (clay → 256-colour 173), or nothing without colour.
fn role(color: bool, c: Color) -> Style {
    if color {
        Style::default().fg(c)
    } else {
        Style::default()
    }
}

const CLAY: Color = Color::Indexed(173);

fn light_style(color: bool, light: Light) -> Style {
    role(
        color,
        match light {
            Light::Green => Color::Green,
            Light::Amber => Color::Yellow,
            Light::Red => Color::Red,
        },
    )
}

fn shape(light: Light) -> &'static str {
    match light {
        Light::Green => "●",
        Light::Amber => "◆",
        Light::Red => "■",
    }
}

fn kind_word(kind: NeedsKind) -> &'static str {
    match kind {
        NeedsKind::DriftRed => "drift",
        NeedsKind::Question => "question",
        NeedsKind::PermissionWait => "waiting",
        NeedsKind::VerifyOverdue => "check",
        NeedsKind::Leftover => "unsaved",
        NeedsKind::PlanScope => "plans",
    }
}

/// `text` cut to `max` characters, with `…` when it was longer (never mid-word).
pub fn fit(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        return text.to_string();
    }
    if max == 0 {
        return String::new();
    }
    let mut short: String = text.chars().take(max - 1).collect();
    if let Some(space) = short.rfind(' ').filter(|&i| i > 0) {
        short.truncate(space);
    }
    short.truncate(short.trim_end().len());
    short.push('…');
    short
}

/// One line, cut to the width.
fn line(spans: Vec<(String, Style)>, width: usize) -> Line<'static> {
    let mut left = width;
    let mut out = Vec::new();
    for (text, style) in spans {
        if left == 0 {
            break;
        }
        let t = fit(&text, left);
        left = left.saturating_sub(t.chars().count());
        out.push(Span::styled(t, style));
    }
    Line::from(out)
}

fn selected_style(app: &App, row: usize) -> Style {
    if row == app.selected {
        Style::default().add_modifier(Modifier::REVERSED)
    } else {
        Style::default()
    }
}

fn header(app: &App, width: usize) -> Line<'static> {
    let g = &app.glance;
    let c = app.color;
    let mut spans = vec![
        ("✱ ".to_string(), role(c, CLAY)),
        (
            "bridge-screen".to_string(),
            Style::default().add_modifier(Modifier::BOLD),
        ),
        (
            format!(
                " · {} sessions · {} projects   ",
                g.live_sessions,
                g.projects.len()
            ),
            Style::default(),
        ),
    ];
    if g.needs_you_count > 0 {
        let mut style = role(c, Color::Red).add_modifier(Modifier::BOLD);
        if g.escalated {
            style = style.add_modifier(Modifier::REVERSED);
        }
        spans.push((format!("■ {} need you", g.needs_you_count), style));
    } else {
        spans.push(("● All clear".to_string(), role(c, Color::Green)));
    }
    line(spans, width)
}

fn item_line(app: &App, i: &NeedsYou, row: usize, width: usize, narrow: bool) -> Line<'static> {
    let sel = selected_style(app, row);
    let red = role(app.color, Color::Red).add_modifier(Modifier::BOLD);
    if narrow {
        let who = i
            .session_id
            .as_deref()
            .map(|s| s.chars().take(6).collect::<String>())
            .or_else(|| i.task.as_ref().map(|t| t.id.clone()))
            .unwrap_or_default();
        return line(
            vec![
                (format!("■ {} ", kind_word(i.kind)), red.patch(sel)),
                (who, sel),
            ],
            width,
        );
    }
    // `[a]` comes before the detail, so a narrow line keeps it.
    let keys = if i.actions.is_empty() { "" } else { " [a]" };
    line(
        vec![
            (format!("■ {:<10}", kind_word(i.kind)), red.patch(sel)),
            (i.title.clone(), sel),
            (keys.to_string(), sel.add_modifier(Modifier::BOLD)),
            (format!(" · {}", i.detail), sel),
        ],
        width,
    )
}

fn bar(done: u32, claimed: u32, cells: usize) -> (String, String, String) {
    let done_n = cells * usize::try_from(done).unwrap_or(0) / 100;
    let claimed_n = (cells * usize::try_from(claimed).unwrap_or(0) / 100).min(cells - done_n);
    (
        "█".repeat(done_n),
        "▒".repeat(claimed_n),
        "░".repeat(cells - done_n - claimed_n),
    )
}

fn project_lines(app: &App, row: usize, width: usize, narrow: bool) -> Vec<Line<'static>> {
    let start = app.glance.needs_you.len();
    let p = &app.glance.projects[row - start];
    let sel = selected_style(app, row);
    let c = app.color;
    let pct = format!("{}% done", p.done_pct);
    if narrow {
        return vec![line(vec![(format!("{} {pct}", p.name), sel)], width)];
    }
    let (d, cl, rest) = bar(p.done_pct, p.claimed_pct, 12);
    let claimed = if p.claimed_pct > 0 {
        format!(" · +{}% claimed", p.claimed_pct)
    } else {
        String::new()
    };
    let mut extra = String::new();
    if let Some(f) = &p.forecast {
        let _ = write!(extra, " · {}", f.short);
    }
    if let Some(cost) = &p.cost {
        let _ = write!(extra, " · {}", cost.text);
    }
    let mut spans = vec![
        (format!("{:<12} ", p.name), sel.add_modifier(Modifier::BOLD)),
        (d, role(c, CLAY)),
        (cl, role(c, CLAY)),
        (rest, Style::default()),
        (format!(" {pct}{claimed}{extra}"), sel),
    ];
    for m in &p.milestones {
        let mut hill = crate::words::milestone_hill(m)
            .map(|(icon, word)| format!(" {icon} {word}"))
            .unwrap_or_default();
        if let Some(mark) = crate::words::rollup_mark(m.status) {
            let _ = write!(hill, " {mark}");
        }
        spans.push((
            format!("  {} {}%{hill}", m.shown, m.done_pct),
            role(c, Color::Yellow),
        ));
        if m.stuck {
            spans.push((
                format!(" stuck·{}", m.stuck_sessions),
                role(c, Color::Yellow),
            ));
        }
    }
    vec![line(spans, width)]
}

fn session_line(
    app: &App,
    s: &SessionRow,
    row: usize,
    width: usize,
    narrow: bool,
) -> Line<'static> {
    let sel = selected_style(app, row);
    let lit = light_style(app.color, s.light).patch(sel);
    if narrow {
        return line(
            vec![
                (format!("{} ", shape(s.light)), lit),
                (
                    format!("{} {} {}m", s.short, s.light_word, s.active_min),
                    sel,
                ),
            ],
            width,
        );
    }
    let what = match (&s.detour, &s.task) {
        (Some(d), _) => format!("{} {}", d.label, d.reason),
        (None, Some(t)) => format!("{} {}", t.shown, t.title),
        (None, None) => "(no task)".to_string(),
    };
    let cost = s
        .cost
        .as_ref()
        .map(|c| format!(" · {}", c.text))
        .unwrap_or_default();
    let mut flags = s.flags.iter().fold(String::new(), |mut out, f| {
        let age = f.age_s.map(|a| format!(" {}m", a / 60)).unwrap_or_default();
        let _ = write!(out, " · {}{age}", f.kind.text());
        out
    });
    if let Some(w) = crate::words::waiting(&s.waiting) {
        let _ = write!(flags, " · {w}");
    }
    line(
        vec![
            (format!("{} {:<10}", shape(s.light), s.light_word), lit),
            (format!("S {}  ", s.short), sel),
            (what, sel),
            (flags, sel),
            (format!("  {}m{cost}", s.active_min), sel),
        ],
        width,
    )
}

/// Units listed under an expanded session.
const UNITS_SHOWN: usize = 5;

const LEGEND: &str = "↑↓/jk move · Enter expand · a act · ? help · q quit";

/// Draw the whole screen.
pub fn render(f: &mut Frame, app: &App) {
    let area = f.area();
    let width = usize::from(area.width);
    let narrow = area.width < NARROW;
    let mut lines = vec![header(app, width), Line::from("")];
    let mut row = 0;
    let detail = |lines: &mut Vec<Line<'static>>, row: usize| {
        if let Some((r, text)) = &app.detail
            && *r == row
        {
            for t in text {
                lines.push(line(vec![(format!("    {t}"), Style::default())], width));
            }
        }
    };
    if !app.glance.needs_you.is_empty() {
        lines.push(line(
            vec![(
                "Needs you".to_string(),
                Style::default().add_modifier(Modifier::BOLD),
            )],
            width,
        ));
        for i in &app.glance.needs_you {
            lines.push(item_line(app, i, row, width, narrow));
            detail(&mut lines, row);
            row += 1;
        }
        lines.push(Line::from(""));
    }
    for _ in &app.glance.projects {
        lines.extend(project_lines(app, row, width, narrow));
        detail(&mut lines, row);
        row += 1;
    }
    // Branch groups under the projects (not selectable).
    for g in &app.glance.branch_groups {
        lines.push(line(
            vec![(
                format!("  ⑂ {}", crate::words::branch_group(g)),
                role(app.color, CLAY),
            )],
            width,
        ));
    }
    lines.push(Line::from(""));
    for s in app.sessions() {
        lines.push(session_line(app, s, row, width, narrow));
        detail(&mut lines, row);
        row += 1;
    }
    let body = Rect {
        height: area.height.saturating_sub(2),
        ..area
    };
    f.render_widget(Paragraph::new(lines), body);
    let foot = Rect {
        y: area.y + area.height.saturating_sub(1),
        height: area.height.min(1),
        ..area
    };
    let footer = match &app.message {
        Some(m) => line(
            vec![(m.clone(), Style::default().add_modifier(Modifier::BOLD))],
            width,
        ),
        None => line(vec![(LEGEND.to_string(), Style::default())], width),
    };
    f.render_widget(Paragraph::new(footer), foot);
    overlays(f, app);
}

/// The help overlay and the action picker, over the glance.
fn overlays(f: &mut Frame, app: &App) {
    if app.help {
        popup(
            f,
            "Help",
            &[
                "↑/↓ or j/k: select a row",
                "Enter: expand the session or project",
                "a: act on the selected needs-you item",
                "  1–9 pick the action; type, Enter; Esc cancels",
                "?: this help · q: quit",
            ],
        );
    }
    if let Some(p) = &app.picker {
        let text: Vec<String> = match p {
            Picker::Choose { item } => {
                let Some(i) = app.glance.needs_you.iter().find(|i| &i.id == item) else {
                    return;
                };
                let mut t = vec![i.title.clone()];
                t.extend(
                    i.actions
                        .iter()
                        .enumerate()
                        .map(|(n, a)| format!("{}  {}", n + 1, a.label)),
                );
                t.push("Esc cancels".into());
                t
            }
            Picker::Input { fields, at, .. } => {
                let f = &fields[*at];
                let hint = if f.required {
                    "Enter to go on"
                } else {
                    "Enter (empty skips)"
                };
                vec![
                    format!("{}: {}▏", f.label, f.value),
                    format!("{hint} · Esc cancels"),
                ]
            }
        };
        let refs: Vec<&str> = text.iter().map(String::as_str).collect();
        popup(f, "Act", &refs);
    }
}

fn popup(f: &mut Frame, title: &str, text: &[&str]) {
    let area = f.area();
    let w = area.width.saturating_sub(4).min(64);
    let h = u16::try_from(text.len() + 2)
        .unwrap_or(u16::MAX)
        .min(area.height);
    let rect = Rect {
        x: area.x + (area.width - w) / 2,
        y: area.y + (area.height - h) / 2,
        width: w,
        height: h,
    };
    f.render_widget(Clear, rect);
    let lines: Vec<Line> = text.iter().map(|t| Line::from((*t).to_string())).collect();
    f.render_widget(
        Paragraph::new(lines).wrap(Wrap { trim: true }).block(
            Block::default()
                .borders(Borders::ALL)
                .title(title.to_string()),
        ),
        rect,
    );
}

/// The store behind `watch`.
pub struct StoreActor {
    pub home: Home,
    pub cfg: Config,
}

impl StoreActor {
    fn store(&self) -> Option<Store> {
        Store::open(&self.home.db_path()).ok()
    }
}

impl Actor for StoreActor {
    fn glance(&mut self) -> Option<Glance> {
        let s = self.store()?;
        view::glance(s.conn(), &self.cfg, jiff::Timestamp::now()).ok()
    }

    fn cursor(&mut self) -> Option<i64> {
        view::last_event_pos(self.store()?.conn()).ok()
    }

    fn detail(&mut self, target: &Target) -> Vec<String> {
        let Some(s) = self.store() else {
            return vec!["the database can't be read".into()];
        };
        let now = jiff::Timestamp::now();
        match target {
            Target::Item(_) => vec!["press a to act on it".into()],
            Target::Session(id) => {
                match view::drill::session(s.conn(), id, &self.cfg, now) {
                    Ok(Some(d)) => {
                        let mut out = vec![format!(
                            "{} units · on task {} · drift {} · handoff understood: {}",
                            d.units.len(),
                            crate::words::mins(d.split.on_task_s),
                            crate::words::mins(d.split.drift_s),
                            crate::words::readback(d.readback)
                        )];
                        // The last few units with their labels in words.
                        let skip = d.units.len().saturating_sub(UNITS_SHOWN);
                        out.extend(d.units.iter().skip(skip).map(|u| {
                            format!(
                                "#{} {:>7}  {}",
                                u.unit_no,
                                crate::words::mins(u.active_s),
                                crate::words::unit_label(u.label, u.label_source)
                            )
                        }));
                        out.extend(
                            d.split
                                .detours
                                .iter()
                                .map(|x| format!("{} {} · {}m", x.label, x.reason, x.secs / 60)),
                        );
                        out.extend(d.suspended.iter().map(|x| {
                            format!("suspended {} — on return: {}", x.what, x.resume_cue)
                        }));
                        out.extend(d.checkpoints.last().map(|c| {
                            format!(
                                "last saved at {} — next: {}",
                                c.sha.chars().take(8).collect::<String>(),
                                c.next_action.clone().unwrap_or_default()
                            )
                        }));
                        out
                    }
                    _ => vec!["no details".into()],
                }
            }
            Target::Project(repo) => match view::drill::project(s.conn(), repo, now) {
                Ok(p) => p
                    .tree
                    .iter()
                    .filter(|n| n.depth <= 2)
                    .map(|n| {
                        format!(
                            "{}{} {} [{}]",
                            "  ".repeat(n.depth),
                            n.id,
                            n.title,
                            crate::words::status(
                                n.rollup.map_or_else(|| n.status.as_str(), |r| r.as_str())
                            )
                        )
                    })
                    .collect(),
                Err(e) => vec![e.to_string()],
            },
        }
    }

    fn act(&mut self, action: &OwnerAction) -> Result<String, String> {
        let mut store = Store::open(&self.home.db_path()).map_err(|e| e.to_string())?;
        let ctx = owner::ActCtx {
            home: &self.home,
            cfg: &self.cfg,
            now: jiff::Timestamp::now(),
            dir: None,
        };
        owner::act(&mut store, &ctx, action).map_err(|e| e.to_string())
    }
}

/// Restores the terminal when dropped, including while unwinding from a panic.
pub struct Guard {
    restore: Option<Box<dyn FnOnce()>>,
}

impl Guard {
    pub fn new(restore: impl FnOnce() + 'static) -> Self {
        Self {
            restore: Some(Box::new(restore)),
        }
    }
}

impl Drop for Guard {
    fn drop(&mut self) {
        if let Some(r) = self.restore.take() {
            r();
        }
    }
}

fn restore_terminal() {
    use ratatui::crossterm::{cursor, execute, terminal};
    let _ = terminal::disable_raw_mode();
    let _ = execute!(
        std::io::stdout(),
        terminal::LeaveAlternateScreen,
        cursor::Show
    );
}

/// Draw, read keys and refresh until `q`.
pub fn event_loop<B: Backend>(
    terminal: &mut Terminal<B>,
    app: &mut App,
    actor: &mut dyn Actor,
) -> Result<()>
where
    B::Error: Send + Sync + 'static,
{
    let mut cursor = actor.cursor();
    let mut last_read = Instant::now();
    while !app.quit {
        terminal.draw(|f| render(f, app))?;
        // Any event (a resize too) loops straight back to draw, so the layout follows.
        if event::poll(Duration::from_secs(1))?
            && let Event::Key(k) = event::read()?
        {
            app.message = None;
            app.on_key(k, actor);
        }
        let now = actor.cursor();
        if now != cursor || last_read.elapsed() >= Duration::from_secs(5) {
            cursor = now;
            last_read = Instant::now();
            if let Some(g) = actor.glance() {
                app.set_glance(g);
            }
        }
    }
    Ok(())
}

/// `bridge-screen watch`. Not a terminal: print `status` once.
pub fn run() -> Result<()> {
    if !std::io::stdout().is_terminal() {
        return crate::commands::print_status();
    }
    let home = Home::resolve()?;
    home.ensure()?;
    let cfg = Config::load(&home.config_path())?;
    let mut actor = StoreActor { home, cfg };
    let glance = actor
        .glance()
        .ok_or_else(|| anyhow::anyhow!("can't read the bridge-screen database"))?;
    let mut app = App::new(glance, crate::status::stdout_color());
    {
        use ratatui::crossterm::{cursor, execute, terminal};
        terminal::enable_raw_mode()?;
        execute!(
            std::io::stdout(),
            terminal::EnterAlternateScreen,
            cursor::Hide
        )?;
    }
    let _guard = Guard::new(restore_terminal);
    let previous = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        restore_terminal();
        previous(info);
    }));
    let mut terminal = Terminal::new(ratatui::backend::CrosstermBackend::new(std::io::stdout()))?;
    event_loop(&mut terminal, &mut app, &mut actor)
}

#[cfg(test)]
mod tests;
