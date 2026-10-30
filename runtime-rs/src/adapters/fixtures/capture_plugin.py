#!/usr/bin/python3
"""Authored local-only process fixture; no live provider or brokerage support."""
import hashlib
import json
import sys
import time
import urllib.request


def digest(value):
    return hashlib.sha256(json.dumps(value, sort_keys=True, separators=(",", ":"), ensure_ascii=False).encode()).hexdigest()


request = json.loads(sys.stdin.readline())
payload = request["payload"]["payload"]
operation = request["metadata"]["operationId"]
if operation == "acquire":
    assert request.get("ephemeralCredentialGrant")
    url = payload["config"]["dataApiUrl"]
    assert url.startswith("http://127.0.0.1:")
    with urllib.request.build_opener(urllib.request.ProxyHandler({})).open(url + "/observations", timeout=2) as response:
        observed = json.load(response)
    if payload.get("gap"):
        observed["values"] = observed["values"][:-1]
    snapshot = dict(observed, observedAtMs=int(time.time() * 1000) + (60000 if payload.get("future") else 0))
    result = {"snapshot": snapshot, "digest": digest(snapshot)}
    if payload.get("headers"):
        result["headers"] = {"NonSecretFixture": "must-not-persist"}
    schema = "schema://capture-fixture/response@1"
elif operation == "replay":
    assert not request.get("ephemeralCredentialGrant")
    snapshot = payload["parameters"]["snapshot"]
    assert digest(snapshot) == payload["parameters"]["digest"]
    points = []
    for index, value in enumerate(snapshot["values"]):
        start = snapshot["start"] + index * snapshot["interval"]
        end = start + snapshot["interval"]
        if start < payload["window"]["eventStartMs"] or end > payload["window"]["eventEndMs"]:
            continue
        point = {"instrumentId": snapshot["instrument"], "sourceEventId": "fixture:%s" % end,
                 "result": {"state": "ready", "value": value}}
        proof = dict(point, eventStartMs=start, eventEndMs=end,
                     knownAtMs=snapshot["observedAtMs"], completed=True,
                     sourceRevision="fixture.capture.1", upstreamContentSha256=[])
        point["provenance"] = {key: proof[key] for key in ["eventStartMs", "eventEndMs", "knownAtMs", "completed", "sourceRevision", "upstreamContentSha256"]}
        point["provenance"].update(arrivedAtMs=snapshot["observedAtMs"], contentSha256=digest(proof))
        points.append(point)
    assert len(points) <= payload["maxPoints"]
    number_schema = {"type": "array", "items": {"type": "number"}}
    result = {"schemaVersion": "tradeassembly.data_evaluation.v1", "state": {}, "outputs": {
        "value": {"schema": {"schemaRef": "schema://test/series@1", "sha256": digest(number_schema)}, "shape": "series", "points": points}}}
    schema = "schema://tradeassembly/data-evaluation-response@1"
elif operation == "derive":
    assert not request.get("ephemeralCredentialGrant")
    result = {"schemaVersion": "tradeassembly.data_evaluation.v1", "outputs": {"value": payload["inputs"]["source"]}, "state": {}}
    schema = "schema://tradeassembly/data-evaluation-response@1"
else:
    raise ValueError("operation unsupported")
print(json.dumps({"schemaVersion": "1", "requestId": request["requestId"], "status": "succeeded",
                  "responseSchema": schema, "payload": result,
                  "providerOutcome": {"code": "fixture_observed"}, "reconciliation": "not_required"}))
