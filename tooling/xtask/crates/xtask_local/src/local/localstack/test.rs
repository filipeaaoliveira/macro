use super::*;

#[test]
fn an_existing_kms_alias_keeps_its_key() {
    let all: Vec<String> = KMS_KEYS
        .iter()
        .map(|(alias, _)| alias.to_string())
        .collect();
    assert!(missing_kms_keys(&all).is_empty());

    // LocalStack's own aliases and other keys don't count as ours.
    let mut some = vec![
        "alias/aws/s3".to_string(),
        resources::CURSOR_API_KEY_KMS_ALIAS.to_string(),
        resources::WHATSAPP_SESSION_KMS_ALIAS.to_string(),
    ];
    some.push(format!("{}-old", resources::MCP_OAUTH_KMS_ALIAS));
    let missing: Vec<&str> = missing_kms_keys(&some)
        .into_iter()
        .map(|(alias, _)| alias)
        .collect();
    assert_eq!(
        missing,
        [
            resources::CODEX_OAUTH_KMS_ALIAS,
            resources::MCP_OAUTH_KMS_ALIAS,
            resources::MICROSOFT_TOKEN_KMS_ALIAS,
        ]
    );

    assert_eq!(missing_kms_keys(&[]).len(), KMS_KEYS.len());
}
