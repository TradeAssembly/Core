// Copyright (c) 2026 OptionLab LLC. All rights reserved.
//! Optional customer-owned WorkOS session custody through the Bitwarden CLI.
//! Never pass secret values as arguments or include provider output in errors.
use base64::{engine::general_purpose::STANDARD, Engine as _};
use serde_json::{json, Value};
use std::io::{Read, Write};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

const ERROR: &str = "bitwarden_session_store_unavailable";
const LIMIT: u64 = 1_048_576;

pub(crate) struct Store {
    name: String,
}

impl Store {
    pub(crate) fn new(account: &str) -> Self {
        Self {
            name: format!("TradeAssembly WorkOS session {account}"),
        }
    }

    fn item(&self) -> Result<Option<Value>, String> {
        let result = run(&["list", "items", "--search", &self.name], None)?;
        select_item(&result, &self.name)
    }

    pub(crate) fn load(&self) -> Result<String, String> {
        let item = self.item()?.ok_or("oidc_session_required")?;
        item["notes"]
            .as_str()
            .filter(|raw| !raw.is_empty())
            .map(str::to_owned)
            .ok_or_else(|| "oidc_session_required".into())
    }

    pub(crate) fn save(&self, raw: &str) -> Result<(), String> {
        if raw.len() > LIMIT as usize / 4 {
            return Err(ERROR.into());
        }
        let existing = self.item()?;
        let mut item = existing.clone().unwrap_or_else(|| {
            json!({
                "type":2,"name":self.name,"notes":"","secureNote":{"type":0},
                "organizationId":null,"folderId":null,"favorite":false,"reprompt":0
            })
        });
        item["notes"] = json!(raw);
        let encoded = STANDARD.encode(serde_json::to_vec(&item).map_err(|_| ERROR)?);
        if let Some(old) = existing {
            let id = old["id"].as_str().ok_or(ERROR)?;
            run(&["edit", "item", id], Some(encoded))?;
        } else {
            run(&["create", "item"], Some(encoded))?;
        }
        // A successful CLI exit alone is not durable-secret evidence.
        if self.load()? != raw {
            return Err(ERROR.into());
        }
        Ok(())
    }

    pub(crate) fn migrate(&self, raw: &str) -> Result<(), String> {
        if let Some(item) = self.item()? {
            return if item["notes"].as_str() == Some(raw) {
                Ok(())
            } else {
                Err("bitwarden_session_conflict".into())
            };
        }
        self.save(raw)
    }

    pub(crate) fn delete(&self) -> Result<(), String> {
        if let Some(item) = self.item()? {
            run(&["delete", "item", item["id"].as_str().ok_or(ERROR)?], None)?;
        }
        if self.item()?.is_some() {
            return Err(ERROR.into());
        }
        Ok(())
    }
}

fn select_item(raw: &[u8], name: &str) -> Result<Option<Value>, String> {
    let items: Value = serde_json::from_slice(raw).map_err(|_| ERROR)?;
    let matches: Vec<_> = items
        .as_array()
        .ok_or(ERROR)?
        .iter()
        .filter(|item| item["name"] == name && item["deletedDate"].is_null())
        .collect();
    match matches.as_slice() {
        [] => Ok(None),
        [item] if item["type"] == 2 && item["id"].as_str().is_some_and(valid_id) => {
            Ok(Some((*item).clone()))
        }
        _ => Err(ERROR.into()),
    }
}

fn valid_id(id: &str) -> bool {
    id.len() == 36
        && id.bytes().enumerate().all(|(index, byte)| {
            if [8, 13, 18, 23].contains(&index) {
                byte == b'-'
            } else {
                byte.is_ascii_hexdigit()
            }
        })
}

