use super::*;
use crate::historical_data::{BarSession, DatasetSnapshotContent};
use serde_json::json;
const DAY: i64 = 86_400_000;
fn fixture() -> (
    TypedBacktestBinding,
    DatasetBinding,
    DatasetSnapshot,
    crate::spec::ClockPolicy,
) {
    let start = chrono::DateTime::parse_from_rfc3339("2026-01-01T00:00:00Z")
        .unwrap()
        .timestamp_millis();
    let binding: TypedBacktestBinding = serde_json::from_value(json!({
        "schemaVersion":"tradeassembly.typed-backtest-binding.v2","planRevisionId":"dataplan_fixture",
        "planRevisionSha256":"a".repeat(64),"sourceCaptures":{"source":{"captureId":"capture_fixture","receiptSha256":"b".repeat(64)}},
        "eventStartMs":start,"eventEndMs":start+3*DAY,"asOfMs":start+3*DAY+1000,"intervalMs":DAY,
        "maxFramesPerStep":2,"maximumFrames":3,"quantityMicros":1000000,"initialPosition":"flat",
        "knowledgePolicy":"retrospective_capture_cutoff","fillTimestampBasis":"event_start","warmupObservations":1,
        "calendarEvidence":{"kind":"declared_dataset_calendar","calendar":"crypto_24x7","timezone":"UTC",
            "purpose":"research_simulation_only","acknowledgeResearchAssumption":true}
    })).unwrap();
    let observations = (1..=3)
        .map(|day| {
            json!({"instrumentId":"SYNTHETIC/USD","timestamp":format!("2026-01-0{day}T00:00:00Z"),
        "data":{"kind":"bar","data":{"open":10,"high":11,"low":9,"close":10,"volume":1}}})
        })
        .collect::<Vec<_>>();
    let content: DatasetSnapshotContent = serde_json::from_value(json!({
        "schema":crate::historical_data::DATASET_SNAPSHOT_SCHEMA,"strategyId":"synthetic_strategy","strategyVersionId":"synthetic_version",
        "source":{"sourceClass":"test","pluginInstanceRef":"synthetic","pluginRef":"synthetic","operationId":"synthetic.bars",
            "pluginManifestFingerprint":"fixture-manifest","capabilityGraphRevisionId":"fixture-graph","capabilityGraphFingerprint":"fixture-graph-hash",
            "sourceRequestHash":"fixture-request","upstreamRefs":{}},
        "assetClasses":["crypto_spot"],"instruments":["SYNTHETIC/USD"],"dataKind":"bars","granularity":"1d",
        "timeSlice":{"start":"2026-01-01T00:00:00Z","end":"2026-01-04T00:00:00Z"},"calendar":"crypto_24x7","timezone":"UTC",
        "normalizationPolicy":{"timestampUnit":"rfc3339","timezone":"UTC","duplicatePolicy":"reject","priceAdjustment":"raw"},
        "qualityPolicy":{"missingIntervals":"reject","staleObservations":"reject","outliers":"warn","invalidMarkets":"reject",
            "calendarMismatch":"reject","corporateActionGaps":"warn","missingDerivativeFields":"reject"},"qualityFindings":[],
        "observations":observations
    })).unwrap();
    let snapshot = DatasetSnapshot::build(content, start + 3 * DAY + 1000).unwrap();
    let source = &snapshot.content.source;
    let dataset: DatasetBinding = serde_json::from_value(json!({"datasetId":snapshot.dataset_id,"snapshotRef":snapshot.snapshot_ref,
        "contentHash":snapshot.content_hash,"schema":snapshot.content.schema,"sourcePluginRef":source.plugin_ref,
        "sourcePluginInstanceRef":source.plugin_instance_ref,"sourceOperationId":source.operation_id,"sourceManifestFingerprint":source.plugin_manifest_fingerprint,
        "sourceClass":"test","capabilityGraphRevisionId":source.capability_graph_revision_id,"capabilityGraphFingerprint":source.capability_graph_fingerprint,
        "instruments":snapshot.content.instruments,"selectedTimeSlice":snapshot.content.time_slice,"granularity":"1d","calendar":"crypto_24x7",
        "timezone":"UTC","corporateActionPolicy":"raw","benchmark":null})).unwrap();
    let clock = serde_json::from_value(json!({"timezone":"UTC","calendar_refs":["crypto_24x7"],"windows":[{"start":"00:00","end":"24:00"}],
        "days":["monday","tuesday","wednesday","thursday","friday","saturday","sunday"],"holiday_behavior":"follow_calendar","early_close_behavior":"follow_calendar"})).unwrap();
    (binding, dataset, snapshot, clock)
}
#[test]
fn typed_declared_calendar_requires_explicit_model_and_never_synthesizes_sessions() {
    let (binding, dataset, snapshot, clock) = fixture();
    snapshot.verify().unwrap();
    let before = serde_json::to_value(&snapshot).unwrap();
    validate_calendar_evidence(&binding, &dataset, &snapshot.content, &clock, "crypto_spot")
        .unwrap();
    assert_eq!(serde_json::to_value(&snapshot).unwrap(), before);
    assert!(snapshot.content.observations.iter().all(
        |row| matches!(&row.data,HistoricalObservationData::Bar(bar) if bar.session.is_none())
    ));
    let mut missing = binding.clone();
    missing.calendar_evidence = None;
    assert_eq!(
        validate_calendar_evidence(&missing, &dataset, &snapshot.content, &clock, "crypto_spot")
            .unwrap_err(),
        "typed_backtest_calendar_evidence_binding_invalid"
    );
    let mut provider = binding.clone();
    let evidence = provider.calendar_evidence.as_mut().unwrap();
    evidence.kind = TypedCalendarEvidenceKind::ProviderBarSessions;
    evidence.acknowledge_research_assumption = false;
    assert_eq!(
        validate_calendar_evidence(
            &provider,
            &dataset,
            &snapshot.content,
            &clock,
            "crypto_spot"
        )
        .unwrap_err(),
        "typed_backtest_session_evidence_required"
    );
    provider.schema_version = "tradeassembly.typed-backtest-binding.v1".into();
    provider.calendar_evidence = None;
    provider.warmup_observations = None;
    assert_eq!(
        validate_calendar_evidence(
            &provider,
            &dataset,
            &snapshot.content,
            &clock,
            "crypto_spot"
        )
        .unwrap_err(),
        "typed_backtest_session_evidence_required"
    );
}
#[test]
fn typed_declared_calendar_refuses_dataset_clock_or_profile_drift() {
    let (binding, dataset, snapshot, clock) = fixture();
    for field in ["calendar", "timezone"] {
        let mut wrong = dataset.clone();
        if field == "calendar" {
            wrong.calendar = "different".into();
        } else {
            wrong.timezone = "Etc/UTC".into();
        }
        assert!(validate_calendar_evidence(
            &binding,
            &wrong,
            &snapshot.content,
            &clock,
            "crypto_spot"
        )
        .is_err());
    }
    for change in 0..5 {
        let mut wrong = clock.clone();
        match change {
            0 => wrong.timezone = "America/New_York".into(),
            1 => wrong.calendar_refs = vec!["different".into()],
            2 => {
                wrong.days.pop();
            }
            3 => wrong.windows[0].end = "23:59".into(),
            _ => wrong.days[6] = "monday".into(),
        }
        assert!(validate_calendar_evidence(
            &binding,
            &dataset,
            &snapshot.content,
            &wrong,
            "crypto_spot"
        )
        .is_err());
    }
    assert!(
        validate_calendar_evidence(&binding, &dataset, &snapshot.content, &clock, "equity")
            .is_err()
    );
    let mut content = snapshot.content.clone();
    content.granularity = "1h".into();
    assert!(
        validate_calendar_evidence(&binding, &dataset, &content, &clock, "crypto_spot").is_err()
    );
    let mut unacknowledged = binding.clone();
    unacknowledged
        .calendar_evidence
        .as_mut()
        .unwrap()
        .acknowledge_research_assumption = false;
    assert!(validate_calendar_evidence(
        &unacknowledged,
        &dataset,
        &snapshot.content,
        &clock,
        "crypto_spot"
    )
    .is_err());
}
#[test]
fn typed_declared_calendar_refuses_future_misaligned_gapped_and_conflicting_bars() {
    let (binding, dataset, snapshot, clock) = fixture();
    // A self-consistent one-hour shift is still outside the declared UTC-day
    // model. Relative one-day spacing alone cannot establish the daily anchor.
    let shift = 3_600_000;
    let mut shifted_binding = binding.clone();
    shifted_binding.event_start_ms += shift;
    shifted_binding.event_end_ms += shift;
    shifted_binding.as_of_ms += shift;
    let mut shifted_content = snapshot.content.clone();
    let shifted = |value: &str| {
        (chrono::DateTime::parse_from_rfc3339(value).unwrap()
            + chrono::Duration::milliseconds(shift))
        .to_rfc3339()
    };
    shifted_content.time_slice.start = shifted(&shifted_content.time_slice.start);
    shifted_content.time_slice.end = shifted(&shifted_content.time_slice.end);
    for row in &mut shifted_content.observations {
        row.timestamp = shifted(&row.timestamp);
    }
    let mut shifted_dataset = dataset.clone();
    shifted_dataset.selected_time_slice = shifted_content.time_slice.clone();
    assert_eq!(
        validate_calendar_evidence(
            &shifted_binding,
            &shifted_dataset,
            &shifted_content,
            &clock,
            "crypto_spot"
        )
        .unwrap_err(),
        "typed_backtest_declared_calendar_coverage_invalid"
    );
    for replacement in ["2026-01-02T00:00:00.001Z", "2026-01-05T00:00:00Z"] {
        let mut content = snapshot.content.clone();
        content.observations[1].timestamp = replacement.into();
        assert!(
            validate_calendar_evidence(&binding, &dataset, &content, &clock, "crypto_spot")
                .is_err()
        );
    }
    let mut content = snapshot.content.clone();
    content.observations.remove(1);
    assert!(
        validate_calendar_evidence(&binding, &dataset, &content, &clock, "crypto_spot").is_err()
    );
    let mut cutoff = binding.clone();
    cutoff.as_of_ms = cutoff.event_end_ms - 1;
    assert!(validate_calendar_evidence(
        &cutoff,
        &dataset,
        &snapshot.content,
        &clock,
        "crypto_spot"
    )
    .is_err());
    let mut wrong_basis = binding.clone();
    wrong_basis.fill_timestamp_basis = "event_end".into();
    assert!(validate_calendar_evidence(
        &wrong_basis,
        &dataset,
        &snapshot.content,
        &clock,
        "crypto_spot"
    )
    .is_err());
    let mut content = snapshot.content.clone();
    let HistoricalObservationData::Bar(bar) = &mut content.observations[0].data else {
        unreachable!()
    };
    bar.session = Some(BarSession {
        calendar: "different".into(),
        start: "2026-01-01T00:00:00Z".into(),
        end: "2026-01-02T00:00:00Z".into(),
        source_ref: "synthetic-unit-evidence".into(),
    });
    assert_eq!(
        validate_calendar_evidence(&binding, &dataset, &content, &clock, "crypto_spot")
            .unwrap_err(),
        "typed_backtest_declared_calendar_session_conflict"
    );
}
