// Copyright (c) 2026 OptionLab LLC. All rights reserved.

use clap::Parser;
use serde_json::json;
use std::{path::PathBuf, process, sync::Arc, thread, time::Duration};
use tradeassembly_runtime::adapters::connected_runner::{
    read_ed25519_seed_file, Ed25519InstallationSigner, HttpProductRunnerTransport, OsRunnerEntropy,
};
use tradeassembly_runtime::cli_identity::{authenticated_service_from_config, verify_cli_session};
use tradeassembly_runtime::ports::{ConnectedRunnerError, ConnectedRunnerPoll};
use tradeassembly_runtime::runtime_config::{RuntimeConfig, RuntimeConfigLayer};
use tradeassembly_runtime::service::connected_runner::ConnectedPaperRunnerConsumer;
use tradeassembly_runtime::service::paper_runner::DurableCorePaperRunner;

#[derive(Debug, Parser)]
#[command(name = "tradeassembly-connected-runner")]
struct Args {
    #[arg(long)]
    db: String,
    #[arg(long)]
    core_release: String,
    #[arg(long)]
    product_url: String,
    #[arg(long)]
    installation_id: String,
    #[arg(long)]
    installation_key_file: PathBuf,
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
            json!({"status": "failed", "code": format!("{:?}", error.code)})
        );
        process::exit(2);
    }
}

fn run(args: Args) -> Result<(), ConnectedRunnerError> {
    let seed = read_ed25519_seed_file(&args.installation_key_file)?;
    let signer = Arc::new(Ed25519InstallationSigner::from_seed(
        args.installation_id,
        seed,
    )?);
    let transport = Arc::new(HttpProductRunnerTransport::new(&args.product_url)?);
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
    let runner = Arc::new(
        DurableCorePaperRunner::new(service.clone(), args.core_release.clone())
            .map_err(|_| ConnectedRunnerError::core_failure())?,
    );
    let consumer = ConnectedPaperRunnerConsumer::new(
        args.core_release,
        transport,
        signer,
        runner,
        service.runtime().clock.clone(),
        Arc::new(OsRunnerEntropy),
    )?;
    let interval = Duration::from_millis(args.poll_interval_ms);
    loop {
        match consumer.poll_once() {
            Ok(ConnectedRunnerPoll::Idle) => {
                println!("{}", json!({"status": "idle"}));
            }
            Ok(ConnectedRunnerPoll::Completed { receipt, .. }) => {
                println!(
                    "{}",
                    json!({
                        "status": "completed",
                        "commandId": receipt.command_id,
                        "operation": receipt.operation,
                        "outcome": receipt.state,
                    })
                );
            }
            Err(error) if !args.once && error.is_retryable() => {
                eprintln!(
                    "{}",
                    json!({"status": "retrying", "code": format!("{:?}", error.code)})
                );
            }
            Err(error) => {
                return Err(error);
            }
        }
        if args.once {
            return Ok(());
        }
        thread::sleep(interval);
    }
}