fn run(args: &[&str], input: Option<String>) -> Result<Vec<u8>, String> {
    // The default helper reads the user's mode-0600 Bitwarden session file in
    // its own process. TradeAssembly never reads or exports that vault session.
    // An explicit CLI override preserves headless/CI integrations that provide
    // their own credential process.
    let (binary, prefix): (_, &[&str]) = match std::env::var_os("TRADEASSEMBLY_BITWARDEN_CLI") {
        Some(binary) => (binary, &[]),
        None => bitwarden_helper().ok_or(ERROR)?,
    };
    let mut command = Command::new(binary);
    command
        .args(prefix)
        .args(args)
        .arg("--raw")
        .env("BW_NOINTERACTION", "true")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        command.process_group(0);
    }
    let mut child = command.spawn().map_err(|_| ERROR)?;
    let mut stdin = child.stdin.take().ok_or(ERROR)?;
    let output = child.stdout.take().ok_or(ERROR)?;
    let writer = std::thread::spawn(move || {
        if let Some(input) = input {
            let _ = stdin.write_all(input.as_bytes());
        }
    });
    let reader = std::thread::spawn(move || {
        let mut bytes = Vec::new();
        output
            .take(LIMIT + 1)
            .read_to_end(&mut bytes)
            .map(|_| bytes)
    });
    let deadline = Instant::now() + Duration::from_secs(30);
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break Some(status),
            Ok(None) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(20)),
            _ => break None,
        }
    };
    // Close descendant-held pipes as well as the child on timeout/exit.
    // SAFETY: the child was placed in a dedicated process group above. No
    // caller-provided PID or process group can reach this cleanup operation.
    #[cfg(unix)]
    unsafe {
        libc::kill(-(child.id() as i32), libc::SIGKILL);
    }
    if status.is_none() {
        let _ = child.kill();
    }
    let _ = child.wait();
    let _ = writer.join();
    let bytes = reader.join().map_err(|_| ERROR)?.map_err(|_| ERROR)?;
    if !status.is_some_and(|status| status.success()) || bytes.len() > LIMIT as usize {
        return Err(ERROR.into());
    }
    Ok(bytes)
}

fn bitwarden_helper() -> Option<(std::ffi::OsString, &'static [&'static str])> {
    let bundled = std::env::current_exe()
        .ok()?
        .parent()?
        .join("tradeassembly-bitwarden");
    if bundled.is_file() {
        return Some((bundled.into_os_string(), &[]));
    }
    let home = std::env::var_os("HOME")?;
    let migrated = std::path::PathBuf::from(home).join(".local/bin/bitwarden-keychain-secret");
    migrated
        .is_file()
        .then(|| (migrated.into_os_string(), &["exec"][..]))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    #[ignore = "explicit live Bitwarden session; creates and soft-deletes a disposable non-secret note"]
    fn real_bitwarden_round_trip() {
        let id = format!(
            "test-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        );
        let store = Store::new(&id);
        store.save("non-secret acceptance value 1").unwrap();
        assert_eq!(store.load().unwrap(), "non-secret acceptance value 1");
        assert_eq!(
            store.migrate("different").unwrap_err(),
            "bitwarden_session_conflict"
        );
        store.save("non-secret acceptance value 2").unwrap();
        // Each operation starts a fresh CLI process; a new adapter must agree.
        assert_eq!(
            Store::new(&id).load().unwrap(),
            "non-secret acceptance value 2"
        );
        store.delete().unwrap();
        assert_eq!(store.load().unwrap_err(), "oidc_session_required");
    }
    #[test]
    fn exact_unique_secure_note_is_required() {
        let item = json!({"id":"8af0c751-8f3c-4e79-aab7-729a726f44ad","name":"wanted","type":2,"notes":"sentinel"});
        let bytes = |value: Value| serde_json::to_vec(&value).unwrap();
        assert!(select_item(&bytes(json!([item])), "wanted")
            .unwrap()
            .is_some());
        assert!(select_item(&bytes(json!([item])), "want")
            .unwrap()
            .is_none());
        assert!(select_item(&bytes(json!([item, item])), "wanted").is_err());
        let mut wrong = item.clone();
        wrong["type"] = json!(1);
        assert!(select_item(&bytes(json!([wrong])), "wanted").is_err());
        let mut deleted = item;
        deleted["deletedDate"] = json!("deleted");
        assert!(select_item(&bytes(json!([deleted])), "wanted")
            .unwrap()
            .is_none());
        assert_eq!(
            select_item(b"secret-sentinel", "wanted").unwrap_err(),
            ERROR
        );
    }
}
