//! Persistent mode: a local stack kept for real use, whose data has to survive
//! restarts, reboots and the routine commands that otherwise start clean.
//!
//! Off unless switched on. `MACRO_LOCAL_PERSISTENT` names the instances it
//! applies to, comma-separated (`macro` is the default instance). An instance
//! also stays persistent while its LocalStack data volume exists, so a shell
//! that lacks the variable still can't wipe it.
//!
//! For a persistent instance, an attached `run_local` resumes over the
//! existing volumes, and nothing deletes them without `--wipe-persistent-data`
//! ([`ensure_may_wipe`]).
//!
//! Everything else behaves as upstream, so this module holds the whole mode and
//! the orchestrator only calls into it.

use anyhow::{Result, bail};

use super::Start;
use super::instance::Instance;

#[cfg(test)]
mod test;

/// Names the instances that run in persistent mode, comma-separated.
pub const SWITCH_VAR: &str = "MACRO_LOCAL_PERSISTENT";

/// The volume LocalStack keeps its state on.
pub fn localstack_volume(instance: &Instance) -> String {
    format!("{}_localstack_data", instance.project_name())
}

/// Whether `instance` runs in persistent mode: the switch names it, or its
/// LocalStack data volume exists.
pub fn is_on(instance: &Instance) -> Result<bool> {
    let switch = macro_env_var::maybe_read_env(SWITCH_VAR);
    if names(switch.as_deref(), instance.name()) {
        return Ok(true);
    }
    super::docker::volume_exists(&localstack_volume(instance))
}

/// Whether the switch's value names `instance`.
fn names(switch: Option<&str>, instance: &str) -> bool {
    switch.is_some_and(|value| value.split(',').any(|name| name.trim() == instance))
}

/// Refuse a command that would delete the data of a persistent instance,
/// unless `wipe` (`--wipe-persistent-data`) says to. An instance with no
/// volumes has nothing to lose, so its first start may run clean.
pub fn ensure_may_wipe(instance: &Instance, wipe: bool) -> Result<()> {
    if wipe {
        return Ok(());
    }
    let persistent = is_on(instance)?;
    let has_data = persistent && super::instance_has_volumes(instance)?;
    if may_wipe(persistent, has_data, wipe) {
        Ok(())
    } else {
        bail!(refusal(instance))
    }
}

/// Whether a command may delete an instance's data: always upstream, and for
/// a persistent instance only with nothing to lose or with `wipe`.
fn may_wipe(persistent: bool, has_data: bool, wipe: bool) -> bool {
    wipe || !persistent || !has_data
}

/// Why a persistent instance's data was kept, and how to delete it on purpose.
fn refusal(instance: &Instance) -> String {
    format!(
        "instance {name} is a persistent stack ({SWITCH_VAR} names it, or its volume {volume} \
         exists), so its data is kept. `just run_local --instance {name}` resumes it. To delete \
         its data on purpose, back it up and pass --wipe-persistent-data",
        name = instance.name(),
        volume = localstack_volume(instance),
    )
}

/// How an attached `run_local` starts. Upstream it always starts clean; a
/// persistent instance resumes over its volumes unless `--fresh` asks for a
/// clean start, which [`ensure_may_wipe`] then still guards.
pub fn attached_start(instance: &Instance, fresh: bool) -> Result<Start> {
    if fresh {
        return Ok(Start::Fresh);
    }
    let persistent = is_on(instance)?;
    let has_data = persistent && super::instance_has_volumes(instance)?;
    Ok(attached_start_for(persistent, has_data, fresh))
}

fn attached_start_for(persistent: bool, has_data: bool, fresh: bool) -> Start {
    if persistent && has_data && !fresh {
        Start::Resume
    } else {
        Start::Fresh
    }
}
