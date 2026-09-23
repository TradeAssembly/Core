// Copyright (c) 2026 OptionLab LLC. All rights reserved.

use crate::{
    auth,
    cli_identity::{CliIdentity, CliIdentityManager},
    demos,
    finance_authority::WardenSidecarAuthority,
    http,
    runtime_config::{EnvSecretResolver, RuntimeConfig, RuntimeConfigLayer, SecretResolver},
    service::{ServiceResponse, TradeAssemblyService},
};
use clap::{Args, Parser, Subcommand};
use serde_json::{json, Value};
use std::fs;
use std::io::{self, BufRead};
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

mod browser_onboarding;
pub mod external_mcp;
pub mod output;

#[derive(Parser)]
#[command(name = "tradeassembly")]
#[command(about = "TradeAssembly local Rust runtime")]
pub struct Cli {
    #[arg(long, global = true)]
    pub db: Option<String>,
    #[arg(long, global = true)]
    pub config: Option<String>,
    #[command(subcommand)]
    pub command: Command,
}

#[derive(Subcommand)]
pub enum Command {
    /// Prepare an account-free local installation using a packaged Warden binary.
    Setup {
        #[arg(long)]
        state_dir: PathBuf,
        /// Defaults to the warden executable beside tradeassembly.
        #[arg(long)]
        warden_binary: Option<PathBuf>,
        #[arg(long, default_value_t = 8181)]
        warden_port: u16,
    },
    /// Run this installation's deterministic policy service.
    Policy {
        #[command(subcommand)]
        command: PolicyCommand,
    },
    Api {
        #[arg(long, default_value = "127.0.0.1")]
        host: String,
        #[arg(long, default_value_t = 8090)]
        port: u16,
    },
    Auth {
        #[command(subcommand)]
        command: AuthCommand,
    },
    Mcp {
        #[command(subcommand)]
        command: McpCommand,
    },
    Strategy {
        #[command(subcommand)]
        command: StrategyCommand,
    },
    Journal {
        #[command(subcommand)]
        command: JournalCommand,
    },
    Providers {
        #[command(subcommand)]
        command: ProviderCommand,
    },
    Credentials {
        #[command(subcommand)]
        command: CredentialCommand,
    },
    Account {
        #[command(subcommand)]
        command: AccountCommand,
    },
    Install {
        #[command(subcommand)]
        command: InstallCommand,
    },
    Telemetry {
        #[command(subcommand)]
        command: TelemetryCommand,
    },
    /// Queue a durable backtest from a complete request JSON file.
    Backtest(BacktestCreateArgs),
    Backtests {
        #[command(subcommand)]
        command: BacktestsCommand,
    },
    Robustness {
        #[command(subcommand)]
        command: RobustnessCommand,
    },
    Derivatives {
        #[command(subcommand)]
        command: DerivativesCommand,
    },
    Comparison {
        #[command(subcommand)]
        command: ComparisonCommand,
    },
    Research {
        #[command(subcommand)]
        command: ResearchCommand,
    },
    DatasetIngestion {
        #[command(subcommand)]
        command: DatasetIngestionCommand,
    },
    Marketdata {
        #[command(subcommand)]
        command: MarketdataCommand,
    },
    Options {
        #[command(subcommand)]
        command: OptionCommand,
    },
    Scheduler {
        #[command(subcommand)]
        command: SchedulerCommand,
    },
    Workers {
        #[command(subcommand)]
        command: WorkerCommand,
    },
    Orders {
        #[command(subcommand)]
        command: OrdersCommand,
    },
    Risk {
        #[command(subcommand)]
        command: RiskCommand,
    },
    Positions,
    Replay,
    Demos {
        #[command(subcommand)]
        command: DemoCommand,
    },
    Demo {
        #[arg(default_value = "local-simbroker-paper-btc")]
        demo_id: String,
    },
    Seed,
    SeedBtcDemo,
    Reset,
    ResetBtcDemo,
    Readiness,
    Report,
    ReportEnvelope {
        #[arg(long, default_value = "backtest")]
        kind: String,
        #[arg(long, default_value = "strat_local_btc_demo")]
        strategy_id: String,
        #[arg(long, default_value = "backtest-local")]
        backtest_id: String,
        #[arg(long, default_value = "checkpoint-local")]
        checkpoint_id: String,
    },
    Plugins {
        #[command(subcommand)]
        command: PluginCommand,
    },
    Instruments {
        #[command(subcommand)]
        command: InstrumentCommand,
    },
    Selectors {
        #[command(subcommand)]
        command: SelectorCommand,
    },
    Scenario {
        #[command(subcommand)]
        command: ScenarioCommand,
    },
    MonteCarlo {
        #[command(subcommand)]
        command: MonteCarloCommand,
    },
    Lifecycle {
        #[command(subcommand)]
        command: LifecycleCommand,
    },
    LifecycleCalendar {
        #[command(subcommand)]
        command: LifecycleCalendarCommand,
    },
    FillQuality {
        #[command(subcommand)]
        command: FillQualityCommand,
    },
    AttributionJournal {
        #[command(subcommand)]
        command: AttributionJournalCommand,
    },
    ResearchNotebook {
        #[command(subcommand)]
        command: ResearchNotebookCommand,
    },
    Execution {
        #[command(subcommand)]
        command: ExecutionCommand,
    },
    Events {
        #[command(subcommand)]
        command: EventCommand,
    },
    Agent {
        #[command(subcommand)]
        command: AgentCommand,
    },
}

#[derive(Subcommand)]
pub enum PolicyCommand {
    /// Run in the foreground; does not activate strategies or start an agent.
    Serve {
        #[arg(long)]
        state_dir: PathBuf,
    },
    /// Manage only the local policy sidecar, not trading or agent activation.
    Launchd {
        #[command(subcommand)]
        command: PolicyLaunchdCommand,
    },
}

#[derive(Subcommand)]
pub enum PolicyLaunchdCommand {
    Install(PolicyLaunchdArgs),
    Verify(PolicyLaunchdArgs),
    Status(PolicyLaunchdArgs),
    /// Remove policy supervision; preserves all local data and broker positions.
    Uninstall(PolicyLaunchdArgs),
}

#[derive(Args)]
pub struct PolicyLaunchdArgs {
    #[arg(long)]
    state_dir: PathBuf,
    #[arg(long)]
    launch_agents_dir: Option<PathBuf>,
}

/// Installation commands must work before runtime secrets or services exist.
pub fn run_installation_command(cli: &Cli) -> Option<i32> {
    let result = match &cli.command {
        Command::Setup {
            state_dir,
            warden_binary,
            warden_port,
        } => {
            let binary = warden_binary.clone().map(Ok).unwrap_or_else(|| {
                std::env::current_exe()
                    .map(|path| path.with_file_name("warden"))
                    .map_err(|_| "installation_executable_unavailable".to_string())
            });
            binary
                .and_then(|binary| crate::local_install::prepare(state_dir, &binary, *warden_port))
        }
        Command::Policy {
            command: PolicyCommand::Launchd { command },
        } => run_policy_launchd(command),
        Command::Policy {
            command: PolicyCommand::Serve { state_dir },
        } => crate::local_install::policy_command(state_dir).and_then(|mut command| {
            #[cfg(unix)]
            {
                use std::os::unix::process::CommandExt;
                let _error = command.exec();
                Err("local_policy_start_failed".to_string())
            }
            #[cfg(not(unix))]
            {
                command
                    .status()
                    .map_err(|_| "local_policy_start_failed".to_string())
                    .and_then(|status| {
                        if status.success() {
                            Ok(json!({"ok":true}))
                        } else {
                            Err("local_policy_failed".to_string())
                        }
                    })
            }
        }),
        _ => return None,
    };
    let (code, payload) = match result {
        Ok(payload) => (0, payload),
        Err(code) => (1, json!({"ok":false,"error":{"code":code}})),
    };
    println!(
        "{}",
        serde_json::to_string_pretty(&payload).expect("installation response JSON")
    );
    Some(code)
}

fn run_policy_launchd(command: &PolicyLaunchdCommand) -> Result<Value, String> {
    use crate::{agent_launchd, warden_launchd};
    let (PolicyLaunchdCommand::Install(args)
    | PolicyLaunchdCommand::Verify(args)
    | PolicyLaunchdCommand::Status(args)
    | PolicyLaunchdCommand::Uninstall(args)) = command;
    if !args.state_dir.is_absolute() {
        return Err("local_install_absolute_state_required".into());
    }
    if matches!(
        command,
        PolicyLaunchdCommand::Install(_) | PolicyLaunchdCommand::Verify(_)
    ) {
        // Validate the exact persisted Warden binary/policy before supervision.
        let _ = crate::local_install::policy_command(&args.state_dir)?;
    }
    let profile = warden_launchd::PolicyLaunchdProfile {
        executable: std::env::current_exe().map_err(|_| "installation_executable_unavailable")?,
        state_dir: args.state_dir.clone(),
    };
    let directory = args
        .launch_agents_dir
        .clone()
        .map(Ok)
        .unwrap_or_else(agent_launchd::default_launch_agents_dir)?;
    let uid = agent_launchd::current_uid()?;
    let launchctl = agent_launchd::SystemLaunchctl;
    let status = match command {
        PolicyLaunchdCommand::Install(_) => {
            warden_launchd::install(&launchctl, &profile, &directory, uid)
        }
        PolicyLaunchdCommand::Verify(_) => {
            warden_launchd::verify(&launchctl, &profile, &directory, uid)
        }
        PolicyLaunchdCommand::Status(_) => {
            warden_launchd::status(&launchctl, &profile, &directory, uid)
        }
        PolicyLaunchdCommand::Uninstall(_) => {
            warden_launchd::uninstall(&launchctl, &profile, &directory, uid)
        }
    }?;
    if matches!(command, PolicyLaunchdCommand::Verify(_)) {
        if status.profile_matches != Some(true) {
            return Err("policy_launchd_profile_mismatch".into());
        }
        if !status.loaded {
            return Err("policy_launchd_not_loaded".into());
        }
    }
    Ok(
        json!({"ok":true,"supervision":status,"scope":"local_policy_sidecar",
        "strategyState":"not_evaluated",
        "message":"This command manages only the local policy process. It does not activate or deactivate strategies, submit orders, or liquidate positions."}),
    )
}

#[derive(Subcommand)]
pub enum AuthCommand {
    Login,
    Status,
    Logout,
    /// Copy the current WorkOS session to Bitwarden, verify it, and retain Keychain.
    MigrateToBitwarden,
}

#[derive(Subcommand)]
pub enum McpCommand {
    Serve {
        #[arg(long, default_value = "stdio")]
        transport: String,
        #[arg(long)]
        studio_base_url: Option<String>,
    },
}

#[derive(Subcommand)]
pub enum StrategyCommand {
    List,
    Get {
        strategy_id: String,
    },
    /// Publish the exact reviewed draft after an explicit owner acknowledgement.
    Publish {
        strategy_id: String,
        #[arg(long)]
        expected_draft_hash: String,
        #[arg(long, default_value_t = false)]
        acknowledge_publication: bool,
        #[arg(long)]
        idempotency_key: String,
    },
    Create {
        #[arg(long, default_value = "blank")]
        mode: String,
        #[arg(long)]
        name: Option<String>,
    },
    Validate,
}

#[derive(Subcommand)]
pub enum JournalCommand {
    /// List journal events visible to the authenticated owner.
    List,
    /// Export owner-scoped journal events as JSON on stdout.
    Export,
    /// Replay owner-scoped journal events and return deterministic counts.
    Replay,
}

#[derive(Subcommand)]
pub enum ProviderCommand {
    List,
    CredentialStatus {
        #[arg(long, default_value = "plugin-instance")]
        provider_ref: String,
    },
    CredentialTest {
        #[arg(long, default_value = "plugin-instance")]
        provider_ref: String,
    },
    Policy,
}

#[derive(Subcommand)]
pub enum CredentialCommand {
    Status {
        #[arg(long, default_value = "plugin-instance")]
        provider_ref: String,
    },
    Test {
        #[arg(long, default_value = "plugin-instance")]
        provider_ref: String,
    },
}

#[derive(Subcommand)]
pub enum AccountCommand {
    Status(ProfileArgs),
    Login(ProfileArgs),
}

#[derive(Subcommand)]
pub enum InstallCommand {
    Claim(InstallClaimArgs),
}

#[derive(Subcommand)]
pub enum TelemetryCommand {
    Emit(TelemetryEmitArgs),
    OptOut(TelemetryOptOutArgs),
    Replay(ProfileArgs),
}

#[derive(Args, Clone)]
pub struct ProfileArgs {
    #[arg(long, default_value = "local")]
    profile: String,
    #[arg(long, default_value = "http://127.0.0.1:3001")]
    studio_base_url: String,
}

#[derive(Args, Clone)]
pub struct InstallClaimArgs {
    #[arg(long, default_value = "local")]
    profile: String,
    #[arg(long)]
    install_id: Option<String>,
    #[arg(long, default_value = "http://127.0.0.1:3001")]
    studio_base_url: String,
}

#[derive(Args, Clone)]
pub struct TelemetryEmitArgs {
    #[arg(long, default_value = "local")]
    profile: String,
    #[arg(long, default_value = "first_backtest")]
    event: String,
    #[arg(long)]
    install_id: Option<String>,
    #[arg(long, default_value = "http://127.0.0.1:3001")]
    studio_base_url: String,
}

#[derive(Args, Clone)]
pub struct TelemetryOptOutArgs {
    #[arg(long, default_value = "local")]
    profile: String,
    telemetry_opt_out: String,
    #[arg(long, default_value = "http://127.0.0.1:3001")]
    studio_base_url: String,
}

#[derive(Subcommand)]
pub enum ResearchCommand {
    Universe,
    Jobs,
    Sweep,
}

#[derive(Subcommand)]
pub enum DatasetIngestionCommand {
    Create(DatasetIngestionCreateArgs),
    List,
    Get { ingestion_id: String },
    Status { ingestion_id: String },
    Cancel { ingestion_id: String },
    Verify { ingestion_id: String },
}

#[derive(Subcommand)]
pub enum BacktestsCommand {
    Create(BacktestCreateArgs),
    Get {
        run_id: String,
    },
    List,
    Cancel(BacktestRunRequestArgs),
    Retry(BacktestRunRequestArgs),
    Process {
        #[arg(long, default_value = "local-backtest-worker")]
        worker: String,
        #[arg(long)]
        run_id: Option<String>,
    },
    Replay {
        run_id: String,
    },
    Report {
        run_id: String,
    },
    Export {
        run_id: String,
        #[arg(long, default_value = "reportJson")]
        export_kind: String,
    },
}

#[derive(Subcommand)]
pub enum RobustnessCommand {
    Run(RobustnessCreateArgs),
    Create(RobustnessCreateArgs),
    List,
    Get {
        run_id: String,
    },
    #[command(name = "get-report", visible_alias = "report")]
    GetReport {
        run_id: String,
    },
    Replay(RobustnessReplayArgs),
    Export(RobustnessExportArgs),
    Process {
        #[arg(long, default_value = "local-robustness-worker")]
        worker: String,
    },
    Cancel(RobustnessRunRequestArgs),
    Retry(RobustnessRunRequestArgs),
}

