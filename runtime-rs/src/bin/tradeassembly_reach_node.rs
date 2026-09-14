// Copyright (c) 2026 OptionLab LLC. All rights reserved.

use clap::Parser;
use serde_json::json;
use std::{path::PathBuf, process, sync::Arc, thread, time::Duration};
use tradeassembly_runtime::adapters::connected_runner::{
    read_ed25519_seed_file, Ed25519InstallationSigner, HttpReachTransport, OsRunnerEntropy,
};
use tradeassembly_runtime::cli_identity::{authenticated_service_from_config, verify_cli_session};
use tradeassembly_runtime::ports::{ConnectedRunnerError, ReachNodeScope, ReachPoll};
use tradeassembly_runtime::runtime_config::{RuntimeConfig, RuntimeConfigLayer};
use tradeassembly_runtime::service::reach::ConnectedReachConsumer;

#[derive(Debug, Parser)]
#[command(name = "tradeassembly-reach-node")]
struct Args {
    #[arg(long)]
    db: String,
    #[arg(long)]
    relay_url: String,
    #[arg(long)]
    tenant_id: String,
    #[arg(long)]
    workspace_id: String,
    #[arg(long)]
    node_id: String,
    #[arg(long)]
    actor_ref: String,
    #[arg(long)]
    node_key_file: PathBuf,
    #[arg(
        long,
        default_value_t = 1_000,
        value_parser = clap::value_parser!(u64).range(100..=60_000)
    )]
    poll_interval_ms: u64,
    #[arg(long)]
    once: bool,
}

fn main() {
    if let Err(error) = run(Args::parse()) {
        eprintln!(
            "{}",
            json!({"status":"failed","code":format!("{:?}", error.code)})
        );
        process::exit(2);
    }
}

fn run(args: Args) -> Result<(), ConnectedRunnerError> {
    let seed = read_ed25519_seed_file(&args.node_key_file)?;
    let signer = Arc::new(Ed25519InstallationSigner::from_seed(
        args.node_id.clone(),
        seed,
    )?);
    let transport = Arc::new(HttpReachTransport::new(&args.relay_url)?);
    let config = RuntimeConfig::local_runner(
        args.db,
        RuntimeConfigLayer::from_env().map_err(|_| ConnectedRunnerError::core_failure())?,
    )
    .map_err(|_| ConnectedRunnerError::core_failure())?;
    let runtime =
        tokio::runtime::Runtime::new().map_err(|_| ConnectedRunnerError::core_failure())?;
    let verified_session = runtime
        .block_on(verify_cli_session(&config))
        .map_err(|_| ConnectedRunnerError::core_failure())?;
    drop(runtime);
    let service = authenticated_service_from_config(config, verified_session)
        .map_err(|_| ConnectedRunnerError::core_failure())?;
    let consumer = ConnectedReachConsumer::new(
        transport,
        signer,
        service.clone(),
        ReachNodeScope {
            tenant_id: args.tenant_id,
            workspace_id: args.workspace_id,
            node_id: args.node_id,
            actor_ref: args.actor_ref,
        },
        service.runtime().clock.clone(),
        Arc::new(OsRunnerEntropy),
    )?;
    let interval = Duration::from_millis(args.poll_interval_ms);
    loop {
        match consumer.poll_once() {
            Ok(ReachPoll::Idle) => println!("{}", json!({"status":"idle"})),
            Ok(ReachPoll::Completed {
                request_id,
                state,
                evidence_ref,
            }) => println!(
                "{}",
                json!({
                    "status":"terminal",
                    "requestId":request_id,
                    "outcome":state,
                    "evidenceRef":evidence_ref,
                })
            ),
            Err(error) if !args.once && error.is_retryable() => eprintln!(
                "{}",
                json!({"status":"retrying","code":format!("{:?}", error.code)})
            ),
            Err(error) => return Err(error),
        }
        if args.once {
            return Ok(());
        }
        thread::sleep(interval);
    }
}
