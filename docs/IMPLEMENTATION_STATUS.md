# Статус реализации — 0.1.3, обновлено 2026-10-03

[План приёмки и выпуска 1.0](superpowers/plans/2026-10-03-full-release.md) и
[состав релиза](RELEASE_SCOPE_1_0.md) определяют оставшиеся работы.
Native CI на текущем source **e10b8dd** успешно завершился на Linux/macOS/Windows:
[run 37035427836](https://github.com/Arkus61/agent-harness/actions/runs/37035427836).
Это подтверждает проверенные facets; полные S01–S42 автоматически не закрываются.
Локальные отчёты ниже остаются привязаны к указанным в них историческим исходникам.

**191 локальный Rust-тест PASS; полная готовность не подтверждена.** ChatGPT
smoke PASS, но свежий H trial остановился на Codex timeout с unknown usage.
[Текущий отчёт](VALIDATION_REPORT_0.1.3_2026-10-02.md),
[JSON](validation-summary-0.1.3-2026-10-02.json),
[131 критерий](readiness-matrix-0.1.3.json).

| Область | Реализовано | Оставшаяся работа |
|---|---|---|
| Agent core | Rust CLI, WAL/FULL, intents/receipts, budgets/cancel/resume, exact Git candidate, 4 reviewers, enforced DecisionService | Full model/cross-platform acceptance и calibration |
| DAG | Coverage/ownership/cycle guards, bounded parallel builders, безопасная model fallback revision | Полный multiagent corpus и measured speedup |
| Runtime | Native supervisor/owner lease, Windows lifecycle JobObject с native CI, probed Linux bubblewrap/protected mounts | Полные Windows/macOS system scenarios, isolation и aggregate quotas |
| Process limits | CPU/AS/FSIZE/NPROC hard caps, task/check binding и negative probes | Aggregate CPU/RAM/disk; Windows resource limits |
| Context | Bounded cache, scope/role/root keys, dirty-source revalidation, redacted hash/range evidence | Entailment/compaction, model-response cache, measured gains |
| Outbox | schema v3, leases/fences/retries/UNKNOWN/reconciliation, local idempotent journal CLI | Проверенные external mutating adapters и worker protocol |
| Replay | READ_ONLY consistent journal, partial projection check, zero effect/model dispatch | Full projections, authenticated event chain, GC/snapshots |
| Memory/skills/strategy | Explicit curated memory, promotion/fingerprints, pinned skills/quarantine и selector | Automatic common DAG/probe loop и benchmark setup hooks |
| Corpus | 30 разных задач, 30/30 baseline/control pairs, isolated oracles и AST/mutation traps | Настоящие B0/B1, скрытый holdout, 270 comparative runs |

Исторический [pilot 0.1.2](VALIDATION_REPORT_0.1.2_2026-10-02.md) сохраняет
18 accepted native trials с отдельной Q03 grader amendment и один isolated
pilot. Эти receipts не сертифицируют 0.1.3. Новый source прошёл fmt/clippy/
release, component eval и fixture demo; исходные thresholds не уменьшены.

Native команды сохраняют права пользователя. Same-user hostile races и
намеренно detached native processes не считаются sandbox confinement.
Process limits не считаются aggregate quotas. Unknown usage остаётся
зарезервированной. Power-loss durability отдельно не сертифицирована.

[Новые контракты](LOCAL_RUNTIME_CONTRACTS.md),
[corpus workflow](../evals/benchmark/README.md),
[план оценки](EVALUATION_PLAN.md), [архитектура](ARCHITECTURE_PLAN.md).