#[derive(Subcommand)]
pub enum DerivativesCommand {
    Create(DerivativesCreateArgs),
    List,
    Get {
        analysis_id: String,
    },
    Export {
        analysis_id: String,
        #[arg(long, value_parser = ["json", "csv"])]
        format: String,
    },
}

#[derive(Args)]
pub struct DerivativesCreateArgs {
    #[arg(long)]
    request_file: PathBuf,
}

#[derive(Subcommand)]
pub enum ComparisonCommand {
    Create(ComparisonCreateArgs),
    List,
    Get {
        comparison_id: String,
    },
    Export {
        comparison_id: String,
        #[arg(long, value_parser = ["json", "csv"])]
        format: String,
    },
}

#[derive(Args)]
pub struct ComparisonCreateArgs {
    #[arg(long)]
    request_file: PathBuf,
}

#[derive(Args)]
pub struct BacktestCreateArgs {
    #[arg(long)]
    request_file: PathBuf,
}

#[derive(Args, Clone)]
pub struct RobustnessCreateArgs {
    #[arg(long, conflicts_with = "source_run_id")]
    request_file: Option<PathBuf>,
    #[arg(long)]
    source_run_id: Option<String>,
    #[arg(long, requires = "source_run_id")]
    study_kind: Option<String>,
    #[arg(long, requires = "source_run_id")]
    assumptions_json: Option<String>,
    #[arg(long, requires = "source_run_id")]
    maximum_samples: Option<u32>,
    #[arg(long, requires = "source_run_id")]
    maximum_grid_points: Option<u32>,
    #[arg(long, requires = "source_run_id")]
    maximum_windows: Option<u32>,
    #[arg(long, requires = "source_run_id")]
    maximum_scenarios: Option<u32>,
    #[arg(long, requires = "source_run_id")]
    maximum_output_bytes: Option<u64>,
    #[arg(long, requires = "source_run_id")]
    maximum_attempts: Option<u32>,
    #[arg(long, requires = "source_run_id")]
    deterministic_seed: Option<u64>,
    #[arg(long, requires = "source_run_id")]
    idempotency_key: Option<String>,
}

#[derive(Args)]
pub struct BacktestRunRequestArgs {
    pub run_id: String,
    #[arg(long)]
    request_file: Option<PathBuf>,
}

#[derive(Args, Clone)]
pub struct RobustnessRunRequestArgs {
    pub run_id: String,
    #[arg(long)]
    request_file: Option<PathBuf>,
}

#[derive(Args)]
pub struct RobustnessReplayArgs {
    pub run_id: String,
    #[arg(long)]
    idempotency_key: String,
    #[arg(long, default_value = "local-user")]
    actor: String,
}

#[derive(Args)]
pub struct RobustnessExportArgs {
    pub run_id: String,
    #[arg(long, value_parser = ["json", "csv"])]
    format: String,
    #[arg(long)]
    idempotency_key: String,
    #[arg(long, default_value = "local-user")]
    actor: String,
}

#[derive(Args)]
pub struct DatasetIngestionCreateArgs {
    #[arg(
        long,
        conflicts_with = "request_file",
        required_unless_present = "request_file"
    )]
    request_json: Option<String>,
    #[arg(
        long,
        conflicts_with = "request_json",
        required_unless_present = "request_json"
    )]
    request_file: Option<PathBuf>,
}

#[derive(Subcommand)]
pub enum MarketdataCommand {
    Bars,
    Quote,
    Indicators,
    OptionChain,
    OptionSelect,
}

#[derive(Subcommand)]
pub enum OptionCommand {
    Chain,
    Select,
}

#[derive(Subcommand)]
pub enum SchedulerCommand {
    Status,
    Start,
    Run,
    Stop,
}

#[derive(Subcommand)]
pub enum WorkerCommand {
    Status,
    Run,
}

#[derive(Subcommand)]
pub enum OrdersCommand {
    Reconcile,
    Attempts,
}

#[derive(Subcommand)]
pub enum RiskCommand {
    Status,
    AccountSet,
    Stress,
    Overlay(RiskOverlayArgs),
}

#[derive(Subcommand)]
pub enum DemoCommand {
    List,
    Run { demo_id: String },
}

#[derive(Subcommand)]
pub enum PluginCommand {
    Status,
    InstallDefault(PluginDefaultInstallArgs),
    Install(PluginPackageArgs),
    Create(PluginCreateArgs),
    Configure(PluginConfigureArgs),
    Enable { instance_ref: String },
    Disable { instance_ref: String },
    RefreshHealth { instance_ref: String },
    Upgrade(PluginPackageInstanceArgs),
    Rollback { instance_ref: String },
    Remove { instance_ref: String },
    Invoke(PluginInvokeArgs),
}

#[derive(Args)]
pub struct PluginDefaultInstallArgs {
    #[arg(long)]
    package_source: Option<String>,
    #[arg(long, default_value_t = false)]
    offline: bool,
    #[arg(long, default_value_t = false)]
    skip_default_plugins: bool,
}

#[derive(Args)]
pub struct PluginPackageArgs {
    #[arg(long)]
    source: String,
    #[arg(long)]
    package_sha256: String,
    #[arg(long)]
    manifest_sha256: String,
    #[arg(long, default_value_t = false)]
    offline: bool,
}

#[derive(Args)]
pub struct PluginCreateArgs {
    #[arg(long)]
    plugin_ref: String,
    #[arg(long)]
    instance_ref: String,
    #[arg(long)]
    provider_ref: Option<String>,
}

#[derive(Args)]
pub struct PluginConfigureArgs {
    instance_ref: String,
    #[arg(long)]
    configuration_file: PathBuf,
}

#[derive(Args)]
pub struct PluginPackageInstanceArgs {
    instance_ref: String,
    #[command(flatten)]
    package: PluginPackageArgs,
}

#[derive(Args)]
pub struct PluginInvokeArgs {
    instance_ref: String,
    operation_id: String,
    #[arg(long)]
    request_file: PathBuf,
}

#[derive(Subcommand)]
pub enum InstrumentCommand {
    Resolve,
    Aliases,
    PackStatus,
}

#[derive(Subcommand)]
pub enum SelectorCommand {
    Evaluate,
    Explain,
}

#[derive(Subcommand)]
pub enum ScenarioCommand {
    Valuation {
        #[arg(long, default_value = "strat_local_btc_demo")]
        strategy_id: String,
        #[arg(long, default_value = "checkpoint-local")]
        checkpoint_id: String,
    },
}

#[derive(Subcommand)]
pub enum MonteCarloCommand {
    Run(MonteCarloArgs),
    Status(MonteCarloArgs),
    Report(MonteCarloArgs),
    Replay(MonteCarloArgs),
    Export(MonteCarloArgs),
}

#[derive(Args, Clone)]
pub struct MonteCarloArgs {
    #[arg(long, default_value = "strat_local_btc_demo")]
    strategy_id: String,
    #[arg(long)]
    assumption_hash: Option<String>,
}

#[derive(Args, Clone)]
pub struct RiskOverlayArgs {
    #[arg(long, default_value = "strat_local_btc_demo")]
    strategy_id: String,
    #[arg(long, default_value = "strategy")]
    scope_kind: String,
}

#[derive(Subcommand)]
pub enum LifecycleCommand {
    Import,
    Inspect,
    Status,
    Replay,
}

#[derive(Subcommand)]
pub enum LifecycleCalendarCommand {
    List(LifecycleCalendarArgs),
    Inspect(LifecycleCalendarArgs),
    Export(LifecycleCalendarArgs),
    Replay(LifecycleCalendarArgs),
}

#[derive(Args, Clone)]
pub struct LifecycleCalendarArgs {
    #[arg(long, default_value = "strat_local_btc_demo")]
    strategy_id: String,
    #[arg(long, default_value = "lifecycle_calendar_latest")]
    timeline_id: String,
}

#[derive(Subcommand)]
pub enum FillQualityCommand {
    Inspect(FillQualityArgs),
    Report(FillQualityArgs),
    Replay(FillQualityArgs),
    Export(FillQualityArgs),
}

#[derive(Args, Clone)]
pub struct FillQualityArgs {
    #[arg(long, default_value = "strat_local_btc_demo")]
    strategy_id: String,
    #[arg(long, default_value = "fill_quality_latest")]
    analysis_id: String,
    #[arg(long)]
    benchmark_ref: Option<String>,
    #[arg(long)]
    benchmark_price: Option<f64>,
}

#[derive(Subcommand)]
pub enum AttributionJournalCommand {
    Review(AttributionJournalArgs),
    Report(AttributionJournalArgs),
    Inspect(AttributionJournalArgs),
    Replay(AttributionJournalArgs),
    Export(AttributionJournalArgs),
}

#[derive(Args, Clone)]
pub struct AttributionJournalArgs {
    #[arg(long, default_value = "strat_local_btc_demo")]
    strategy_id: String,
    #[arg(long, default_value = "attribution_journal_latest")]
    review_id: String,
}

#[derive(Subcommand)]
pub enum ResearchNotebookCommand {
    Create(ResearchNotebookArgs),
    Compose(ResearchNotebookArgs),
    Attach(ResearchNotebookArgs),
    List(ResearchNotebookArgs),
    Inspect(ResearchNotebookArgs),
    Export(ResearchNotebookArgs),
    Replay(ResearchNotebookArgs),
}

#[derive(Args, Clone)]
pub struct ResearchNotebookArgs {
    #[arg(long, default_value = "strat_local_btc_demo")]
    strategy_id: String,
    #[arg(long)]
    notebook_id: Option<String>,
    #[arg(long = "artifact-selector", value_parser = parse_artifact_selector)]
    artifact_selectors: Vec<String>,
    #[arg(long)]
    title: Option<String>,
}

#[derive(Subcommand)]
pub enum ExecutionCommand {
    Run,
    /// Manage owner-approved local Live authority; does not activate trading.
    LiveMandate {
        #[command(subcommand)]
        command: LiveMandateCommand,
    },
    ConfigSave(ExecutionConfigArgs),
    Readiness(ExecutionReadinessArgs),
    Activate(ExecutionActivateArgs),
    Control(ExecutionControlArgs),
    Deactivate(ExecutionActivationArgs),
    Status(ExecutionStatusArgs),
}

#[derive(Subcommand)]
pub enum LiveMandateCommand {
    /// Approve an exact configuration and optional named agent deployment.
    Issue {
        #[arg(long)]
        config_id: String,
        #[arg(long)]
        expires_at_ms: i64,
        #[arg(long)]
        delegate_deployment_id: Option<String>,
        #[arg(long)]
        idempotency_key: String,
    },
    /// Revoke future use of a mandate; does not liquidate positions.
    Revoke {
        #[arg(long)]
        mandate_id: String,
        #[arg(long)]
        idempotency_key: String,
    },
    /// Check the mandate against current configuration and credentials.
    Status {
        #[arg(long)]
        mandate_id: String,
    },
}

#[derive(Args, Clone)]
pub struct ExecutionConfigArgs {
    #[arg(long, default_value = "strat_local_btc_demo")]
    strategy_id: String,
    #[arg(long, default_value = "paper")]
    mode: String,
    #[arg(long, default_value = "sim")]
    provider_ref: String,
}

#[derive(Args, Clone)]
pub struct ExecutionReadinessArgs {
    #[arg(long)]
    config_id: String,
    #[arg(long, default_value = "strat_local_btc_demo")]
    strategy_id: String,
}

#[derive(Args, Clone)]
pub struct ExecutionActivateArgs {
    #[arg(long)]
    config_id: String,
    #[arg(long, default_value = "strat_local_btc_demo")]
    strategy_id: String,
    #[arg(long)]
    idempotency_key: String,
    #[arg(long)]
    local_live_mandate_id: Option<String>,
}

#[derive(Args, Clone)]
pub struct ExecutionControlArgs {
    #[arg(long)]
    activation_id: String,
    #[arg(long)]
    action: String,
    #[arg(long)]
    idempotency_key: Option<String>,
}

#[derive(Args, Clone)]
pub struct ExecutionActivationArgs {
    #[arg(long)]
    activation_id: String,
}

#[derive(Args, Clone)]
pub struct ExecutionStatusArgs {
    #[arg(long, default_value = "strat_local_btc_demo")]
    strategy_id: String,
    #[arg(long)]
    activation_id: Option<String>,
    #[arg(long, default_value_t = 0)]
    after_sequence: u64,
    #[arg(long, default_value_t = 100)]
    event_limit: usize,
}

#[derive(Subcommand)]
pub enum EventCommand {
    Publish,
    Pull,
}

#[derive(Subcommand)]
pub enum AgentCommand {
    /// Install the local agent supervisor as the current macOS user's LaunchAgent.
    Install(AgentLaunchdInstallArgs),
    /// Verify the rendered local LaunchAgent and its launchctl state.
    Verify(AgentLaunchdProfileArgs),
    /// Read the current macOS user's LaunchAgent state without changing it.
    Status(AgentLaunchdProfileArgs),
    /// Stop and remove the current macOS user's local LaunchAgent.
    Uninstall(AgentLaunchdProfileArgs),
    Create {
        #[arg(long)]
        deployment_file: PathBuf,
    },
    List,
    Start {
        deployment_id: String,
    },
    Pause {
        deployment_id: String,
    },
    /// Clear a quarantined agent run only after its deterministic evidence was reconciled.
    Recover {
        deployment_id: String,
        #[arg(long, default_value_t = false)]
        acknowledge_reconciled: bool,
    },
    /// Run one non-interactive all-active local supervisor pass. Suitable for launchd.
    Run {
        #[arg(long, default_value = "local-agent-runner")]
        runner_id: String,
        #[arg(long, default_value_t = false)]
        once: bool,
    },
}

#[derive(Args, Clone)]
pub struct AgentLaunchdLocationArgs {
    /// Override ~/Library/LaunchAgents. Intended for controlled local environments.
    #[arg(long)]
    launch_agents_dir: Option<PathBuf>,
}

#[derive(Args, Clone)]
pub struct AgentLaunchdProfileArgs {
    #[command(flatten)]
    location: AgentLaunchdLocationArgs,
    /// Absolute TradeAssembly executable path. Defaults to the executing binary.
    #[arg(long)]
    tradeassembly_bin: Option<PathBuf>,
    /// Absolute Codex executable path. Defaults to the executable resolved from this shell's PATH.
    #[arg(long)]
    codex_bin: Option<PathBuf>,
    #[arg(long, default_value = "local-launchd")]
    runner_id: String,
}

#[derive(Args, Clone)]
pub struct AgentLaunchdInstallArgs {
    #[command(flatten)]
    profile: AgentLaunchdProfileArgs,
}

