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
fn the_guard_grants_a_permit_only_when_the_data_may_go() {
    let stage = Stage::from_env();
    let instance = Instance::derive(Some("persistent-guard-test"), None).unwrap();
    {
        let _probe = fake_probe(true, true);
        assert_eq!(
            ensure_may_wipe(&stage, &instance, false).unwrap_err().to_string(),
            refusal(&instance)
        );
        assert!(ensure_may_wipe(&stage, &instance, true).unwrap().wipes_persistent_data());
    }
    {
        let _probe = fake_probe(true, false);
        assert!(!ensure_may_wipe(&stage, &instance, false).unwrap().wipes_persistent_data());
    }
    {
        let _probe = fake_probe(false, true);
        assert!(!ensure_may_wipe(&stage, &instance, false).unwrap().wipes_persistent_data());
    }
}

#[test]
fn an_attached_start_resumes_a_persistent_instance_with_data() {
    let stage = Stage::from_env();
    let instance = Instance::derive(Some("persistent-guard-test"), None).unwrap();
    {
        let _probe = fake_probe(true, true);
        assert_eq!(attached_start(&stage, &instance, false, false).unwrap(), Start::Resume);
        assert_eq!(
            attached_start(&stage, &instance, true, false).unwrap_err().to_string(),
            refusal(&instance)
        );
        assert!(matches!(
            attached_start(&stage, &instance, true, true).unwrap(),
            Start::Fresh(permit) if permit.wipes_persistent_data()
        ));
    }
    {
        let _probe = fake_probe(true, false);
        assert!(matches!(
            attached_start(&stage, &instance, false, false).unwrap(),
            Start::Fresh(_)
        ));
    }
    {
        // Off, an attached start is always clean.
        let _probe = fake_probe(false, true);
        assert!(matches!(
            attached_start(&stage, &instance, false, false).unwrap(),
            Start::Fresh(_)
        ));
    }
}

#[test]
fn the_base_compose_files_are_read_with_their_includes() {
    let base = BaseCompose::read(&repo_root()).unwrap();
    for service in ["authentication-service", "postgres", "kafka", "fusionauth", "db"] {
        assert!(
            base.services.iter().any(|name| name == service),
            "{service} missing from {:?}",
            base.services
        );
    }
    let command = base.postgres_command.as_str().unwrap();
    assert!(command.starts_with("postgres "));
    assert!(!command.contains("jit"));
}

#[test]
fn jit_is_appended_to_either_command_form() {
    assert_eq!(
        without_jit(&Value::from("postgres -c max_connections=500")),
        Value::from("postgres -c max_connections=500 -c jit=off")
    );
    assert_eq!(
        without_jit(&serde_yaml::from_str("[postgres, -c, max_connections=500]").unwrap()),
        serde_yaml::from_str::<Value>("[postgres, -c, max_connections=500, -c, jit=off]").unwrap()
    );
}

#[test]
fn an_unexpected_compose_shape_is_an_error_not_replaced() {
    let mut service: Mapping =
        serde_yaml::from_str("environment: [SERVICES=s3]\nvolumes: {data: /data}").unwrap();
    assert!(child_mapping(&mut service, "environment").is_err());
    assert!(child_sequence(&mut service, "volumes").is_err());
    assert_eq!(
        service,
        serde_yaml::from_str::<Mapping>("environment: [SERVICES=s3]\nvolumes: {data: /data}")
            .unwrap()
    );
    child_mapping(&mut service, "labels").unwrap();
    child_sequence(&mut service, "ports").unwrap();
}

/// The guards ask Docker whether persistent data exists, and anything short of
/// a clear answer must stop them rather than read as "no".
#[test]
fn a_volume_lookup_that_fails_is_an_error_not_a_missing_volume() {
    assert!(volume_exists_via("docker-that-does-not-exist", "macro_localstack_data").is_err());
    // Exits nonzero without saying "no such volume".
    assert!(volume_exists_via("false", "macro_localstack_data").is_err());
}
