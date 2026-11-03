// Copyright (c) 2026 OptionLab LLC. All rights reserved.
//! Validate the explicitly pinned plugin identity before using its filename.

use crate::{is_digest, Result, TARGETS};
use serde_json::Value;

pub(crate) fn package_file<'a>(lock: &'a Value, target: &str) -> Result<&'a str> {
    let version = lock["version"].as_str().ok_or("plugin_lock_invalid")?;
    let parsed = semver::Version::parse(version).map_err(|_| "plugin_lock_invalid")?;
    let package = lock["packageFile"].as_str().ok_or("plugin_lock_invalid")?;
    if parsed.to_string() != version
        || !TARGETS.contains(&target)
        || lock["target"] != target
        || package != format!("tradeassembly-plugin-alpaca-{version}.tar.gz")
        || ["packageSha256", "manifestSha256"]
            .iter()
            .any(|field| lock[field].as_str().is_none_or(|sha| !is_digest(sha)))
    {
        return Err("plugin_lock_invalid".into());
    }
    Ok(package)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn pinned_versions_require_exact_safe_filename_digests_and_target() {
        for version in ["0.1.17", "0.1.18"] {
            let lock = json!({"version":version,"target":TARGETS[0],
                "packageFile":format!("tradeassembly-plugin-alpaca-{version}.tar.gz"),
                "packageSha256":"a".repeat(64),"manifestSha256":"b".repeat(64)});
            assert!(package_file(&lock, TARGETS[0]).is_ok());
            for (field, value) in [
                ("version", "0.1.19"),
                ("version", "01.1.18"),
                ("version", "0.1"),
                ("version", "../0.1.18"),
                ("version", "0.1.18/escape"),
                ("version", "v0.1.18"),
                (
                    "packageFile",
                    "../tradeassembly-plugin-alpaca-0.1.18.tar.gz",
                ),
                (
                    "packageFile",
                    "dir/tradeassembly-plugin-alpaca-0.1.18.tar.gz",
                ),
                (
                    "packageFile",
                    "dir\\tradeassembly-plugin-alpaca-0.1.18.tar.gz",
                ),
                ("packageFile", "alpaca.tar.gz"),
                ("packageSha256", "bad"),
                ("manifestSha256", "bad"),
                ("target", TARGETS[4]),
            ] {
                let mut wrong = lock.clone();
                wrong[field] = json!(value);
                assert!(package_file(&wrong, TARGETS[0]).is_err(), "{field}={value}");
            }
            let mut malformed = lock.clone();
            malformed["version"] = json!("../escape");
            malformed["packageFile"] = json!("tradeassembly-plugin-alpaca-../escape.tar.gz");
            assert!(package_file(&malformed, TARGETS[0]).is_err());
        }
    }
}