pub async fn run_with_service(
    cli: Cli,
    service: &TradeAssemblyService,
    config: &RuntimeConfig,
) -> i32 {
    let identity_manager = CliIdentityManager::new(config.clone());
    match cli.command {
        Command::Setup { .. } | Command::Policy { .. } => {
            eprintln!("installation commands must run before service initialization");
            2
        }
        Command::Api { host, port } => {
            if let Err(error) = http::serve(service.clone(), &host, port).await {
                eprintln!("{error}");
                1
            } else {
                0
            }
        }
        Command::Mcp {
            command:
                McpCommand::Serve {
                    transport,
                    studio_base_url,
                },
        } => {
            if transport != "stdio" {
                eprintln!("only stdio MCP transport is supported in local Rust runtime");
                return 2;
            }
            let bootstrap = crate::identity_bootstrap::IdentityBootstrap::new(config.clone());
            serve_mcp_stdio(
                Some(service),
                &identity_manager,
                &bootstrap,
                resolved_mcp_studio_url(studio_base_url.as_deref(), config),
            );
            0
        }
        Command::Auth { command } => {
            let response = match command {
                AuthCommand::MigrateToBitwarden => {
                    match crate::workos_identity::WorkosAuthManager::new(config.clone())
                        .migrate_to_bitwarden()
                        .await
                    {
                        Ok(()) => {
                            json!({"ok":true,"copied":true,"verified":true,"legacyEntryPreserved":true,"nextConfiguration":{"oidcSessionStore":"bitwarden"}})
                        }
                        Err(error) => json!({"ok":false,"error":{"code":error}}),
                    }
                }
                AuthCommand::Login => match identity_manager.login().await {
                    Ok(identity) => identity.redacted_status(),
                    Err(error) => json!({"authenticated": false, "error": {"code": error}}),
                },
                AuthCommand::Status => identity_manager.status().await,
                AuthCommand::Logout => match identity_manager.logout() {
                    Ok(()) => json!({"authenticated": false, "loggedOut": true}),
                    Err(error) => json!({"authenticated": false, "error": {"code": error}}),
                },
            };
            let failed = response.get("error").is_some();
            println!(
                "{}",
                serde_json::to_string_pretty(&response).expect("auth response JSON")
            );
            i32::from(failed)
        }
        Command::Readiness => {
            let response = execute_command(service, Command::Readiness);
            let failed = command_response_failed(&response);
            let output = if failed {
                output::failure(response, "readiness")
            } else {
                output::success(response, "readiness")
            };
            println!(
                "{}",
                serde_json::to_string_pretty(&output).expect("readiness response JSON")
            );
            i32::from(failed)
        }
        Command::Backtest(args) => {
            let authenticated =
                match require_authenticated_service(service, &identity_manager).await {
                    Ok(service) => service,
                    Err(code) => return print_authentication_error(&code),
                };
            let response =
                execute_backtests_command(&authenticated, BacktestsCommand::Create(args));
            println!(
                "{}",
                serde_json::to_string_pretty(&response.body).expect("service response JSON")
            );
            i32::from(response.status >= 400)
        }
        Command::Backtests { command } => {
            let authenticated =
                match require_authenticated_service(service, &identity_manager).await {
                    Ok(service) => service,
                    Err(code) => return print_authentication_error(&code),
                };
            let response = execute_backtests_command(&authenticated, command);
            println!(
                "{}",
                serde_json::to_string_pretty(&response.body).expect("service response JSON")
            );
            i32::from(response.status >= 400)
        }
        Command::Robustness { command } => {
            let authenticated =
                match require_authenticated_service(service, &identity_manager).await {
                    Ok(service) => service,
                    Err(code) => return print_authentication_error(&code),
                };
            let response = execute_robustness_command(&authenticated, command);
            println!(
                "{}",
                serde_json::to_string_pretty(&response.body).expect("service response JSON")
            );
            i32::from(response.status >= 400)
        }
        Command::Derivatives { command } => {
            let authenticated =
                match require_authenticated_service(service, &identity_manager).await {
                    Ok(service) => service,
                    Err(code) => return print_authentication_error(&code),
                };
            let response = execute_derivatives_command(&authenticated, command);
            println!(
                "{}",
                serde_json::to_string_pretty(&response.body).expect("service response JSON")
            );
            i32::from(response.status >= 400)
        }
        Command::Comparison { command } => {
            let authenticated =
                match require_authenticated_service(service, &identity_manager).await {
                    Ok(service) => service,
                    Err(code) => return print_authentication_error(&code),
                };
            let response = execute_comparison_command(&authenticated, command);
            println!(
                "{}",
                serde_json::to_string_pretty(&response.body).expect("service response JSON")
            );
            i32::from(response.status >= 400)
        }
        Command::Agent {
            command: AgentCommand::Install(args),
        } => print_launchd_response(agent_launchd_install(
            config,
            cli.config.as_deref(),
            args.profile,
        )),
        Command::Agent {
            command: AgentCommand::Verify(args),
        } => print_launchd_response(agent_launchd_verify(config, cli.config.as_deref(), args)),
        Command::Agent {
            command: AgentCommand::Status(args),
        } => print_launchd_response(agent_launchd_status(config, cli.config.as_deref(), args)),
        Command::Agent {
            command: AgentCommand::Uninstall(args),
        } => print_launchd_response(agent_launchd_uninstall(config, cli.config.as_deref(), args)),
        command => {
            let authenticated =
                match require_authenticated_service(service, &identity_manager).await {
                    Ok(service) => service,
                    Err(code) => return print_authentication_error(&code),
                };
            let command_name = command_name(&command);
            let response = if matches!(
                command,
                Command::Agent {
                    command: AgentCommand::Run { .. }
                }
            ) {
                let adapter = launchd_config_path(config, cli.config.as_deref()).and_then(|path| {
                    let executable = std::env::current_exe()
                        .map_err(|_| "agent_mcp_binding_path_invalid".to_string())?;
                    let database = absolute_path(PathBuf::from(authenticated.db()))?;
                    crate::agent_runner::CodexCliAdapter::new(&executable, &path, &database)
                });
                match adapter {
                    Ok(adapter) => {
                        execute_command_with_agent(&authenticated, command, Some(&adapter))
                    }
                    Err(code) => json!({"ok": false, "error": {"code": code}}),
                }
            } else {
                execute_command(&authenticated, command)
            };
            let failed = command_response_failed(&response);
            let output = if failed {
                output::failure(response, command_name)
            } else {
                output::success(response, command_name)
            };
            println!(
                "{}",
                serde_json::to_string_pretty(&output).expect("CLI output JSON")
            );
            i32::from(failed)
        }
    }
}

fn print_launchd_response(response: Result<Value, String>) -> i32 {
    let (body, failed) = launchd_output(response);
    println!(
        "{}",
        serde_json::to_string_pretty(&body).expect("launchd lifecycle response JSON")
    );
    i32::from(failed)
}

fn launchd_output(response: Result<Value, String>) -> (Value, bool) {
    match response {
        Ok(status) => (output::success(json!({"launchd": status}), "agent"), false),
        Err(code) => (
            output::failure(json!({"error": {"code": code}}), "agent"),
            true,
        ),
    }
}

fn agent_launchd_install(
    config: &RuntimeConfig,
    config_path: Option<&str>,
    args: AgentLaunchdProfileArgs,
) -> Result<Value, String> {
    let (profile, directory, uid) = agent_launchd_context(config, config_path, args)?;
    serde_json::to_value(crate::agent_launchd::install(
        &crate::agent_launchd::SystemLaunchctl,
        &profile,
        &directory,
        uid,
    )?)
    .map_err(|_| "launchd_response_encode_failed".to_string())
}

fn agent_launchd_verify(
    config: &RuntimeConfig,
    config_path: Option<&str>,
    args: AgentLaunchdProfileArgs,
) -> Result<Value, String> {
    let (profile, directory, uid) = agent_launchd_context(config, config_path, args)?;
    let status = crate::agent_launchd::verify(
        &crate::agent_launchd::SystemLaunchctl,
        &profile,
        &directory,
        uid,
    )?;
    if !status.loaded || status.profile_matches != Some(true) {
        return Err("launchd_profile_not_running".into());
    }
    serde_json::to_value(status).map_err(|_| "launchd_response_encode_failed".to_string())
}

fn agent_launchd_status(
    config: &RuntimeConfig,
    config_path: Option<&str>,
    args: AgentLaunchdProfileArgs,
) -> Result<Value, String> {
    let (profile, directory, uid) = agent_launchd_context(config, config_path, args)?;
    serde_json::to_value(crate::agent_launchd::status(
        &crate::agent_launchd::SystemLaunchctl,
        &profile,
        &directory,
        uid,
    )?)
    .map_err(|_| "launchd_response_encode_failed".to_string())
}

fn agent_launchd_uninstall(
    config: &RuntimeConfig,
    config_path: Option<&str>,
    args: AgentLaunchdProfileArgs,
) -> Result<Value, String> {
    let (profile, directory, uid) = agent_launchd_context(config, config_path, args)?;
    serde_json::to_value(crate::agent_launchd::uninstall(
        &crate::agent_launchd::SystemLaunchctl,
        &profile,
        &directory,
        uid,
    )?)
    .map_err(|_| "launchd_response_encode_failed".to_string())
}

fn agent_launchd_context(
    config: &RuntimeConfig,
    config_path: Option<&str>,
    args: AgentLaunchdProfileArgs,
) -> Result<(crate::agent_launchd::LaunchdProfile, PathBuf, u32), String> {
    let runtime_config_path = launchd_config_path(config, config_path)?;
    let database_path = config
        .database_path
        .as_deref()
        .ok_or_else(|| "launchd_database_path_required".to_string())?;
    let database_path = absolute_path(PathBuf::from(database_path))?;
    let executable = match args.tradeassembly_bin {
        Some(path) => absolute_path(path)?,
        None => std::env::current_exe().map_err(|_| "launchd_tradeassembly_binary_unavailable")?,
    };
    let codex_executable = match args.codex_bin {
        Some(path) => absolute_path(path)?,
        None => resolve_executable_from_path("codex")
            .ok_or_else(|| "launchd_codex_binary_unavailable".to_string())?,
    };
    let codex_executable = crate::agent_runner::resolve_codex_executable(&codex_executable)?;
    let log_directory = database_path
        .parent()
        .ok_or_else(|| "launchd_database_parent_required".to_string())?
        .to_path_buf();
    let directory = launch_agents_dir(args.location.launch_agents_dir)?;
    Ok((
        crate::agent_launchd::LaunchdProfile {
            executable,
            codex_executable,
            database_path,
            runtime_config_path,
            runner_id: args.runner_id,
            log_directory,
        },
        directory,
        crate::agent_launchd::current_uid()?,
    ))
}

fn launchd_config_path(
    config: &RuntimeConfig,
    config_path: Option<&str>,
) -> Result<PathBuf, String> {
    let path = config_path.ok_or_else(|| "launchd_runtime_config_required".to_string())?;
    let path = absolute_path(PathBuf::from(path))?;
    let file = RuntimeConfigLayer::from_json_file(
        path.to_str()
            .ok_or_else(|| "launchd_runtime_config_path_invalid".to_string())?,
    )
    .map_err(|_| "launchd_runtime_config_invalid".to_string())?;
    let durable = RuntimeConfig::resolve(
        Some(file),
        RuntimeConfigLayer::default(),
        RuntimeConfigLayer {
            database_path: config.database_path.clone(),
            ..RuntimeConfigLayer::default()
        },
    )
    .map_err(|_| "launchd_runtime_config_invalid".to_string())?;
    if durable != *config {
        return Err("launchd_runtime_config_has_shell_overrides".into());
    }
    for reference in [
        Some(durable.warden_token_ref.as_str()),
        durable.oidc_client_secret_ref.as_deref(),
        durable.studio_core_token_ref.as_deref(),
        durable.postgres_url_ref.as_deref(),
        durable.nats_url_ref.as_deref(),
        durable.object_store_credential_ref.as_deref(),
    ]
    .into_iter()
    .flatten()
    {
        if reference.starts_with("env://") {
            return Err("launchd_environment_secret_not_durable".into());
        }
        if reference
            .strip_prefix("file://")
            .is_some_and(|path| !std::path::Path::new(path).is_absolute())
        {
            return Err("launchd_secret_path_must_be_absolute".into());
        }
    }
    path.canonicalize()
        .map_err(|_| "launchd_runtime_config_path_invalid".to_string())
}

fn resolve_executable_from_path(name: &str) -> Option<PathBuf> {
    let paths = std::env::var_os("PATH")?;
    std::env::split_paths(&paths)
        .map(|directory| directory.join(name))
        .find(|candidate| candidate.is_file())
}

fn launch_agents_dir(override_path: Option<PathBuf>) -> Result<PathBuf, String> {
    match override_path {
        Some(path) => absolute_path(path),
        None => crate::agent_launchd::default_launch_agents_dir(),
    }
}

fn absolute_path(path: PathBuf) -> Result<PathBuf, String> {
    if path.is_absolute() {
        Ok(path)
    } else {
        std::env::current_dir()
            .map(|directory| directory.join(path))
            .map_err(|_| "launchd_current_directory_unavailable".to_string())
    }
}

async fn require_authenticated_service(
    service: &TradeAssemblyService,
    manager: &CliIdentityManager,
) -> Result<TradeAssemblyService, String> {
    let identity = manager.current_identity().await?;
    register_verified_identity(service, &identity)?;
    Ok(authenticated_service(service, &identity))
}

fn register_verified_identity(
    service: &TradeAssemblyService,
    identity: &CliIdentity,
) -> Result<(), String> {
    service
        .register_verified_invocation_identity(
            &identity.stable_identity_id,
            &crate::ports::IdentityClaims {
                issuer: identity.issuer.clone(),
                subject: identity.subject.clone(),
                audience: identity.audience.clone(),
                assurance: identity.assurance.clone(),
                expires_at_ms: identity.expires_at_ms,
            },
            chrono::Utc::now().timestamp_millis(),
        )
        .map_err(|_| "oidc_session_required".to_string())
}

fn authenticated_service(
    service: &TradeAssemblyService,
    identity: &CliIdentity,
) -> TradeAssemblyService {
    service.for_authenticated_invocation(
        identity.issuer.clone(),
        identity.stable_identity_id.clone(),
        identity.email.clone(),
        identity.display_name.clone(),
    )
}

fn print_authentication_error(code: &str) -> i32 {
    println!(
        "{}",
        serde_json::to_string_pretty(
            &json!({"ok": false, "error": {"code": code, "message": "OIDC login is required."}})
        )
        .expect("authentication error JSON")
    );
    1
}

fn command_response_failed(response: &Value) -> bool {
    response.get("error").is_some_and(|error| !error.is_null())
        || response.get("ok").and_then(Value::as_bool) == Some(false)
        || response.get("status").and_then(Value::as_str) == Some("failed")
}

pub fn service_from_cli(cli: &Cli) -> Result<TradeAssemblyService, String> {
    let service = TradeAssemblyService::from_config_without_local_scheduler_resume(
        runtime_config_from_cli(cli)?,
    )?;
    if cli_owns_local_scheduler(&cli.command) {
        service.resume_local_scheduler_workers();
    }
    Ok(service)
}

fn cli_owns_local_scheduler(command: &Command) -> bool {
    matches!(command, Command::Api { .. })
}

