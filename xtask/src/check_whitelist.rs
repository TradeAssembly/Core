// Copyright (c) 2026 OptionLab LLC. All rights reserved.

use globset::{Glob, GlobSet, GlobSetBuilder};
use serde_json::json;
use serde_yaml::Value;
use std::collections::BTreeMap;
use std::fmt;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};
use walkdir::WalkDir;

const DEFAULT_REPO_WHITELIST_FILES: [&str; 2] = [
    "docs/rust-source-whitelist.yaml",
    "tradeassembly-rust-source-whitelist.yaml",
];

const DEFAULT_IGNORE_PATTERNS: &[&str] = &[
    "**/.git",
    "**/.git/**",
    "**/.github",
    "**/.github/**",
    "**/.tradeassembly",
    "**/.tradeassembly/**",
    "**/.sdlc",
    "**/.sdlc/**",
    "**/target",
    "**/target/**",
    "**/node_modules",
    "**/node_modules/**",
    "**/.venv",
    "**/.venv/**",
    "**/.venv*",
    "**/.venv*/**",
    "**/vendor",
    "**/vendor/**",
    "**/dist",
    "**/dist/**",
    "**/.idea",
    "**/.idea/**",
    "**/tmp",
    "**/tmp/**",
    "**/.mypy_cache",
    "**/.mypy_cache/**",
    "**/.pytest_cache",
    "**/.pytest_cache/**",
    "**/.terraform",
    "**/.terraform/**",
    "**/build",
    "**/build/**",
    "**/.gradle",
    "**/.gradle/**",
];

const DEFAULT_REPORT_PATH: &str = ".sdlc/language-invariant-report.json";
const DEFAULT_MAX_FILES: usize = 250;

#[derive(Clone, Copy, Debug, PartialEq)]
enum OutputFormat {
    Text,
    Json,
}

impl OutputFormat {
    fn parse(value: &str) -> Option<Self> {
        match value {
            "text" => Some(Self::Text),
            "json" => Some(Self::Json),
            _ => None,
        }
    }
}

#[derive(Debug)]
struct CheckOptions {
    format: OutputFormat,
    max_files: Option<usize>,
    report_path: PathBuf,
    allow_dirty: bool,
}

pub struct RepoSpec {
    pub name: String,
    pub root: PathBuf,
    pub whitelist_path: PathBuf,
}

#[derive(Debug)]
struct SourcePolicy {
    allowed_by_language: BTreeMap<&'static str, GlobSet>,
    dependency_paths: GlobSet,
}

#[derive(Debug)]
struct Violation {
    path: String,
    language: &'static str,
    reason: String,
}

#[derive(Debug)]
struct RepoResult {
    name: String,
    root: String,
    total_violations: usize,
    language_totals: BTreeMap<&'static str, usize>,
    violations: Vec<Violation>,
}

#[derive(Debug)]
struct CheckReport {
    status: &'static str,
    generated_from: String,
    generated_at_unix: u64,
    options: BTreeMap<&'static str, String>,
    repositories: Vec<RepoResult>,
    total_violations: usize,
    language_totals: BTreeMap<&'static str, usize>,
}

#[derive(Debug)]
struct ScanResult {
    pub repo_name: String,
    pub repo_root: String,
    pub violations: Vec<Violation>,
}

