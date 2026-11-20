use super::*;

#[test]
fn the_switch_names_instances_exactly() {
    assert!(names(Some("macro"), "macro"));
    assert!(names(Some("bench, macro"), "macro"));
    assert!(!names(Some("macro"), "local-e2e"));
    assert!(!names(Some("macro-old"), "macro"));
    assert!(!names(Some(""), "macro"));
    assert!(!names(None, "macro"));
}

#[test]
fn a_persistent_instance_with_data_is_wiped_only_with_the_flag() {
    assert!(!may_wipe(true, true, false));
    assert!(may_wipe(true, true, true));
    // Nothing to lose yet: the first start of a persistent instance runs clean.
    assert!(may_wipe(true, false, false));
    // Off, every wipe behaves as upstream.
    assert!(may_wipe(false, true, false));
}

#[test]
fn the_refusal_names_the_instance_and_the_flag() {
    let instance = Instance::derive(Some("bench"), None).unwrap();
    let refusal = refusal(&instance);
    assert!(refusal.contains("instance bench"));
    assert!(refusal.contains(&localstack_volume(&instance)));
    assert!(refusal.contains("--wipe-persistent-data"));
}

#[test]
fn an_attached_start_resumes_a_persistent_instance_with_data() {
    assert_eq!(attached_start_for(true, true, false), Start::Resume);
    assert_eq!(attached_start_for(true, true, true), Start::Fresh);
    assert_eq!(attached_start_for(true, false, false), Start::Fresh);
    // Off, an attached start is always clean.
    assert_eq!(attached_start_for(false, true, false), Start::Fresh);
}
