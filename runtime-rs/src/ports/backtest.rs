// Copyright (c) 2026 OptionLab LLC. All rights reserved.

use crate::backtest_contracts::{
    BacktestAttempt, BacktestLifecycleEvent, BacktestResult, BacktestRunManifest, BacktestRunRecord,
};
use crate::ports::{SideEffectContext, VersionedPort};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BacktestImmutableWrite {
    Created,
    AlreadyPresent,
}

pub trait BacktestRunRepository: VersionedPort {
    fn put_manifest(
        &self,
        manifest: &BacktestRunManifest,
        context: &SideEffectContext,
    ) -> Result<BacktestImmutableWrite, String>;
    fn get_manifest(&self, run_id: &str) -> Result<Option<BacktestRunManifest>, String>;
    fn put_result(
        &self,
        result: &BacktestResult,
        context: &SideEffectContext,
    ) -> Result<BacktestImmutableWrite, String>;
    fn get_result(&self, run_id: &str) -> Result<Option<BacktestResult>, String>;
    fn create_run(
        &self,
        run: &BacktestRunRecord,
        context: &SideEffectContext,
    ) -> Result<BacktestImmutableWrite, String>;
    fn get_run(&self, run_id: &str) -> Result<Option<BacktestRunRecord>, String>;
    fn list_runs(&self) -> Result<Vec<BacktestRunRecord>, String>;
    fn compare_and_put_run(
        &self,
        expected: &BacktestRunRecord,
        replacement: &BacktestRunRecord,
        context: &SideEffectContext,
    ) -> Result<bool, String>;
    fn put_attempt(
        &self,
        attempt: &BacktestAttempt,
        context: &SideEffectContext,
    ) -> Result<(), String>;
    fn compare_and_put_attempt(
        &self,
        expected: &BacktestAttempt,
        replacement: &BacktestAttempt,
        context: &SideEffectContext,
    ) -> Result<bool, String>;
    fn get_attempt(&self, attempt_id: &str) -> Result<Option<BacktestAttempt>, String>;
    fn list_attempts(&self, run_id: &str) -> Result<Vec<BacktestAttempt>, String>;
    fn append_event(
        &self,
        event: &BacktestLifecycleEvent,
        context: &SideEffectContext,
    ) -> Result<BacktestImmutableWrite, String>;
    fn list_events(&self, run_id: &str) -> Result<Vec<BacktestLifecycleEvent>, String>;
    fn find_run_by_idempotency_key(
        &self,
        idempotency_key: &str,
    ) -> Result<Option<BacktestRunRecord>, String>;
}