pub fn run_check_whitelist(args: &[String], repo_root: &Path) -> i32 {
    let mut options = CheckOptions {
        format: OutputFormat::Text,
        max_files: None,
        report_path: PathBuf::from(DEFAULT_REPORT_PATH),
        allow_dirty: false,
    };

    let mut repo_args = Vec::new();
    let mut i = 0usize;

    while i < args.len() {
        match args[i].as_str() {
            "--format" => {
                let Some(value) = args.get(i + 1) else {
                    eprintln!("--format requires text or json");
                    return 1;
                };
                options.format = match OutputFormat::parse(value) {
                    Some(format) => format,
                    None => {
                        eprintln!("invalid --format value: {value}");
                        return 1;
                    }
                };
                i += 2;
            }
            "--json" => {
                options.format = OutputFormat::Json;
                i += 1;
            }
            "--max-files" => {
                let Some(value) = args.get(i + 1) else {
                    eprintln!("--max-files requires an integer");
                    return 1;
                };
                options.max_files = match value.parse::<usize>() {
                    Ok(parsed) => Some(parsed),
                    Err(_) => {
                        eprintln!("invalid --max-files value: {value}");
                        return 1;
                    }
                };
                i += 2;
            }
            "--report" => {
                let Some(path) = args.get(i + 1) else {
                    eprintln!("--report requires a path");
                    return 1;
                };
                options.report_path = PathBuf::from(path);
                i += 2;
            }
            "--allow-dirty" => {
                options.allow_dirty = true;
                i += 1;
            }
            arg => {
                repo_args.push(arg.to_string());
                i += 1;
            }
        }
    }

    let repos = parse_repo_specs(&repo_args, repo_root);
    if repos.is_empty() {
        eprintln!("no repositories configured for check-whitelist");
        return 1;
    }

    let mut repo_results = Vec::new();
    let mut all_language_totals: BTreeMap<&'static str, usize> = BTreeMap::new();
    let mut total_violations = 0usize;

    for repo in repos {
        match scan_repo(&repo, options.allow_dirty) {
            Ok(result) => {
                total_violations += result.violations.len();
                let mut language_totals: BTreeMap<&'static str, usize> = BTreeMap::new();
                for violation in &result.violations {
                    *language_totals.entry(violation.language).or_insert(0) += 1;
                    *all_language_totals.entry(violation.language).or_insert(0) += 1;
                }

                if options.format == OutputFormat::Text {
                    if result.violations.is_empty() {
                        println!("OK: {} ({})", result.repo_name, result.repo_root);
                    } else {
                        println!(
                            "{}: {} violation(s)",
                            result.repo_name,
                            result.violations.len()
                        );
                        let shown = options.max_files.unwrap_or(result.violations.len());
                        for violation in result.violations.iter().take(shown) {
                            println!(
                                "  - {} [{}] {}",
                                violation.path, violation.language, violation.reason
                            );
                        }
                        if shown < result.violations.len() {
                            println!(
                                "  - ... and {} additional file(s) omitted; rerun with --max-files={} to view",
                                result.violations.len() - shown,
                                result.violations.len()
                            );
                        }
                    }
                }

                repo_results.push(RepoResult {
                    name: result.repo_name,
                    root: result.repo_root,
                    total_violations: result.violations.len(),
                    language_totals,
                    violations: result.violations,
                });
            }
            Err(error) => {
                eprintln!("failed to check {}: {}", repo.name, error);
                return 1;
            }
        }
    }

    let has_violation = total_violations > 0;
    let mut options_map = BTreeMap::new();
    options_map.insert(
        "format",
        match options.format {
            OutputFormat::Text => "text".to_string(),
            OutputFormat::Json => "json".to_string(),
        },
    );
    options_map.insert(
        "max_files",
        options.max_files.unwrap_or(DEFAULT_MAX_FILES).to_string(),
    );
    options_map.insert(
        "report_path",
        options.report_path.to_string_lossy().to_string(),
    );
    options_map.insert("allow_dirty", options.allow_dirty.to_string());

    let report = CheckReport {
        status: if has_violation { "failed" } else { "passed" },
        generated_from: repo_root.to_string_lossy().to_string(),
        generated_at_unix: SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|time| time.as_secs())
            .unwrap_or(0),
        options: options_map,
        repositories: repo_results,
        total_violations,
        language_totals: all_language_totals,
    };

    let report_payload = json!({
        "status": report.status,
        "generated_from": report.generated_from,
        "generated_at_unix": report.generated_at_unix,
        "options": report.options,
        "total_violations": report.total_violations,
        "language_totals": report.language_totals,
        "repositories": report.repositories.iter().map(|repo| json!({
            "name": repo.name,
            "root": repo.root,
            "total_violations": repo.total_violations,
            "language_totals": repo.language_totals,
            "violations": repo.violations.iter().map(|violation| json!({
                "path": violation.path,
                "language": violation.language,
                "reason": violation.reason,
            })).collect::<Vec<_>>(),
        })).collect::<Vec<_>>(),
    });

    if let Some(parent) = options.report_path.parent() {
        let _ = fs::create_dir_all(parent);
    }
    if let Err(error) = fs::write(
        &options.report_path,
        serde_json::to_string_pretty(&report_payload).unwrap_or_else(|write_error| {
            eprintln!("failed to format report: {write_error}");
            "{}".to_string()
        }),
    ) {
        eprintln!(
            "failed to write report {}: {error}",
            options.report_path.display()
        );
        return 1;
    }

    if options.format == OutputFormat::Json {
        match serde_json::to_string_pretty(&report_payload) {
            Ok(serialized) => println!("{serialized}"),
            Err(error) => {
                eprintln!("failed to serialize check report: {error}");
                return 1;
            }
        }
    }

    if has_violation {
        eprintln!("Hard-language invariant failed. Update whitelist/dependency policy or rewrite.");
        1
    } else {
        println!(
            "language whitelist invariant passed (python: {}, ruby: {})",
            report
                .language_totals
                .get("python")
                .copied()
                .unwrap_or(0usize),
            report
                .language_totals
                .get("ruby")
                .copied()
                .unwrap_or(0usize)
        );
        0
    }
}

