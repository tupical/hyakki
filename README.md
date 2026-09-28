# Hyakki 百鬼 — coordination layer of Meisei

> **Meisei** 明晰 (“clarity”) is an open pipeline that carries raw intent through
> understanding → decision → plan → action to a finished result.

[![Meisei](https://img.shields.io/badge/meisei-明晰-1f2937.svg)](https://meisei.ru)
[![License: Apache-2.0 WITH Commons-Clause](https://img.shields.io/badge/license-Apache--2.0%20WITH%20Commons--Clause-blue.svg)](LICENSE)

<sub>
torii · satori · enma · yatagarasu · fujin · daruma
&nbsp;—&nbsp; <b>hyakki</b> watches the whole procession from outside the chain
</sub>

## What it is

Hyakki is the **cluster / coordination** layer: a pure, read-only projection
that answers “where is every pipeline run right now and what is holding it”.
It is not a hop in the torii → … → daruma chain. The host (e.g. MCPBox) builds
a `RunSnapshot` per run from its own records; Hyakki turns the snapshots into
`ProcessionEntry` rows.

What it does **not** do: orchestrate, reroute, mutate domain objects, assess
maturity (gates stay with their layers), alert, read a clock, store anything,
or talk to the network. No async, no daruma/mcpbox/sibling-layer dependencies.

## API

```rust
pub fn project(runs: &[RunSnapshot], now: Timestamp, thresholds: &Thresholds)
    -> Vec<ProcessionEntry>;
```

- Time is Unix milliseconds (`Timestamp = i64`), durations are milliseconds
  (`Milliseconds = u64`). `now` is a parameter.
- `Thresholds::default()` — `stale_after` = 10 minutes; a run is stale
  strictly after it. Pass a value below the host's abandoned-run sweeper TTL
  (MCPBox: `MCPBOX_PIPELINE_RUN_TTL_MINUTES` = 30).
- `completed` is **not** terminal: the chain finished and the run awaits
  owner approval / handoff. Only `handed_off` and `superseded` are terminal.
- Blocker priority (first match wins): handed_off / superseded ⇒ no blocker;
  loop (`loop_detected:<layer>`) → gate not ready → approval → unresolved
  incident → failed run → stale running run. A completed run that passes
  loop/gate/approval/incident is ready for handoff (no blocker).
- `stale_for` is reported for running and completed runs; the `Stale` blocker
  applies to running runs only.
- `responsible_layer`: the loop's layer; none for approval blockers (the owner
  holds the run); otherwise the current (last) hop.
- The host mapper builds `HopSnapshot`s itself and skips hops of unknown
  layers (`Layer` is closed); it does not deserialize host hop records
  directly. `gate_verdict = None` is normal for a host that does not persist
  the fujin verdict (MCPBox currently does not).

All types are serde-serializable; see the rustdoc in `src/lib.rs`.

## Decision

[ADR-0020: Hyakki v0 — read-only procession projection](https://github.com/tupical/meisei.ru/blob/main/adr/0020-hyakki-v0-read-only-procession-projection.md)
(all ADRs: [meisei.ru/adr](https://github.com/tupical/meisei.ru/tree/main/adr)).

## License

Apache-2.0 WITH Commons-Clause — see [LICENSE](LICENSE) and
[LICENSE.commons-clause.md](LICENSE.commons-clause.md).
