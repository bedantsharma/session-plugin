//! `bridge-screen hook <event>`: Claude Code runs this on every hook event (spec §11.1).
//!
//! It reads the hook JSON from stdin, records events, and may print one JSON object with
//! context for the agent. It must never break a Claude session: every error and panic is
//! written to `<home>/hook-errors.log` and the process still exits 0 with empty stdout.

pub mod context;
pub mod handlers;
pub mod handoff;
pub mod nav;
pub mod question;

use std::io::{Read, Write};
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::path::PathBuf;

use anyhow::{Context, Result};
use bridge_screen_core::config::Config;
use bridge_screen_core::paths::Home;
use bridge_screen_core::store::Store;
use serde::{Deserialize, Deserializer, Serialize};
use serde_json::Value;

/// The hook events bridge-screen handles. Kebab-case on the command line.
#[derive(Debug, Clone, Copy, PartialEq, Eq, clap::ValueEnum)]
pub enum HookEvent {
    SessionStart,
    UserPromptSubmit,
    PreToolUse,
    PostToolUse,
    Stop,
    SubagentStop,
    PreCompact,
    Notification,
    SessionEnd,
}

impl HookEvent {
    /// The name Claude Code uses (`hookEventName`).
    pub fn pascal(self) -> &'static str {
        match self {
            Self::SessionStart => "SessionStart",
            Self::UserPromptSubmit => "UserPromptSubmit",
            Self::PreToolUse => "PreToolUse",
            Self::PostToolUse => "PostToolUse",
            Self::Stop => "Stop",
            Self::SubagentStop => "SubagentStop",
            Self::PreCompact => "PreCompact",
            Self::Notification => "Notification",
            Self::SessionEnd => "SessionEnd",
        }
    }

    /// The name used on the command line and in the error log.
    pub fn kebab(self) -> &'static str {
        match self {
            Self::SessionStart => "session-start",
            Self::UserPromptSubmit => "user-prompt-submit",
            Self::PreToolUse => "pre-tool-use",
            Self::PostToolUse => "post-tool-use",
            Self::Stop => "stop",
            Self::SubagentStop => "subagent-stop",
            Self::PreCompact => "pre-compact",
            Self::Notification => "notification",
            Self::SessionEnd => "session-end",
        }
    }
}

/// The hook payload. The event itself comes from the command line, so `hook_event_name`
/// isn't read. Unknown fields are ignored; optional fields that are missing or of an
/// unexpected type are read as `None` rather than failing the whole payload.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct HookInput {
    pub session_id: String,
    pub cwd: PathBuf,
    #[serde(default, deserialize_with = "lenient_string")]
    pub transcript_path: Option<String>,
    #[serde(default, deserialize_with = "lenient_string")]
    pub prompt_id: Option<String>,
    #[serde(default, deserialize_with = "lenient_string")]
    pub agent_id: Option<String>,
    #[serde(default, deserialize_with = "lenient_string")]
    pub agent_type: Option<String>,
    /// SubagentStop: the subagent's own transcript.
    #[serde(default, deserialize_with = "lenient_string")]
    pub agent_transcript_path: Option<String>,
    /// SessionStart: startup, resume, clear, compact or fork.
    #[serde(default, deserialize_with = "lenient_string")]
    pub source: Option<String>,
    #[serde(default, deserialize_with = "lenient_string")]
    pub model: Option<String>,
    /// UserPromptSubmit only. Used in memory for claim candidates, never stored.
    #[serde(default, deserialize_with = "lenient_string")]
    pub prompt: Option<String>,
    #[serde(default, deserialize_with = "lenient_string")]
    pub tool_name: Option<String>,
    #[serde(default)]
    pub tool_input: Option<Value>,
    /// Keys the MCP call binding (see `bridge_screen_core::binding`).
    #[serde(default, deserialize_with = "lenient_string")]
    pub tool_use_id: Option<String>,
    /// Stop and SubagentStop.
    #[serde(default, deserialize_with = "lenient_string")]
    pub last_assistant_message: Option<String>,
    /// Stop: Claude Code is already continuing because of a stop hook.
    #[serde(default, deserialize_with = "lenient_bool")]
    pub stop_hook_active: bool,
    /// Notification: permission_prompt, idle_prompt, …
    #[serde(default, deserialize_with = "lenient_string")]
    pub notification_type: Option<String>,
    /// SessionEnd: clear, resume, logout, prompt_input_exit or other.
    #[serde(default, deserialize_with = "lenient_string")]
    pub reason: Option<String>,
}

/// Accept any JSON value; keep it only if it is a string.
fn lenient_string<'de, D: Deserializer<'de>>(d: D) -> Result<Option<String>, D::Error> {
    Ok(match Value::deserialize(d)? {
        Value::String(s) => Some(s),
        _ => None,
    })
}

/// Accept any JSON value; `true` only for JSON `true`.
fn lenient_bool<'de, D: Deserializer<'de>>(d: D) -> Result<bool, D::Error> {
    Ok(matches!(Value::deserialize(d)?, Value::Bool(true)))
}

