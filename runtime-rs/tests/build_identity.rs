use tradeassembly_runtime::{
    adapters::legal_receipts::FileLegalReceiptVerifier,
    build_identity,
    ports::{LegalReceiptExpectation, LegalReceiptFailure, LegalReceiptPort},
};

#[test]
fn receipt_verification_requires_compiled_startup_identity() {
    assert!(build_identity::current().is_none());
    let root = tempfile::tempdir().unwrap();
    let verifier = FileLegalReceiptVerifier::new(root.path(), root.path(), root.path());
    let expected = LegalReceiptExpectation {
        receipt_ref: "receipt".into(),
        identity_issuer: "issuer".into(),
        identity_subject: "subject".into(),
        resource_ref: "resource".into(),
        resource_version_refs: vec![],
        environment: "test".into(),
    };
    assert_eq!(
        verifier.verify(&expected, 0),
        Err(LegalReceiptFailure::Unavailable)
    );
    build_identity::install("1111111111111111111111111111111111111111").unwrap();
    assert_eq!(
        verifier.verify(&expected, 0),
        Err(LegalReceiptFailure::Missing)
    );
    assert!(build_identity::install("2222222222222222222222222222222222222222").is_err());
}
