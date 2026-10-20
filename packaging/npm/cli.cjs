#!/usr/bin/env node
// Copyright (c) 2026 OptionLab LLC. All rights reserved.
'use strict';
const fs = require('node:fs');
const path = require('node:path');
const crypto = require('node:crypto');
const { spawnSync } = require('node:child_process');
const packages = {
  'darwin-arm64': 'tradeassembly-darwin-arm64',
  'linux-x64': 'tradeassembly-linux-x64',
  'linux-arm64': 'tradeassembly-linux-arm64',
  'win32-x64': 'tradeassembly-win32-x64',
};
const releasePolicy = 'macos-arm64-qualified-experimental-v1';
try {
  if (Number(process.versions.node.split('.')[0]) < 22) throw new Error('node_version_unsupported');
  if (process.platform === 'linux' && !process.report.getReport().header.glibcVersionRuntime) throw new Error('libc_unsupported_gnu_release_required');
  const name = packages[`${process.platform}-${process.arch}`];
  if (!name) throw new Error('platform_unsupported');
  const launcher = require('./package.json');
  let metadata;
  try { metadata = require.resolve(`${name}/package.json`); }
  catch { throw new Error('native_package_missing_reinstall_with_optional_dependencies'); }
  const pkg = JSON.parse(fs.readFileSync(metadata, 'utf8'));
  if (pkg.name !== name || pkg.version !== launcher.version) throw new Error('native_package_version_mismatch');
  if (launcher.tradeassemblyReleasePolicy !== releasePolicy ||
      pkg.tradeassemblyReleasePolicy !== releasePolicy) throw new Error('release_policy_mismatch');
  const qualification = process.platform === 'darwin' ? 'required' : 'deferred-experimental';
  if (pkg.tradeassemblyPlatformQualification !== qualification) throw new Error('platform_qualification_mismatch');
  const root = path.dirname(metadata);
  const release = JSON.parse(fs.readFileSync(path.join(root, 'release.json'), 'utf8'));
  if (release.version !== pkg.version) throw new Error('release_package_version_mismatch');
  const installer = JSON.parse(fs.readFileSync(path.join(root, 'installer.json'), 'utf8'));
  const expected = process.platform === 'win32' ? 'tradeassembly-distribution.exe' : 'tradeassembly-distribution';
  if (installer.filename !== expected || !/^[a-f0-9]{64}$/.test(installer.sha256)) throw new Error('installer_metadata_invalid');
  const binary = path.join(root, expected);
  const actual = crypto.createHash('sha256').update(fs.readFileSync(binary)).digest('hex');
  if (actual !== installer.sha256) throw new Error('installer_digest_mismatch');
  if (qualification === 'deferred-experimental') {
    console.error(JSON.stringify({ warning: 'platform_acceptance_not_yet_qualified', target: `${process.platform}-${process.arch}` }));
  }
  const result = spawnSync(binary, process.argv.slice(2), { stdio: 'inherit', shell: false });
  if (result.error) throw new Error('native_installer_launch_failed');
  process.exitCode = result.status === null ? 1 : result.status;
} catch (error) {
  // Only our fixed diagnostic identifiers: never print child environments/paths.
  const safe = /^[a-z_]+$/.test(error.message) ? error.message : 'distribution_launch_failed';
  console.error(JSON.stringify({ ok: false, code: safe }));
  process.exitCode = 1;
}
