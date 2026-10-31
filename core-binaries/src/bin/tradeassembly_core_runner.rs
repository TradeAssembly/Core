// Copyright (c) 2026 OptionLab LLC. All rights reserved.

use clap::Parser;
use serde::Deserialize;
use serde_json::{json, Value};
use std::io::{self, BufRead, Write};
use std::path::{Path, PathBuf};
use std::process;
use tradeassembly_runtime::ports::{
    CorePaperRunnerPort, CoreRunnerBridgePort, PaperRunnerCommand, RunnerBridgeCommand,
    RunnerBridgeError, RunnerBridgeErrorCode,
};
use tradeassembly_runtime::runtime_config::{RuntimeConfig, RuntimeConfigLayer, RuntimeProfile};
use tradeassembly_runtime::service::core_runner::DurableCoreRunnerBridge;
use tradeassembly_runtime::service::data_runner::{DataRunnerCommand, DurableCoreDataRunner};
use tradeassembly_runtime::service::paper_runner::DurableCorePaperRunner;
use tradeassembly_runtime::service::TradeAssemblyService;

const MAX_PROTOCOL_LINE_BYTES: usize = 300 * 1024;
const BUILD_IDENTITY_SCHEMA: &str = "tradeassembly.core-runner.build-identity/v1";
const CORE_REVISION: &str = env!("TRADEASSEMBLY_CORE_REVISION");

#[derive(Debug, Parser)]
#[command(name = "tradeassembly-core-runner")]
struct Args {
    /// Accept only finite data commands; do not resume existing scheduler work.
    #[arg(long)]
    data_only: bool,
    #[arg(long, conflicts_with_all = ["db", "runtime_config", "core_release"])]
    build_identity: bool,
    #[arg(long, conflicts_with_all = ["db", "runtime_config", "core_release", "build_identity"])]
    compatibility: bool,
    #[arg(long, conflicts_with = "runtime_config")]
    db: Option<String>,
    #[arg(long, conflicts_with = "db")]
    runtime_config: Option<PathBuf>,
    #[arg(long, required_unless_present_any = ["build_identity", "compatibility"])]
    core_release: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(tag = "operation", rename_all = "snake_case", deny_unknown_fields)]
enum ProtocolRequest {
    Dispatch {
        command: Box<RunnerBridgeCommand>,
    },
    Receipt {
        workspace_id: String,
        command_id: String,
    },
    DispatchPaper {
        command: Box<PaperRunnerCommand>,
    },
    DispatchData {
        command: Value,
    },
    DataBinding {
        command: Value,
    },
    DataReceipt {
        workspace_id: String,
        command_id: String,
    },
    PaperReceipt {
        workspace_id: String,
        command_id: String,
    },
}

fn main() {
    tradeassembly_runtime::build_identity::install(env!("TRADEASSEMBLY_CORE_REVISION"))
        .expect("compiled Core build identity");
    let args = Args::parse();
    if args.build_identity {
        write_response(&json!({
            "schemaVersion": BUILD_IDENTITY_SCHEMA,
            "coreRevision": CORE_REVISION,
        }))
        .unwrap_or_else(|_| process::exit(1));
        return;
    }
    if args.compatibility {
        let descriptor: Value =
            serde_json::from_str(include_str!("../../../contracts/compatibility.json"))
                .expect("embedded compatibility descriptor");
        write_response(&descriptor).unwrap_or_else(|_| process::exit(1));
        return;
    }
    let core_release = args.core_release.unwrap_or_else(|| process::exit(2));
    let data_only = args.data_only;
    let config = resolve_runner_config(args.db, args.runtime_config.as_deref());
    let factory = if data_only {
        TradeAssemblyService::from_config_data_only
    } else {
        TradeAssemblyService::from_config
    };
    let service = match config.and_then(factory) {
        Ok(service) => service,
        Err(_) => {
            write_response(&error_response(RunnerBridgeError {
                code: RunnerBridgeErrorCode::AdapterFailure,
                message: "runner bridge runtime configuration is invalid".to_string(),
            }))
            .ok();
            process::exit(2);
        }
    };
    let bridge = match DurableCoreRunnerBridge::new(service.clone(), core_release.clone()) {
        Ok(bridge) => bridge,
        Err(error) => {
            write_response(&json!({"ok": false, "error": error})).ok();
            process::exit(2);
        }
    };
    let data_bridge = match DurableCoreDataRunner::new(service.clone(), core_release.clone()) {
        Ok(bridge) => bridge,
        Err(code) => {
            write_response(&data_error_response(code)).ok();
            process::exit(2);
        }
    };
    let data_core_release = core_release.clone();
    let paper_bridge = match DurableCorePaperRunner::new(service, core_release) {
        Ok(bridge) => bridge,
        Err(error) => {
            write_response(&json!({"ok": false, "error": error})).ok();
            process::exit(2);
        }
    };
    let stdin = io::stdin();
    let mut reader = stdin.lock();
    loop {
        let line = match read_bounded_line(&mut reader) {
            Ok(Some(line)) => line,
            Ok(None) => break,
            Err(error) => {
                if write_response(&error_response(error)).is_err() {
                    process::exit(1);
                }
                continue;
            }
        };
        if line.iter().all(u8::is_ascii_whitespace) {
            continue;
        }
        let request = serde_json::from_slice::<ProtocolRequest>(&line);
        if let Ok(request) = &request {
            let data_request = matches!(
                request,
                ProtocolRequest::DispatchData { .. }
                    | ProtocolRequest::DataBinding { .. }
                    | ProtocolRequest::DataReceipt { .. }
            );
            if data_only != data_request {
                let code = if data_only {
                    "data_runner_legacy_operation_forbidden"
                } else {
                    "data_runner_data_only_mode_required"
                };
                if write_response(&data_error_response(code.into())).is_err() {
                    process::exit(1);
                }
                continue;
            }
        }
        let response = match request {
            Ok(ProtocolRequest::Dispatch { command }) => bridge
                .dispatch(command.as_ref())
                .map(|receipt| json!({"ok": true, "receipt": receipt}))
                .unwrap_or_else(error_response),
            Ok(ProtocolRequest::Receipt {
                workspace_id,
                command_id,
            }) => bridge
                .receipt(&workspace_id, &command_id)
                .map(|receipt| json!({"ok": true, "receipt": receipt}))
                .unwrap_or_else(error_response),
            Ok(ProtocolRequest::DispatchPaper { command }) => paper_bridge
                .dispatch(command.as_ref())
                .map(|receipt| json!({"ok": true, "receipt": receipt}))
                .unwrap_or_else(error_response),
            Ok(ProtocolRequest::PaperReceipt {
                workspace_id,
                command_id,
            }) => paper_bridge
                .receipt(&workspace_id, &command_id)
                .map(|receipt| json!({"ok": true, "receipt": receipt}))
                .unwrap_or_else(error_response),
            Ok(ProtocolRequest::DispatchData { command }) => serde_json::to_vec(&command)
                .map_err(|_| "data_runner_command_invalid".to_string())
                .and_then(|bytes| DataRunnerCommand::parse(&bytes, &data_core_release))
                .and_then(|command| data_bridge.dispatch(&command))
                .map(|receipt| json!({"ok":true,"receipt":receipt}))
                .unwrap_or_else(data_error_response),
            Ok(ProtocolRequest::DataBinding { command }) => serde_json::to_vec(&command)
                .map_err(|_| "data_runner_command_invalid".to_string())
                .and_then(|bytes| DataRunnerCommand::parse(&bytes, &data_core_release))
                .and_then(|command| data_bridge.mutation_binding(&command))
                .map(|binding| json!({"ok":true,"binding":binding}))
                .unwrap_or_else(data_error_response),
            Ok(ProtocolRequest::DataReceipt {
                workspace_id,
                command_id,
            }) => data_bridge
                .receipt(&workspace_id, &command_id)
                .map(|receipt| json!({"ok":true,"receipt":receipt}))
                .unwrap_or_else(data_error_response),
            Err(_) => error_response(RunnerBridgeError {
                code: RunnerBridgeErrorCode::MalformedCommand,
                message: "runner bridge protocol request is not valid strict JSON".to_string(),
            }),
        };
        if write_response(&response).is_err() {
            process::exit(1);
        }
    }
}

