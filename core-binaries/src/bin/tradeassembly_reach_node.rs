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
    #[arg(long, requires = "core_release")]
    data_channel: bool,
    #[arg(long, requires = "data_channel")]
    core_release: Option<String>,
    #[arg(long, requires = "data_channel", conflicts_with = "command_binding")]
    describe_source: bool,
    #[arg(long, requires = "data_channel")]
    command_binding: bool,
    #[arg(long, requires="data_channel", conflicts_with_all=["command_binding","describe_source","prepare_command"])]
    select_source: bool,
    #[arg(long, requires="data_channel", conflicts_with_all=["command_binding","describe_source"])]
    prepare_command: bool,
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
    tradeassembly_runtime::build_identity::install(env!("TRADEASSEMBLY_CORE_REVISION"))
        .expect("compiled Core build identity");
    let args = Args::parse();
    if args.data_channel {
        if let Err(code) = run_data(args) {
            let code = if code.len() <= 128
                && code.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_')
            {
                code.as_str()
            } else {
                "reach_data_unavailable"
            };
            eprintln!("{}", json!({"status":"blocked","code":code}));
            process::exit(2);
        }
        return;
    }
    if let Err(error) = run(args) {
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

fn run_data(args: Args) -> Result<(), String> {
    use std::io::Read;
    use tradeassembly_runtime::{
        adapters::reach_data_http::HttpReachDataTransport,
        cli_identity::authenticated_data_service_from_config,
        service::{
            data_runner::{DataRunnerCommand, DurableCoreDataRunner, MAX_DATA_COMMAND_BYTES},
            reach_data_consumer::{describe_source, ReachDataConsumer, ReachDataPoll},
        },
    };
    let release = args.core_release.ok_or("reach_data_release_required")?;
    let config = RuntimeConfig::local_runner(
        args.db,
        RuntimeConfigLayer::from_env().map_err(|_| "reach_data_config_invalid")?,
    )
    .map_err(|_| "reach_data_config_invalid")?;
    let runtime = tokio::runtime::Runtime::new().map_err(|_| "reach_data_identity_unavailable")?;
    let session = runtime
        .block_on(tradeassembly_runtime::cli_identity::verify_existing_cli_session(&config))
        .map_err(|_| "reach_data_current_identity_unavailable")?;
    drop(runtime);
    let service = authenticated_data_service_from_config(config, session)
        .map_err(|_| "reach_data_current_identity_unavailable")?;
    let node = ReachNodeScope {
        tenant_id: args.tenant_id,
        workspace_id: args.workspace_id,
        node_id: args.node_id,
        actor_ref: args.actor_ref,
    };
    let source = describe_source(&service, &node)?;
    if args.select_source || args.prepare_command {
        use tradeassembly_runtime::service::reach_data_proofs::DataStatementProducer;
        let mut bytes = Vec::new();
        std::io::stdin()
            .take((MAX_DATA_COMMAND_BYTES + 16385) as u64)
            .read_to_end(&mut bytes)
            .map_err(|_| "reach_data_statement_input_invalid")?;
        if bytes.len() > MAX_DATA_COMMAND_BYTES + 16384 {
            return Err("reach_data_statement_input_too_large".into());
        }
        let seed = read_ed25519_seed_file(&args.node_key_file)
            .map_err(|_| "reach_data_signer_unavailable")?;
        let signer = Ed25519InstallationSigner::from_seed(node.node_id.clone(), seed)
            .map_err(|_| "reach_data_signer_unavailable")?;
        let producer =
            DataStatementProducer::new(&service, &node, &signer, &OsRunnerEntropy, &release)?;
        if args.select_source {
            #[derive(serde::Deserialize)]
            #[serde(rename_all = "camelCase", deny_unknown_fields)]
            struct Request {
                request_id: String,
                expected_revision: u64,
            }
            let r: Request =
                serde_json::from_slice(&bytes).map_err(|_| "reach_data_statement_input_invalid")?;
            let statement = producer.source_selection(r.request_id, r.expected_revision)?;
            println!(
                "{}",
                json!({"selection":statement,"executionAuthorityGranted":false})
            );
            return Ok(());
        }
        #[derive(serde::Deserialize)]
        #[serde(
            tag = "kind",
            rename_all = "snake_case",
            rename_all_fields = "camelCase",
            deny_unknown_fields
        )]
        enum Request {
            Prepare {
                source_revision: u64,
                command: serde_json::Value,
                ttl_ms: u64,
            },
            Retry {
                source_revision: u64,
                envelope: serde_json::Value,
            },
        }
        let r: Request =
            serde_json::from_slice(&bytes).map_err(|_| "reach_data_statement_input_invalid")?;
        let statement = match r {
            Request::Prepare {
                source_revision,
                command,
                ttl_ms,
            } => producer.prepare_command(
                DataRunnerCommand::parse(
                    &serde_json::to_vec(&command)
                        .map_err(|_| "reach_data_statement_input_invalid")?,
                    &release,
                )?,
                source_revision,
                ttl_ms,
            )?,
            Request::Retry {
                source_revision,
                envelope,
            } => producer.retry_preparation(
                tradeassembly_runtime::service::reach_data::ReachDataCommand::parse(
                    &serde_json::to_vec(&envelope)
                        .map_err(|_| "reach_data_statement_input_invalid")?,
                )?,
                source_revision,
            )?,
        };
        println!(
            "{}",
            json!({"preparation":statement,"executionAuthorityGranted":false})
        );
        return Ok(());
    }
    if args.describe_source {
        println!(
            "{}",
            json!({"source":source,"executionAuthorityGranted":false})
        );
        return Ok(());
    }
    if args.command_binding {
        let mut bytes = Vec::new();
        std::io::stdin()
            .take((MAX_DATA_COMMAND_BYTES + 1) as u64)
            .read_to_end(&mut bytes)
            .map_err(|_| "reach_data_command_invalid")?;
        let command = DataRunnerCommand::parse(&bytes, &release)?;
        if command.workspace_id != node.workspace_id {
            return Err("reach_data_workspace_mismatch".into());
        }
        let binding = DurableCoreDataRunner::new(service, release)?.mutation_binding(&command)?;
        if binding.owner_binding_sha256.as_deref() != Some(source.owner_binding_sha256.as_str())
            || binding.core_revision != source.core_revision
        {
            return Err("reach_data_owner_binding_unavailable".into());
        }
        println!(
            "{}",
            json!({"source":source,"binding":binding,"executionAuthorityGranted":false})
        );
        return Ok(());
    }
    let seed =
        read_ed25519_seed_file(&args.node_key_file).map_err(|_| "reach_data_signer_unavailable")?;
    let signer = Arc::new(
        Ed25519InstallationSigner::from_seed(node.node_id.clone(), seed)
            .map_err(|_| "reach_data_signer_unavailable")?,
    );
    let consumer = ReachDataConsumer::new(
        service,
        release,
        node,
        signer,
        Arc::new(HttpReachDataTransport::new(&args.relay_url)?),
        Arc::new(OsRunnerEntropy),
    )?;
    loop {
        match consumer.poll_once() {
            Ok(poll) => {
                let blocked = matches!(poll, ReachDataPoll::Blocked { .. });
                println!(
                    "{}",
                    serde_json::to_string(&poll).map_err(|_| "reach_data_encoding_invalid")?
                );
                if blocked || args.once {
                    return Ok(());
                }
            }
            Err(code)
                if !args.once
                    && matches!(
                        code.as_str(),
                        "reach_data_transport_unavailable" | "reach_data_revision_conflict"
                    ) =>
            {
                eprintln!("{}", json!({"status":"retrying","code":code}))
            }
            Err(code) => return Err(code),
        }
        thread::sleep(Duration::from_millis(args.poll_interval_ms));
    }
}
