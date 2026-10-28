# Local portable plugin packaging input

This local/unpublished Core candidate selects Alpaca
`0.1.20-local.20261028`, source `5860ddbc6cc6fc61641ec6a26a61abe3622f9ec1`,
package SHA-256 `ea715795d1eefe2344eac6c54ff20edb97107ea58f4aff0c7133acb5071c29b2`
and canonical manifest SHA-256
`674626be9ccc4bb811c9dd125aa0109cf16a8fe6b3ef0f25f2cb1c521c40fa24`.

The historical 0.1.19 archive retains its old digest and is not overwritten or
relabeled. Its native dependency tracing metadata includes builder-home paths,
which the existing Core distribution guard correctly rejects. The new package
uses compiler path remapping, with no runtime provider behavior changes or
binary string patching. Its source packager rejects builder paths and conflicting
archive replacement. Native extraction/relocation discovery passes with cleared
environment and no credentials, from a fresh path containing spaces.

This input follows the existing immutable package/manifest contract. No public
release, production assembly lock change, signing, provider call or new credential
is implied. Rebuilding the local CLI is required because its default offline
installer embeds this exact input; a receipt-only substitution is insufficient.
The existing package and installation guards remain enabled.
