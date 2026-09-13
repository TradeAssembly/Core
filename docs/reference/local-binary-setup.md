# Local binary setup (F2 implementation preview)

This describes the binary command surface, not an available public release.
Public downloads and signing/notarization still need release acceptance.
An unsigned local candidate now bundles Alpaca and its sandbox dependencies.
Do not send users to build from source as the
released installation procedure.

In a release package, `tradeassembly` and its compatible `warden` executable live
side by side. An agent can run:

```text
/absolute/package/bin/tradeassembly setup --state-dir /absolute/new/installation
```

For an isolated developer acceptance run, `--warden-binary /absolute/path/warden`
selects a prebuilt Warden and `--warden-port PORT` selects a loopback port. No Cargo
or source files are used by the setup command itself. The executable version is
checked and its digest recorded; signed release provenance remains a separate
packaging requirement, not something this initial local digest establishes.

The structured result returns `runtimeConfigPath` and argument arrays in
`nextCommands`. Run commands as executable plus arguments; do not concatenate
paths into a shell string. It is legitimate for a path to contain spaces.

1. Run `tradeassembly policy serve --state-dir /absolute/new/installation` in
   the foreground. A process supervisor can own this process; the command does
   not create global PID files, kill other processes, or install launchd jobs.
   On macOS, `tradeassembly policy launchd install --state-dir
   /absolute/new/installation` is the unattended alternative. It installs a
   user-scoped policy service with an installation-specific label. `verify` and
   `status` inspect supervision; `uninstall` removes only that owned service and
   preserves state/logs. These commands do not activate/deactivate strategies or
   liquidate positions. Supervision status is not broker/strategy health.
   This is a LaunchAgent: it runs in the signed-in macOS user session. It does
   not keep the computer awake or execute while the machine is powered off.
2. Configure the agent's MCP server with the absolute `tradeassembly` executable
   and arguments `--config`, the returned config path, `mcp`, `serve`.
3. Run the same executable with `--config /absolute/new/installation/runtime.json
   plugins install-default --offline`. A bundle selects its pinned Alpaca archive
   and verifies its package/manifest digests; a missing bundled archive fails
   instead of falling back to a different remote version.
4. Call `tradeassembly.setup.inspect` with `{"surface":"agent"}`. Local identity
   does not require Hub. Broker verification and restart proof are still required.
   Omitting `surface` preserves the separate Studio acceptance path.

Setup generates local policy keys and a private token, embeds the compatible
policy/PEP assets, and separates Warden's database from Core's database. It does
not collect broker credentials, select investments, install a trading strategy,
start an agent, or activate trading. The result intentionally reports
`policyRunning: false` and `automationReady: false`.

Repeated setup against the same recognized installation preserves identity,
keys, token and databases. Incompatible executable/port changes, changed assets,
unsafe permissions or unrecognized existing state fail closed. Use a new state
directory for a clean test; never reuse or reset someone else's rig. Upgrades
and rollback need a dedicated migration procedure before public distribution.

Setup provisions policy state and, when run from the bundle, records the absolute
bundled sandbox launcher in runtime configuration. Node and SRT are bundled;
customer npm/Node installation is not required. `sandboxConfigured` reports that
configuration, not broker connectivity or strategy readiness. Plugin installation
is the separate command above. Policy/agent supervision and the end-to-end
broker/agent journey remain required F2 work; setup alone is not complete onboarding.
