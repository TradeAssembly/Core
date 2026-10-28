# Repeated setup after plugin initialization

The owning source-free installation fixture exposed a repeat-setup failure after
offline plugin installation and capability discovery. The local credential
adapter creates a sibling `credentials` directory even when reading a missing
credential record. Setup's recognized-state list omitted that generated entry,
so the final repeat installation failed with `local_install_state_unrecognized`.

Setup now recognizes exactly this directory. It validates the existing directory
as private state, rejects symlinks and wrong types, and requires current-user
ownership on Unix. It does not read credential contents, relax credential-store
validation, change permissions, adopt unmarked installations or delete files.
Windows follows the existing private-directory owner/DACL/reparse validation;
this local checkpoint does not claim a Windows runtime test.

The focused local-install suite passed 11 tests. Its new regression verifies
preservation of fixture material and rejection of insecure modes, file and
symlink substitutions, while retaining unmarked-install rejection. The existing
real-binary installation fixture remains the owning end-to-end repeat check; no
assertion is skipped or changed to accommodate the failure.

This is a local producer fix. It does not alter a consumer's immutable producer
pin or establish publication, complete owning CI, signing or release readiness.