/// What a hook prints: context for the agent, or nothing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HookOutput {
    pub event: HookEvent,
    pub additional_context: Option<String>,
}

impl HookOutput {
    /// Claude Code's hook output format, or `None` when there is nothing to say.
    pub fn to_json(&self) -> Option<String> {
        #[derive(Serialize)]
        #[serde(rename_all = "camelCase")]
        struct Specific<'a> {
            hook_event_name: &'a str,
            additional_context: &'a str,
        }
        #[derive(Serialize)]
        #[serde(rename_all = "camelCase")]
        struct Out<'a> {
            hook_specific_output: Specific<'a>,
        }
        let context = self.additional_context.as_deref()?;
        let out = Out {
            hook_specific_output: Specific {
                hook_event_name: self.event.pascal(),
                additional_context: context,
            },
        };
        serde_json::to_string(&out).ok()
    }
}

/// Handle one hook call. Pure entry point used by `main` and by tests.
pub fn run(event: HookEvent, stdin: &str, home: &Home) -> Result<Option<HookOutput>> {
    let input: HookInput = serde_json::from_str(stdin).context("hook input is not valid JSON")?;
    home.ensure()?;
    let mut store = Store::open(&home.db_path())?;
    // A broken config file must not break the session: fall back to defaults.
    let config = Config::load(&home.config_path()).unwrap_or_default();
    handlers::handle(event, &input, &mut store, &config)
}

/// The `hook` command as the binary runs it: always exits 0.
pub fn main_entry(event: HookEvent) {
    // Panics are reported through the log below, not on stderr.
    std::panic::set_hook(Box::new(|_| {}));
    let home = Home::resolve().ok();
    let outcome = catch_unwind(AssertUnwindSafe(|| -> Result<Option<HookOutput>> {
        let mut buf = Vec::new();
        std::io::stdin()
            .read_to_end(&mut buf)
            .context("reading stdin")?;
        let text = String::from_utf8(buf).context("hook input is not UTF-8")?;
        let home = home
            .as_ref()
            .context("could not find a home directory; set BRIDGE_SCREEN_HOME")?;
        run(event, &text, home)
    }));
    let message = match outcome {
        Ok(Ok(Some(out))) => {
            if let Some(json) = out.to_json() {
                let mut stdout = std::io::stdout().lock();
                let _ = writeln!(stdout, "{json}");
            }
            return;
        }
        Ok(Ok(None)) => return,
        Ok(Err(err)) => format!("{err:#}"),
        Err(panic) => {
            let text = panic
                .downcast_ref::<&str>()
                .map(ToString::to_string)
                .or_else(|| panic.downcast_ref::<String>().cloned())
                .unwrap_or_else(|| "unknown panic".to_string());
            format!("panic: {text}")
        }
    };
    if let Some(home) = home {
        log_error(&home, event.kebab(), &message);
    }
}

/// `bridge-screen hook …` with an event this binary doesn't know (e.g. a newer plugin's
/// hooks.json) or a malformed command line: log it and exit 0 like any other hook error.
/// Clap's own exit code 2 would block the tool call or prompt.
pub fn bad_command_line(message: &str) {
    if let Ok(home) = Home::resolve() {
        let message: Vec<&str> = message
            .lines()
            .map(str::trim)
            .filter(|l| !l.is_empty())
            .collect();
        log_error(&home, "hook", &message.join(" "));
    }
}

/// Append one line to `<home>/hook-errors.log`, creating the folder if needed. Failing to
/// log is ignored.
pub(crate) fn log_error(home: &Home, label: &str, message: &str) {
    let line = format!(
        "{} {} {}\n",
        jiff::Timestamp::now(),
        label,
        message.replace('\n', " ")
    );
    let _ = std::fs::create_dir_all(home.root());
    let _ = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(home.root().join("hook-errors.log"))
        .and_then(|mut f| f.write_all(line.as_bytes()));
}

#[cfg(test)]
mod tests {
    use super::{HookEvent, HookInput, HookOutput};

    #[test]
    fn output_shape() {
        let out = HookOutput {
            event: HookEvent::UserPromptSubmit,
            additional_context: Some("x".into()),
        };
        assert_eq!(
            out.to_json().unwrap(),
            r#"{"hookSpecificOutput":{"hookEventName":"UserPromptSubmit","additionalContext":"x"}}"#
        );
        let none = HookOutput {
            event: HookEvent::Stop,
            additional_context: None,
        };
        assert_eq!(none.to_json(), None);
    }

    #[test]
    fn input_is_lenient() {
        let input: HookInput = serde_json::from_str(
            r#"{"session_id":"s","cwd":"/x","model":{"id":"m"},"brand_new_field":1,
                "notification_type":"idle_prompt"}"#,
        )
        .unwrap();
        assert_eq!(input.session_id, "s");
        assert_eq!(input.model, None);
        assert_eq!(input.notification_type.as_deref(), Some("idle_prompt"));
        assert!(serde_json::from_str::<HookInput>(r#"{"cwd":"/x"}"#).is_err());
    }
}
