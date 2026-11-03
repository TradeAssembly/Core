# Evaluation knowledge and arrival cutoffs

`EvaluationWindow.asOfMs` is the caller's availability cutoff. Every input/output
point must have both `knownAtMs` and `arrivedAtMs` at or before that cutoff.
These clocks retain their separate meanings; neither is replaced by the other.
A transform receives the declared outer cutoff, rather than a cutoff truncated
to the latest input knowledge time. This allows legitimate inputs known at50
and arriving at60 under cutoff100 without inventing knowledge time60 or100.

Checkpoint reuse checks both clocks in node/partition state availability and
all prior-frame points before source invocation or dispatch-fence writes.
This includes source-only strategies using `Sample::Previous`, which can carry
prior-frame evidence without any opaque transform state. A checkpoint containing
arrival60 cannot be resumed at cutoff55. Rejection leaves provider invocation,
checkpoint, receipt and source records unchanged. A later admissible cutoff can
reuse the original checkpoint; the host never raises the caller's cutoff.

`DataStrategy::evaluate` also rejects rule evidence arriving after its cutoff,
independently of the service guard. Existing wire/checkpoint versions and V3
unavailable-input reset behavior are unchanged. In particular, this correction
does not promise metadata continuity across repeated missing numeric inputs;
that requires an explicit compatible state/continuity contract.
