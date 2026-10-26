// Copyright (c) 2026 OptionLab LLC. All rights reserved.
//! Parser migration must preserve the JSON values used for signed manifest hashes.

#[test]
fn manifest_scalars_and_aliases_preserve_json_types() {
    let value: serde_json::Value = serde_yaml::from_str(
        "name: core\nversion: '1.0'\nactive: true\ncount: 12\nnone: null\na: &a [read, status]\nb: *a\n",
    ).unwrap();
    assert_eq!(
        value,
        serde_json::json!({"name":"core", "version":"1.0",
        "active":true,"count":12,"none":null,"a":["read","status"],"b":["read","status"]})
    );
}

#[test]
fn malformed_yaml_and_multiple_documents_are_not_manifests() {
    for input in ["key: [unterminated", "a: *missing", "a: 1\n---\nb: 2"] {
        assert!(
            serde_yaml::from_str::<serde_json::Value>(input).is_err(),
            "{input}"
        );
    }
}

#[test]
fn tooling_mapping_rejects_duplicate_keys() {
    assert!(serde_yaml::from_str::<serde_yaml::Value>("name: first\nname: second").is_err());
}