pub fn runtime_config_from_cli(cli: &Cli) -> Result<RuntimeConfig, String> {
    let file = cli
        .config
        .as_deref()
        .map(RuntimeConfigLayer::from_json_file)
        .transpose()?;
    let mut environment = RuntimeConfigLayer::from_env()?;
    // Only opt an unconfigured fresh installation into local ownership. Existing
    // configuration files and databases retain their previous identity semantics.
    if file.is_none()
        && environment.oidc_profile.is_none()
        && environment.oidc_issuer.is_none()
        && environment.oidc_client_id.is_none()
        && environment
            .profile
            .as_deref()
            .is_none_or(|profile| profile == "local")
    {
        let database = cli
            .db
            .as_deref()
            .or(environment.database_path.as_deref())
            .unwrap_or(".tradeassembly/tradeassembly.db");
        let mut owner_directory = std::ffi::OsString::from(database);
        owner_directory.push(".local-owner");
        let owner_record = PathBuf::from(owner_directory).join("local-owner.json");
        if database != ":memory:"
            && (!std::path::Path::new(database).exists() || owner_record.exists())
        {
            environment.oidc_profile = Some("local_owner".to_string());
        }
    }
    let command_line = RuntimeConfigLayer {
        database_path: cli.db.clone(),
        ..RuntimeConfigLayer::default()
    };
    let config = RuntimeConfig::resolve(file, environment, command_line)?;
    if matches!(&cli.command, Command::Api { .. }) {
        preflight_api_warden(&config)?;
    }
    Ok(config)
}

fn preflight_api_warden(config: &RuntimeConfig) -> Result<(), String> {
    WardenSidecarAuthority::new(
        &config.warden_sidecar_url,
        EnvSecretResolver.resolve(&config.warden_token_ref)?,
        &config.warden_required_version,
    )?
    .preflight()
}

/// Keep identity/setup tools available when local service dependencies need repair.
/// No service-backed operation can execute in this mode.
pub fn run_setup_only_mcp(cli: &Cli, config: &RuntimeConfig) -> Option<i32> {
    let Command::Mcp {
        command: McpCommand::Serve {
            transport,
            studio_base_url,
        },
    } = &cli.command
    else {
        return None;
    };
    if transport != "stdio" {
        return None;
    }
    let manager = CliIdentityManager::new(config.clone());
    let bootstrap = crate::identity_bootstrap::IdentityBootstrap::new(config.clone());
    serve_mcp_stdio(
        None,
        &manager,
        &bootstrap,
        resolved_mcp_studio_url(studio_base_url.as_deref(), config),
    );
    Some(0)
}

fn resolved_mcp_studio_url<'a>(explicit: Option<&'a str>, config: &'a RuntimeConfig) -> &'a str {
    explicit
        .filter(|url| !url.trim().is_empty())
        .or(config.studio_base_url.as_deref())
        .unwrap_or("http://127.0.0.1:3000")
}

fn serve_mcp_stdio(
    service: Option<&TradeAssemblyService>,
    identity_manager: &CliIdentityManager,
    bootstrap: &crate::identity_bootstrap::IdentityBootstrap,
    studio_base_url: &str,
) {
    let onboarding = service
        .map(|service| browser_onboarding::BrowserOnboarding::new(service, identity_manager));
    let inherited_capability = std::env::var(crate::agent_runner::MCP_CAPABILITY_ENV).ok();
    let inherited_execution_context = match service {
        Some(service) => crate::agent_runner::resolve_mcp_execution_context(
            service.runtime().as_ref(),
            inherited_capability.as_deref(),
        ),
        None if inherited_capability.is_some() => {
            Err("agent_mcp_execution_context_invalid".to_string())
        }
        None => Ok(None),
    };
    let external_connection = external_mcp::ExternalMcpConnection::new();
    for line in io::stdin().lock().lines().map_while(Result::ok) {
        let runner_execution_context = if inherited_capability.is_some() {
            inherited_execution_context.clone()
        } else {
            match &external_connection {
                Ok(connection) => connection.execution_context(),
                Err(_) => Ok(None), // Keep setup usable; attachment itself fails closed.
            }
        };
        let Ok(message) = serde_json::from_str::<Value>(&line) else {
            println!(
                "{}",
                json!({"jsonrpc": "2.0", "id": null, "error": {"code": -32700, "message": "invalid JSON"}})
            );
            continue;
        };
        let id = message.get("id").cloned().unwrap_or(Value::Null);
        let method = message.get("method").and_then(Value::as_str).unwrap_or("");
        let response = match method {
            "initialize" => {
                json!({"jsonrpc": "2.0", "id": id, "result": {"protocolVersion": crate::mcp::MCP_PROTOCOL_VERSION, "capabilities": {"tools": {"listChanged": true}}, "serverInfo": {"name": "tradeassembly", "version": "0.1.0"}}})
            }
            "notifications/initialized" => continue,
            "tools/list" => match &runner_execution_context {
                Ok(Some(context)) => json!({
                    "jsonrpc": "2.0",
                    "id": id,
                    "result": {"tools": if inherited_capability.is_none() {
                        let mut allowed = context.allowed_tools().to_vec();
                        allowed.extend([external_mcp::ATTACH.to_string(), external_mcp::DETACH.to_string()]);
                        crate::mcp::tool_definitions_for_agent_execution_context(&allowed)
                    } else { crate::mcp::tool_definitions_for_agent_execution_context(context.allowed_tools()) }}
                }),
                Ok(None) => {
                    json!({"jsonrpc": "2.0", "id": id, "result": {"tools": crate::mcp::tool_definitions()}})
                }
                Err(_) => json!({
                    "jsonrpc": "2.0",
                    "id": id,
                    "error": {"code": -32000, "message": "runner MCP execution context invalid"}
                }),
            },
            "tools/call" => {
                let params = message.get("params").cloned().unwrap_or_else(|| json!({}));
                let name = params.get("name").and_then(Value::as_str).unwrap_or("");
                let args = params
                    .get("arguments")
                    .cloned()
                    .unwrap_or_else(|| json!({}));
                let result = if matches!(name, external_mcp::ATTACH | external_mcp::DETACH) {
                    let action = if inherited_capability.is_some() {
                        Err("external_attach_operator_required".to_string())
                    } else if let Ok(connection) = &external_connection {
                        if name == external_mcp::DETACH {
                            connection.detach()
                        } else {
                            match service
                                .zip(current_mcp_identity(service, identity_manager).as_ref())
                            {
                                Some((service, identity)) => connection
                                    .attach(&authenticated_service(service, identity), &args),
                                None => Err("external_attach_identity_required".to_string()),
                            }
                        }
                    } else {
                        Err("external_agent_heartbeat_unavailable".to_string())
                    };
                    match action {
                        Ok(payload) => crate::mcp::call_tool_with_payload(name, payload),
                        Err(code) => crate::mcp::tool_error(
                            name,
                            &code,
                            "External agent session action failed closed.",
                            None,
                        ),
                    }
                } else if runner_execution_context.is_err() {
                    crate::mcp::tool_error(
                        name,
                        "agent_mcp_execution_context_invalid",
                        "Runner-scoped MCP capability is invalid.",
                        None,
                    )
                } else if runner_execution_context
                    .as_ref()
                    .ok()
                    .and_then(Option::as_ref)
                    .is_some_and(|context| !context.allowed_tools().iter().any(|tool| tool == name))
                {
                    crate::mcp::tool_error(
                        name,
                        "agent_mcp_tool_not_allowed",
                        "Runner-scoped MCP capability does not allow this tool.",
                        None,
                    )
                } else {
                    let identity = current_mcp_identity(service, identity_manager);
                    if name.starts_with("tradeassembly.onboarding.")
                        || name == "tradeassembly.broker.connect"
                    {
                        match onboarding.as_ref() {
                            Some(onboarding) => crate::mcp::call_tool_with_payload(
                                name,
                                tokio::task::block_in_place(|| onboarding.call(name, args)),
                            ),
                            None => crate::mcp::tool_error(
                                name,
                                "runtime_unavailable",
                                "Repair the installation before starting browser setup.",
                                None,
                            ),
                        }
                    } else if name == "tradeassembly.account.login"
                        || name == "tradeassembly.account.login.status"
                    {
                        let result = tokio::task::block_in_place(|| {
                            tokio::runtime::Handle::current().block_on(async {
                                if let Some(onboarding) = onboarding.as_ref() {
                                    onboarding
                                        .login(name == "tradeassembly.account.login")
                                        .await
                                } else if name == "tradeassembly.account.login" {
                                    bootstrap.start().await
                                } else {
                                    bootstrap.status().await
                                }
                            })
                        });
                        match result {
                            Ok(payload) => crate::mcp::call_tool_with_payload(name, payload),
                            Err(_) => crate::mcp::tool_error(name, "sign_in_unavailable", "Sign-in could not start. Check the configured identity service and try again.", None),
                        }
                    } else if matches!(
                        name,
                        "tradeassembly.setup.inspect" | "tradeassembly.setup.status"
                    ) && args.get("surface").is_some_and(|surface| {
                        !matches!(surface.as_str(), Some("agent" | "studio"))
                    }) {
                        crate::mcp::tool_error(
                            name,
                            "invalid_surface",
                            "surface must be agent or studio.",
                            None,
                        )
                    } else if matches!(
                        name,
                        "tradeassembly.setup.inspect" | "tradeassembly.setup.status"
                    ) {
                        let authenticated = service
                            .zip(identity.as_ref())
                            .map(|(service, identity)| authenticated_service(service, identity));
                        let mut payload = crate::onboarding::inspect_for_surface(
                            authenticated.as_ref(),
                            identity.as_ref().map(CliIdentity::redacted_status),
                            studio_base_url,
                            args["surface"] == "agent",
                        );
                        if service.is_none() {
                            payload["tooling"]["available"] = json!(false);
                            payload["acceptance"]["tooling"] = json!({"status":"failed", "reason":"Local runtime initialization failed."});
                            payload["acceptance"]["complete"] = json!(false);
                            payload["nextAction"] = json!({"action": "repair_installation", "message": "The local service needs repair. Re-run the installation setup before connecting a broker."});
                        }
                        if let Some(problem) = bootstrap.configuration_problem() {
                            payload["identity"] = problem.clone();
                            payload["acceptance"]["identity"] =
                                json!({"status":"failed", "reason":problem["message"]});
                            payload["acceptance"]["complete"] = json!(false);
                            payload["nextAction"] = json!({"action": "repair_sign_in_configuration", "message": problem["message"], "requiredConfiguration": problem["requiredConfiguration"]});
                        }
                        crate::mcp::call_tool_with_payload(name, payload)
                    } else if matches!(
                        name,
                        "tradeassembly.auth.status" | "tradeassembly.account.status"
                    ) {
                        identity.as_ref().map_or_else(
                        || {
                            crate::mcp::call_tool_with_payload(
                                name,
                                json!({"authenticated": false, "reason": "oidc_session_required"}),
                            )
                        },
                        |identity| {
                            crate::mcp::call_tool_with_payload(name, identity.redacted_status())
                        },
                    )
                    } else if identity.is_none()
                        && !matches!(name, "tradeassembly.health" | "tradeassembly.setup.status")
                    {
                        crate::mcp::tool_error(
                            name,
                            "oidc_session_required",
                            "Use tradeassembly.account.login to open sign-in, then tradeassembly.account.login.status to check completion.",
                            None,
                        )
                    } else if let Some(service) = service {
                        let authenticated = identity.as_ref().map_or_else(
                            || service.clone(),
                            |identity| authenticated_service(service, identity),
                        );
                        let authenticated = runner_execution_context
                            .as_ref()
                            .ok()
                            .and_then(|context| context.as_ref())
                            .map_or(authenticated.clone(), |context| {
                                authenticated
                                    .with_verified_agent_mcp_execution_context(context.clone())
                            });
                        let mut args = args;
                        if name == "tradeassembly.broker.connect" {
                            args["studio_base_url"] = json!(studio_base_url);
                        }
                        crate::mcp::call_service_tool(&authenticated, name, args)
                    } else {
                        crate::mcp::tool_error(
                            name,
                            "setup_required",
                            "The local service needs repair. Use tradeassembly.setup.inspect.",
                            None,
                        )
                    }
                };
                json!({"jsonrpc": "2.0", "id": id, "result": result})
            }
            _ => {
                json!({"jsonrpc": "2.0", "id": id, "error": {"code": -32601, "message": "unknown MCP method"}})
            }
        };
        println!("{}", response);
        if method == "tools/call"
            && matches!(
                message.pointer("/params/name").and_then(Value::as_str),
                Some(external_mcp::ATTACH | external_mcp::DETACH)
            )
            && response.pointer("/result/isError") == Some(&Value::Bool(false))
        {
            println!(
                "{}",
                json!({"jsonrpc":"2.0","method":"notifications/tools/list_changed"})
            );
        }
    }
}

fn current_mcp_identity(
    service: Option<&TradeAssemblyService>,
    identity_manager: &CliIdentityManager,
) -> Option<CliIdentity> {
    if service.is_none() && !identity_manager.uses_local_owner() {
        return None;
    }
    tokio::task::block_in_place(|| {
        tokio::runtime::Handle::current()
            .block_on(identity_manager.current_identity())
            .ok()
            .filter(|identity| {
                service.is_none_or(|service| register_verified_identity(service, identity).is_ok())
            })
    })
}

pub fn execute_command(service: &TradeAssemblyService, command: Command) -> Value {
    execute_command_with_agent(service, command, None)
}

