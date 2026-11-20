//! Persistent mode: a local stack kept for real use, whose data has to survive
//! restarts, reboots and the routine commands that otherwise start clean.
//!
//! Off unless switched on. `MACRO_LOCAL_PERSISTENT` names the instances it
//! applies to, comma-separated (`macro` is the default instance). An instance
//! also stays persistent while its LocalStack data volume exists, so a shell
//! that lacks the variable still can't wipe it.
//!
//! For a persistent instance:
//! - an attached `run_local` resumes over the existing volumes, and nothing
//!   deletes them without `--wipe-persistent-data` ([`ensure_may_wipe`]);
//! - LocalStack keeps S3, DynamoDB, SQS and KMS on a volume, Postgres runs
//!   without JIT, and every container restarts with Docker ([`apply_to_compose`]);
//! - tests can't use its Postgres or Temporal ([`refuse_test_databases`], and
//!   Temporal's gRPC port stays off the host).
//!
//! Everything else behaves as upstream, so this module holds the whole mode and
//! the orchestrator only calls into it.

use std::path::Path;

use anyhow::{Context, Result, bail};
use serde_yaml::{Mapping, Value};

use super::instance::Instance;
use super::stage::Stage;
use super::{Mode, Start, repo_root};

#[cfg(test)]
mod test;

/// Names the instances that run in persistent mode, comma-separated.
pub const SWITCH_VAR: &str = "MACRO_LOCAL_PERSISTENT";

/// LocalStack 4.14.0, the version `gen_compose::LOCALSTACK_IMAGE` resolves to,
/// with persistence hooks that write every service's state to
/// `/persisted-data`. Pinned by digest: the image is a third-party build.
pub const LOCALSTACK_IMAGE: &str = "gresau/localstack-persist:4.14.0@sha256:02045e045f1f3868c3cadef78de2a7c048840f48ffd506babf4a36f42a4845ba";

/// How often LocalStack writes its state, in seconds: a crash loses at most
/// this much.
const PERSIST_FREQUENCY: &str = "2";

/// The compose key of LocalStack's data volume.
const LOCALSTACK_VOLUME_KEY: &str = "localstack_data";

/// Raised by the bench's Postgres when a `#[sqlx::test]` registers a test
/// database. `sqlx` records each test database in `_sqlx_test.databases`
/// before it creates it, so refusing that insert stops the test before
/// anything is created. Creating the table here first is harmless: `sqlx`
/// creates it with `if not exists` and the same columns.
const REFUSE_TEST_DATABASES_SQL: &str = r#"
create schema if not exists _sqlx_test;
create table if not exists _sqlx_test.databases (
    db_name text primary key,
    test_path text not null,
    created_at timestamptz not null default now()
);
create or replace function _sqlx_test.refuse_persistent_stack() returns trigger
language plpgsql as $$
begin
    raise exception 'this Postgres holds a persistent local stack''s data: point DATABASE_URL at a throwaway Postgres to run tests';
end
$$;
drop trigger if exists refuse_persistent_stack on _sqlx_test.databases;
create trigger refuse_persistent_stack before insert on _sqlx_test.databases
    for each row execute function _sqlx_test.refuse_persistent_stack();
"#;

/// The volume LocalStack keeps its state on.
pub fn localstack_volume(instance: &Instance) -> String {
    format!("{}_localstack_data", instance.project_name())
}

/// Whether to render `instance`'s files for persistent mode: the switch names
/// it, or its LocalStack data volume exists. Only for rendering: a failed
/// volume lookup, or a dry run, counts as off, so commands that only write
/// files, such as `gen-compose`, need no Docker daemon. The guards use
/// [`probe`], which refuses instead.
pub fn is_on(instance: &Instance) -> bool {
    #[cfg(test)]
    if let Some(probe) = FAKE_PROBE.get() {
        return probe.persistent;
    }
    if switch_names(instance) {
        return true;
    }
    let dry_run = macro_env_var::maybe_read_env("MACRO_LOCAL_DRY_RUN").as_deref() == Some("1");
    !dry_run && volume_exists(&localstack_volume(instance)).unwrap_or(false)
}

fn switch_names(instance: &Instance) -> bool {
    names(
        macro_env_var::maybe_read_env(SWITCH_VAR).as_deref(),
        instance.name(),
    )
}

