// Copyright (c) 2026 OptionLab LLC. All rights reserved.

use regex::Regex;
use sha2::{Digest, Sha256};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::OnceLock;

const COPYRIGHT_NOTICE: &str = "Copyright (c) 2026 OptionLab LLC. All rights reserved.";
pub const COPYRIGHT_CATEGORY: &str = "missing copyright notice";

const DEFAULT_SCAN_TARGETS: &[&str] = &[
    ".github",
    ".sdlc",
    "AGENTS.md",
    "LICENSE",
    "Justfile",
    "README.md",
    "deploy",
    "dev",
    "docker-compose.distributed.yml",
    "docs",
    "examples",
    "infra",
    "plugin-contracts",
    "packaging",
    "identity-sdk",
    "plugin-sdk",
    "sightline-sidecar",
    "render.yaml",
    "runtime-rs",
    "runtime",
    "scripts",
    "xtask",
    "webapp",
];

const EXCLUDED_PARTS: &[&str] = &[
    ".git",
    ".mypy_cache",
    ".next",
    ".pytest_cache",
    ".terraform",
    ".venv",
    ".venv-research",
    "__pycache__",
    "dist",
    "node_modules",
    "out",
    "playwright-report",
    "test-results",
];

const EXCLUDED_PREFIXES: &[&str] = &[
    ".sdlc/imports/",
    ".sdlc/run-packets/",
    "runtime/tests/",
    "webapp/package-lock.json",
    "webapp/tests/",
];

const SOURCE_PREFIXES: &[&str] = &["runtime-rs/src/", "runtime/src/", "webapp/"];
const USER_SURFACE_PREFIXES: &[&str] = &[
    "runtime-rs/src/",
    "runtime/src/",
    "webapp/app/",
    "webapp/components/",
    "webapp/lib/",
];
const DOC_PREFIXES: &[&str] = &["docs/"];
const DEPLOYMENT_PREFIXES: &[&str] = &["deploy/", "infra/"];
const USER_SURFACE_EXACT: &[&str] = &["README.md"];
const DEPLOYMENT_EXACT: &[&str] = &["Justfile", "docker-compose.distributed.yml", "render.yaml"];

const BINARY_SUFFIXES: &[&str] = &[
    ".db", ".gif", ".ico", ".jpg", ".jpeg", ".lock", ".pdf", ".png", ".pyc", ".sqlite", ".sqlite3",
    ".woff", ".woff2",
];

const COPYRIGHT_INCLUDED_CLASSES: &[&str] = &[
    "Rust source under runtime-rs and xtask plus Python source/tests under runtime/src, runtime/scripts, runtime/tests, and authored infra helpers",
    "TypeScript, TSX, MJS, and CSS source under webapp app/components/lib/tests plus authored webapp config",
    "Shell entrypoints under deploy, dev, infra/scripts, infra/tofu/scripts, and scripts",
    "IaC/config authored by this repo: YAML, TOML, HCL/Terraform, env examples, Dockerfiles, Justfile, and static HTML/CSS source",
];
const COPYRIGHT_EXCLUDED_CLASSES: &[&str] = &[
    "Markdown documentation and existing LICENSE text",
    "Generated/package metadata that cannot carry comments: JSON manifests, package locks, and Terraform lockfiles",
    "Generated or visual fixture assets: screenshots, SVG, bitmap images, and Playwright snapshots",
    "Third-party/generated templates or format-sensitive files where a leading notice can alter output semantics",
    "Immutable cross-repository imports whose bytes are pinned by provenance hash",
    "Transient SDLC run packets and build/cache output",
];
const COPYRIGHT_INCLUDE_PREFIXES: &[&str] = &[
    ".github/",
    ".sdlc/",
    "deploy/",
    "dev/",
    "infra/",
    "plugin-contracts/",
    "runtime/",
    "scripts/",
    "webapp/",
];
const COPYRIGHT_INCLUDE_EXACT: &[&str] = &[
    ".dockerignore",
    ".gitignore",
    ".gitleaksignore",
    "Justfile",
    "docker-compose.distributed.yml",
    "render.yaml",
    "runtime/Dockerfile",
    "webapp/Dockerfile",
];
const COPYRIGHT_INCLUDE_SUFFIXES: &[&str] = &[
    ".cfg",
    ".css",
    ".dockerignore",
    ".env.example",
    ".example.env",
    ".gitignore",
    ".gitleaksignore",
    ".hcl",
    ".html",
    ".mjs",
    ".py",
    ".rs",
    ".sh",
    ".tf",
    ".tfvars.example",
    ".toml",
    ".ts",
    ".tsx",
    ".yaml",
    ".yml",
];
const COPYRIGHT_EXCLUDE_PREFIXES: &[&str] = &[
    ".sdlc/imports/",
    ".sdlc/run-packets/",
    "webapp/tests/e2e/visual-layout.spec.ts-snapshots/",
];
const COPYRIGHT_EXCLUDE_EXACT: &[&str] = &[
    "LICENSE",
    "webapp/package-lock.json",
    "webapp/package.json",
    "webapp/tsconfig.json",
];
const COPYRIGHT_EXCLUDE_SUFFIXES: &[&str] = &[
    ".j2",
    ".json",
    ".lock",
    ".md",
    ".pdf",
    ".png",
    ".svg",
    ".terraform.lock.hcl",
];

