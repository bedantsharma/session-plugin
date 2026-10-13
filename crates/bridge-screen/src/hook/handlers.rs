//! One handler per hook event. Filled in by later tasks; for now every event is accepted.

use anyhow::Result;
use bridge_screen_core::store::Store;

use super::{HookEvent, HookInput, HookOutput};

// The real handlers (next task) can fail; the stub keeps their signature.
#[allow(clippy::unnecessary_wraps)]
/// Record what `event` means for the session and return any context for the agent.
pub fn handle(
    _event: HookEvent,
    _input: &HookInput,
    _store: &mut Store,
) -> Result<Option<HookOutput>> {
    Ok(None)
}