fn parse_repo_specs(raw: &[String], repo_root: &Path) -> Vec<RepoSpec> {
    let mut explicit: Vec<RepoSpec> = Vec::new();
    let mut current: Option<RepoSpec> = None;
    let mut i = 0usize;
    let mut had_unknown = false;

    while i < raw.len() {
        let arg = &raw[i];
        match arg.as_str() {
            "--repo" => {
                if let Some(repo) = current.take() {
                    explicit.push(repo);
                }

                let Some(raw_root) = raw.get(i + 1) else {
                    eprintln!("--repo requires a path argument");
                    return Vec::new();
                };

                let root = resolve_path(raw_root, repo_root);
                let whitelist_path = find_default_whitelist(&root);

                current = Some(RepoSpec {
                    name: repo_name_for_root(&root),
                    root,
                    whitelist_path,
                });
                i += 2;
            }
            "--whitelist" => {
                if let Some(mut repo) = current.take() {
                    if let Some(raw_whitelist) = raw.get(i + 1) {
                        repo.whitelist_path = resolve_path(raw_whitelist, &repo.root);
                        explicit.push(repo);
                        i += 2;
                    } else {
                        eprintln!("--whitelist requires a path argument");
                        return Vec::new();
                    }
                } else {
                    eprintln!("--whitelist requires a preceding --repo");
                    return Vec::new();
                }
            }
            _ => {
                eprintln!("unknown argument: {arg}");
                had_unknown = true;
                i += 1;
            }
        }
    }

    if had_unknown {
        return Vec::new();
    }

    if let Some(repo) = current.take() {
        explicit.push(repo);
    }

    if !explicit.is_empty() {
        return explicit;
    }

    default_repo_specs(repo_root)
}

fn resolve_path(value: &str, base: &Path) -> PathBuf {
    let raw = PathBuf::from(value);
    if raw.is_absolute() {
        raw
    } else {
        base.join(raw)
    }
}

fn repo_name_for_root(root: &Path) -> String {
    root.file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("repo")
        .to_string()
}