#[derive(Clone, Debug, PartialEq)]
pub struct PublicScanFinding {
    pub category: String,
    pub path: String,
    pub line: usize,
    pub excerpt: String,
}

impl PublicScanFinding {
    fn format(&self) -> String {
        format!(
            "{}:{}: {}: {}",
            self.path, self.line, self.category, self.excerpt
        )
    }
}

#[derive(Clone, Debug, PartialEq)]
struct CopyrightInventory {
    included_classes: Vec<&'static str>,
    excluded_classes: Vec<&'static str>,
    included_paths: Vec<String>,
    excluded_paths: Vec<String>,
    missing_notice_paths: Vec<String>,
}

#[derive(Debug)]
struct Rule {
    category: &'static str,
    pattern: Regex,
    include_prefixes: Vec<&'static str>,
    include_exact: Vec<&'static str>,
    exclude_exact: Vec<&'static str>,
    exclude_suffixes: Vec<&'static str>,
}

impl Rule {
    fn applies_to(&self, path: &str) -> bool {
        if self.exclude_exact.contains(&path) {
            return false;
        }
        if self
            .exclude_suffixes
            .iter()
            .any(|suffix| path.ends_with(suffix))
        {
            return false;
        }
        if self.include_prefixes.is_empty() && self.include_exact.is_empty() {
            return true;
        }
        self.include_exact.contains(&path)
            || self
                .include_prefixes
                .iter()
                .any(|prefix| path.starts_with(prefix))
    }
}

pub fn run_public_scan(args: &[String], default_root: &Path) -> i32 {
    let root = default_root;
    if args == ["--copyright-inventory"] {
        print_copyright_inventory(&copyright_inventory(root));
        return 0;
    }
    let targets = if args.is_empty() {
        None
    } else {
        Some(args.iter().map(String::as_str).collect::<Vec<_>>())
    };
    let findings = scan(root, targets.as_deref());
    if findings.is_empty() {
        println!("Public boundary scan passed.");
        0
    } else {
        println!("Public boundary scan failed:");
        for finding in findings {
            println!("{}", finding.format());
        }
        1
    }
}

pub fn scan(root: &Path, targets: Option<&[&str]>) -> Vec<PublicScanFinding> {
    let mut findings = Vec::new();
    for path in target_files(root, targets) {
        let Ok(rel) = path.strip_prefix(root) else {
            continue;
        };
        let rel = rel.to_string_lossy().replace('\\', "/");
        let Some(text) = read_text(&path) else {
            continue;
        };
        findings.extend(scan_text(&rel, &text));
    }
    for rel in copyright_inventory(root).missing_notice_paths {
        findings.push(PublicScanFinding {
            category: COPYRIGHT_CATEGORY.to_string(),
            path: rel,
            line: 1,
            excerpt: format!("expected {COPYRIGHT_NOTICE}"),
        });
    }
    findings
}

