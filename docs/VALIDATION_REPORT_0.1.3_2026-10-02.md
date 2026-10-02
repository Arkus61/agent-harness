# Проверки 0.1.3 — 2026-10-02

**191 Rust-тест PASS; fmt, clippy и release build PASS. Полная готовность
харнесса не подтверждена.** Новый live H trial завершился BLOCKED после
реального Codex timeout без полной usage-квитанции. Windows/macOS, comparative
benchmark и несколько архитектурных подсистем ещё не закрыты.

[Машиночитаемый результат](validation-summary-0.1.3-2026-10-02.json),
[131 исходный критерий](readiness-matrix-0.1.3.json),
[контракты](LOCAL_RUNTIME_CONTRACTS.md). Условия приёмки не уменьшались.
Исторические [результаты 0.1.2](VALIDATION_REPORT_0.1.2_2026-10-02.md) сохранены
и не выдаются за сертификацию нового бинарника.

## Изменения

- Hard limits CPU/address space/file size/UID process count в supervisor target,
  наследование и binding в task/check/DecisionScope. Defaults могут только
  ужесточать explicit values. Linux `no_new_privs`, проверка привилегий и
  реальный syscall probe. Aggregate requests блокируются заранее.
- Engine использует bounded ContextCache, повторно проверяет scoped источники
  и сохраняет redacted EvidencePacket с полным hash и byte range. Claims
  остаются UNKNOWN. Устранены утечки части известного секрета при обрезке.
- SQLite outbox schema v3: immutable policy, lease/fence, bounded retry/backoff,
  dead letter/UNKNOWN, reconciliation и local idempotent journal CLI.
- `replay` читает согласованный READ_ONLY snapshot, проверяет task/state/
  candidate/generation projection и не запускает эффекты. Read-only Git
  discovery и `doctor` не инициализируют state.
- Strict CLI diagnostics не выводят ошибочные scalar values. DAG diagnostics
  различают cycle/missing dependency; неверный свежий model plan получает
  отдельную fallback revision в исходных grants.

## Прогон на Linux

| Проверка | Результат |
|---|---|
| Rust, все targets | **191 PASS, 0 FAIL, 1 ignored** |
| Новые integration tests | 15 context + 13 outbox + 18 resources + 10 CLI scenarios + 4 replay + 2 journal CLI |
| fmt / clippy `-D warnings` / release | PASS |
| Component `eval` / release demo | 20/20; FIXTURE_VERIFIED; fixture evidence |
| Старый AST oracle / Python grader | 8 Rust + 23 Python PASS |
| Frozen manifest controller | 9 PASS, отдельно 9 PASS при `python -O` |
| Новый benchmark controller / AST | 14 Python + 2 Rust PASS; clippy/fmt PASS |
| Benchmark baseline/control pairs | **30/30 PASS**, реальные Linux isolated oracles |
| Живое ChatGPT-подключение | PASS, gpt-6.1-sol, Plus; 5497 input + 45 output tokens |
| Новый live H Q02-D1 trial 1 | **BLOCKED**, candidate не создан |
| Windows/macOS | NOT_EXECUTED |

Linux isolation tests выполнены с `HARNESS_REQUIRE_LINUX_ISOLATION=1`:
недоступный backend не превращается в skip/PASS. Process limits проверены
реальными bounded CPU/memory/file/fork probes, также внутри bubblewrap.
Это не aggregate CPU/RAM/disk quotas; host cgroup не изменялся.

Raw receipts: [Rust-прогон](../artifacts/readiness-0.1.3/cargo-test.log),
[controller tests](../artifacts/readiness-0.1.3/benchmark-controller-final.log),
[freeze](../artifacts/readiness-0.1.3/benchmark/freeze-final/manifest.json),
[doctor](../artifacts/readiness-0.1.3/doctor.json). Неуспешные подготовительные
прогоны сохранены отдельно: PATH configuration, отсутствие read-only Git
discovery, экранирование Rust fixture и linker mounts исправлены до финального
freeze. Их результаты не суммируются с итоговыми test counts.

## Corpus и реальная модель

Заморожены 30 разных задач: 20 dev и 10 holdout-labelled. Каждый baseline
компилируется и проваливает нужный внешний критерий; каждый исправленный
контроль проходит. Q06 mutants уничтожены compiled assertion failures;
Q08-D1 содержит 10 000 файлов. Candidate check failure не маскируется
успешным external contract. Candidate/oracle sources монтируются read-only.

Это проверка **корпуса**, а не 30 успешных runs харнесса. Holdout виден
разработчикам. B0/B1 ещё требуют настоящих ablation modes; полный протокол —
**30×3×3 = 270 свежих runs**. Memory/skill setup hooks и measured strategy
selection не выполнены; code PASS не повышает их scenario verdict.

Свежий H Q02-D1 работал в `isolated/enforced`. Четыре model calls получили
полную usage-квитанцию; два read actions имеют receipts. Пятый call превысил
120 секунд без полной telemetry:

- Run `fd72d4df-ec37-4971-bb76-941637007b0c`: BLOCKED, candidate отсутствует,
  HEAD сохранён. Внешний candidate oracle поэтому не исполнялся.
- 25 181 observed tokens; **27 134 остаются зарезервированы**. Unknown usage
  не обнуляли; automatic retry и фиктивный settlement не выполнялись.
- Read-only replay подтвердил согласованность исторической projection.
  Причина remote timeout не установлена. Provider quota error в этом trial
  не наблюдался; quota errors подагентов — отдельное наблюдение.

[Live summary](../artifacts/readiness-0.1.3/live-summary.json),
[original report](../artifacts/readiness-0.1.3/live-Q02-D1.stdout.json),
[replay](../artifacts/readiness-0.1.3/live-replay.json).

## Скорость и происхождение

Warm p95: `--version` **2,163 ms**, `doctor` **26,726 ms**, `eval` **6,659 ms**.
Для каждой команды — 3 warmups и 30 raw subprocess samples; machine и hashes
сохранены в [performance.json](../artifacts/readiness-0.1.3/performance.json).
Во время измерений мог выполняться live pilot. Cold startup, idle/whole-tree
RSS, no-I/O dispatch и matched serial/parallel speedup этим не подтверждены.

Runtime commit: `48d9f09`; source SHA256:
`dcf0639d0dc79bde731080c648a3e511258f3522822b37db3570e0461dfe7b18`.
Frozen executable: 12 265 000 bytes; SHA256:
`efa0ce84973e0d71f9975d0235341c5b0cd0833710aba2c59d8f7d4ce5cb950f`.
Rust 1.99.0. [Provenance](../artifacts/readiness-0.1.3/release/provenance.json).

## Незакрытая приёмка

1. Завершить real-model acceptance и получить независимую telemetry unknown
   call. Стабильность 3/3 для новой версии не доказана.
2. Выполнить CI на настоящих Windows/macOS. Исполнителей и git remote в среде
   нет; подготовленный workflow не считается завершённым CI run.
3. Реализовать B0/B1, скрытый holdout и comparative runner; выполнить 270 runs.
   Закрыть reviewer/decision calibration и все 42 сценария/131 критерий.
4. Доработать aggregate quotas, automatic memory/skill/strategy hooks и probes,
   entailment/compaction, внешний worker protocol, response cache, полный
   replay/GC/snapshots и controlled power-loss verification.
5. Провести свежий независимый review финального объединённого дерева.
   Подагенты остановились на usage limit; baseline audit и subsystem receipts
   сохранены, но не подменяют такой review.

Версия пригодна для дальнейшей локальной разработки и проверки перечисленных
контрактов. Полный production/cross-platform READY не объявляется.
