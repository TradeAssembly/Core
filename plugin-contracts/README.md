# Plugin Contracts

This directory contains the public plugin contract foundation:

- `schemas/plugin-manifest.schema.json`: manifest shape.
- `schemas/plugin-lock.schema.json`: lockfile shape.
- `manifests/*.yaml`: contract fixtures for built-in and first-party external plugin manifests.
- `default-bundle.yaml`: default local plugin bundle.
- `default-external-plugins.json`: optional, pinned external plugin selections
  used by the installer. These are not built-in runtime plugins.
- `tradeassembly.plugins.lock.yaml`: immutable plugin source lock.

Run:

```bash
cargo xtask plugin-contract
```

These files define contracts only. They do not add hosted Product code or plugin runtime implementations to Core.
