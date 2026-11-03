# Core data runner V3: bounded session dispatch

V3 adds one finite paper-only operation, `dispatch_session`, with a closed
`dispatch` union: `enqueue { job }`, `advance { jobId }`, `inspect { jobId }`, or
`pause { jobId, paused }`. `job` is the existing versioned session dispatch
request. There is no arbitrary HTTP, MCP, tool name, principal, credential,
broker operation or background-worker variant.

The command, mutation binding and result receipt use
`tradeassembly.core-data-runner.v3`,
`tradeassembly.core-data-mutation-binding.v3` and
`tradeassembly.core-data-runner.receipt.v3`. The coupled Reach envelope and signed
preparation use `tradeassembly.relay.reach-data-command/v3` and
`tradeassembly.relay.reach-data-preparation/v3`. Older operations continue to
require their original versions; V1 and V2 cannot carry scheduler dispatch.

All four variants require the existing `scheduler.run` C3 command, policy action
`scheduler.manage`, actual resource kind `scheduler`, and the current independent
human/strategy ownership and source bindings. Even inspection requires that
explicit authority in this version. A read-source grant is insufficient.

The immutable domain binding covers the actual owner, compiled source revision,
complete original dispatch request and verified current schedule receipt.
Existing-job operations resolve the owned stored job before preparing a binding.
The outer command separately binds the exact requested operation and transport
idempotency key. Mutable job progress is excluded from the domain digest; a
recovered original receipt must remain usable after later bounded advances.

A completed result contains `kind: dispatch` and the original immutable `job`
snapshot, including its generation, status and retained child receipts. The
snapshot is historical, not a claim of current liveness or readiness. A fresh
inspection requires a new bounded outer command. A completed computation can
still contain warming-up signals. No result grants broker execution authority.
The normal command/receipt limits remain in force; oversized snapshots fail
rather than truncate.

Remote commands cannot start, stop or resume local background intents. Unpause
also refuses if it would implicitly start an enabled local intent. Pause may
signal an already-local worker to stop at its safe boundary. A signed source or
bounded remote command never establishes persistent agent delegation.

The consumer durably stages one original record acknowledgment per node/source
before sending it. On a subsequent poll or service recreation, it reconciles
that pending receipt before claiming another delivery: current source, owner,
resource/policy binding and original local receipt must still match. The retry
uses a fresh nonce/signature and the original accepted revision/receipt. Loss of
an acknowledgment does not authorize another calculation. Concurrent pending
records conflict explicitly rather than overwrite each other. No caller
principal or transport signature is persisted in the acknowledgment slot.