fn execute_command_with_agent(
    service: &TradeAssemblyService,
    command: Command,
    agent: Option<&crate::agent_runner::CodexCliAdapter>,
) -> Value {
    match command {
        Command::Strategy { command } => match command {
            StrategyCommand::List => {
                service
                    .handle_http_from_source("cli", "GET", "/strategies", json!({}))
                    .body
            }
            StrategyCommand::Get { strategy_id } => {
                service
                    .handle_http("GET", &format!("/strategies/{strategy_id}"), json!({}))
                    .body
            }
            StrategyCommand::Publish {
                strategy_id,
                expected_draft_hash,
                acknowledge_publication,
                idempotency_key,
            } => execute_strategy_publication(
                service,
                strategy_id,
                expected_draft_hash,
                acknowledge_publication,
                idempotency_key,
            ),
            StrategyCommand::Create { mode, name } => {
                service
                    .handle_http(
                        "POST",
                        "/product/strategies/create",
                        json!({"mode": mode, "name": name}),
                    )
                    .body
            }
            StrategyCommand::Validate => {
                service
                    .handle_http("POST", "/strategy/contracts/validate", json!({}))
                    .body
            }
        },
        Command::Journal { command } => match command {
            JournalCommand::List => {
                service
                    .handle_http_from_source("cli", "GET", "/journal/events", json!({}))
                    .body
            }
            JournalCommand::Export => {
                service
                    .handle_http_from_source("cli", "GET", "/journal/export", json!({}))
                    .body
            }
            JournalCommand::Replay => {
                service
                    .handle_http_from_source("cli", "POST", "/journal/replay-report", json!({}))
                    .body
            }
        },
        Command::Providers { command } => match command {
            ProviderCommand::List => {
                service
                    .handle_http_from_source("cli", "GET", "/providers", json!({}))
                    .body
            }
            ProviderCommand::CredentialStatus { provider_ref } => {
                service
                    .handle_http(
                        "GET",
                        &format!("/providers/{provider_ref}/credentials"),
                        json!({}),
                    )
                    .body
            }
            ProviderCommand::CredentialTest { provider_ref } => {
                service
                    .handle_http(
                        "POST",
                        &format!("/providers/{provider_ref}/credentials/test"),
                        json!({}),
                    )
                    .body
            }
            ProviderCommand::Policy => {
                service
                    .handle_http("GET", "/providers/policy", json!({}))
                    .body
            }
        },
        Command::Credentials { command } => match command {
            CredentialCommand::Status { provider_ref } => {
                service
                    .handle_http(
                        "GET",
                        &format!("/providers/{provider_ref}/credentials"),
                        json!({}),
                    )
                    .body
            }
            CredentialCommand::Test { provider_ref } => {
                service
                    .handle_http(
                        "POST",
                        &format!("/providers/{provider_ref}/credentials/test"),
                        json!({}),
                    )
                    .body
            }
        },
        Command::Account { command } => match command {
            AccountCommand::Status(args) => {
                auth::local_account_status(&args.profile, &args.studio_base_url)
            }
            AccountCommand::Login(args) => {
                auth::local_account_login(&args.profile, &args.studio_base_url)
            }
        },
        Command::Install { command } => match command {
            InstallCommand::Claim(args) => auth::local_install_claim(
                &args.profile,
                args.install_id.as_deref(),
                &args.studio_base_url,
            ),
        },
        Command::Telemetry { command } => match command {
            TelemetryCommand::Emit(args) => auth::local_telemetry_emit(
                &args.profile,
                &args.event,
                args.install_id.as_deref(),
                &args.studio_base_url,
            ),
            TelemetryCommand::OptOut(args) => auth::local_telemetry_opt_out(
                &args.profile,
                parse_bool_arg(&args.telemetry_opt_out),
                &args.studio_base_url,
            ),
            TelemetryCommand::Replay(args) => {
                auth::local_telemetry_replay(&args.profile, &args.studio_base_url)
            }
        },
        Command::Backtest(args) => {
            execute_backtests_command(service, BacktestsCommand::Create(args)).body
        }
        Command::Backtests { command } => execute_backtests_command(service, command).body,
        Command::Robustness { command } => execute_robustness_command(service, command).body,
        Command::Derivatives { command } => execute_derivatives_command(service, command).body,
        Command::Comparison { command } => execute_comparison_command(service, command).body,
        Command::Research { command } => match command {
            ResearchCommand::Universe => {
                service
                    .handle_http("POST", "/research/universes", json!({}))
                    .body
            }
            ResearchCommand::Jobs => {
                service
                    .handle_http_from_source("cli", "GET", "/research/jobs", json!({}))
                    .body
            }
            ResearchCommand::Sweep => {
                service
                    .handle_http("POST", "/research/sweeps", json!({}))
                    .body
            }
        },
        Command::DatasetIngestion { command } => match command {
            DatasetIngestionCommand::Create(args) => {
                let body = dataset_ingestion_request(args);
                if body.get("error").is_some() {
                    body
                } else {
                    service
                        .handle_http_from_source("cli", "POST", "/dataset-ingestions", body)
                        .body
                }
            }
            DatasetIngestionCommand::List => {
                service
                    .handle_http_from_source("cli", "GET", "/dataset-ingestions", json!({}))
                    .body
            }
            DatasetIngestionCommand::Get { ingestion_id }
            | DatasetIngestionCommand::Status { ingestion_id } => {
                service
                    .handle_http_from_source(
                        "cli",
                        "GET",
                        &format!("/dataset-ingestions/{ingestion_id}"),
                        json!({}),
                    )
                    .body
            }
            DatasetIngestionCommand::Cancel { ingestion_id } => {
                service
                    .handle_http_from_source(
                        "cli",
                        "POST",
                        &format!("/dataset-ingestions/{ingestion_id}/cancel"),
                        json!({}),
                    )
                    .body
            }
            DatasetIngestionCommand::Verify { ingestion_id } => {
                service
                    .handle_http_from_source(
                        "cli",
                        "POST",
                        &format!("/dataset-ingestions/{ingestion_id}/verify"),
                        json!({}),
                    )
                    .body
            }
        },
        Command::Marketdata { command } => match command {
            MarketdataCommand::Bars => {
                service
                    .handle_http("GET", "/marketdata/bars", json!({}))
                    .body
            }
            MarketdataCommand::Quote => {
                service
                    .handle_http("GET", "/marketdata/quote", json!({}))
                    .body
            }
            MarketdataCommand::Indicators => {
                service
                    .handle_http("POST", "/marketdata/indicators", json!({}))
                    .body
            }
            MarketdataCommand::OptionChain => {
                service
                    .handle_http("GET", "/marketdata/options/chain", json!({}))
                    .body
            }
            MarketdataCommand::OptionSelect => {
                service
                    .handle_http("POST", "/marketdata/options/select", json!({}))
                    .body
            }
        },
        Command::Options { command } => match command {
            OptionCommand::Chain => {
                service
                    .handle_http("GET", "/marketdata/options/chain", json!({}))
                    .body
            }
            OptionCommand::Select => {
                service
                    .handle_http("POST", "/marketdata/options/select", json!({}))
                    .body
            }
        },
        Command::Scheduler { command } => match command {
            SchedulerCommand::Status => {
                service
                    .handle_http("GET", "/scheduler/status", json!({}))
                    .body
            }
            SchedulerCommand::Start => {
                service
                    .handle_http("POST", "/scheduler/start", json!({}))
                    .body
            }
            SchedulerCommand::Run => {
                service
                    .handle_http("POST", "/scheduler/run", json!({}))
                    .body
            }
            SchedulerCommand::Stop => {
                service
                    .handle_http("POST", "/scheduler/stop", json!({}))
                    .body
            }
        },
        Command::Workers { command } => match command {
            WorkerCommand::Status => {
                service
                    .handle_http("GET", "/scheduler/status", json!({}))
                    .body
            }
            WorkerCommand::Run => {
                service
                    .handle_http("POST", "/scheduler/run", json!({}))
                    .body
            }
        },
        Command::Orders { command } => match command {
            OrdersCommand::Reconcile => {
                service
                    .handle_http("POST", "/orders/reconcile", json!({}))
                    .body
            }
            OrdersCommand::Attempts => {
                service
                    .handle_http("GET", "/orders/attempts", json!({}))
                    .body
            }
        },
        Command::Risk { command } => match command {
            RiskCommand::Status => {
                service
                    .handle_http_from_source("cli", "GET", "/risk/status", json!({}))
                    .body
            }
            RiskCommand::AccountSet => {
                service
                    .handle_http_from_source("cli", "PUT", "/risk/account", json!({}))
                    .body
            }
            RiskCommand::Stress => {
                service
                    .handle_http("POST", "/risk/stress-test", json!({}))
                    .body
            }
            RiskCommand::Overlay(args) => {
                service
                    .handle_http(
                        "POST",
                        "/risk/portfolio-overlay",
                        json!({"strategyId": args.strategy_id, "scopeKind": args.scope_kind}),
                    )
                    .body
            }
        },
        Command::Positions => {
            service
                .handle_http("GET", "/portfolio/positions", json!({}))
                .body
        }
        Command::Replay => {
            service
                .handle_http("POST", "/journal/replay-report", json!({}))
                .body
        }
        Command::Demos { command } => match command {
            DemoCommand::List => {
                json!({"demos": demos::demo_descriptors().into_iter().map(|item| json!({"id": item.id, "title": item.title})).collect::<Vec<_>>()})
            }
            DemoCommand::Run { demo_id } => {
                service
                    .handle_http("POST", "/demo/btc-exit", json!({"demoId": demo_id}))
                    .body
            }
        },
        Command::Demo { demo_id: _ } => {
            service
                .handle_http("POST", "/demo/btc-exit", json!({}))
                .body
        }
        Command::Seed | Command::SeedBtcDemo => service.seed_btc_demo(),
        Command::Reset | Command::ResetBtcDemo => {
            service
                .handle_http("POST", "/demo/reset-btc", json!({}))
                .body
        }
        Command::Readiness => {
            service
                .handle_http_from_source("cli", "GET", "/ready", json!({}))
                .body
        }
        Command::Report => {
            service
                .handle_http("POST", "/product/reports/envelope", json!({}))
                .body
        }
        Command::ReportEnvelope {
            kind,
            strategy_id,
            backtest_id,
            checkpoint_id,
        } => {
            service
                .handle_http(
                    "POST",
                    "/product/reports/envelope",
                    json!({
                        "kind": kind,
                        "strategyId": strategy_id,
                        "backtestId": backtest_id,
                        "checkpointId": checkpoint_id
                    }),
                )
                .body
        }
        Command::Plugins { command } => match command {
            PluginCommand::Status => {
                service
                    .handle_http_from_source("cli", "GET", "/plugins/instances", json!({}))
                    .body
            }
            PluginCommand::InstallDefault(args) => install_default_external_plugin(service, args),
            PluginCommand::Install(args) => {
                service
                    .handle_http_from_source(
                        "cli",
                        "POST",
                        "/plugins/packages",
                        plugin_package_request(&args),
                    )
                    .body
            }
            PluginCommand::Create(args) => {
                service
                    .handle_http_from_source(
                        "cli",
                        "POST",
                        "/plugins/instances",
                        json!({
                            "pluginRef": args.plugin_ref,
                            "instanceRef": args.instance_ref,
                            "providerRef": args.provider_ref,
                            "enabled": false,
                        }),
                    )
                    .body
            }
            PluginCommand::Configure(args) => match plugin_json_file(&args.configuration_file) {
                Ok(configuration) => {
                    service
                        .handle_http_from_source(
                            "cli",
                            "PUT",
                            &format!("/plugins/instances/{}/configuration", args.instance_ref),
                            json!({"configuration": configuration}),
                        )
                        .body
                }
                Err(code) => json!({"error": {"code": code}, "noAdvice": true}),
            },
            PluginCommand::Enable { instance_ref } => {
                service
                    .handle_http_from_source(
                        "cli",
                        "POST",
                        &format!("/plugins/instances/{instance_ref}:enable"),
                        json!({}),
                    )
                    .body
            }
            PluginCommand::Disable { instance_ref } => {
                service
                    .handle_http_from_source(
                        "cli",
                        "POST",
                        &format!("/plugins/instances/{instance_ref}:disable"),
                        json!({}),
                    )
                    .body
            }
            PluginCommand::RefreshHealth { instance_ref } => {
                service
                    .handle_http_from_source(
                        "cli",
                        "POST",
                        &format!("/plugins/instances/{instance_ref}/health:refresh"),
                        json!({}),
                    )
                    .body
            }
            PluginCommand::Upgrade(args) => {
                service
                    .handle_http_from_source(
                        "cli",
                        "POST",
                        &format!("/plugins/instances/{}:upgrade", args.instance_ref),
                        plugin_package_request(&args.package),
                    )
                    .body
            }
            PluginCommand::Rollback { instance_ref } => {
                service
                    .handle_http_from_source(
                        "cli",
                        "POST",
                        &format!("/plugins/instances/{instance_ref}:rollback"),
                        json!({}),
                    )
                    .body
            }
            PluginCommand::Remove { instance_ref } => {
                service
                    .handle_http_from_source(
                        "cli",
                        "DELETE",
                        &format!("/plugins/instances/{instance_ref}"),
                        json!({}),
                    )
                    .body
            }
            PluginCommand::Invoke(args) => match plugin_json_file(&args.request_file) {
                Ok(request) => {
                    service
                        .handle_http_from_source(
                            "cli",
                            "POST",
                            &format!(
                                "/plugins/instances/{}/operations/{}:invoke",
                                args.instance_ref, args.operation_id
                            ),
                            request,
                        )
                        .body
                }
                Err(code) => json!({"error": {"code": code}, "noAdvice": true}),
            },
        },
        Command::Instruments { command } => match command {
            InstrumentCommand::Resolve => {
                service
                    .handle_http("POST", "/marketdata/instruments/resolve", json!({}))
                    .body
            }
            InstrumentCommand::Aliases => {
                service
                    .handle_http("GET", "/marketdata/instruments/aliases", json!({}))
                    .body
            }
            InstrumentCommand::PackStatus => {
                service
                    .handle_http("GET", "/marketdata/instrument-packs", json!({}))
                    .body
            }
        },
        Command::Selectors { command } => match command {
            SelectorCommand::Evaluate => {
                service
                    .handle_http("POST", "/marketdata/selectors/evaluate", json!({}))
                    .body
            }
            SelectorCommand::Explain => {
                service
                    .handle_http("POST", "/marketdata/selectors/explain", json!({}))
                    .body
            }
        },
        Command::Scenario { command } => match command {
            ScenarioCommand::Valuation {
                strategy_id,
                checkpoint_id,
            } => {
                service
                    .handle_http(
                        "POST",
                        "/product/strategies/scenario-valuation",
                        json!({"strategyId": strategy_id, "checkpointId": checkpoint_id}),
                    )
                    .body
            }
        },
        Command::MonteCarlo { command } => {
            let (path, args) = match command {
                MonteCarloCommand::Run(args) => ("/product/strategies/monte-carlo", args),
                MonteCarloCommand::Status(args) => ("/product/strategies/monte-carlo/status", args),
                MonteCarloCommand::Report(args) => ("/product/strategies/monte-carlo/report", args),
                MonteCarloCommand::Replay(args) => ("/product/strategies/monte-carlo/replay", args),
                MonteCarloCommand::Export(args) => ("/product/strategies/monte-carlo/export", args),
            };
            let mut body = json!({"strategyId": args.strategy_id});
            if let Some(assumption_hash) = args.assumption_hash {
                body["expectedAssumptionHash"] = json!(assumption_hash);
            }
            service
                .handle_http_from_source("cli", "POST", path, body)
                .body
        }
        Command::Lifecycle { command } => match command {
            LifecycleCommand::Import => {
                service
                    .handle_http("POST", "/execution/position-lifecycle/import", json!({}))
                    .body
            }
            LifecycleCommand::Inspect => {
                service
                    .handle_http("POST", "/execution/position-lifecycle/inspect", json!({}))
                    .body
            }
            LifecycleCommand::Status => {
                service
                    .handle_http("POST", "/execution/position-lifecycle/status", json!({}))
                    .body
            }
            LifecycleCommand::Replay => {
                service
                    .handle_http("POST", "/execution/position-lifecycle/replay", json!({}))
                    .body
            }
        },
        Command::LifecycleCalendar { command } => {
            let (path, args) = match command {
                LifecycleCalendarCommand::List(args) => {
                    ("/product/strategies/lifecycle-calendar/list", args)
                }
                LifecycleCalendarCommand::Inspect(args) => {
                    ("/product/strategies/lifecycle-calendar/inspect", args)
                }
                LifecycleCalendarCommand::Export(args) => {
                    ("/product/strategies/lifecycle-calendar/export", args)
                }
                LifecycleCalendarCommand::Replay(args) => {
                    ("/product/strategies/lifecycle-calendar/replay", args)
                }
            };
            service
                .handle_http(
                    "POST",
                    path,
                    json!({"strategyId": args.strategy_id, "timelineId": args.timeline_id}),
                )
                .body
        }
        Command::FillQuality { command } => {
            let (path, args) = match command {
                FillQualityCommand::Inspect(args) => {
                    ("/product/strategies/fill-quality/inspect", args)
                }
                FillQualityCommand::Report(args) => {
                    ("/product/strategies/fill-quality/report", args)
                }
                FillQualityCommand::Replay(args) => {
                    ("/product/strategies/fill-quality/replay", args)
                }
                FillQualityCommand::Export(args) => {
                    ("/product/strategies/fill-quality/export", args)
                }
            };
            let mut body = json!({
                "strategyId": args.strategy_id,
                "analysisId": args.analysis_id
            });
            if let Some(benchmark_price) = args.benchmark_price {
                body["benchmark"] = json!({
                    "benchmarkRef": args
                        .benchmark_ref
                        .unwrap_or_else(|| "user_selected_benchmark".to_string()),
                    "benchmarkPrice": benchmark_price,
                    "benchmarkSource": "cli"
                });
            }
            service
                .handle_http_from_source("cli", "POST", path, body)
                .body
        }
        Command::AttributionJournal { command } => {
            let (path, args) = match command {
                AttributionJournalCommand::Review(args) => {
                    ("/product/strategies/attribution-journal/review", args)
                }
                AttributionJournalCommand::Report(args) => {
                    ("/product/strategies/attribution-journal/report", args)
                }
                AttributionJournalCommand::Inspect(args) => {
                    ("/product/strategies/attribution-journal/inspect", args)
                }
                AttributionJournalCommand::Replay(args) => {
                    ("/product/strategies/attribution-journal/replay", args)
                }
                AttributionJournalCommand::Export(args) => {
                    ("/product/strategies/attribution-journal/export", args)
                }
            };
            service
                .handle_http(
                    "POST",
                    path,
                    json!({"strategyId": args.strategy_id, "reviewId": args.review_id}),
                )
                .body
        }
        Command::ResearchNotebook { command } => {
            let (path, args) = match command {
                ResearchNotebookCommand::Create(args) => {
                    ("/product/strategies/research-notebook/create", args)
                }
                ResearchNotebookCommand::Compose(args) => {
                    ("/product/strategies/research-notebook/compose", args)
                }
                ResearchNotebookCommand::Attach(args) => {
                    ("/product/strategies/research-notebook/attach", args)
                }
                ResearchNotebookCommand::List(args) => {
                    ("/product/strategies/research-notebook/list", args)
                }
                ResearchNotebookCommand::Inspect(args) => {
                    ("/product/strategies/research-notebook/inspect", args)
                }
                ResearchNotebookCommand::Export(args) => {
                    ("/product/strategies/research-notebook/export", args)
                }
                ResearchNotebookCommand::Replay(args) => {
                    ("/product/strategies/research-notebook/replay", args)
                }
            };
            let mut body = json!({
                "strategyId": args.strategy_id,
            });
            if !args.artifact_selectors.is_empty() {
                body["artifactSelectors"] = json!(args
                    .artifact_selectors
                    .iter()
                    .map(|value| {
                        let (kind, id) =
                            value.split_once(':').expect("validated artifact selector");
                        json!({"kind": kind, "id": id})
                    })
                    .collect::<Vec<_>>());
            }
            if let Some(title) = args.title {
                body["title"] = json!(title);
            }
            if let Some(notebook_id) = args.notebook_id {
                body["notebookId"] = json!(notebook_id);
            }
            service
                .handle_http_from_source("cli", "POST", path, body)
                .body
        }
        Command::Execution { command } => match command {
            ExecutionCommand::LiveMandate { command } => {
                let (path, body) = match command {
                    LiveMandateCommand::Issue {
                        config_id,
                        expires_at_ms,
                        delegate_deployment_id,
                        idempotency_key,
                    } => {
                        let mut body = json!({"configId": config_id, "expiresAtMs": expires_at_ms, "idempotencyKey": idempotency_key});
                        if let Some(deployment) = delegate_deployment_id {
                            body["delegateDeploymentId"] = json!(deployment);
                        }
                        ("/product/live-mandates/issue", body)
                    }
                    LiveMandateCommand::Revoke {
                        mandate_id,
                        idempotency_key,
                    } => (
                        "/product/live-mandates/revoke",
                        json!({"mandateId": mandate_id, "idempotencyKey": idempotency_key}),
                    ),
                    LiveMandateCommand::Status { mandate_id } => (
                        "/product/live-mandates/status",
                        json!({"mandateId": mandate_id}),
                    ),
                };
                service
                    .handle_http_from_source("cli", "POST", path, body)
                    .body
            }
            ExecutionCommand::Run => {
                service
                    .handle_http_from_source("cli", "POST", "/run", json!({}))
                    .body
            }
            ExecutionCommand::ConfigSave(args) => {
                service
                    .handle_http_from_source(
                        "cli",
                        "POST",
                        "/product/strategy-execution-configs/save",
                        json!({
                            "strategyId": args.strategy_id,
                            "mode": args.mode,
                            "providerRef": args.provider_ref,
                        }),
                    )
                    .body
            }
            ExecutionCommand::Readiness(args) => {
                service
                    .handle_http_from_source(
                        "cli",
                        "POST",
                        "/product/strategy-execution-configs/activation-readiness",
                        json!({
                            "configId": args.config_id,
                            "strategyId": args.strategy_id,
                        }),
                    )
                    .body
            }
            ExecutionCommand::Activate(args) => {
                service
                    .handle_http_from_source(
                        "cli",
                        "POST",
                        "/product/strategy-execution-activations/activate",
                        json!({
                            "configId": args.config_id,
                            "strategyId": args.strategy_id,
                            "idempotencyKey": args.idempotency_key,
                            "acknowledgementIds": ["user_logic", "user_risk", "no_advice"],
                            "localLiveMandateId": args.local_live_mandate_id,
                        }),
                    )
                    .body
            }
            ExecutionCommand::Control(args) => {
                service
                    .handle_http_from_source(
                        "cli",
                        "POST",
                        "/product/strategy-execution-activations/control",
                        json!({
                            "activationId": args.activation_id,
                            "action": args.action,
                            "idempotencyKey": args.idempotency_key,
                        }),
                    )
                    .body
            }
            ExecutionCommand::Deactivate(args) => {
                service
                    .handle_http_from_source(
                        "cli",
                        "POST",
                        "/product/strategy-execution-activations/deactivate",
                        json!({"activationId": args.activation_id}),
                    )
                    .body
            }
            ExecutionCommand::Status(args) => {
                service
                    .handle_http_from_source(
                        "cli",
                        "POST",
                        "/product/strategies/execution-workspace",
                        json!({
                            "strategyId": args.strategy_id,
                            "activationId": args.activation_id,
                            "afterSequence": args.after_sequence,
                            "eventLimit": args.event_limit,
                        }),
                    )
                    .body
            }
        },
        Command::Events { command: _ } => json!({"ok": true, "eventBus": "local-memory"}),
        Command::Agent { command } => match command {
            AgentCommand::Install(_)
            | AgentCommand::Verify(_)
            | AgentCommand::Status(_)
            | AgentCommand::Uninstall(_) => {
                json!({"ok": false, "error": {"code": "agent_launchd_requires_runtime_config"}})
            }
            AgentCommand::Run { runner_id, once } => {
                let Some(adapter) = agent else {
                    return json!({"ok": false, "error": {"code": "agent_mcp_binding_required"}});
                };
                let mut last = Vec::new();
                let mut failure = None;
                loop {
                    match crate::agent_runner::supervise_once(
                        &service.runtime(),
                        &runner_id,
                        service.runtime().clock.now_ms(),
                        adapter,
                    ) {
                        Ok(receipts) => last = receipts,
                        Err(error) => {
                            failure = Some(error);
                            break;
                        }
                    }
                    if once {
                        break;
                    }
                    std::thread::sleep(std::time::Duration::from_secs(1));
                }
                match failure {
                    Some(error) => {
                        json!({"ok": false, "error": {"code": "agent_runner_failed", "message": error}})
                    }
                    None => {
                        json!({"ok": true, "runner": "codex-cli", "foreground": true, "receipts": last})
                    }
                }
            }
            AgentCommand::Create { deployment_file } => match fs::read_to_string(deployment_file)
                .ok()
                .and_then(|body| {
                    serde_json::from_str::<crate::agent_runner::AgentDeployment>(&body).ok()
                }) {
                Some(deployment) => {
                    match crate::agent_runner::put_deployment(&service.runtime(), &deployment) {
                        Ok(()) => json!({"ok": true, "deploymentId": deployment.deployment_id}),
                        Err(error) => json!({"ok": false, "error": {"code": error}}),
                    }
                }
                None => json!({"ok": false, "error": {"code": "agent_deployment_file_invalid"}}),
            },
            AgentCommand::List => match crate::agent_runner::deployments(&service.runtime()) {
                Ok(deployments) => json!({"ok": true, "deployments": deployments}),
                Err(error) => json!({"ok": false, "error": {"code": error}}),
            },
            AgentCommand::Start { deployment_id } => {
                match crate::agent_runner::set_desired_state(
                    &service.runtime(),
                    &deployment_id,
                    "active",
                ) {
                    Ok(()) => {
                        json!({"ok": true, "deploymentId": deployment_id, "desiredState": "active"})
                    }
                    Err(error) => json!({"ok": false, "error": {"code": error}}),
                }
            }
            AgentCommand::Pause { deployment_id } => {
                match crate::agent_runner::set_desired_state(
                    &service.runtime(),
                    &deployment_id,
                    "paused",
                ) {
                    Ok(()) => {
                        json!({"ok": true, "deploymentId": deployment_id, "desiredState": "paused"})
                    }
                    Err(error) => json!({"ok": false, "error": {"code": error}}),
                }
            }
            AgentCommand::Recover {
                deployment_id,
                acknowledge_reconciled,
            } => {
                if !acknowledge_reconciled {
                    json!({"ok": false, "error": {"code": "agent_recovery_ack_required"}})
                } else {
                    match crate::agent_runner::recover_pending_run(
                        &service.runtime(),
                        &deployment_id,
                        service.runtime().clock.now_ms(),
                    ) {
                        Ok(receipt) => json!({"ok": true, "receipt": receipt}),
                        Err(error) => json!({"ok": false, "error": {"code": error}}),
                    }
                }
            }
        },
        Command::Setup { .. }
        | Command::Policy { .. }
        | Command::Api { .. }
        | Command::Mcp { .. }
        | Command::Auth { .. } => unreachable!(),
    }
}

