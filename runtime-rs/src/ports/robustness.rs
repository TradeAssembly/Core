use crate::ports::{SideEffectContext, VersionedPort};
use crate::robustness_contracts::{
    RobustnessAttempt, RobustnessLifecycleEvent, RobustnessResult, RobustnessRunManifest,
    RobustnessRunRecord,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RobustnessImmutableWrite {
    Created,
    AlreadyPresent,
}

pub trait RobustnessRunRepository: VersionedPort {
    fn put_manifest(
        &self,
        manifest: &RobustnessRunManifest,
        context: &SideEffectContext,
    ) -> Result<RobustnessImmutableWrite, String>;
    fn get_manifest(&self, run_id: &str) -> Result<Option<RobustnessRunManifest>, String>;
    fn put_result(
        &self,
        result: &RobustnessResult,
        context: &SideEffectContext,
    ) -> Result<RobustnessImmutableWrite, String>;
    fn get_result(&self, run_id: &str) -> Result<Option<RobustnessResult>, String>;
    fn create_run(
        &self,
        run: &RobustnessRunRecord,
        context: &SideEffectContext,
    ) -> Result<RobustnessImmutableWrite, String>;
    fn get_run(&self, run_id: &str) -> Result<Option<RobustnessRunRecord>, String>;
    fn list_runs(&self) -> Result<Vec<RobustnessRunRecord>, String>;
    fn compare_and_put_run(
        &self,
        expected: &RobustnessRunRecord,
        replacement: &RobustnessRunRecord,
        context: &SideEffectContext,
    ) -> Result<bool, String>;
    fn put_attempt(
        &self,
        attempt: &RobustnessAttempt,
        context: &SideEffectContext,
    ) -> Result<(), String>;
    fn get_attempt(&self, attempt_id: &str) -> Result<Option<RobustnessAttempt>, String>;
    fn list_attempts(&self, run_id: &str) -> Result<Vec<RobustnessAttempt>, String>;
    fn compare_and_put_attempt(
        &self,
        expected: &RobustnessAttempt,
        replacement: &RobustnessAttempt,
        context: &SideEffectContext,
    ) -> Result<bool, String>;
    fn append_event(
        &self,
        event: &RobustnessLifecycleEvent,
        context: &SideEffectContext,
    ) -> Result<RobustnessImmutableWrite, String>;
    fn list_events(&self, run_id: &str) -> Result<Vec<RobustnessLifecycleEvent>, String>;
    fn find_run_by_idempotency_key(
        &self,
        idempotency_key: &str,
    ) -> Result<Option<RobustnessRunRecord>, String>;
}
