// Copyright (c) 2026 OptionLab LLC. All rights reserved.

use clap::Parser;
use std::process;
use tradeassembly_runtime::cli::{
    run_with_service, runtime_config_from_cli, service_from_cli, Cli,
};

fn main() {
    let cli = Cli::parse();
    if let Some(code) = tradeassembly_runtime::cli::run_installation_command(&cli) {
        process::exit(code);
    }
    let config = match runtime_config_from_cli(&cli) {
        Ok(config) => config,
        Err(error) => {
            eprintln!("runtime configuration failed: {error}");
            process::exit(2);
        }
    };
    let service = match service_from_cli(&cli) {
        Ok(service) => service,
        Err(error) => {
            let runtime = tokio::runtime::Builder::new_multi_thread()
                .enable_all()
                .build()
                .expect("TradeAssembly async runtime initialization failed");
            if let Some(code) = runtime
                .block_on(async { tradeassembly_runtime::cli::run_setup_only_mcp(&cli, &config) })
            {
                process::exit(code);
            }
            eprintln!("runtime initialization failed: {error}");
            process::exit(2);
        }
    };
    let exit_code = {
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
            .expect("TradeAssembly async runtime initialization failed");
        runtime.block_on(run_with_service(cli, &service, &config))
    };
    process::exit(exit_code);
}
