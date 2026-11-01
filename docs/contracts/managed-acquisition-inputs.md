# Managed acquisition input templates

This local management configuration describes bounded JSON input materialization. It grants no provider, credential, order or execution authority. Every acquisition uses the existing Core HTTP, source binding and input-witness contracts.

Existing `literal`, `trade_instrument_id`, `event_start_ms`, `event_end_ms`, `as_of_ms`, `interval_ms` and `origin_start_ms` wire forms remain unchanged. A literal can contain static JSON arrays/objects. The trade instrument is the exact owned Core ID; no provider-symbol conversion is performed.

Two additional closed input forms are supported:

```json
{
  "symbol": {"kind":"trade_instrument_id"},
  "range": {"kind":"object","fields":{
    "begin":{"kind":"window_timestamp","basis":"event_start","format":"rfc3339_utc_millis"},
    "finish":{"kind":"window_timestamp","basis":"event_end","format":"rfc3339_utc_millis"},
    "observedThrough":{"kind":"window_timestamp","basis":"as_of","format":"rfc3339_utc_millis"}
  }},
  "profile":{"kind":"literal","value":"explicit-source-profile"}
}
```

`object.fields` recursively contains the same input union. `window_timestamp.basis` is exactly one of `event_start`, `event_end`, `as_of` or `origin_start`. Its only format is `rfc3339_utc_millis`: UTC, literal `Z`, exactly three fractional digits and a four-digit year. The instant comes from the trusted evaluation window or retained original capture origin. No interval conversion, calendar/session boundary, market time, host timezone or arbitrary expression is inferred.

The top map has at most32 fields. The typed Input tree has at most128 nodes and depth8, counting each top Input as depth1 and each object as a node. Literal JSON retains its existing metadata bounds rather than being interpreted as additional template nodes. Serialized templates and the complete management config remain bounded to32KiB; materialized input retains the normal metadata limits. Nested field names use the same bounded field-name/reserved-name rules. New object templates reuse the retained-query credential-material guard. They cannot embed credentials through nested field names or literal objects.

For original V2 capture evidence, every declared `as_of_ms` and formatted `as_of` leaf is extracted recursively. All leaves must agree, must be at or after policy.end, and must be no later than actual acquisition.observedAt. Formatted witnesses must equal the canonical UTC millisecond representation; equivalent timezone offsets, alternate precision and invalid dates refuse. The producer then rematerializes the complete expected input and compares its canonical hash with the protected original input witness, including every literal/profile/filter/instrument field. No malformed/stale evidence becomes a false signal or financial permission.

Changing an input template changes the immutable config/source binding. Legacy configuration, state and action serialization remain unchanged when these new kinds are omitted. Source APIs, provider schemas, SDK contracts and remote financial wire versions are not extended by this template feature. A provider that expects a different instrument namespace still needs an independently verified mapping and output-normalization contract.