/// Whether the Docker volume exists, asked through the `docker` CLI: the same
/// daemon and context the teardown's `docker` commands use. Anything but a
/// clear "no such volume" is an error.
fn volume_exists(name: &str) -> Result<bool> {
    volume_exists_via("docker", name)
}

fn volume_exists_via(docker: &str, name: &str) -> Result<bool> {
    let inspected = std::process::Command::new(docker)
        .args(["volume", "inspect", name])
        .output()
        .with_context(|| format!("running `{docker} volume inspect {name}`"))?;
    if inspected.status.success() {
        return Ok(true);
    }
    let stderr = String::from_utf8_lossy(&inspected.stderr);
    if stderr.to_ascii_lowercase().contains("no such volume") {
        return Ok(false);
    }
    bail!("`{docker} volume inspect {name}` failed: {}", stderr.trim())
}

/// Whether the switch's value names `instance`.
fn names(switch: Option<&str>, instance: &str) -> bool {
    switch.is_some_and(|value| value.split(',').any(|name| name.trim() == instance))
}

/// What the guards need to know about an instance.
#[derive(Clone, Copy, Debug)]
struct Probe {
    persistent: bool,
    /// The instance has a data volume. Only looked up for a persistent one.
    has_data: bool,
}

#[cfg(test)]
thread_local! {
    /// Set by [`fake_probe`]: what the guards see instead of Docker.
    static FAKE_PROBE: std::cell::Cell<Option<Probe>> = const { std::cell::Cell::new(None) };
}

/// Make this thread's guards see `persistent` and `has_data` instead of
/// asking Docker, until the returned value is dropped.
#[cfg(test)]
pub fn fake_probe(persistent: bool, has_data: bool) -> impl Drop {
    struct Reset;
    impl Drop for Reset {
        fn drop(&mut self) {
            FAKE_PROBE.set(None);
        }
    }
    FAKE_PROBE.set(Some(Probe {
        persistent,
        has_data,
    }));
    Reset
}

/// What the guards need to know. Fails closed: when Docker can't say whether
/// the instance has persistent data, the guards refuse rather than assume not.
fn probe(instance: &Instance) -> Result<Probe> {
    #[cfg(test)]
    if let Some(probe) = FAKE_PROBE.get() {
        return Ok(probe);
    }
    let unknown = |error: anyhow::Error| {
        error.context(format!(
            "can't tell whether instance {} holds persistent data, so nothing is wiped",
            instance.name()
        ))
    };
    let persistent =
        switch_names(instance) || volume_exists(&localstack_volume(instance)).map_err(unknown)?;
    let mut has_data = false;
    if persistent {
        for volume in super::instance_volumes(instance) {
            if volume_exists(&volume).map_err(unknown)? {
                has_data = true;
                break;
            }
        }
    }
    Ok(Probe {
        persistent,
        has_data,
    })
}

/// Proof that a command may delete an instance's data. Every function that
/// deletes volumes or the database takes one, and only [`ensure_may_wipe`]
/// makes one, so no command can wipe without asking the guard first.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct WipePermit {
    wipes_persistent_data: bool,
}

impl WipePermit {
    /// Whether the wipe deletes a persistent instance's data, which also
    /// switches persistent mode off unless `MACRO_LOCAL_PERSISTENT` names it.
    pub fn wipes_persistent_data(self) -> bool {
        self.wipes_persistent_data
    }

    /// A permit for tests that build bring-up plans without wiping anything.
    #[cfg(test)]
    pub fn for_test() -> Self {
        WipePermit {
            wipes_persistent_data: false,
        }
    }
}

/// Refuse a command that would delete the data of a persistent instance,
/// unless `wipe` (`--wipe-persistent-data`) says to. An instance with no
/// volumes has nothing to lose, so its first start may run clean. A dry run
/// deletes nothing and asks Docker nothing.
pub fn ensure_may_wipe(stage: &Stage, instance: &Instance, wipe: bool) -> Result<WipePermit> {
    if stage.is_dry_run() {
        return Ok(WipePermit {
            wipes_persistent_data: false,
        });
    }
    let probe = probe(instance)?;
    if !may_wipe(probe.persistent, probe.has_data, wipe) {
        bail!(refusal(instance));
    }
    Ok(WipePermit {
        wipes_persistent_data: probe.persistent && probe.has_data,
    })
}

/// Whether a command may delete an instance's data: always upstream, and for
/// a persistent instance only with nothing to lose or with `wipe`.
fn may_wipe(persistent: bool, has_data: bool, wipe: bool) -> bool {
    wipe || !persistent || !has_data
}

