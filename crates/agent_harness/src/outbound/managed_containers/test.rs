use super::*;

fn session(n: u128) -> AgentSessionId {
    AgentSessionId::new_from_uuid(macro_uuid::Uuid::from_u128(n))
}

fn never_skip<Id>(_: &Id) -> bool {
    false
}

fn activate(managed: &ManagedContainers<&str>, id: &'static str, last_activity: Instant) {
    managed.register(id, session(1));
    assert!(managed.activate(&id, last_activity));
}

fn seconds_ago(now: Instant, seconds: u64) -> Instant {
    now.checked_sub(Duration::from_secs(seconds))
        .expect("the test instant should support subtraction")
}

#[test]
fn reap_stale_returns_only_containers_idle_for_the_limit() {
    let managed = ManagedContainers::new();
    let now = Instant::now();
    activate(&managed, "stale", seconds_ago(now, 300));
    activate(&managed, "active", seconds_ago(now, 299));

    assert_eq!(
        managed.reap_stale(now, Duration::from_secs(300), never_skip),
        ["stale"]
    );
}

#[test]
fn recording_activity_keeps_a_container_alive() {
    let managed = ManagedContainers::new();
    let now = Instant::now();
    activate(&managed, "active", seconds_ago(now, 300));

    managed.record_activity(&"active", now);

    assert!(
        managed
            .reap_stale(now, Duration::from_secs(300), never_skip)
            .is_empty()
    );
}

#[test]
fn pending_and_stopping_containers_are_not_reaped_again() {
    let managed = ManagedContainers::new();
    let now = Instant::now();
    managed.register("pending", session(1));
    activate(&managed, "stopping", seconds_ago(now, 300));

    assert_eq!(
        managed.reap_stale(now, Duration::from_secs(300), never_skip),
        ["stopping"]
    );
    assert!(
        managed
            .reap_stale(now, Duration::ZERO, never_skip)
            .is_empty()
    );
}

#[test]
fn activity_during_a_failed_stop_is_preserved() {
    let managed = ManagedContainers::new();
    let now = Instant::now();
    activate(&managed, "active", seconds_ago(now, 300));
    assert_eq!(
        managed.reap_stale(now, Duration::from_secs(300), never_skip),
        ["active"]
    );

    managed.record_activity(&"active", now);
    managed.finish_stop(&"active", false);

    assert!(
        managed
            .reap_stale(now, Duration::from_secs(300), never_skip)
            .is_empty()
    );
}

#[test]
fn a_session_with_a_pending_command_is_not_reaped() {
    let managed = ManagedContainers::new();
    let now = Instant::now();
    let pending_session = session(2);
    managed.register("busy", pending_session);
    assert!(managed.activate(&"busy", seconds_ago(now, 300)));

    assert!(
        managed
            .reap_stale(now, Duration::from_secs(300), |id| managed
                .session_of(id)
                .is_some_and(|s| s == pending_session))
            .is_empty(),
        "a container whose session has a pending command must not be reaped"
    );
}

#[test]
fn a_container_running_a_command_is_not_reaped_until_it_idles_after_it() {
    let managed = ManagedContainers::new();
    let now = Instant::now();
    activate(&managed, "checking", seconds_ago(now, 900));

    managed.begin_command(&"checking", session(1), seconds_ago(now, 600));
    assert!(
        managed
            .reap_stale(now, Duration::from_secs(300), never_skip)
            .is_empty()
    );

    managed.end_command(&"checking", seconds_ago(now, 299));
    assert!(
        managed
            .reap_stale(now, Duration::from_secs(300), never_skip)
            .is_empty()
    );
    assert_eq!(
        managed.reap_stale(
            now + Duration::from_secs(1),
            Duration::from_secs(300),
            never_skip
        ),
        ["checking"]
    );
}

/// A container no session of this replica's is in is tracked only while
/// its commands run: another replica may hold it, so this one never reaps
/// it.
#[test]
fn a_container_tracked_only_for_its_commands_is_dropped_after_them() {
    let managed = ManagedContainers::new();
    let now = Instant::now();

    managed.begin_command(&"elsewhere", session(2), seconds_ago(now, 600));
    managed.begin_command(&"elsewhere", session(2), seconds_ago(now, 600));
    managed.end_command(&"elsewhere", seconds_ago(now, 500));
    assert_eq!(managed.session_of(&"elsewhere"), Some(session(2)));
    managed.end_command(&"elsewhere", seconds_ago(now, 400));

    assert_eq!(managed.session_of(&"elsewhere"), None);
    assert!(
        managed
            .reap_stale(now, Duration::from_secs(300), never_skip)
            .is_empty()
    );
}

/// A container this replica attached while a command ran in it stays
/// tracked after the command.
#[test]
fn a_container_attached_during_a_command_stays_tracked() {
    let managed = ManagedContainers::new();
    let now = Instant::now();

    managed.begin_command(&"attached", session(3), seconds_ago(now, 600));
    managed.register("attached", session(3));
    assert!(managed.activate(&"attached", seconds_ago(now, 600)));
    managed.end_command(&"attached", seconds_ago(now, 300));

    assert_eq!(
        managed.reap_stale(now, Duration::from_secs(300), never_skip),
        ["attached"]
    );
}
