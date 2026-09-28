# Hyakki 百鬼 — coordination-слой Meisei

> **Meisei** 明晰 («ясность») — открытый конвейер, который проводит сырой замысел
> через понимание → решение → план → действие к готовому результату.

[![Meisei](https://img.shields.io/badge/meisei-明晰-1f2937.svg)](https://meisei.ru)
[![License: Apache-2.0 WITH Commons-Clause](https://img.shields.io/badge/license-Apache--2.0%20WITH%20Commons--Clause-blue.svg)](LICENSE)

<sub>
torii · satori · enma · yatagarasu · fujin · daruma
&nbsp;—&nbsp; <b>hyakki</b> видит всю процессию со стороны, вне цепочки
</sub>

## Что это

Hyakki — слой **cluster / координации**: чистая read-only проекция, отвечающая
на вопрос «где сейчас стоит каждый прогон конвейера и что его держит». Это не
хоп цепочки torii → … → daruma. Хост (например, MCPBox) собирает `RunSnapshot`
по каждому прогону из своих записей, Hyakki превращает снимки в строки
`ProcessionEntry`.

Чего Hyakki **не** делает: не оркестрирует, не перемаршрутизирует, не мутирует
доменные объекты, не оценивает зрелость (гейты остаются за слоями), не
алертит, не читает часы, ничего не хранит и не ходит в сеть. Без async и без
зависимостей на daruma/mcpbox/соседние слои.

## API

```rust
pub fn project(runs: &[RunSnapshot], now: Timestamp, thresholds: &Thresholds)
    -> Vec<ProcessionEntry>;
```

- Время — Unix-миллисекунды (`Timestamp = i64`), длительности — миллисекунды
  (`Milliseconds = u64`). `now` передаётся параметром.
- `Thresholds::default()` — `stale_after` = 10 минут; прогон считается
  застрявшим строго после порога. Хост передаёт порог меньше TTL своего
  sweeper'а брошенных прогонов (у MCPBox `MCPBOX_PIPELINE_RUN_TTL_MINUTES` = 30).
- `completed` — **не** терминальный: цепочка пройдена, прогон ждёт одобрения
  владельца / handoff. Терминальные только `handed_off` и `superseded`.
- Приоритет blocker (первое совпадение): handed_off / superseded ⇒ blocker
  нет; петля (`loop_detected:<layer>`) → gate not ready → approval →
  нерешённый инцидент → failed-прогон → застрявший running-прогон.
  Completed-прогон, прошедший петлю/gate/approval/инциденты, готов к handoff
  (blocker нет).
- `stale_for` считается для running и completed; блокер `Stale` — только для
  running.
- `responsible_layer`: слой петли; для approval-блокеров — нет (прогон держит
  владелец); иначе текущий (последний) хоп.
- Маппер хоста сам строит `HopSnapshot` и пропускает хопы неизвестных слоёв
  (`Layer` закрыт), а не десериализует записи хопов хоста напрямую.
  `gate_verdict = None` — нормальный случай для хоста, который не сохраняет
  вердикт fujin (MCPBox сейчас не сохраняет).

Все типы сериализуются через serde; подробности — rustdoc в `src/lib.rs`.

## Решение

[ADR-0020: Hyakki v0 — read-only проекция процессии](https://github.com/tupical/meisei.ru/blob/main/adr/0020-hyakki-v0-read-only-procession-projection.md)
(все ADR: [meisei.ru/adr](https://github.com/tupical/meisei.ru/tree/main/adr)).

## Лицензия

Apache-2.0 WITH Commons-Clause — см. [LICENSE](LICENSE) и
[LICENSE.commons-clause.md](LICENSE.commons-clause.md).