fn default_repo_specs(repo_root: &Path) -> Vec<RepoSpec> {
    let product_root = repo_root
        .to_path_buf()
        .canonicalize()
        .unwrap_or_else(|_| repo_root.to_path_buf());

    let mut specs = Vec::new();

    if let Some(parent) = product_root.parent() {
        let current = product_root
            .file_name()
            .and_then(|segment| segment.to_str())
            .unwrap_or_default();
        for entry in read_workspace_repos(parent) {
            let name = repo_name_for_root(&entry);
            if name != current && !is_known_opmam_repo(&name) {
                continue;
            }

            if has_whitelist_file(&entry) {
                let whitelist_path = find_default_whitelist(&entry);
                specs.push(RepoSpec {
                    name,
                    root: entry.clone(),
                    whitelist_path,
                });
            }
        }
    }

    if specs.is_empty() {
        specs.push(RepoSpec {
            name: repo_name_for_root(&product_root),
            root: product_root.clone(),
            whitelist_path: find_default_whitelist(&product_root),
        });
    }

    specs.sort_by(|a, b| a.name.cmp(&b.name));

    specs
}

fn read_workspace_repos(parent: &Path) -> Vec<PathBuf> {
    let mut repos = Vec::new();
    let entries = match fs::read_dir(parent) {
        Ok(entries) => entries,
        Err(_) => return repos,
    };

    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }

        let name = match path.file_name().and_then(|segment| segment.to_str()) {
            Some(name) => name,
            None => continue,
        };

        if !is_known_opmam_repo(name) {
            continue;
        }

        repos.push(path);
    }

    repos
}

fn is_known_opmam_repo(name: &str) -> bool {
    name == "TradeAssembly-Product"
        || name == "TradeAssembly"
        || name.starts_with("TradeAssembly-Product-")
}

fn has_whitelist_file(root: &Path) -> bool {
    DEFAULT_REPO_WHITELIST_FILES
        .iter()
        .any(|candidate| root.join(candidate).exists())
}

fn find_default_whitelist(root: &Path) -> PathBuf {
    for candidate in DEFAULT_REPO_WHITELIST_FILES {
        if root.join(candidate).exists() {
            return PathBuf::from(candidate);
        }
    }
    PathBuf::from("docs/rust-source-whitelist.yaml")
}

fn scan_repo(spec: &RepoSpec, allow_dirty: bool) -> Result<ScanResult, ScanError> {
    let policy = SourcePolicy::from_yaml(&spec.root.join(&spec.whitelist_path))?;
    let mut violations = Vec::new();

    if !allow_dirty {
        if let Some(dirty_status) = git_repo_dirty_status(&spec.root)? {
            violations.push(Violation {
                path: ".".to_string(),
                language: "repository",
                reason: format!(
                    "repository has uncommitted changes; commit/merge before relying on language invariant: {dirty_status}"
                ),
            });
        }
    }

    for entry in WalkDir::new(&spec.root) {
        let entry = entry.map_err(ScanError::WalkDir)?;
        if entry.file_type().is_dir() {
            continue;
        }

        let relative = entry
            .path()
            .strip_prefix(&spec.root)
            .unwrap_or(entry.path())
            .to_string_lossy()
            .replace('\\', "/");

        if policy.is_dependency(&relative) {
            continue;
        }

        let Some(language) = classify_file(entry.path(), &relative) else {
            continue;
        };

        if !policy.allows(language, &relative) {
            violations.push(Violation {
                path: relative,
                language,
                reason: format!("{language} not in whitelist or dependency policy"),
            });
        }
    }

    Ok(ScanResult {
        repo_name: spec.name.clone(),
        repo_root: spec.root.to_string_lossy().to_string(),
        violations,
    })
}

