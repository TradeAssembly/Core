//! Immutable process identity, installed by compiled executable startup code.
//! Libraries never discover a revision from the environment or request input.
use sha2::{Digest, Sha256};
use std::sync::OnceLock;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BuildIdentity(String);

impl BuildIdentity {
    pub fn revision(&self) -> &str {
        &self.0
    }

    pub fn application_digest(&self) -> String {
        format!("{:x}", Sha256::digest(self.0.as_bytes()))
    }
}

static IDENTITY: OnceLock<BuildIdentity> = OnceLock::new();

fn install_into(cell: &OnceLock<BuildIdentity>, revision: &str) -> Result<(), &'static str> {
    if revision.len() != 40
        || !revision
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    {
        return Err("invalid Core build identity");
    }
    let proposed = BuildIdentity(revision.to_owned());
    let existing = cell.get_or_init(|| proposed.clone());
    if existing != &proposed {
        return Err("Core build identity already installed");
    }
    Ok(())
}

/// Call once at executable startup with the compiled source revision.
pub fn install(revision: &str) -> Result<(), &'static str> {
    install_into(&IDENTITY, revision)
}

pub fn current() -> Option<&'static BuildIdentity> {
    IDENTITY.get()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identity_is_validated_immutable_and_preserves_receipt_digest() {
        let cell = OnceLock::new();
        assert!(cell.get().is_none());
        assert!(install_into(&cell, "unknown").is_err());
        let revision = "1111111111111111111111111111111111111111";
        install_into(&cell, revision).unwrap();
        install_into(&cell, revision).unwrap();
        assert!(install_into(&cell, "2222222222222222222222222222222222222222").is_err());
        assert_eq!(cell.get().unwrap().revision(), revision);
        assert_eq!(
            cell.get().unwrap().application_digest(),
            format!("{:x}", Sha256::digest(revision.as_bytes()))
        );
    }
}
