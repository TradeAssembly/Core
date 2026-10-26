# Core OIDC consumer backend qualification

This candidate adopts the existing checksum-pinned OIDC 4.0.1 consumer patch. It preserves upstream OIDC protocol/claim validation and uses ring 0.17.14 for RSA public signature verification. No hosted Relay implementation is included.

Patch and released-archive hashes are in patches/crypto-consumers/provenance.json. Original license notices are retained. just setup materializes the exact patched dependency; just verify validates its bytes. No advisory ignore is added; the prior RSA exception is removed.

Compatibility limits requiring caller proof: upstream private RSA signing is test-only and removed from release exports; RSA verification accepts 2048–8192-bit moduli. Core public OIDC service types remain unchanged. Existing callback, PKCE, token/issuer/audience/nonce/time, access-token-hash, refresh, rotation and custody tests must pass on Core itself.

The compatible transport, YAML and native-keyring remediation is included from the existing Core candidate. No issuer, credential, provider scope, production activation, broker activity or deployment is changed. Source qualification is separate from release-artifact and real-issuer qualification.

## Core caller qualification

Focused identity tests pass: 9 units, 13 integration cases. New coverage exercises a real 8192-bit signed issuer token through Core's OIDC service and denies out-of-range public JWK moduli. Existing 2048-bit flow, refresh, rotation, PKCE, replay and claim/security-fault cases remain. No new private key fixture is persisted.

Compatibility decision before merge: adopt the explicit 2048–8192-bit issuer-key policy, or defer this backend change for installations requiring previously accepted sub-2048-bit issuer keys. No verification weakening is proposed. Other actual Core caller compilation and full owning CI remain required.

Current dependency preflight: required cargo-deny checks pass for advisories, bans, licenses and sources; cargo-audit passes without ignores or warnings against the cached advisory snapshot. Cargo selected crypto-bigint 0.7.10 and ctutils 0.4.6 within the existing compatible ranges to replace the now-yanked 0.7.7 input. The unchanged retained OIDC patch hash is 97c1d611054d8e0ba5823f4f2c55effd6dc8f923aa3e90e2c166533439f12eab. Focused identity tests were rerun on this exact lock: 9 units and 13 integration cases pass. Four bootstrap/provenance tests and exact vendor-source verification pass. Full owning CI remains pending.
