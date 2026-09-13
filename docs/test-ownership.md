# Test ownership after Core extraction

`runtime-rs/tests/full_parity_gap_closure.rs` remains in Studio, not Core.
It verifies Studio UI ledgers, Next-generated state, lifecycle wrappers and
historical migration documentation. Its six assertions are unchanged.

Original Studio repair copy SHA256:
e9f9edc6e8b53011d8bb69a22f2e4a84998fc71b3fca4853f9727aa18404351b.
`cargo test --locked -p tradeassembly-runtime --test full_parity_gap_closure`
passed6/0/0 in its owner repository, session16798. The duplicate Core extraction
copy was compared byte-for-byte before removal. Original remains recoverable.

Studio must continue this gate when consuming pinned Core. Removing the duplicate
does not satisfy Studio release qualification or waive any F2 M8 owning gate.
Core keeps runtime behavioral, protocol, safety, replay and idempotency tests,
including Studio API-contract compatibility tests that exercise shared runtime.

`studio_contract_parity.rs` is split by assertion ownership, not skipped:
Studio retains operation-map backing and Playwright supervisor checks; all three
tests in the original owner file passed session51862. Core retains the exact
`studio_runtime_payloads_preserve_legacy_contract_shapes` test and its required
helpers. No payload assertion changed; no ignore/cfg/file-existence bypass added.