/// Why a persistent instance's data was kept, and how to delete it on purpose.
pub fn refusal(instance: &Instance) -> String {
    format!(
        "instance {name} is a persistent stack ({SWITCH_VAR} names it, or its volume {volume} \
         exists), so its data is kept. `just run_local --instance {name}` resumes it. To delete \
         its data on purpose, back it up and pass --wipe-persistent-data",
        name = instance.name(),
        volume = localstack_volume(instance),
    )
}

/// How an attached `run_local` starts. Upstream it always starts clean; a
/// persistent instance with data resumes over its volumes, unless `--fresh`
/// asks for a clean start, which the guard then still has to allow.
pub fn attached_start(stage: &Stage, instance: &Instance, fresh: bool, wipe: bool) -> Result<Start> {
    if stage.is_dry_run() {
        return Ok(Start::Fresh(ensure_may_wipe(stage, instance, wipe)?));
    }
    let probe = probe(instance)?;
    if probe.persistent && probe.has_data && !fresh {
        return Ok(Start::Resume);
    }
    Ok(Start::Fresh(ensure_may_wipe(stage, instance, wipe)?))
}

/// Layer persistent mode onto the generated compose override.
pub fn apply_to_compose(value: &mut Value, instance: &Instance) -> Result<()> {
    let base = BaseCompose::read(&repo_root())?;
    apply(value, instance, &base)
}

/// What persistent mode needs from the checked-in compose files, which the
/// generated override can only extend.
struct BaseCompose {
    /// Every service they define.
    services: Vec<String>,
    /// Postgres's server command, which an override replaces rather than
    /// extends.
    postgres_command: Value,
}

impl BaseCompose {
    /// Read `docker/docker-compose.yml` and the files it includes. Include
    /// paths are relative to the project directory, the repository root.
    fn read(repo: &Path) -> Result<Self> {
        let main = read_yaml(&repo.join("docker/docker-compose.yml"))?;
        let mut files = vec![main.clone()];
        for include in main["include"].as_sequence().into_iter().flatten() {
            let path = include
                .as_str()
                .or_else(|| include["path"].as_str())
                .context("docker/docker-compose.yml has an include without a path")?;
            files.push(read_yaml(&repo.join(path))?);
        }

        let mut services = Vec::new();
        let mut postgres_command = None;
        for file in &files {
            for (name, service) in file["services"].as_mapping().into_iter().flatten() {
                let name = name.as_str().context("a compose service name is not a string")?;
                if name == "postgres" {
                    postgres_command = Some(service["command"].clone());
                }
                services.push(name.to_string());
            }
        }
        Ok(Self {
            services,
            postgres_command: postgres_command
                .context("no postgres service in the base compose files")?,
        })
    }
}

fn read_yaml(path: &Path) -> Result<Value> {
    let raw = std::fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
    serde_yaml::from_str(&raw).with_context(|| format!("parsing {}", path.display()))
}

/// The override changes, given the base compose files:
/// - LocalStack runs the persisting image on its own volume. The volume is
///   external, like Postgres's, so a hand-typed `docker compose down -v` keeps
///   it; the orchestrator creates it (see `ensure_external_resources`);
/// - Postgres runs with `jit=off`. Compiling the plan of a large query costs
///   seconds on every call, and the stack's tables are far too small to gain
///   from it;
/// - Temporal keeps its gRPC port off the host, so tests that default to
///   `127.0.0.1:7233` can't start workflows on it. Its UI stays published;
/// - every service restarts with Docker. `unless-stopped` honours an explicit
///   stop, so `run_local stop` still stops the stack.
fn apply(value: &mut Value, instance: &Instance, base: &BaseCompose) -> Result<()> {
    let root = value
        .as_mapping_mut()
        .context("the compose override is not a mapping")?;
    let mut volume = Mapping::new();
    volume.insert("external".into(), true.into());
    volume.insert("name".into(), localstack_volume(instance).into());
    child_mapping(root, "volumes")?.insert(LOCALSTACK_VOLUME_KEY.into(), Value::Mapping(volume));

    let services = child_mapping(root, "services")?;
    if let Some(localstack) = services.get_mut("localstack") {
        let localstack = localstack
            .as_mapping_mut()
            .context("the localstack service is not a mapping")?;
        localstack.insert("image".into(), LOCALSTACK_IMAGE.into());
        child_mapping(localstack, "environment")?
            .insert("PERSIST_FREQUENCY".into(), PERSIST_FREQUENCY.into());
        child_sequence(localstack, "volumes")?
            .push(format!("{LOCALSTACK_VOLUME_KEY}:/persisted-data").into());
    }

    let postgres = child_mapping(services, "postgres")?;
    postgres.insert("command".into(), without_jit(&base.postgres_command));

    if let Some(temporal) = services.get_mut("temporal") {
        let temporal = temporal
            .as_mapping_mut()
            .context("the temporal service is not a mapping")?;
        child_sequence(temporal, "ports")?
            .retain(|port| !port.as_str().is_some_and(|port| port.ends_with(":7233")));
    }

    let mut names: Vec<Value> = services.keys().cloned().collect();
    names.extend(base.services.iter().map(|name| Value::from(name.as_str())));
    for name in names {
        let service = services
            .entry(name.clone())
            .or_insert_with(|| Value::Mapping(Mapping::new()))
            .as_mapping_mut()
            .with_context(|| format!("compose service {name:?} is not a mapping"))?;
        service.insert("restart".into(), "unless-stopped".into());
    }
    Ok(())
}