pub fn scan_text(path: &str, text: &str) -> Vec<PublicScanFinding> {
    let authorized_core_license = path == "LICENSE"
        && format!("{:x}", Sha256::digest(text.as_bytes()))
            == crate::foss_core_boundary::APACHE_2_LICENSE_SHA256;
    let mut findings = Vec::new();
    for (line_index, line) in text.lines().enumerate() {
        let normalized = line.trim();
        if normalized.is_empty() {
            continue;
        }
        for rule in rules() {
            if authorized_core_license && rule.category == "unauthorized license grant" {
                continue;
            }
            if rule.applies_to(path) && rule.pattern.is_match(line) {
                findings.push(PublicScanFinding {
                    category: rule.category.to_string(),
                    path: path.to_string(),
                    line: line_index + 1,
                    excerpt: excerpt(normalized, 160),
                });
            }
        }
        if hardcoded_secret_assignment(line) {
            findings.push(PublicScanFinding {
                category: "hardcoded secret assignment".to_string(),
                path: path.to_string(),
                line: line_index + 1,
                excerpt: excerpt(normalized, 160),
            });
        }
    }
    findings
}

fn rules() -> &'static [Rule] {
    static RULES: OnceLock<Vec<Rule>> = OnceLock::new();
    RULES.get_or_init(|| {
        vec![
            Rule {
                category: "hosted-only import or private package surface",
                pattern: Regex::new(r"(?i)(\bfrom\s+(tradeassembly_(hosted|product|commercial|private)|hosted_tradeassembly)\b|\bimport\s+(tradeassembly_(hosted|product|commercial|private)|hosted_tradeassembly)\b|@tradeassembly/(hosted|private|commercial)|tradeassembly-(hosted|private|commercial|product)|TradeAssembly-(Hosted|Private|Commercial|Product))").unwrap(),
                include_prefixes: SOURCE_PREFIXES.to_vec(),
                include_exact: Vec::new(),
                exclude_exact: Vec::new(),
                exclude_suffixes: Vec::new(),
            },
            Rule {
                category: "commercial or hosted-only product surface",
                pattern: Regex::new(r"(?i)\b(stripe|billing|checkout|purchase|payout|enterprise\s+sso|hosted[-_ ]oauth(?:[-_ ]callback)?|managed[- ]worker|break[- ]glass|token[- ]budget)\b").unwrap(),
                include_prefixes: merged(USER_SURFACE_PREFIXES, DEPLOYMENT_PREFIXES),
                include_exact: DEPLOYMENT_EXACT.to_vec(),
                exclude_exact: vec![
                    "runtime-rs/src/auth.rs",
                    "runtime-rs/src/research_notebook.rs",
                    "runtime-rs/src/service/plugin_lifecycle.rs",
                ],
                exclude_suffixes: vec![".md"],
            },
            Rule {
                category: "production secret-management material",
                pattern: Regex::new(r"(?i)(\bproduction[-_ ]+(kms|vault)(?:[-_ ][a-z0-9]+)?|(^|[^A-Za-z0-9_-])hosted[-_ ]+(vault|kms)(?:[-_ ][a-z0-9]+)?|\bmanaged[-_ ]+secrets?)\b").unwrap(),
                include_prefixes: merged(USER_SURFACE_PREFIXES, DEPLOYMENT_PREFIXES),
                include_exact: DEPLOYMENT_EXACT.to_vec(),
                exclude_exact: vec!["runtime-rs/src/research_notebook.rs"],
                exclude_suffixes: vec![".md"],
            },
            Rule {
                category: "TradeAssembly.org credential custody internals",
                pattern: Regex::new(r"(?i)\b(tradeassembly\.org|hosted)\s+(broker|data|alpaca|provider)\s+credential\s+(custody|storage|store|vault)").unwrap(),
                include_prefixes: merged(USER_SURFACE_PREFIXES, DEPLOYMENT_PREFIXES),
                include_exact: merged(USER_SURFACE_EXACT, DEPLOYMENT_EXACT),
                exclude_exact: Vec::new(),
                exclude_suffixes: vec![".md"],
            },
            Rule {
                category: "unauthorized license grant",
                pattern: Regex::new(r"(?i)(permission is hereby granted|spdx-license-identifier:\s*(MIT|Apache|GPL|AGPL|LGPL|MPL|CC)|licensed under (the )?(MIT|Apache|GPL|AGPL|LGPL|MPL|Creative Commons)|GNU GENERAL PUBLIC LICENSE|Apache License, Version 2\.0|Creative Commons Attribution)").unwrap(),
                include_prefixes: Vec::new(),
                include_exact: Vec::new(),
                exclude_exact: vec!["xtask/src/public_scan.rs"],
                exclude_suffixes: Vec::new(),
            },
            Rule {
                category: "unsupported regulatory compliance claim",
                pattern: Regex::new(r"(?i)\b(FINRA|SEC|broker[- ]dealer|investment[- ]adviser|RIA|KYC|AML|suitability)\s+(compliant|certified|approved|ready|validated)\b|\b(compliant|certified)\s+with\s+(FINRA|SEC|broker[- ]dealer|investment[- ]adviser|RIA|KYC|AML|suitability)\b|\bguarantees?\s+(regulatory\s+)?compliance\b").unwrap(),
                include_prefixes: merged(merged(DOC_PREFIXES, USER_SURFACE_PREFIXES).as_slice(), DEPLOYMENT_PREFIXES),
                include_exact: USER_SURFACE_EXACT.to_vec(),
                exclude_exact: vec!["xtask/src/public_scan.rs"],
                exclude_suffixes: Vec::new(),
            },
            Rule {
                category: "legacy private product name",
                pattern: Regex::new(r"\b(from\s+autotrade|import\s+autotrade|autotrade-runtime|Autotrade)\b").unwrap(),
                include_prefixes: USER_SURFACE_PREFIXES.to_vec(),
                include_exact: vec!["README.md", "AGENTS.md"],
                exclude_exact: Vec::new(),
                exclude_suffixes: Vec::new(),
            },
            Rule {
                category: "secret-like token",
                pattern: Regex::new(r"(-----BEGIN [A-Z ]*PRIVATE KEY-----|\bAKIA[0-9A-Z]{16}\b|\bASIA[0-9A-Z]{16}\b|\bghp_[A-Za-z0-9_]{30,}\b|\bgithub_pat_[A-Za-z0-9_]{30,}\b|\bxox[baprs]-[A-Za-z0-9-]{20,}\b)").unwrap(),
                include_prefixes: Vec::new(),
                include_exact: Vec::new(),
                exclude_exact: Vec::new(),
                exclude_suffixes: Vec::new(),
            },
            Rule {
                category: "advice-like trading language",
                pattern: Regex::new(r"(?i)\brecommend(ed|s)?\s+(trade|allocation|position|entry|exit)\b").unwrap(),
                include_prefixes: USER_SURFACE_PREFIXES.to_vec(),
                include_exact: USER_SURFACE_EXACT.to_vec(),
                exclude_exact: vec![
                    "runtime-rs/src/attribution_journal.rs",
                    "runtime-rs/src/instrument_packs.rs",
                    "runtime-rs/src/monte_carlo.rs",
                ],
                exclude_suffixes: Vec::new(),
            },
            Rule {
                category: "machine-specific customer-state path in executable test",
                pattern: Regex::new(
                    r#"(?i)(/Users/[^/\s"']+|/home/[^/\s"']+)/(?:\.local/share/tradeassembly|\.tradeassembly)(?:[/\\][^\s"']*)?"#,
                )
                .unwrap(),
                include_prefixes: vec!["runtime-rs/tests/"],
                include_exact: Vec::new(),
                exclude_exact: Vec::new(),
                exclude_suffixes: Vec::new(),
            },
        ]
    })
}

