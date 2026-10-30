# Update publication continuation — 2026-10-29

The existing updater draft branch adds a canonical offline native-proof verifier
and sealed signing request to the public distribution library. The maintainer
CLI invokes that verifier before its sole production-key loader; no hosted
publisher, permission, release/tag or production-key invocation is installed/run.
The contract carries native artifact hashes and source identities only; it does
not import hosted implementations or depend on private repositories. Consumer
update profiles and existing runtime/SDK pins are unchanged.

Four library contracts and three fixture-key CLI contracts cover exact-byte
signing/consumer authentication, missing or uncertain proof, archive/manifest/
component substitution, validity and output clobbering. Production keys/Apple
credentials are not used. Strict distribution all-target Clippy and formatting
pass. Complete owning CI on the final pushed source remains the integration
receipt; no native or production update acceptance is claimed.