fn git_repo_dirty_status(root: &Path) -> Result<Option<String>, ScanError> {
    if !root.join(".git").exists() {
        return Ok(None);
    }

    let output = Command::new("git")
        .args([
            "status",
            "--porcelain=v1",
            "--untracked-files=all",
            "--",
            ".",
            ":(exclude).sdlc/language-invariant-report.json",
            ":(exclude)xtask/.sdlc/language-invariant-report.json",
        ])
        .current_dir(root)
        .output()
        .map_err(ScanError::Io)?;

    if !output.status.success() {
        return Ok(None);
    }

    if output.stdout.is_empty() {
        return Ok(None);
    }

    let status = String::from_utf8_lossy(&output.stdout)
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .take(10)
        .collect::<Vec<_>>()
        .join("; ");
    Ok(Some(status))
}

fn classify_file(path: &Path, _relative: &str) -> Option<&'static str> {
    if let Some(file_name) = path.file_name().and_then(|name| name.to_str()) {
        if file_name == "Gemfile" || file_name == "Rakefile" || file_name == "config.ru" {
            return Some("ruby");
        }
    }

    if let Some(ext) = path.extension().and_then(|ext| ext.to_str()) {
        match ext.to_ascii_lowercase().as_str() {
            "rb" | "rake" | "gemspec" | "ru" => return Some("ruby"),
            "py" | "pyi" => return Some("python"),
            _ => {}
        }
    }

    let mut file = match fs::File::open(path) {
        Ok(file) => file,
        Err(_) => return None,
    };

    let mut buffer = [0u8; 512];
    let count = match std::io::Read::read(&mut file, &mut buffer) {
        Ok(count) => count,
        Err(_) => return None,
    };

    if count == 0 {
        return None;
    }

    if buffer.first().is_some_and(|byte| *byte != b'#') {
        return None;
    }

    let first_line = String::from_utf8_lossy(&buffer[..count])
        .lines()
        .next()
        .unwrap_or_default()
        .to_lowercase();

    if !first_line.starts_with("#!") {
        return None;
    }

    if first_line.contains("python") {
        return Some("python");
    }

    if first_line.contains("ruby") {
        return Some("ruby");
    }

    None
}

impl SourcePolicy {
    fn from_yaml(path: &Path) -> Result<Self, ScanError> {
        let contents = fs::read_to_string(path).map_err(ScanError::Io)?;
        let config: Value = serde_yaml::from_str(&contents).map_err(ScanError::Yaml)?;

        let allowed = config.get("allowed").cloned().unwrap_or_default();
        let forbidden = config.get("forbidden").cloned().unwrap_or_default();

        let mut allowed_by_language = BTreeMap::new();
        for language in [
            "rust",
            "typescript",
            "javascript",
            "python",
            "shell",
            "ruby",
        ] {
            let patterns = extract_language_patterns(&allowed, language);
            if !patterns.is_empty() {
                allowed_by_language
                    .insert(language, compile_globs(&patterns).map_err(ScanError::Glob)?);
            }
        }

        let mut dependency_patterns =
            extract_patterns(&forbidden, &["ignore_paths", "dependency_paths"]);

        dependency_patterns.extend(DEFAULT_IGNORE_PATTERNS.iter().copied().map(str::to_string));

        let dependency_paths = compile_globs(&dependency_patterns).map_err(ScanError::Glob)?;

        Ok(Self {
            allowed_by_language,
            dependency_paths,
        })
    }

    fn allows(&self, language: &'static str, path: &str) -> bool {
        self.allowed_by_language
            .get(language)
            .is_some_and(|set| set.is_match(path))
    }

    fn is_dependency(&self, path: &str) -> bool {
        self.dependency_paths.is_match(path)
    }
}

fn extract_language_patterns(config: &Value, language: &str) -> Vec<String> {
    match config.get(language) {
        Some(value) => normalize_pattern_block(value),
        None => Vec::new(),
    }
}

fn extract_patterns(config: &Value, keys: &[&str]) -> Vec<String> {
    keys.iter().fold(Vec::new(), |mut acc, key| {
        if let Some(block) = config.get(*key) {
            acc.extend(normalize_pattern_block(block));
        }
        acc
    })
}