fn hardcoded_secret_assignment(line: &str) -> bool {
    static SECRET_ASSIGNMENT: OnceLock<Regex> = OnceLock::new();
    let regex = SECRET_ASSIGNMENT.get_or_init(|| {
        Regex::new(
            r#"(?i)\b(api_secret|secret_key|client_secret|password)\s*[:=]\s*['"]([^'"]{8,})['"]"#,
        )
        .unwrap()
    });
    let Some(captures) = regex.captures(line) else {
        return false;
    };
    let Some(secret) = captures.get(2).map(|value| value.as_str().trim()) else {
        return false;
    };
    let lower = secret.to_ascii_lowercase();
    ![
        "$",
        "{{",
        "<",
        "local-",
        "test-",
        "fake-",
        "example",
        "placeholder",
        "redacted",
        "none",
        "paper-",
        "your-",
        "env:",
    ]
    .iter()
    .any(|prefix| lower.starts_with(prefix))
}

fn target_files(root: &Path, targets: Option<&[&str]>) -> Vec<PathBuf> {
    let selected_targets = targets.unwrap_or(DEFAULT_SCAN_TARGETS);
    tracked_files(root)
        .into_iter()
        .chain(untracked_rust_tests(root))
        .filter(|path| {
            let Ok(rel) = path.strip_prefix(root) else {
                return false;
            };
            let rel = rel.to_string_lossy().replace('\\', "/");
            is_under_targets(&rel, selected_targets) && should_scan_path(&rel, path)
        })
        .collect()
}