/// The Postgres command with `-c jit=off` appended, in its own form.
fn without_jit(command: &Value) -> Value {
    match command {
        Value::Sequence(args) => {
            let mut args = args.clone();
            args.extend(["-c".into(), "jit=off".into()]);
            Value::Sequence(args)
        }
        Value::String(command) => format!("{command} -c jit=off").into(),
        _ => "postgres -c jit=off".into(),
    }
}

/// The mapping under `key`, created when absent. Any other shape is an error:
/// replacing it would silently drop what upstream put there.
fn child_mapping<'a>(parent: &'a mut Mapping, key: &str) -> Result<&'a mut Mapping> {
    parent
        .entry(key.into())
        .or_insert_with(|| Value::Mapping(Mapping::new()))
        .as_mapping_mut()
        .with_context(|| format!("compose key {key:?} is not a mapping"))
}

/// The sequence under `key`, created when absent. Any other shape is an error.
fn child_sequence<'a>(parent: &'a mut Mapping, key: &str) -> Result<&'a mut Vec<Value>> {
    parent
        .entry(key.into())
        .or_insert_with(|| Value::Sequence(Vec::new()))
        .as_sequence_mut()
        .with_context(|| format!("compose key {key:?} is not a sequence"))
}

/// The databases on the stack's Postgres that tests name in `DATABASE_URL`.
const TEST_MASTER_DATABASES: &[&str] = &["macrodb", "contacts", "macrodb_pwconc"];

/// Make the persistent instance's Postgres refuse `#[sqlx::test]` databases,
/// which tests on this machine would otherwise create on it through the
/// default `localhost:5432` URL. `sqlx` registers test databases in the
/// database the URL names, so the refusal goes into each of
/// [`TEST_MASTER_DATABASES`] that exists. Runs after any init snapshot is saved,
/// so the machine-wide snapshot never carries the refusal to other instances.
pub fn refuse_test_databases(
    stage: &Stage,
    mode: Mode,
    instance: &Instance,
    env: &super::env_layer::ResolvedEnv,
) -> Result<()> {
    if stage.is_dry_run() || !mode.spec().migrates_db || !is_on(instance) {
        return Ok(());
    }
    let psql = |database: &str| {
        let mut psql = super::compose_cmd(instance, env);
        psql.args([
            "exec", "-T", "postgres", "psql", "-U", "user", "-d", database, "-v",
            "ON_ERROR_STOP=1",
        ]);
        psql
    };

    let listed = psql("macrodb")
        .args(["-At", "-c", "select datname from pg_database"])
        .output()
        .context("listing the persistent Postgres's databases")?;
    anyhow::ensure!(
        listed.status.success(),
        "listing the persistent Postgres's databases failed: {}",
        String::from_utf8_lossy(&listed.stderr)
    );
    let existing = String::from_utf8_lossy(&listed.stdout).into_owned();
    for database in TEST_MASTER_DATABASES {
        if !existing.lines().any(|name| name.trim() == *database) {
            continue;
        }
        stage.run(
            &format!("Refusing test databases in {database}"),
            psql(database).args(["-c", REFUSE_TEST_DATABASES_SQL]),
        )?;
    }
    Ok(())
}