fn execute_strategy_publication(
    service: &TradeAssemblyService,
    strategy_id: String,
    expected_draft_hash: String,
    acknowledge_publication: bool,
    idempotency_key: String,
) -> Value {
    // This is an owner protocol acknowledgement, not a claim of physical
    // human presence. Reject it before dispatch so a missing flag cannot
    // mutate the draft/version store.
    if !acknowledge_publication {
        return json!({
            "ok": false,
            "body": {"strategyId": strategy_id, "published": false},
            "error": {"code": "strategy_publication_acknowledgement_required"}
        });
    }
    if expected_draft_hash.trim().is_empty() {
        return json!({
            "ok": false,
            "body": {"strategyId": strategy_id, "published": false},
            "error": {"code": "strategy_draft_hash_required"}
        });
    }
    if idempotency_key.trim().is_empty() {
        return json!({
            "ok": false,
            "body": {"strategyId": strategy_id, "published": false},
            "error": {"code": "idempotency_key_required"}
        });
    }
    service
        .handle_http_from_source(
            "cli",
            "POST",
            "/product/strategies/publish",
            json!({
                "strategyId": strategy_id,
                "expectedDraftHash": expected_draft_hash,
                "acknowledgePublication": true,
                "idempotencyKey": idempotency_key,
            }),
        )
        .body
}

fn parse_artifact_selector(value: &str) -> Result<String, String> {
    let Some((kind, id)) = value.split_once(':') else {
        return Err("artifact selector must be kind:id".to_string());
    };
    if !matches!(
        kind,
        "backtest" | "robustness" | "comparison" | "derivatives"
    ) || id.trim().is_empty()
    {
        return Err("artifact selector kind must be backtest, robustness, comparison, or derivatives and include an ID".to_string());
    }
    Ok(value.to_string())
}

pub fn execute_backtests_command(
    service: &TradeAssemblyService,
    command: BacktestsCommand,
) -> ServiceResponse {
    match command {
        BacktestsCommand::Create(args) => match json_request_file(&args.request_file) {
            Ok(body) => service.handle_http_from_source("cli", "POST", "/backtests", body),
            Err(response) => response,
        },
        BacktestsCommand::Get { run_id } => service.handle_http_from_source(
            "cli",
            "GET",
            &format!("/backtests/{run_id}"),
            json!({}),
        ),
        BacktestsCommand::List => {
            service.handle_http_from_source("cli", "GET", "/backtests", json!({}))
        }
        BacktestsCommand::Cancel(args) => backtest_run_request(service, "cancel", args, |run_id| {
            format!("/backtests/{run_id}/cancel")
        }),
        BacktestsCommand::Retry(args) => backtest_run_request(service, "retry", args, |run_id| {
            format!("/backtests/{run_id}/retry")
        }),
        BacktestsCommand::Process { worker, run_id } => service.handle_http_from_source(
            "cli",
            "POST",
            "/backtests:process",
            json!({
                "worker": worker,
                "runId": run_id,
                "idempotencyKey": format!(
                    "cli-backtest-process-{}-{}",
                    std::process::id(),
                    SystemTime::now()
                        .duration_since(UNIX_EPOCH)
                        .unwrap_or_default()
                        .as_nanos()
                ),
            }),
        ),
        BacktestsCommand::Replay { run_id } => service.handle_http_from_source(
            "cli",
            "POST",
            &format!("/backtests/{run_id}/replay"),
            json!({}),
        ),
        BacktestsCommand::Report { run_id } => service.handle_http_from_source(
            "cli",
            "GET",
            &format!("/backtests/{run_id}/report"),
            json!({}),
        ),
        BacktestsCommand::Export {
            run_id,
            export_kind,
        } => service.handle_http_from_source(
            "cli",
            "POST",
            &format!("/backtests/{run_id}/export"),
            json!({"exportKind": export_kind}),
        ),
    }
}

pub fn execute_robustness_command(
    service: &TradeAssemblyService,
    command: RobustnessCommand,
) -> ServiceResponse {
    match command {
        RobustnessCommand::Run(args) | RobustnessCommand::Create(args) => {
            let body = match robustness_create_request(args) {
                Ok(body) => body,
                Err(response) => return response,
            };
            service.handle_http_from_source("cli", "POST", "/robustness-runs", body)
        }
        RobustnessCommand::List => {
            service.handle_http_from_source("cli", "GET", "/robustness-runs", json!({}))
        }
        RobustnessCommand::Get { run_id } => service.handle_http_from_source(
            "cli",
            "GET",
            &format!("/robustness-runs/{run_id}"),
            json!({}),
        ),
        RobustnessCommand::GetReport { run_id } => service.handle_http_from_source(
            "cli",
            "GET",
            &format!("/robustness-runs/{run_id}/report"),
            json!({}),
        ),
        RobustnessCommand::Replay(args) => service.handle_http_from_source(
            "cli",
            "POST",
            &format!("/robustness-runs/{}/replay", args.run_id),
            json!({
                "idempotencyKey": args.idempotency_key,
                "authorityContext": {
                    "actor": args.actor,
                    "surface": "cli",
                },
            }),
        ),
        RobustnessCommand::Export(args) => service.handle_http_from_source(
            "cli",
            "POST",
            &format!("/robustness-runs/{}/export", args.run_id),
            json!({
                "format": args.format,
                "idempotencyKey": args.idempotency_key,
                "authorityContext": {
                    "actor": args.actor,
                    "surface": "cli",
                },
            }),
        ),
        RobustnessCommand::Process { worker } => service.handle_http_from_source(
            "cli",
            "POST",
            "/robustness-runs:process",
            json!({"worker": worker}),
        ),
        RobustnessCommand::Cancel(args) => robustness_run_request(service, args, |run_id| {
            format!("/robustness-runs/{run_id}/cancel")
        }),
        RobustnessCommand::Retry(args) => robustness_run_request(service, args, |run_id| {
            format!("/robustness-runs/{run_id}/retry")
        }),
    }
}