fn tracked_files(root: &Path) -> Vec<PathBuf> {
    let output = Command::new("git")
        .args(["ls-files"])
        .current_dir(root)
        .output()
        .unwrap_or_else(|error| panic!("failed to run git ls-files: {error}"));
    if !output.status.success() {
        return Vec::new();
    }
    String::from_utf8_lossy(&output.stdout)
        .lines()
        .filter(|line| !line.trim().is_empty())
        .map(|line| root.join(line))
        .collect()
}

// Cargo discovers integration-test source before it is added to Git. Include
// even ignored Rust tests: ignore rules do not prevent Cargo from executing them.
fn untracked_rust_tests(root: &Path) -> Vec<PathBuf> {
    let output = Command::new("git")
        .args(["ls-files", "--others", "-z", "--", "runtime-rs/tests/"])
        .current_dir(root)
        .output()
        .expect("list untracked integration tests");
    assert!(
        output.status.success(),
        "cannot inspect untracked integration tests"
    );
    String::from_utf8_lossy(&output.stdout)
        .split('\0')
        .filter(|path| path.ends_with(".rs"))
        .map(|path| root.join(path))
        .collect()
}

fn should_scan_path(rel: &str, path: &Path) -> bool {
    if has_excluded_part(path) {
        return false;
    }
    if EXCLUDED_PREFIXES
        .iter()
        .any(|prefix| rel == prefix.trim_end_matches('/') || rel.starts_with(prefix))
    {
        return false;
    }
    if BINARY_SUFFIXES.iter().any(|suffix| {
        path.to_string_lossy()
            .to_ascii_lowercase()
            .ends_with(suffix)
    }) {
        return false;
    }
    path.is_file()
}

fn copyright_inventory(root: &Path) -> CopyrightInventory {
    let mut included = Vec::new();
    let mut excluded = Vec::new();
    let mut missing = Vec::new();

    for path in tracked_files(root) {
        if !path.is_file() {
            continue;
        }
        let Ok(rel_path) = path.strip_prefix(root) else {
            continue;
        };
        let rel = rel_path.to_string_lossy().replace('\\', "/");
        if requires_copyright_notice(&rel, &path) {
            included.push(rel.clone());
            let missing_notice = read_text(&path)
                .map(|text| {
                    !text
                        .chars()
                        .take(500)
                        .collect::<String>()
                        .contains(COPYRIGHT_NOTICE)
                })
                .unwrap_or(true);
            if missing_notice {
                missing.push(rel);
            }
        } else {
            excluded.push(rel);
        }
    }

    included.sort();
    excluded.sort();
    missing.sort();
    CopyrightInventory {
        included_classes: COPYRIGHT_INCLUDED_CLASSES.to_vec(),
        excluded_classes: COPYRIGHT_EXCLUDED_CLASSES.to_vec(),
        included_paths: included,
        excluded_paths: excluded,
        missing_notice_paths: missing,
    }
}

fn requires_copyright_notice(rel: &str, path: &Path) -> bool {
    if has_excluded_part(path) {
        return false;
    }
    if COPYRIGHT_EXCLUDE_EXACT.contains(&rel) {
        return false;
    }
    if COPYRIGHT_EXCLUDE_PREFIXES
        .iter()
        .any(|prefix| rel == prefix.trim_end_matches('/') || rel.starts_with(prefix))
    {
        return false;
    }
    if COPYRIGHT_EXCLUDE_SUFFIXES
        .iter()
        .any(|suffix| rel.ends_with(suffix))
    {
        return false;
    }
    if COPYRIGHT_INCLUDE_EXACT.contains(&rel) {
        return true;
    }
    if !COPYRIGHT_INCLUDE_PREFIXES
        .iter()
        .any(|prefix| rel.starts_with(prefix))
    {
        return false;
    }
    COPYRIGHT_INCLUDE_SUFFIXES
        .iter()
        .any(|suffix| rel.ends_with(suffix))
}

