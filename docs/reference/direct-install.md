# TradeAssembly installation and setup

Apple Silicon macOS is the supported customer platform. Use the combined
TradeAssembly installation archive identified in the release notes. Its version
and SHA-256 identify the exact qualified download; a source checkout or a
standalone Core archive is not the guided customer installer.

The 0.2.0 correction is a candidate until its installation, affected transition,
and publication receipts exist. Existing v0.1.0 downloads remain immutable.

1. Download the combined archive and open it in Finder.
2. Open `TradeAssembly.command` inside the extracted folder.
3. Follow the setup questions and the browser sign-in and broker consent pages.
4. Choose whether to enable paid Relay, notification destinations, encrypted
   backup, and the detected supported agent integration.

The package supplies Core, the companion, the Alpaca plugin, Node and sandbox
runtime. Setup creates its own private state, keys and services. No compiler,
package manager, manual configuration, tokens, MCP JSON or LLM is required.
The customer can skip agent integration. If Codex CLI is selected, setup uses
its actual configuration and tool-discovery interfaces while preserving
unrelated settings. The starter prompt is:

> Help me finish setting up TradeAssembly and connect my Alpaca paper account.

Core can run locally without buying Relay. Hub owns paid Relay access; returning
from checkout does not establish an entitlement. Core and Relay sign in
independently. Setup never activates a strategy or places an order.

Close and reopen the same setup entrypoint to resume interrupted work. Completed
choices, installation identity, registration and services are preserved.
Use the new version's entrypoint for an update; the owning installer verifies
its supported transition and retains previous exact distributions. An update
is not a general state migration or permission to reactivate strategies.

## Local credentials and recovery

Credentials and keys default to the application's private local state. Private
directories use mode 0700 and secret files mode 0600. Corrupt, unsafe or wrongly
bound records fail closed. Bitwarden and operating-system credential managers
are optional. Explicit legacy migration verifies the intended identity and
preserves source entries; setup never silently replaces an established key.

When enabling backup, explicitly export the recovery key to the protected folder
selected in setup. Keep it available independently of this installation. On a
second computer, run the same guided setup, sign in and enroll its new identity,
choose recovery, select the key, then choose an authorized saved backup.
Strategies restore stopped. Login and broker credentials, installation private
keys and active execution authority are excluded. Reconnection and activation
remain explicit customer actions.

## Platform limits

The initial delivery is unsigned or ad-hoc signed, not Developer ID signed or
notarized. Use normal macOS approval if required; do not disable Gatekeeper or
other security protections. Local services run in the signed-in user session;
they do not keep the computer awake or run while it is powered off.
Windows and Linux distributions remain experimental. Intel Mac is not qualified.

Installation and process-derived qualification are distinct from production
provider acceptance. Release notes identify the changed checks, retained base,
exact artifacts and any remaining customer-visible acceptance items.