fn normalize_pattern_block(node: &Value) -> Vec<String> {
    match node {
        Value::String(pattern) => vec![pattern.to_string()],
        Value::Sequence(items) => items
            .iter()
            .filter_map(|value| value.as_str().map(str::to_string))
            .collect(),
        Value::Mapping(map) => {
            if let Some(paths) = map.get("allowed_paths").or_else(|| map.get("paths")) {
                return normalize_pattern_block(paths);
            }
            Vec::new()
        }
        _ => Vec::new(),
    }
}

fn compile_globs(patterns: &[String]) -> Result<GlobSet, String> {
    let mut builder = GlobSetBuilder::new();
    for pattern in patterns {
        builder
            .add(Glob::new(pattern).map_err(|error| format!("invalid glob '{pattern}': {error}"))?);
    }
    builder
        .build()
        .map_err(|error| format!("globset build error: {error}"))
}

#[derive(Debug)]
enum ScanError {
    Io(std::io::Error),
    Yaml(serde_yaml::Error),
    Glob(String),
    WalkDir(walkdir::Error),
}

impl fmt::Display for ScanError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ScanError::Io(err) => write!(f, "{err}"),
            ScanError::Yaml(err) => write!(f, "{err}"),
            ScanError::Glob(err) => write!(f, "{err}"),
            ScanError::WalkDir(err) => write!(f, "{err}"),
        }
    }
}

impl std::error::Error for ScanError {}

impl From<std::io::Error> for ScanError {
    fn from(value: std::io::Error) -> Self {
        Self::Io(value)
    }
}

impl From<serde_yaml::Error> for ScanError {
    fn from(value: serde_yaml::Error) -> Self {
        Self::Yaml(value)
    }
}