pub fn execute_derivatives_command(
    service: &TradeAssemblyService,
    command: DerivativesCommand,
) -> ServiceResponse {
    match command {
        DerivativesCommand::Create(args) => {
            let body = match json_request_file_with_codes(
                &args.request_file,
                "derivatives_request_unreadable",
                "derivatives_request_invalid",
            ) {
                Ok(body) if body.is_object() => body,
                Ok(_) => return ServiceResponse::bad_request("derivatives_request_invalid"),
                Err(response) => return response,
            };
            service.handle_http_from_source("cli", "POST", "/derivatives-analyses", body)
        }
        DerivativesCommand::List => {
            service.handle_http_from_source("cli", "GET", "/derivatives-analyses", json!({}))
        }
        DerivativesCommand::Get { analysis_id } => service.handle_http_from_source(
            "cli",
            "GET",
            &format!("/derivatives-analyses/{analysis_id}"),
            json!({}),
        ),
        DerivativesCommand::Export {
            analysis_id,
            format,
        } => service.handle_http_from_source(
            "cli",
            "GET",
            &format!("/derivatives-analyses/{analysis_id}/exports/{format}"),
            json!({}),
        ),
    }
}

pub fn execute_comparison_command(
    service: &TradeAssemblyService,
    command: ComparisonCommand,
) -> ServiceResponse {
    match command {
        ComparisonCommand::Create(args) => {
            let body = match json_request_file_with_codes(
                &args.request_file,
                "comparison_request_unreadable",
                "comparison_request_invalid",
            ) {
                Ok(body) if body.is_object() => body,
                Ok(_) => return ServiceResponse::bad_request("comparison_request_invalid"),
                Err(response) => return response,
            };
            service.handle_http_from_source("cli", "POST", "/research-comparisons", body)
        }
        ComparisonCommand::List => {
            service.handle_http_from_source("cli", "GET", "/research-comparisons", json!({}))
        }
        ComparisonCommand::Get { comparison_id } => service.handle_http_from_source(
            "cli",
            "GET",
            &format!("/research-comparisons/{comparison_id}"),
            json!({}),
        ),
        ComparisonCommand::Export {
            comparison_id,
            format,
        } => service.handle_http_from_source(
            "cli",
            "GET",
            &format!("/research-comparisons/{comparison_id}/exports/{format}"),
            json!({}),
        ),
    }
}

fn robustness_create_request(args: RobustnessCreateArgs) -> Result<Value, ServiceResponse> {
    if let Some(request_file) = args.request_file {
        return json_request_file_with_codes(
            &request_file,
            "robustness_request_unreadable",
            "robustness_request_invalid",
        );
    }
    let source_run_id = required_cli_value(args.source_run_id, "robustness_source_required")?;
    let study_kind = required_cli_value(args.study_kind, "robustness_study_kind_required")?;
    let assumptions_json =
        required_cli_value(args.assumptions_json, "robustness_assumptions_required")?;
    let assumptions = serde_json::from_str::<Value>(&assumptions_json)
        .map_err(|_| ServiceResponse::bad_request("robustness_assumptions_invalid"))?;
    if !assumptions.is_object() {
        return Err(ServiceResponse::bad_request(
            "robustness_assumptions_invalid",
        ));
    }
    let body = json!({
        "sourceRunId": source_run_id,
        "studyKind": study_kind,
        "assumptions": assumptions,
        "budget": {
            "maximumSamples": required_cli_value(args.maximum_samples, "robustness_budget_required")?,
            "maximumGridPoints": required_cli_value(args.maximum_grid_points, "robustness_budget_required")?,
            "maximumWindows": required_cli_value(args.maximum_windows, "robustness_budget_required")?,
            "maximumScenarios": required_cli_value(args.maximum_scenarios, "robustness_budget_required")?,
            "maximumOutputBytes": required_cli_value(args.maximum_output_bytes, "robustness_budget_required")?,
            "maximumAttempts": required_cli_value(args.maximum_attempts, "robustness_budget_required")?,
        },
        "deterministicSeed": required_cli_value(args.deterministic_seed, "robustness_seed_required")?,
        "idempotencyKey": required_cli_value(args.idempotency_key, "robustness_idempotency_key_required")?,
    });
    Ok(body)
}

fn robustness_run_request(
    service: &TradeAssemblyService,
    args: RobustnessRunRequestArgs,
    path: impl FnOnce(&str) -> String,
) -> ServiceResponse {
    let body = match args.request_file {
        Some(request_file) => match json_request_file_with_codes(
            &request_file,
            "robustness_request_unreadable",
            "robustness_request_invalid",
        ) {
            Ok(body) => body,
            Err(response) => return response,
        },
        None => json!({}),
    };
    service.handle_http_from_source("cli", "POST", &path(&args.run_id), body)
}

fn required_cli_value<T>(value: Option<T>, code: &'static str) -> Result<T, ServiceResponse> {
    value.ok_or_else(|| ServiceResponse::bad_request(code))
}

fn backtest_run_request(
    service: &TradeAssemblyService,
    operation: &str,
    args: BacktestRunRequestArgs,
    path: impl FnOnce(&str) -> String,
) -> ServiceResponse {
    let body = match args.request_file {
        Some(request_file) => match json_request_file(&request_file) {
            Ok(body) => body,
            Err(response) => return response,
        },
        None => json!({"idempotencyKey": format!("cli-backtest-{operation}-{}", args.run_id)}),
    };
    service.handle_http_from_source("cli", "POST", &path(&args.run_id), body)
}

fn json_request_file(path: &PathBuf) -> Result<Value, ServiceResponse> {
    json_request_file_with_codes(
        path,
        "backtest_request_unreadable",
        "backtest_request_invalid",
    )
}

fn json_request_file_with_codes(
    path: &PathBuf,
    unreadable_code: &'static str,
    invalid_code: &'static str,
) -> Result<Value, ServiceResponse> {
    let raw =
        fs::read_to_string(path).map_err(|_| ServiceResponse::bad_request(unreadable_code))?;
    serde_json::from_str(&raw).map_err(|_| ServiceResponse::bad_request(invalid_code))
}

fn plugin_package_request(args: &PluginPackageArgs) -> Value {
    let source_type = if args.source.starts_with("https://") {
        "https"
    } else {
        "file"
    };
    json!({
        "idempotencyKey": plugin_package_idempotency_key(args),
        "source": {
            "type": "package",
            "locator": args.source,
            "package": {
                "type": source_type,
                "locator": args.source,
            },
        },
        "integrity": {
            "packageSha256": args.package_sha256,
            "manifestSha256": args.manifest_sha256,
        },
        "offline": args.offline,
    })
}

fn plugin_package_idempotency_key(args: &PluginPackageArgs) -> String {
    format!(
        "plugin-package:{}:{}",
        args.package_sha256, args.manifest_sha256
    )
}

fn install_default_external_plugin(
    service: &TradeAssemblyService,
    args: PluginDefaultInstallArgs,
) -> Value {
    let request = match default_external_plugin_request(args) {
        Ok(request) => request,
        Err(response) => return response,
    };
    let invocation_id = format!(
        "{}-{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos()
    );
    service
        .handle_http_from_source(
            "cli",
            "POST",
            "/plugins/packages",
            scoped_plugin_package_request(&request, "cli-install-default", &invocation_id),
        )
        .body
}

pub fn install_default_external_plugin_for_local_setup(
    config: RuntimeConfig,
    package_source: Option<String>,
    offline: bool,
    skip_default_plugins: bool,
) -> Result<Value, String> {
    let request = match default_external_plugin_request(PluginDefaultInstallArgs {
        package_source,
        offline,
        skip_default_plugins,
    }) {
        Ok(request) => request,
        Err(response) => return Ok(response),
    };
    let service = TradeAssemblyService::from_config_without_local_scheduler_resume(config)?;
    let setup_invocation = format!(
        "{}-{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos()
    );
    let response = service
        .install_plugin_package_for_local_setup(local_setup_plugin_package_request(
            &request,
            &setup_invocation,
        ))?
        .body;
    if command_response_failed(&response) {
        return Err(response
            .pointer("/error/code")
            .and_then(Value::as_str)
            .or_else(|| response.get("error").and_then(Value::as_str))
            .or_else(|| response.pointer("/details/code").and_then(Value::as_str))
            .unwrap_or("default_plugin_install_failed")
            .to_string());
    }
    Ok(response)
}

fn local_setup_plugin_package_request(args: &PluginPackageArgs, invocation_id: &str) -> Value {
    scoped_plugin_package_request(args, "local-setup", invocation_id)
}

fn scoped_plugin_package_request(
    args: &PluginPackageArgs,
    scope: &str,
    invocation_id: &str,
) -> Value {
    let mut request = plugin_package_request(args);
    request["idempotencyKey"] = json!(format!(
        "{scope}:{}:{invocation_id}",
        plugin_package_idempotency_key(args)
    ));
    request
}

fn default_external_plugin_request(
    args: PluginDefaultInstallArgs,
) -> Result<PluginPackageArgs, Value> {
    if args.skip_default_plugins {
        return Err(json!({
            "ok": true,
            "status": "skipped",
            "reason": "not_selected",
            "optional": true,
        }));
    }
    if args.package_source.is_none() {
        if let Ok(executable) = std::env::current_exe() {
            if let Some(request) = bundled_default_plugin_request(&executable, args.offline) {
                return request;
            }
        }
    }
    let selections: Value = serde_json::from_str(include_str!(
        "../../../plugin-contracts/default-external-plugins.json"
    ))
    .expect("default external plugin metadata is valid JSON");
    let target = runtime_target();
    let Some(selection) = selections["plugins"].as_array().and_then(|plugins| {
        plugins
            .iter()
            .find(|plugin| plugin["selectedByDefault"] == true)
    }) else {
        return Err(json!({
            "ok": true,
            "status": "skipped",
            "reason": "no_default_selection",
            "optional": true,
        }));
    };
    let Some(package) = selection["targets"].as_array().and_then(|targets| {
        targets
            .iter()
            .find(|candidate| candidate["target"] == target)
    }) else {
        return Err(json!({
            "ok": true,
            "status": "skipped",
            "reason": "unsupported_target",
            "target": target,
            "pluginRef": selection["pluginRef"],
            "optional": true,
        }));
    };
    let source = args.package_source.unwrap_or_else(|| {
        package["packageUrl"]
            .as_str()
            .unwrap_or_default()
            .to_string()
    });
    Ok(PluginPackageArgs {
        source,
        package_sha256: package["packageSha256"]
            .as_str()
            .unwrap_or_default()
            .to_string(),
        manifest_sha256: package["manifestSha256"]
            .as_str()
            .unwrap_or_default()
            .to_string(),
        offline: args.offline,
    })
}

fn runtime_target() -> String {
    let arch = std::env::consts::ARCH;
    let os = match std::env::consts::OS {
        "macos" => "apple-darwin",
        "linux" => "unknown-linux-gnu",
        "windows" => "pc-windows-msvc",
        other => other,
    };
    format!("{arch}-{os}")
}

fn bundled_default_plugin_request(
    executable: &std::path::Path,
    offline: bool,
) -> Option<Result<PluginPackageArgs, Value>> {
    let bin = executable.parent()?;
    let root = bin.parent()?;
    if bin.file_name()? != "bin" || !root.join("bundle.json").is_file() {
        return None;
    }
    let pin: Value = serde_json::from_str(include_str!("../../../packaging/alpaca.json"))
        .expect("Alpaca bundle pin JSON");
    let package = root.join("plugins").join(pin["packageFile"].as_str()?);
    if !package.is_file() || pin["target"] != runtime_target() {
        return Some(Err(
            json!({"ok":false,"error":{"code":"bundled_plugin_unavailable"}}),
        ));
    }
    Some(Ok(PluginPackageArgs {
        source: package.display().to_string(),
        package_sha256: pin["packageSha256"].as_str()?.into(),
        manifest_sha256: pin["manifestSha256"].as_str()?.into(),
        offline,
    }))
}

fn plugin_json_file(path: &PathBuf) -> Result<Value, &'static str> {
    let raw = fs::read_to_string(path).map_err(|_| "plugin_request_unreadable")?;
    serde_json::from_str(&raw).map_err(|_| "plugin_request_invalid")
}

fn parse_bool_arg(value: &str) -> bool {
    matches!(
        value.trim().to_ascii_lowercase().as_str(),
        "1" | "true" | "yes" | "on"
    )
}

fn dataset_ingestion_request(args: DatasetIngestionCreateArgs) -> Value {
    let raw = match (args.request_json, args.request_file) {
        (Some(raw), None) => Ok(raw),
        (None, Some(path)) => fs::read_to_string(path).map_err(|_| "dataset_request_unreadable"),
        _ => Err("dataset_request_required"),
    };
    match raw.and_then(|raw| serde_json::from_str(&raw).map_err(|_| "dataset_request_invalid")) {
        Ok(body) => body,
        Err(code) => json!({"status": "failed", "error": {"code": code}, "noAdvice": true}),
    }
}

