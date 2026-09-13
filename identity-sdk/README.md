# TradeAssembly Identity SDK

`tradeassembly-identity-sdk` is the public, provider-neutral OpenID Connect client
used by TradeAssembly Core. Applications consume a normalized identity and do not
branch on WorkOS, Keycloak, or another provider brand.

The crate implements OIDC discovery and Authorization Code with S256 PKCE.
Pending state, nonce, and verifier records are consume-once. Callback
processing validates issuer, client audience, configured audience, asymmetric
signature algorithm and signature, expiration, issue time, nonce, access-token
hash when present, and configured claim types.

## Profiles

| Profile | Use |
| --- | --- |
| `TradeAssemblyHubProduction` | Default production identity |
| `TradeAssemblyHubStaging` | Pre-release Hub integration |
| `KeycloakLocalDev` | Human local development |
| `OidcServerMockTest` | Automated conformance |
| `ConfiguredOidc` | Supported alternate HTTPS deployment |

Only the named development and test profiles accept a loopback HTTP issuer.
Configured issuers require HTTPS. Redirects require HTTPS or loopback HTTP.
Client secrets and authorization material are redacted from diagnostics.

## Contract

- Applications call `OidcProviderPort`.
- Stable identity derives from exact `(issuer, subject)`.
- Email and display name are presentation claims, never identity keys.
- The caller owns encrypted application-session storage.
- Refresh validates a fresh ID token, preserves exact issuer and subject, and
  rotates refresh material when supplied.
- Tokens, codes, raw claims, state, nonce, and PKCE verifier values must never
  be logged.

Run the protocol suite with:

```bash
cargo test -p tradeassembly-identity-sdk -- --test-threads=1
```
