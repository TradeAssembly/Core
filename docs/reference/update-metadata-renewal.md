# Release-triggered metadata renewal

Stable metadata remains valid for at most seven days. Renew it before expiry
using an explicit new release tag, never a scheduled job, ordinary branch push,
PR, automatic retry, or longer verifier lifetime.

The `private-customer-update-input` workflow accepts two distinct lanes:

- `update-input-vVERSION`: first metadata for a sealed candidate.
- `update-renewal-vVERSION-seqN`: metadata-only renewal for the current published
  stable release, using a strictly higher sequence and the same sealed artifacts.

Both require a fresh tag, first attempt, exact default-branch ancestry, passing
owning CI, an approved record and an exact three-asset draft. Neither publishes
that draft, changes latest, nor replaces an existing feed. No new credentials or
Apple submission are needed for unchanged artifact bytes.

A renewal record under `packaging/update-candidates/TAG.json` retains the existing
`tradeassembly.private-update-candidate/v1` fields and adds:

```json
"renewalOf": {
  "releaseTag": "vVERSION",
  "metadataSha256": "INDEPENDENTLY_REVIEWED_CURRENT_METADATA_DIGEST"
}
```

`sequence` must equal N. The existing archive, disk image and notarization proof
digests must remain unchanged. The workflow requires that predecessor release to
be published, stable and current latest; it downloads and authenticates the exact
approved predecessor metadata before signing-key access. The offline command is:

```sh
tradeassembly-distribution update-renewal-plan \
  --proof proof.json --expected-proof-sha256 APPROVED_PROOF_DIGEST \
  --previous-manifest current-metadata.json \
  --expected-previous-sha256 APPROVED_METADATA_DIGEST \
  --public-key TRUSTED_BASE64_PUBLIC_KEY --sequence N
```

The command emits an unsigned plan only. It rejects expired/future/invalid
predecessors, changed proof/component identity, stale sequence, unsafe sequence
integers and a nonadvancing clock. It advances only sequence and timestamps;
expiry is exactly seven days after the current clock. The existing sealed
publication verifier and signer consume the prepared payload. Signing remains
inside the explicit release-tag workflow and writes a new draft asset only.

Public promotion is a separate held operation. Before enabling a stable feed,
its owning release process must establish how the verified renewal reaches the
configured customer feed URL, with idempotent recovery and no artifact changes.
This implementation does not claim that public renewal publication is qualified.
There is no unattended renewal: an operator must admit a release renewal before
expiry. If that window is missed, this lane fails closed; use an explicitly
reviewed normal release/recovery path rather than weakening authenticity or time
checks. Existing offline/pinned/development behavior remains unchanged.
