# Extraction decisions

- Use the owner-approved Apache-2.0 Core grant; preserve OptionLab LLC legal
  attribution and third-party rights. Validate actual LICENSE/NOTICE, not a
  readiness flag alone. Exact binary distribution notices remain a separate gate.
- Preserve the Studio repair source; copy and qualify Core independently. Remove
  uncompiled Studio tooling only after exact source comparison. The removal
  inventory is docs/removed-studio-tooling.json.
- Core archive proof uses an immutable clean Git revision, an isolated locked
  Rust build, and actual CLI/MCP setup discovery. It must not start Studio or
  substitute fake Warden health for authority evidence. Actual Warden/control
  sink proof remains the separate unchanged M2 gate.
- A private local checkpoint enables source/archive checks; it is not a release
  approval, pin update, merge, or public publication.
- No scheduler rewrite, extra brokers, UI buildout or hosted agent orchestration
  is introduced by this extraction packet. Frozen F2 M0–M8 criteria are unchanged.