fn print_copyright_inventory(inventory: &CopyrightInventory) {
    println!("Copyright inventory");
    println!("\nIncluded classes:");
    for item in &inventory.included_classes {
        println!("- {item}");
    }
    println!("\nExcluded classes:");
    for item in &inventory.excluded_classes {
        println!("- {item}");
    }
    println!("\nIncluded files: {}", inventory.included_paths.len());
    for path in &inventory.included_paths {
        println!("  {path}");
    }
    println!("\nExcluded files: {}", inventory.excluded_paths.len());
    for path in &inventory.excluded_paths {
        println!("  {path}");
    }
    println!(
        "\nMissing notices: {}",
        inventory.missing_notice_paths.len()
    );
    for path in &inventory.missing_notice_paths {
        println!("  {path}");
    }
}

fn read_text(path: &Path) -> Option<String> {
    fs::read_to_string(path).ok()
}

fn excerpt(text: &str, limit: usize) -> String {
    if text.len() <= limit {
        text.to_string()
    } else {
        format!("{}...", &text[..limit - 3])
    }
}

fn is_under_targets(rel: &str, targets: &[&str]) -> bool {
    targets.iter().any(|target| {
        let normalized = target.trim_end_matches('/');
        rel == normalized || rel.starts_with(&format!("{normalized}/"))
    })
}

fn has_excluded_part(path: &Path) -> bool {
    path.components().any(|component| {
        let part = component.as_os_str().to_string_lossy();
        EXCLUDED_PARTS.contains(&part.as_ref())
    })
}

