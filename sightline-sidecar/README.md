# TradeAssembly Sightline Sidecar

This crate is the public contract-only boundary between TradeAssembly Core and
Sightline. It uses TradeAssembly-owned adapter DTO validation and checked-in
conformance fixtures so normal public Core builds do not fetch external
packages. Revision `6620d6add943975b1f0a8ec43975420621dcda83` is the exact
locally reviewed Apache-2.0 release candidate and compatibility target.

The sidecar maps TradeAssembly session, semantic context, selection, action,
navigation, proposal, approval, event, redaction, and MCP transport data to a
stable public adapter contract. It does not depend on Sightline crates,
TradeAssembly Product, Warden Platform, hosted services, or credential material.
TradeAssembly's runtime only accepts selection set/share calls from an authenticated
in-process Studio user session; unauthenticated HTTP and GraphQL calls fail
closed.

`cargo xtask sightline-conformance` verifies the clean source revision,
canonical Apache-2.0 license, package metadata, and exact fixture round-trip
through the external public Rust models. Remote publication of the pinned
revision remains an external release blocker while GitHub activity is paused
and is not implied by the local-source status.