impl From<walkdir::Error> for ScanError {
    fn from(value: walkdir::Error) -> Self {
        Self::WalkDir(value)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn with_temp_dir<F>(name: &str, f: F) -> bool
    where
        F: FnOnce(&Path),
    {
        let root = std::env::temp_dir().join(format!("tradeassembly-whitelist-test-{name}"));
        if root.exists() {
            let _ = fs::remove_dir_all(&root);
        }
        fs::create_dir_all(root.join("src")).is_ok() && {
            f(&root);
            true
        }
    }

    fn add_temp_report_arg(args: &mut Vec<String>, root: &Path) {
        args.push("--report".to_string());
        args.push(
            root.join("language-report.json")
                .to_string_lossy()
                .to_string(),
        );
    }

    #[test]
    fn test_violations_found_for_non_whitelisted_python() {
        let result = with_temp_dir("non_whitelisted_python", |root| {
            let _ = fs::create_dir_all(root.join("docs"));
            let _ = fs::write(root.join("src/main.rs"), b"fn main() {}\n");
            let _ = fs::write(root.join("src/bridge.py"), b"print('hi')\n");
            let _ = fs::write(
                root.join("docs/rust-source-whitelist.yaml"),
                b"allowed:\n  rust:\n    - \"**/*.rs\"\nforbidden:\n  ignore_paths:\n    - \"**/.venv/**\"\n",
            );

            let mut args = vec![
                "--repo".to_string(),
                root.to_string_lossy().to_string(),
                "--whitelist".to_string(),
                "docs/rust-source-whitelist.yaml".to_string(),
                "--format".to_string(),
                "json".to_string(),
                "--max-files".to_string(),
                "10".to_string(),
            ];
            add_temp_report_arg(&mut args, root);

            assert_eq!(run_check_whitelist(&args, root), 1);
        });
        assert!(result);
    }

    #[test]
    fn test_allows_whitelisted_files_only() {
        let result = with_temp_dir("whitelisted_only", |root| {
            let _ = fs::create_dir_all(root.join("docs"));
            let _ = fs::write(root.join("src/main.rs"), b"fn main() {}\n");
            let _ = fs::write(
                root.join("docs/rust-source-whitelist.yaml"),
                b"allowed:\n  rust:\n    - \"**/*.rs\"\nforbidden:\n  ignore_paths:\n    - \"**/.venv/**\"\n",
            );

            let mut args = vec![
                "--repo".to_string(),
                root.to_string_lossy().to_string(),
                "--whitelist".to_string(),
                "docs/rust-source-whitelist.yaml".to_string(),
                "--format".to_string(),
                "text".to_string(),
            ];
            add_temp_report_arg(&mut args, root);

            assert_eq!(run_check_whitelist(&args, root), 0);
        });
        assert!(result);
    }

    #[test]
    fn test_ignores_local_tradeassembly_state() {
        let result = with_temp_dir("local_tradeassembly_state", |root| {
            let _ = fs::create_dir_all(root.join("docs"));
            let _ = fs::create_dir_all(root.join(".tradeassembly/plugins/source"));
            let _ = fs::write(root.join("src/main.rs"), b"fn main() {}\n");
            let _ = fs::write(
                root.join(".tradeassembly/plugins/source/plugin.py"),
                b"print('generated plugin artifact')\n",
            );
            let _ = fs::write(
                root.join("docs/rust-source-whitelist.yaml"),
                b"allowed:\n  rust:\n    - \"**/*.rs\"\n",
            );

            let mut args = vec![
                "--repo".to_string(),
                root.to_string_lossy().to_string(),
                "--whitelist".to_string(),
                "docs/rust-source-whitelist.yaml".to_string(),
                "--format".to_string(),
                "text".to_string(),
            ];
            add_temp_report_arg(&mut args, root);

            assert_eq!(run_check_whitelist(&args, root), 0);
        });
        assert!(result);
    }

    #[test]
    fn test_classifies_implicit_python_and_ruby_files() {
        let result = with_temp_dir("implicit_language_scripts", |root| {
            let _ = fs::create_dir_all(root.join("scripts"));
            let _ = fs::write(
                root.join("scripts/runner"),
                b"#!/usr/bin/env python3\nprint('runner')\n",
            );
            let _ = fs::write(
                root.join("scripts/agent"),
                b"#!/usr/bin/env ruby\nputs 'agent'\n",
            );
            let _ = fs::write(root.join("Gemfile"), b"source 'https://rubygems.org'\n");
            let _ = fs::create_dir_all(root.join(".tradeassembly/plugins/source"));
            let _ = fs::write(
                root.join(".tradeassembly/plugins/source/task.py"),
                b"print('generated')\n",
            );

            let _ = fs::write(
                root.join("docs/rust-source-whitelist.yaml"),
                b"allowed:\n  rust:\n    - \"**/*.rs\"\nforbidden:\n  ignore_paths:\n    - \"**/.venv/**\"\n  dependency_paths:\n    - \".tradeassembly/**\"\n",
            );

            let mut args = vec![
                "--repo".to_string(),
                root.to_string_lossy().to_string(),
                "--whitelist".to_string(),
                "docs/rust-source-whitelist.yaml".to_string(),
                "--format".to_string(),
                "text".to_string(),
            ];
            add_temp_report_arg(&mut args, root);

            assert_eq!(run_check_whitelist(&args, root), 1);
        });
        assert!(result);
    }

    #[test]
    fn test_markdown_python_heading_is_documentation_not_python() {
        let result = with_temp_dir("markdown_python_heading", |root| {
            let _ = fs::create_dir_all(root.join("docs"));
            let _ = fs::write(root.join("src/main.rs"), b"fn main() {}\n");
            let _ = fs::write(
                root.join("docs/python-runtime-migration.md"),
                b"# Python Runtime Migration History\n\nHistorical evidence.\n",
            );
            let _ = fs::write(
                root.join("docs/rust-source-whitelist.yaml"),
                b"allowed:\n  rust:\n    - \"**/*.rs\"\n",
            );

            let mut args = vec![
                "--repo".to_string(),
                root.to_string_lossy().to_string(),
                "--whitelist".to_string(),
                "docs/rust-source-whitelist.yaml".to_string(),
                "--format".to_string(),
                "text".to_string(),
            ];
            add_temp_report_arg(&mut args, root);

            assert_eq!(run_check_whitelist(&args, root), 0);
        });
        assert!(result);
    }

    #[test]
    fn test_dirty_git_repo_fails_before_language_scan_can_pass() {
        let result = with_temp_dir("dirty_git_repo", |root| {
            let _ = fs::create_dir_all(root.join("docs"));
            let _ = fs::write(root.join("src/main.rs"), b"fn main() {}\n");
            let _ = fs::write(
                root.join("docs/rust-source-whitelist.yaml"),
                b"allowed:\n  rust:\n    - \"**/*.rs\"\n",
            );

            let _ = std::process::Command::new("git")
                .arg("init")
                .arg("--quiet")
                .current_dir(root)
                .status();
            let _ = std::process::Command::new("git")
                .args(["add", "."])
                .current_dir(root)
                .status();
            let _ = std::process::Command::new("git")
                .args([
                    "-c",
                    "user.email=tradeassembly@example.invalid",
                    "-c",
                    "user.name=TradeAssembly Test",
                    "commit",
                    "--quiet",
                    "-m",
                    "baseline",
                ])
                .current_dir(root)
                .status();
            let _ = fs::write(root.join("src/uncommitted.rs"), b"pub fn changed() {}\n");

            let mut args = vec![
                "--repo".to_string(),
                root.to_string_lossy().to_string(),
                "--whitelist".to_string(),
                "docs/rust-source-whitelist.yaml".to_string(),
                "--format".to_string(),
                "text".to_string(),
            ];
            add_temp_report_arg(&mut args, root);

            assert_eq!(run_check_whitelist(&args, root), 1);
        });
        assert!(result);
    }

    #[test]
    fn test_generated_language_reports_do_not_dirty_the_gate() {
        let result = with_temp_dir("generated_language_reports", |root| {
            let _ = fs::create_dir_all(root.join("docs"));
            let _ = fs::create_dir_all(root.join(".sdlc"));
            let _ = fs::create_dir_all(root.join("xtask/.sdlc"));
            let _ = fs::write(root.join("src/main.rs"), b"fn main() {}\n");
            let _ = fs::write(root.join(".sdlc/language-invariant-report.json"), b"{}\n");
            let _ = fs::write(
                root.join("xtask/.sdlc/language-invariant-report.json"),
                b"{}\n",
            );
            let _ = fs::write(
                root.join("docs/rust-source-whitelist.yaml"),
                b"allowed:\n  rust:\n    - \"**/*.rs\"\n",
            );

            let _ = std::process::Command::new("git")
                .arg("init")
                .arg("--quiet")
                .current_dir(root)
                .status();
            let _ = std::process::Command::new("git")
                .args(["add", "docs", "src"])
                .current_dir(root)
                .status();
            let _ = std::process::Command::new("git")
                .args([
                    "-c",
                    "user.email=tradeassembly@example.invalid",
                    "-c",
                    "user.name=TradeAssembly Test",
                    "commit",
                    "--quiet",
                    "-m",
                    "baseline",
                ])
                .current_dir(root)
                .status();

            let mut args = vec![
                "--repo".to_string(),
                root.to_string_lossy().to_string(),
                "--whitelist".to_string(),
                "docs/rust-source-whitelist.yaml".to_string(),
                "--format".to_string(),
                "text".to_string(),
            ];
            add_temp_report_arg(&mut args, root);

            assert_eq!(run_check_whitelist(&args, root), 0);
        });
        assert!(result);
    }
}
