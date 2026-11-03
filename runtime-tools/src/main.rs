mod bundle_local;
mod plugin_contract;
fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let root = std::path::Path::new(".");
    let code = match args.first().map(String::as_str) {
        Some("bundle-local") => bundle_local::run(&args[1..], root),
        Some("plugin-contract") => plugin_contract::run_plugin_contract(&args[1..], root),
        _ => {
            eprintln!("Expected bundle-local or plugin-contract");
            2
        }
    };
    std::process::exit(code);
}