fn merged(left: &[&'static str], right: &[&'static str]) -> Vec<&'static str> {
    left.iter().chain(right.iter()).copied().collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_exact_approved_root_license_is_authorized() {
        let license = include_str!("../../LICENSE");
        let has_grant = |findings: Vec<PublicScanFinding>| {
            findings
                .iter()
                .any(|finding| finding.category == "unauthorized license grant")
        };
        assert!(!has_grant(scan_text("LICENSE", license)));
        assert!(has_grant(scan_text(
            "LICENSE",
            &format!("{license}\nChanged grant")
        )));
        assert!(has_grant(scan_text("runtime-rs/LICENSE", license)));
        assert!(has_grant(scan_text(
            "LICENSE",
            "Permission is hereby granted"
        )));
    }
    use std::fs;
    use std::path::Path;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn temp_repo(name: &str) -> std::path::PathBuf {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let root = std::env::temp_dir().join(format!("tradeassembly-public-scan-{name}-{unique}"));
        fs::create_dir_all(root.join("runtime/src/tradeassembly")).unwrap();
        fs::create_dir_all(root.join("webapp/app")).unwrap();
        fs::create_dir_all(root.join("docs")).unwrap();
        run_git(&root, &["init"]);
        run_git(&root, &["config", "user.email", "test@example.com"]);
        run_git(&root, &["config", "user.name", "Test User"]);
        root
    }

    fn run_git(root: &Path, args: &[&str]) {
        let status = std::process::Command::new("git")
            .args(args)
            .current_dir(root)
            .status()
            .unwrap();
        assert!(status.success());
    }

    fn track(root: &Path, rel: &str, text: &str) {
        let path = root.join(rel);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).unwrap();
        }
        fs::write(path, text).unwrap();
        run_git(root, &["add", rel]);
    }

    #[test]
    fn scan_text_flags_public_boundary_and_secret_terms() {
        let findings = scan_text(
            "runtime/src/tradeassembly/api.py",
            "from tradeassembly_hosted import billing\npassword = \"super-secret-value\"\n",
        );

        assert!(findings
            .iter()
            .any(|finding| finding.category == "hosted-only import or private package surface"));
        assert!(findings
            .iter()
            .any(|finding| finding.category == "hardcoded secret assignment"));
    }

    #[test]
    fn scan_text_flags_advice_language_on_user_surfaces() {
        let findings = scan_text(
            "webapp/app/page.tsx",
            "We recommend trade allocation based on this signal.",
        );

        assert_eq!(1, findings.len());
        assert_eq!("advice-like trading language", findings[0].category);
    }

    #[test]
    fn plugin_lifecycle_negative_enforcement_only_skips_commercial_words() {
        let findings = scan_text(
            "runtime-rs/src/service/plugin_lifecycle.rs",
            "const FORBIDDEN: &[&str] = &[\"billing\", \"payouts\"];\nWe recommend trade allocation.\n",
        );

        assert!(!findings
            .iter()
            .any(|finding| { finding.category == "commercial or hosted-only product surface" }));
        assert!(findings
            .iter()
            .any(|finding| finding.category == "advice-like trading language"));
    }

    #[test]
    fn scan_text_flags_unsupported_compliance_claims() {
        let findings = scan_text(
            "docs/reference/contracts/warden-finance-profile-contracts.md",
            "The profile is FINRA compliant and guarantees compliance.",
        );

        assert!(findings
            .iter()
            .any(|finding| { finding.category == "unsupported regulatory compliance claim" }));
    }

    #[test]
    fn scan_text_allows_compliance_support_language() {
        let findings = scan_text(
            "docs/reference/contracts/warden-finance-profile-contracts.md",
            "The profile exposes compliance-support hooks, evidence exports, and legal-review gates.",
        );

        assert!(findings.is_empty(), "{findings:?}");
    }

    #[test]
    fn scan_text_allows_self_hosted_vault_language() {
        let findings = scan_text(
            "runtime/src/tradeassembly/control.py",
            r#""kind": "self-hosted-vault""#,
        );

        assert!(findings.is_empty(), "{findings:?}");
    }

    #[test]
    fn scan_text_flags_machine_specific_customer_state_paths_in_rust_tests() {
        let findings = scan_text(
            "runtime-rs/tests/generated_research.rs",
            r#"let db = "/Users/alice/.local/share/tradeassembly/onboarding/state/tradeassembly.db";
let other = "/home/bob/.tradeassembly/tradeassembly.db";"#,
        );

        assert_eq!(
            findings
                .iter()
                .filter(|finding| {
                    finding.category == "machine-specific customer-state path in executable test"
                })
                .count(),
            2
        );
    }

    #[test]
    fn scan_text_allows_portable_test_fixture_paths() {
        let findings = scan_text(
            "runtime-rs/tests/portable_fixture.rs",
            r#"let db = tempfile::tempdir().unwrap().path().join("tradeassembly.db");
let other = "/tmp/tradeassembly-fixture/tradeassembly.db";"#,
        );

        assert!(!findings.iter().any(|finding| {
            finding.category == "machine-specific customer-state path in executable test"
        }));
    }

    #[test]
    fn scan_includes_untracked_and_ignored_executable_tests() {
        let root = temp_repo("untracked-tests");
        fs::create_dir_all(root.join("runtime-rs/tests")).unwrap();
        fs::write(root.join(".gitignore"), "runtime-rs/tests/ignored.rs\n").unwrap();
        for name in ["generated.rs", "ignored.rs"] {
            fs::write(
                root.join("runtime-rs/tests").join(name),
                r#"let db = "/Users/alice/.local/share/tradeassembly/state/customer.db";"#,
            )
            .unwrap();
        }
        let findings = scan(&root, None);
        assert_eq!(
            findings
                .iter()
                .filter(|finding| finding.category
                    == "machine-specific customer-state path in executable test")
                .count(),
            2
        );
    }

    #[test]
    fn scan_checks_git_tracked_files_and_copyright_notice() {
        let root = temp_repo("copyright");
        track(
            &root,
            "runtime/src/tradeassembly/ok.py",
            "# Copyright (c) 2026 OptionLab LLC. All rights reserved.\nprint('ok')\n",
        );
        track(
            &root,
            "runtime/src/tradeassembly/missing.py",
            "print('missing')\n",
        );
        track(
            &root,
            "runtime/src/tradeassembly/still_missing.py",
            "print('still missing')\n",
        );
        track(
            &root,
            "docs/readme.md",
            "docs do not require source notice\n",
        );
        track(
            &root,
            ".sdlc/imports/product-roadmap.yaml",
            "exact: imported bytes without a Core copyright header\ncopy: recommend a trade\n",
        );
        fs::remove_file(root.join("runtime/src/tradeassembly/missing.py")).unwrap();

        let findings = scan(&root, None);

        assert!(findings.iter().any(|finding| {
            finding.category == COPYRIGHT_CATEGORY
                && finding.path == "runtime/src/tradeassembly/still_missing.py"
        }));
        assert!(!findings
            .iter()
            .any(|finding| finding.path == "docs/readme.md"));
        assert!(!findings
            .iter()
            .any(|finding| finding.path == "runtime/src/tradeassembly/missing.py"));
        assert!(!findings
            .iter()
            .any(|finding| finding.path == ".sdlc/imports/product-roadmap.yaml"));
    }
}