fn data_error_response(code: String) -> Value {
    json!({"ok":false,"error":{"code":code,"message":"Data-only runner failed closed."}})
}

fn resolve_runner_config(
    database_path: Option<String>,
    runtime_config_path: Option<&Path>,
) -> Result<RuntimeConfig, String> {
    let environment = RuntimeConfigLayer::from_env()?;
    match (database_path, runtime_config_path) {
        (Some(database_path), None) => RuntimeConfig::local_runner(database_path, environment),
        (None, Some(path)) => {
            if !path.is_absolute() || !path.is_file() {
                return Err("runner runtime configuration path is invalid".to_string());
            }
            let path = path
                .to_str()
                .ok_or_else(|| "runner runtime configuration path is invalid".to_string())?;
            let file = RuntimeConfigLayer::from_json_file(path)?;
            let config =
                RuntimeConfig::resolve(Some(file), environment, RuntimeConfigLayer::default())?;
            if config.profile != RuntimeProfile::SelfHosted {
                return Err("runner configured mode requires self_hosted profile".to_string());
            }
            Ok(config)
        }
        _ => Err("exactly one runner runtime mode is required".to_string()),
    }
}

fn read_bounded_line(reader: &mut impl BufRead) -> Result<Option<Vec<u8>>, RunnerBridgeError> {
    let mut line = Vec::new();
    let mut exceeded = false;
    loop {
        let available = reader
            .fill_buf()
            .map_err(|_| protocol_error("runner bridge protocol input failed"))?;
        if available.is_empty() {
            if line.is_empty() && !exceeded {
                return Ok(None);
            }
            break;
        }
        let take = available
            .iter()
            .position(|byte| *byte == b'\n')
            .map_or(available.len(), |position| position + 1);
        if !exceeded && line.len() + take <= MAX_PROTOCOL_LINE_BYTES {
            line.extend_from_slice(&available[..take]);
        } else {
            exceeded = true;
        }
        let ended = available[take - 1] == b'\n';
        reader.consume(take);
        if ended {
            break;
        }
    }
    if exceeded {
        Err(protocol_error(
            "runner bridge protocol request exceeds the byte limit",
        ))
    } else {
        Ok(Some(line))
    }
}

fn write_response(value: &Value) -> io::Result<()> {
    let stdout = io::stdout();
    let mut output = stdout.lock();
    serde_json::to_writer(&mut output, value)?;
    output.write_all(b"\n")?;
    output.flush()
}

fn error_response(error: RunnerBridgeError) -> Value {
    json!({"ok": false, "error": error})
}

fn protocol_error(message: &str) -> RunnerBridgeError {
    RunnerBridgeError {
        code: RunnerBridgeErrorCode::PayloadOutOfBounds,
        message: message.to_string(),
    }
}
