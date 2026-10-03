# TradeAssembly Core direct-install guide

Release: `0.1.0-beta.4`. These are the prepared customer instructions, not a
claim that publication or the complete production Relay journey has passed.
Until the release assets are public and the owning download/install checks pass,
do not advertise this URL as an available installer.

## Platform and security status

The initial qualified platform is Apple Silicon macOS. This beta is **unsigned
or ad-hoc signed, not Apple Developer ID signed or notarized**. Do not disable
Gatekeeper or other security protections. A blocked first launch needs the
user's normal macOS approval; an agent must not bypass the warning.

Windows x64 and GNU Linux x64/ARM64 packages have experimental build/integrity
coverage only. Native installation and runtime acceptance on those systems are
not qualified. Intel Mac, Windows ARM, and musl Linux are not in this release.
The package includes its Rust executables, Alpaca plugin, Node and sandbox
dependencies; installing Rust, cloning the source, or installing npm is not the
customer setup procedure. npm/pnpm distribution is deferred.

## Install

After publication, the version-pinned macOS installation command is:

```sh
bash -o pipefail -c 'curl --fail --silent --show-error --location --proto "=https" --proto-redir "=https" https://github.com/TradeAssembly/Core/releases/download/v0.1.0-beta.4/install.sh | sh'
```

The installer checks the downloaded package against its embedded SHA-256 before
running the native installer. An HTTP, digest, sandbox, platform or integrity
failure is a failure, not an invitation to bypass the check. For inspection
before execution, download the same `install.sh` and read it first. The release
also carries `delivery.json` and `install.ps1`; Windows remains experimental.

The installation result is JSON. Its `command`, `stateDirectory`, `mcp` and
`nextCommands` fields are the authoritative paths and argument arrays. Register
that exact `mcp.command` and `mcp.args` in the agent client. Do not guess a PATH
entry, concatenate returned arguments into a shell string, or inspect Rust
source to discover the interface.

The default macOS installation is `~/Library/Application Support/TradeAssembly`.
The stable launcher is `bin/tradeassembly` inside that installation. The
installer prepares state and the default plugins; it does not start a trading
strategy, place orders, or create a persistent agent.

## Start the local policy service and connect the agent

Run the policy-service command from `nextCommands` under a process supervisor.
For unattended macOS use, the installed executable also exposes
`policy launchd install --state-dir ABSOLUTE_STATE_DIRECTORY` and the corresponding
`verify`, `status` and `uninstall` commands. Use the returned state directory.
It is a user-session service, not a cloud runner: a sleeping, stopped or
disconnected computer can interrupt local operation.

Connect the registered MCP server and call `tradeassembly.setup.inspect` with
`{"surface":"agent"}`. Installation or supervision status alone is not broker
readiness. The external agent client owns its agent loop and any sub-agents;
Core and Warden provide tools and deterministic enforcement.

## Browser sign-in, Alpaca and optional Relay

Call `tradeassembly.onboarding.start` with the user's chosen `mode`,
`environment: "production"`, a new `idempotency_key`, and whether Relay is
requested. Open only the returned `browserUrl` in the user's browser. Complete
normal TradeAssembly sign-in and Alpaca consent there. The installed connection
profile and broker plugin supply the public configuration and permission UI.
Do not ask the user to paste API keys, OAuth codes, client IDs or configuration.

Poll `tradeassembly.onboarding.status` using the same `onboardingId`. Preserve
that attempt across an MCP restart; a restarted process may return a fresh local
browser URL. Do not reuse an expired sign-in link. `ready: true` is setup
readiness, not permission to activate a strategy. Confirm the verified broker
account and environment with the user before any separate activation decision.

Account-free local Core does not require a Relay purchase. Optional hosted Relay
uses the user's TradeAssembly Hub identity and paid entitlement. Use the
returned checkout link and the signed-in Hub purchase-management interface;
never claim entitlement merely because checkout opened. Production purchase,
webhook, allowance, portal and cancellation acceptance remain launch gates.
Relay supplies hosted tooling, not a hosted persistent agent runner in this
release. Cancelling a paid subscription is distinct from stopping a local agent
or cancelling broker orders. Natural production expiry is tracked after launch;
the initial lifecycle gate includes sandbox and deterministic boundary evidence.

## Update, reinstall and rollback

Stop the external agent's execution loop and this installation's processes
before changing versions. A stopped process does not establish that broker
orders or positions are gone. Reconcile those separately using the owner's
instructions. Never kill another installation's processes to force an update.

Re-running the same installer against a recognized installation preserves its
identity and durable state. To move an existing stopped installation to this
pinned release:

```sh
bash -o pipefail -c 'curl --fail --silent --show-error --location --proto "=https" --proto-redir "=https" https://github.com/TradeAssembly/Core/releases/download/v0.1.0-beta.4/install.sh | sh -s -- upgrade'
```

Use the future release's explicit versioned URL for a future update, not an
unpinned `latest` link. `pipefail` makes a failed bootstrap download nonzero;
an empty response must not be reported as a successful installation.

The installed launcher exposes `distribution status` and `distribution rollback`.
Rollback requires a retained compatible previous version and a stopped rig; a
first installation has no previous version to restore. Integrity, compatibility,
active-rig and transaction failures must remain nonzero. Preserve installation
state and logs; do not delete databases or manually swap version pointers.

## Export and support

The installed Core command `journal export` returns the local journal export.
Treat it as private trading data; save it only to a user-selected destination.
Local export is not proof that Relay ingest, replay or hosted export succeeded.

For a problem report, provide the installed version/target, non-secret error
code and redacted reproduction steps through the project's support channel.
Do not include credentials, tokens, raw callback URLs, unredacted journals or
private account payloads. An agent should read the installed CLI help and MCP
tool descriptions first. A missing tool, failed login or denied entitlement
must be reported accurately, not worked around by reading source or changing
security controls.

Users supply all strategy logic, research questions, risk scale and activation
decisions. Setup does not select investments or prescribe trading rules.
