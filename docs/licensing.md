# Core licensing qualification

The frozen F2 contract authorizes Apache-2.0 for TradeAssembly-owned Core.
This change applies only to this isolated extraction, not Studio, Product,
Warden, plugins, or third-party dependencies. Existing attribution is preserved.

LICENSE is the official https://www.apache.org/licenses/LICENSE-2.0.txt text,
SHA256 cfc7749b96f63bd31c3c42b5c471bf756814053e847c10f3eb003417bc523d30.
NOTICE attaches the Core attribution and preserves third-party license scope.

## Bounded packet and proof matrix

Twenty active minutes, charged to M3; two equivalent failed attempts require
root diagnosis. Root owns LICENSE, NOTICE, this document, publication manifest,
package notice inclusion and integration. Luna owns only the license validator
and targeted tests in xtask/src/foss_core_boundary.rs.

| Requirement | Deterministic evidence |
| --- | --- |
| Actual authorized Core grant | Exact LICENSE digest and Core NOTICE validation |
| No manifest-only readiness | Missing/altered files and metadata mismatch rejected |
| Failure cannot report ready | Failed scan always has publicLicenseReady false |
| Preserve other owners | Scope stated in NOTICE; no other repositories relicensed |
| Distribution attribution | Existing bundle assembler copies Core LICENSE and NOTICE |

Acceptance: cargo test -p tradeassembly-xtask foss_core_boundary::tests;
cargo xtask foss-core-boundary; targeted bundle tests; cargo fmt --check;
cargo clippy -p tradeassembly-xtask --all-targets -- -D warnings.
Set TRADEASSEMBLY_CORE_REVISION to the recorded extraction source revision while
the local repository is uncommitted. A passing license gate proves the Core
grant only, not chain-of-title review or third-party distribution qualification.

Excluded: public publication, pin changes, changing third-party licenses,
new license-compliance frameworks, and declaring M3/M7/M8 complete.
Remaining release proof: exact binary/source/notices inventory, dependency
attribution for each shipped artifact, clean revision and archive verification.
