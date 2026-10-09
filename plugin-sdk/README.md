# TradeAssembly Plugin SDK

`tradeassembly-plugin-sdk` defines the public V1 JSON Lines contract between the
TradeAssembly host and an external plugin process.

The host writes one `PluginRequest` to stdin, closes stdin, and the plugin
writes one `PluginResponse` to stdout before it exits. Use `read_request` and
`write_response` to enforce the framing and a caller-selected byte limit.

Writers preserve partial output when an inherited sandbox pipe is nonblocking.
Would-block and interrupted writes/flushes retry within one five-second budget;
permanent errors and exhausted backpressure budgets fail without replaying bytes.
This is transport recovery, not permission to retry an effectful operation.

`EphemeralCredentialGrant` is request-scoped transport data. It is intentionally
redacted from `Debug` output and must not be copied into plugin responses,
diagnostics, logs, or durable records.