fn command_name(command: &Command) -> &'static str {
    match command {
        Command::Setup { .. } => "setup",
        Command::Policy { .. } => "policy",
        Command::Api { .. } => "api",
        Command::Auth { .. } => "auth",
        Command::Mcp { .. } => "mcp",
        Command::Strategy { .. } => "strategy",
        Command::Journal { .. } => "journal",
        Command::Providers { .. } => "providers",
        Command::Credentials { .. } => "credentials",
        Command::Account { .. } => "account",
        Command::Install { .. } => "install",
        Command::Telemetry { .. } => "telemetry",
        Command::Backtest(..) => "backtest",
        Command::Backtests { .. } => "backtests",
        Command::Robustness { .. } => "robustness",
        Command::Derivatives { .. } => "derivatives",
        Command::Comparison { .. } => "comparison",
        Command::Research { .. } => "research",
        Command::DatasetIngestion { .. } => "dataset-ingestion",
        Command::Marketdata { .. } => "marketdata",
        Command::Options { .. } => "options",
        Command::Scheduler { .. } => "scheduler",
        Command::Workers { .. } => "workers",
        Command::Orders { .. } => "orders",
        Command::Risk { .. } => "risk",
        Command::Positions => "positions",
        Command::Replay => "replay",
        Command::Demos { .. } => "demos",
        Command::Demo { .. } => "demo",
        Command::Seed => "seed",
        Command::SeedBtcDemo => "seed-btc-demo",
        Command::Reset => "reset",
        Command::ResetBtcDemo => "reset-btc-demo",
        Command::Readiness => "readiness",
        Command::Report => "report",
        Command::ReportEnvelope { .. } => "report-envelope",
        Command::Plugins { .. } => "plugins",
        Command::Instruments { .. } => "instruments",
        Command::Selectors { .. } => "selectors",
        Command::Scenario { .. } => "scenario",
        Command::MonteCarlo { .. } => "monte-carlo",
        Command::Lifecycle { .. } => "lifecycle",
        Command::LifecycleCalendar { .. } => "lifecycle-calendar",
        Command::FillQuality { .. } => "fill-quality",
        Command::AttributionJournal { .. } => "attribution-journal",
        Command::ResearchNotebook { .. } => "research-notebook",
        Command::Execution { .. } => "execution",
        Command::Events { .. } => "events",
        Command::Agent { .. } => "agent",
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn launchd_config_requires_explicit_durable_settings() {
        use super::*;
        let root = tempfile::tempdir().unwrap();
        let file = root.path().join("runtime.json");
        let layer = RuntimeConfigLayer {
            profile: Some("local".into()),
            oidc_profile: Some("local_owner".into()),
            database_path: Some(root.path().join("runtime.db").display().to_string()),
            warden_token_ref: Some(format!(
                "file://{}",
                root.path().join("warden.token").display()
            )),
            warden_sidecar_url: Some("http://127.0.0.1:19991".into()),
            ..RuntimeConfigLayer::default()
        };
        fs::write(&file, serde_json::to_vec(&layer).unwrap()).unwrap();
        let config = RuntimeConfig::resolve(
            Some(layer.clone()),
            RuntimeConfigLayer::default(),
            RuntimeConfigLayer::default(),
        )
        .unwrap();
        assert_eq!(
            launchd_config_path(&config, None).unwrap_err(),
            "launchd_runtime_config_required"
        );
        assert_eq!(
            launchd_config_path(&config, file.to_str()).unwrap(),
            file.canonicalize().unwrap()
        );
        let mut overridden = config.clone();
        overridden.warden_sidecar_url = "http://127.0.0.1:19992".into();
        assert_eq!(
            launchd_config_path(&overridden, file.to_str()).unwrap_err(),
            "launchd_runtime_config_has_shell_overrides"
        );
        let mut environmental = layer;
        environmental.warden_token_ref = Some("env://PRIVATE_TOKEN".into());
        fs::write(&file, serde_json::to_vec(&environmental).unwrap()).unwrap();
        let config = RuntimeConfig::resolve(
            Some(environmental),
            RuntimeConfigLayer::default(),
            RuntimeConfigLayer::default(),
        )
        .unwrap();
        assert_eq!(
            launchd_config_path(&config, file.to_str()).unwrap_err(),
            "launchd_environment_secret_not_durable"
        );
    }

    use super::{
        cli_owns_local_scheduler, command_response_failed, default_external_plugin_request,
        execute_command, launchd_output, local_setup_plugin_package_request,
        plugin_package_request, preflight_api_warden, resolved_mcp_studio_url,
        scoped_plugin_package_request, Command, McpCommand, PluginDefaultInstallArgs,
        PluginPackageArgs,
    };
    use crate::runtime_config::RuntimeConfig;
    use crate::service::TradeAssemblyService;
    use serde_json::json;
    use std::fs;
    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::thread;
    use tempfile::tempdir;

    fn warden_config(base_url: String, token_path: &std::path::Path) -> RuntimeConfig {
        let mut config = RuntimeConfig::local(":memory:");
        config.warden_sidecar_url = base_url;
        config.warden_token_ref = format!("file://{}", token_path.display());
        config.warden_required_version = "0.1.0".to_string();
        config
    }

    fn spawn_health_response(version: &'static str) -> String {
        let listener = TcpListener::bind("127.0.0.1:0").expect("mock Warden listener");
        let address = listener.local_addr().expect("mock Warden address");
        thread::spawn(move || {
            let (mut stream, _) = listener.accept().expect("mock Warden request");
            let mut request = [0_u8; 2048];
            let _ = stream.read(&mut request);
            let body =
                format!(r#"{{"status":"ok","service":"warden-sidecar","version":"{version}"}}"#);
            write!(
                stream,
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                body.len(),
                body
            )
            .expect("mock Warden response");
        });
        format!("http://{address}")
    }

    fn plugin_package_args(
        source: &str,
        package_sha256: char,
        manifest_sha256: char,
        offline: bool,
    ) -> PluginPackageArgs {
        PluginPackageArgs {
            source: source.to_string(),
            package_sha256: package_sha256.to_string().repeat(64),
            manifest_sha256: manifest_sha256.to_string().repeat(64),
            offline,
        }
    }

    #[test]
    fn command_failures_produce_nonzero_cli_status() {
        for response in [
            json!({"error": {"code": "finance_authority_denied"}}),
            json!({"ok": false, "blockers": ["missing_plugin"]}),
            json!({"status": "failed", "reason": "integrity_mismatch"}),
        ] {
            assert!(command_response_failed(&response), "{response}");
        }
        assert!(!command_response_failed(&json!({"ok": true})));
        assert!(!command_response_failed(
            &json!({"ok": true, "body": {"status": "complete"}, "error": null})
        ));
        assert!(!command_response_failed(
            &json!({"status": "skipped", "reason": "unsupported_target"})
        ));
    }

    #[test]
    fn launchd_lifecycle_output_uses_the_canonical_cli_envelope() {
        let (success, failed) = launchd_output(Ok(json!({"loaded": true})));
        assert!(!failed);
        assert_eq!(success["ok"], true);
        assert_eq!(success["data"]["launchd"]["loaded"], true);
        assert_eq!(success["warnings"], json!([]));
        assert_eq!(success["errors"], json!([]));
        assert_eq!(success["authority"]["surface"], "cli");
        assert_eq!(success["idempotency_key"], "cli:agent");

        let (failure, failed) = launchd_output(Err("launchd_unavailable".to_string()));
        assert!(failed);
        assert_eq!(failure["ok"], false);
        assert_eq!(failure["data"]["error"]["code"], "launchd_unavailable");
        assert_eq!(failure["warnings"], json!([]));
        assert_eq!(failure["errors"][0]["code"], "command_failed");
        assert_eq!(failure["authority"]["surface"], "cli");
        assert_eq!(failure["idempotency_key"], "cli:agent");
    }

    #[test]
    fn mcp_studio_url_respects_configuration_and_explicit_override() {
        let mut config = RuntimeConfig::local(":memory:");
        config.studio_base_url = Some("http://127.0.0.1:3004".to_string());
        assert_eq!(
            resolved_mcp_studio_url(None, &config),
            "http://127.0.0.1:3004"
        );
        assert_eq!(
            resolved_mcp_studio_url(Some(""), &config),
            "http://127.0.0.1:3004"
        );
        assert_eq!(
            resolved_mcp_studio_url(Some("http://127.0.0.1:3010"), &config),
            "http://127.0.0.1:3010"
        );
        config.studio_base_url = None;
        assert_eq!(
            resolved_mcp_studio_url(None, &config),
            "http://127.0.0.1:3000"
        );
    }

    #[test]
    fn only_the_api_process_owns_the_local_scheduler() {
        assert!(cli_owns_local_scheduler(&Command::Api {
            host: "127.0.0.1".to_string(),
            port: 8090,
        }));
        assert!(!cli_owns_local_scheduler(&Command::Mcp {
            command: McpCommand::Serve {
                transport: "stdio".to_string(),
                studio_base_url: None,
            },
        }));
        assert!(!cli_owns_local_scheduler(&Command::Seed));
    }

    #[test]
    fn seed_btc_demo_persists_immutable_version_for_a_fresh_service() {
        let directory = tempdir().expect("temporary runtime directory");
        let database = directory.path().join("tradeassembly.sqlite3");
        let token = directory.path().join("warden.token");
        fs::write(&token, "a".repeat(64)).expect("test Warden token");
        let service = || {
            let mut config = RuntimeConfig::local(database.to_string_lossy());
            config.warden_token_ref = format!("file://{}", token.display());
            config.artifact_root = Some(directory.path().join("exports").display().to_string());
            TradeAssemblyService::from_config_without_local_scheduler_resume(config)
                .expect("local test service")
        };
        let first = service();

        let _ = execute_command(&first, Command::SeedBtcDemo);
        let _ = execute_command(&first, Command::SeedBtcDemo);
        drop(first);

        let second = service();
        let versions = second
            .runtime()
            .storage
            .list_json("strategy_versions")
            .expect("durable strategy versions");
        assert!(
            versions.iter().any(|(_, version)| {
                version["strategyId"] == "strat_local_btc_demo"
                    && version["id"].as_str().is_some_and(|id| !id.is_empty())
                    && version["immutable"] == true
            }),
            "persisted versions: {versions:#?}"
        );
        assert_eq!(versions.len(), 1, "seed must remain idempotent");
    }

    #[test]
    fn bundled_default_install_is_pinned_and_missing_payload_does_not_fall_back() {
        let root = tempdir().unwrap();
        let executable = root.path().join("bin/tradeassembly");
        assert!(super::bundled_default_plugin_request(&executable, true).is_none());
        fs::write(root.path().join("bundle.json"), b"{}").unwrap();
        let error = super::bundled_default_plugin_request(&executable, true)
            .unwrap()
            .err()
            .unwrap();
        assert_eq!(error["error"]["code"], "bundled_plugin_unavailable");
        let pin: serde_json::Value =
            serde_json::from_str(include_str!("../../../packaging/alpaca.json")).unwrap();
        fs::create_dir(root.path().join("plugins")).unwrap();
        let package = root
            .path()
            .join("plugins")
            .join(pin["packageFile"].as_str().unwrap());
        fs::write(
            &package,
            b"fixture: digest verification occurs during install",
        )
        .unwrap();
        let request = super::bundled_default_plugin_request(&executable, true).unwrap();
        if pin["target"] != super::runtime_target() {
            assert!(request.is_err());
            return;
        }
        let request = request.unwrap();
        assert_eq!(request.source, package.display().to_string());
        assert_eq!(
            request.package_sha256,
            pin["packageSha256"].as_str().unwrap()
        );
        assert_eq!(
            request.manifest_sha256,
            pin["manifestSha256"].as_str().unwrap()
        );
        assert!(request.offline);
    }

    #[test]
    fn offline_default_install_reaches_digest_cache_without_source_override() {
        let request = default_external_plugin_request(PluginDefaultInstallArgs {
            package_source: None,
            offline: true,
            skip_default_plugins: false,
        })
        .expect("supported default package request");

        assert!(request.offline);
        assert!(!request.source.is_empty());
        assert_eq!(request.package_sha256.len(), 64);
        assert_eq!(request.manifest_sha256.len(), 64);
    }

    #[test]
    fn plugin_package_identity_is_stable_across_locator_and_offline_mode() {
        let local = plugin_package_args("/tmp/tradeassembly-plugin.tar.gz", 'a', 'b', true);
        let remote = plugin_package_args(
            "https://example.invalid/tradeassembly-plugin.tar.gz",
            'a',
            'b',
            false,
        );

        assert_eq!(
            plugin_package_request(&local)["idempotencyKey"],
            plugin_package_request(&remote)["idempotencyKey"]
        );
    }

    #[test]
    fn plugin_package_identity_changes_with_either_digest() {
        let package = plugin_package_args("/tmp/tradeassembly-plugin.tar.gz", 'a', 'b', false);
        let changed_package =
            plugin_package_args("/tmp/tradeassembly-plugin.tar.gz", 'c', 'b', false);
        let changed_manifest =
            plugin_package_args("/tmp/tradeassembly-plugin.tar.gz", 'a', 'd', false);
        let identity = plugin_package_request(&package)["idempotencyKey"].clone();

        assert_ne!(
            identity,
            plugin_package_request(&changed_package)["idempotencyKey"]
        );
        assert_ne!(
            identity,
            plugin_package_request(&changed_manifest)["idempotencyKey"]
        );
    }

    #[test]
    fn explicit_local_setup_run_uses_a_distinct_authorization_attempt() {
        let package = plugin_package_args("/tmp/tradeassembly-plugin.tar.gz", 'a', 'b', true);
        let standard = plugin_package_request(&package);
        let first = local_setup_plugin_package_request(&package, "setup-one");
        let retry = local_setup_plugin_package_request(&package, "setup-two");

        assert_ne!(first["idempotencyKey"], standard["idempotencyKey"]);
        assert_ne!(first["idempotencyKey"], retry["idempotencyKey"]);
        assert_eq!(first["integrity"], standard["integrity"]);
        assert_eq!(first["source"], standard["source"]);
    }

    #[test]
    fn explicit_cli_install_default_retry_revalidates_missing_source() {
        let directory = tempdir().expect("temporary runtime directory");
        let missing_source = directory.path().join("missing-tradeassembly-plugin.tar.gz");
        let package = plugin_package_args(&missing_source.to_string_lossy(), 'a', 'b', true);
        let standard = plugin_package_request(&package);
        let first = scoped_plugin_package_request(&package, "cli-install-default", "attempt-one");
        let retry = scoped_plugin_package_request(&package, "cli-install-default", "attempt-two");

        assert_ne!(first["idempotencyKey"], standard["idempotencyKey"]);
        assert_ne!(first["idempotencyKey"], retry["idempotencyKey"]);
        assert_eq!(first["integrity"], standard["integrity"]);
        assert_eq!(first["source"], standard["source"]);

        let database = directory.path().join("runtime.sqlite3");
        let service = TradeAssemblyService::test_local(database.to_string_lossy());
        let first_response =
            service.handle_http_from_source("cli", "POST", "/plugins/packages", first);
        let retry_response =
            service.handle_http_from_source("cli", "POST", "/plugins/packages", retry);

        assert_eq!(
            first_response.body["error"]["code"],
            "plugin_package_install_failed"
        );
        assert_eq!(
            retry_response.body["error"]["code"], "plugin_package_install_failed",
            "retry response changed error class: first={:?}, retry={:?}",
            first_response.body, retry_response.body
        );
        assert_ne!(
            retry_response.body["error"]["code"],
            "idempotency_replay_failed"
        );
    }

    #[test]
    fn api_preflight_requires_available_exact_warden_version() {
        let directory = tempdir().expect("temporary token directory");
        let token_path = directory.path().join("warden.token");
        fs::write(&token_path, "a".repeat(64)).expect("test token");

        let listener = TcpListener::bind("127.0.0.1:0").expect("unused port");
        let unavailable = format!("http://{}", listener.local_addr().expect("address"));
        drop(listener);
        assert_eq!(
            preflight_api_warden(&warden_config(unavailable, &token_path))
                .expect_err("unavailable Warden must block API startup"),
            "warden_unavailable"
        );

        let mismatch = spawn_health_response("9.9.9");
        assert!(preflight_api_warden(&warden_config(mismatch, &token_path))
            .expect_err("wrong Warden version must block API startup")
            .starts_with("warden_version_mismatch:"));

        let exact = spawn_health_response("0.1.0");
        preflight_api_warden(&warden_config(exact, &token_path))
            .expect("exact Warden health must allow API startup");
    }
}
