# Rust-Native Tooling Command Migration

Status: canonical

`just` and `cargo xtask` are the only repository command planes. The Makefile
was deleted after GC-38. There is no Make compatibility dispatcher.

## Naming Rule

Former dotted target names map to kebab-case Just recipes. Environment inputs
remain environment variables, for example:

```bash
BACKEND=container just iac-local-up
PROVIDER=aws PROFILE=starter just iac-cloud-plan
```

## Complete Migration Inventory

The deleted 514-line source `Makefile` had SHA-256
`c9dc9c3f53d625acdac087b8285d5d4c3f213b30bc32541cf05aa1fc05ac375e` and
defined 119 targets excluding `.PHONY`. The inventory below contains exactly
119 unique rows: 117 map to real Just recipes and two are explicitly removed.

| Former target | Canonical command |
| --- | --- |
| `help` | `just help` |
| `setup` | `just setup` |
| `runtime.ensure` | private `runtime-ensure` recipe |
| `studio.ensure` | private `studio-ensure` recipe |
| `setup.research` | `just setup-research` |
| `iac.bootstrap` | `just iac-bootstrap` |
| `iac.local.plan` | `just iac-local-plan` |
| `iac.local.up` | `just iac-local-up` |
| `iac.local.down` | `just iac-local-down` |
| `iac.local.status` | `just iac-local-status` |
| `iac.local.logs` | `just iac-local-logs` |
| `iac.local.verify` | `just iac-local-verify` |
| `iac.local.destroy` | `just iac-local-destroy` |
| `iac.server.plan` | `just iac-server-plan` |
| `iac.server.apply` | `just iac-server-apply` |
| `iac.cloud.bootstrap` | `just iac-cloud-bootstrap` |
| `iac.cloud.validate` | `just iac-cloud-validate` |
| `iac.cloud.plan` | `just iac-cloud-plan` |
| `iac.cloud.inventory` | `just iac-cloud-inventory` |
| `iac.cloud.apply` | `just iac-cloud-apply` |
| `iac.cloud.destroy` | `just iac-cloud-destroy` |
| `iac.render.validate` | `just iac-render-validate` |
| `iac.prod.audit` | `just iac-prod-audit` |
| `iac.audit` | `just iac-audit` |
| `dev` | `just dev` |
| `dev.start` | `just dev-start` |
| `dev.stop` | `just dev-stop` |
| `dev.restart` | `just dev-restart` |
| `dev.status` | `just dev-status` |
| `dev.logs` | `just dev-logs` |
| `dev.api` | `just dev-api` or `just api` |
| `dev.studio` | `just dev-studio` or `just studio` |
| `dev.distributed` | `just dev-distributed` |
| `dev.distributed.start` | `just dev-distributed-start` |
| `dev.distributed.stop` | `just dev-distributed-stop` |
| `dev.distributed.restart` | `just dev-distributed-restart` |
| `dev.distributed.status` | `just dev-distributed-status` |
| `dev.distributed.logs` | `just dev-distributed-logs` |
| `dev.distributed.compose.seed` | `just dev-distributed-compose-seed` |
| `dev.distributed.compose.up` | `just dev-distributed-compose-up` |
| `dev.distributed.compose.down` | `just dev-distributed-compose-down` |
| `dev.distributed.compose.logs` | `just dev-distributed-compose-logs` |
| `seed` | `just seed` |
| `seed.btc` | `just seed-btc` |
| `reset` | `just reset` |
| `run` | `just run` |
| `run.btc-demo` | `just run-btc-demo` |
| `runtime.demo-gate` | `just runtime-demo-gate` |
| `backtest` | `just backtest` |
| `backtest.btc` | `just backtest-btc` |
| `validate` | `just validate` |
| `preview` | `just preview` |
| `creds` | `just creds` |
| `creds.store` | removed; plugin-defined credential configuration is owned by the generic Studio plugin surface and external package |
| `creds.test` | `just creds-test` |
| `scheduler` | `just scheduler` |
| `scheduler.start` | `just scheduler-start` |
| `scheduler.run` | `just scheduler-run` |
| `scheduler.stop` | `just scheduler-stop` |
| `worker.scheduler` | `just worker-scheduler` |
| `worker.runner` | `just worker-runner` |
| `worker.oms` | `just worker-oms` |
| `worker.status` | `just worker-status` |
| `worker.stream` | removed; it falsely mapped to order-attempt listing and no order-stream worker exists |
| `research.sweep` | `just research-sweep` |
| `research.universe` | `just research-universe` |
| `research.universes` | `just research-universes` |
| `research.job` | `just research-job` |
| `research.jobs` | `just research-jobs` |
| `research.artifacts` | `just research-artifacts` |
| `research.backtest` | `just research-backtest` |
| `provider.policy` | `just provider-policy` |
| `provider.policy.set` | `just provider-policy-set` |
| `orders.reconcile` | `just orders-reconcile` |
| `orders.attempts` | `just orders-attempts` |
| `orders.status-event` | `just orders-status-event` |
| `orders.status-stream` | `just orders-status-stream` |
| `orders.stream-watch` | `just orders-stream-watch` |
| `risk.status` | `just risk-status` |
| `risk.account-set` | `just risk-account-set` |
| `risk.account-sync` | `just risk-account-sync` |
| `risk.margin-preview` | `just risk-margin-preview` |
| `risk.stress` | `just risk-stress` |
| `positions` | `just positions` |
| `positions.sync` | `just positions-sync` |
| `marketdata.bars` | `just marketdata-bars` |
| `marketdata.quote` | `just marketdata-quote` |
| `marketdata.indicators` | `just marketdata-indicators` |
| `marketdata.conformance` | `just marketdata-conformance` |
| `options.chain` | `just options-chain` |
| `options.select` | `just options-select` |
| `e2e.alpaca-paper` | removed; provider-specific acceptance is owned by the standalone Alpaca plugin repository |
| `e2e.alpaca-options-exercise` | `just e2e-options-exercise` |
| `e2e.alpaca-options-strategies` | `just e2e-options-strategies` |
| `e2e-exit-demo-clean` | `just e2e-exit-demo-clean` |
| `test` | `just test` |
| `test.runtime` | `just test-runtime` |
| `test.studio` | `just test-studio` |
| `test.e2e` | `just test-e2e` |
| `build` | `just build` |
| `build.studio` | `just build-studio` |
| `scan.public` | `just scan-public` |
| `release.evidence` | `just release-evidence` |
| `sdlc.observe` | `just sdlc-observe` |
| `sdlc.retrospective` | `just sdlc-retrospective` |
| `architecture-check` | `just architecture-check` |
| `architecture.gate` | `just architecture-gate` |
| `architecture.gate.legacy` | removed; duplicate legacy alias |
| `plugin-contract` | `just plugin-contract` |
| `lifecycle.smoke` | `just lifecycle-smoke` |
| `distributed.smoke` | `just distributed-smoke` |
| `distributed.lifecycle.smoke` | `just distributed-lifecycle-smoke` |
| `deploy.gate` | `just deploy-gate` |
| `check` | `just check` |
| `gate-ci` | `just gate-ci` |
| `check.ci` | `just verify` |
| `clean` | `just clean` |
| `clean.deps` | `just clean-deps` |
| `clean.all` | `just clean-all` |

## Enforcement

`cargo xtask contract tooling` and `cargo xtask architecture-check` fail when:

- a root Makefile exists
- canonical Just recipes are missing
- the 119-row migration inventory is incomplete, duplicated, or points to an
  unknown recipe
- `Justfile` delegates to Make or a legacy xtask dispatcher
- active command documentation or release evidence invokes Make

Historical planning and audit files may retain old command output as evidence;
they are not executable or current operator guidance.

## Accepted Behavior Corrections

`BACKEND=native` is not a supported `iac-local-*` mode. The deleted Makefile
advertised and defaulted to that value, but the GC-38 review base had no native
Ansible role, both local inventories selected `container`, and preflight
rejected every backend except `container`. The advertised native path therefore
could not provision, stop, inspect, or destroy a native stack. GC-38 removes
that false compatibility claim instead of preserving an inoperative mode:

- `iac-local-*` defaults to `BACKEND=container` and rejects other values;
- container-backed local/self-hosted infrastructure remains available through
  the `iac-local-*` recipes; and
- thin local single-process development remains available through `just dev`
  and the lifecycle recipes without a container dependency.

This correction changes a former environment-value claim, not the disposition
of any former target, so all seven `iac.local.*` target rows above remain mapped
to canonical Just recipes.
