# Статус реализации — 0.1.2, 2026-10-02

**129 локальных тестов PASS; 18 свежих native trials завершили enforced цикл с VERIFIED, отдельный Linux isolated Q02 также прошёл.** Независимая приёмка — **18/18: 15 original + 3 supplemental Q03 PASS** после исправления grader, с сохранением исходного 15/18 и FAIL receipts. Реализован Rust CLI с agent loop, DAG, Git worktrees, protected checks и четырьмя reviewer-сессиями. Доступны native и Linux isolated runtime через probed bubblewrap. Полное завершение [архитектуры](ARCHITECTURE_PLAN.md) не заявляется. [Текущий отчёт](VALIDATION_REPORT_0.1.2_2026-10-02.md), [JSON](validation-summary-0.1.2-2026-10-02.json); исторические [0/18 версии 0.1.1](VALIDATION_REPORT_2026-10-02.md) сохранены отдельно.

## Реализовано

| Область | Фактическая реализация |
|---|---|
| CLI | run/resume/status/inspect/report/cancel/reconcile/settle/merge; doctor/demo/eval; memory/skills/strategy; ChatGPT login/status/check через официальный Codex |
| TaskSpec и DAG | Строгий JSON, validation budget/checks/grants; explicit DAG либо model planner, coverage/cycles/ownership guards, bounded parallel builders |
| Git и интеграция | Detached worktrees, точные input/output SHA, интеграция дельт, отключённые служебные hooks/filters; отдельная fast-forward публикация с expected-ref CAS |
| ToolGateway | Read/write/command grants, ownership, read-only reviewers, normalized paths, links/junctions denial; atomic existing-file writes/edits с full-source BLAKE3 CAS |
| Protected acceptance | Gateway запрет и final candidate diff; isolated commands получают read-only protected mounts с консервативным ancestor freezing |
| Runtime | Native supervisor с owner lease и cleanup после owner death/normal exit; Linux isolated filesystem/PID/network namespaces, credential masking, trusted read-only runtime mounts |
| Durability и recovery | SQLite WAL/FULL, state/events/outbox транзакции, action intents/receipts, BLAKE3 CAS; generation fencing, completed-node reuse, unknown effect/usage HOLD |
| Budget | Root spent/reserved tokens, idempotent settlement, deadline от исходного created_at; неизвестный расход не обнуляется |
| Providers | Строгий OpenAI-compatible JSON/usage transport; Codex stdio с fresh tool-free ephemeral thread, outputSchema и complete usage либо HOLD |
| DecisionService | Purpose-specific model/tools/risk subjects, UUID и full-request hash с runtime profile; static model choice, no tool expansion, deterministic policy сохраняется |
| Context и credentials | Scoped deterministic map/snippets с byte limits и source hashes; общая sensitive-path policy для initial context и explicit Gateway; paths относительно worktree |
| Verification и repair | Exact-candidate checks; requirements/code/tests/security roles и source/line/coverage guards; repair от интегрированного кандидата с повтором всех gates |
| Parallel diagnostics | Primary failure сохраняется отдельно; secondary cancellations и unknown reservations остаются видимыми |
| Fixtures | Scripted только FIXTURE_VERIFIED; production gate и publication не закрываются fixture evidence |
| Memory | SQLite FTS5, project/source scope, candidate/promotion/conflicts; до пяти active records builder для exact input SHA, candidate episode после production VERIFIED |
| Skills и SESE | Pinned immutable skill packages, dependencies/capabilities/quarantine и runtime instructions; отдельный bounded selector, без automatic probe execution |
| CI | Linux/macOS/Windows workflow задан; фактические результаты текущей сессии относятся к Linux |

Outbox — durable журнал и основа hook idempotency; универсальный dispatcher с worker leases/retries ещё не реализован. Knowledge/skills имеют отдельные SQLite stores, без общей транзакции с core DB.

## Подтверждённые проверки 0.1.2

Замороженный runtime source: **`d532e47517460893a9e67047969d66c7524ea1c5`**, Linux x86_64, Rust 1.99.0; последующие изменения относятся к независимому grader, CI и отчётам. Final release **harness 0.1.2**: **11 820 328 байт**, SHA256 **`bbd2b6b461137ebdd7609d35c3a31d27ff40aee715eb54c2fbfe0b0a01f9044b`**. [Provenance](../artifacts/contract-validation/release/provenance.json).

| Проверка | Результат | Доказательство |
|---|---|---|
| cargo test --locked --all-targets | **129 PASS**, 0 failures; 1 отдельный manual smoke ignored | [Лог](../artifacts/contract-validation/local/cargo-test.stdout.log) |
| fmt, clippy --all-targets -D warnings, release build | **PASS** | [Команды и exits](../artifacts/contract-validation/local/summary.json) |
| eval, demo, status/report | **20/20 assertions PASS**, demo FIXTURE_VERIFIED | Тот же summary |
| Linux isolation doctor/probe | **available:true**, linux-bubblewrap | [Doctor](../artifacts/contract-validation/local/doctor.stdout.log) |
| Linux boundary fixtures | **8 PASS**, входят в 129; required-availability flag исключает skip в этом прогоне | Итоговый test log |
| Native/Codex owner SIGKILL и normal-exit cleanup | **PASS** для тестовых ordinary descendants | Итоговый test log; deliberately detached native процессы вне process-group границы |
| Primary parallel failure | **PASS**; initiating error не заменён sibling cancellation | Итоговый test log |
| Один реальный risk-contract smoke | **PASS**; полный usage 5388 input + 137 output; Gateway не исполнялся | [Receipt](../artifacts/contract-validation/subscription/live-risk-contract-smoke.json) |
| Native enforced pilot | **18 VERIFIED**, acceptance **18/18 = 15 original + 3 supplemental PASS**, binding failures **0** | [Raw original trials](../artifacts/contract-validation/live/summary.json), [bindings](../artifacts/contract-validation/candidate-binding-audit.json), [amendment](../artifacts/contract-validation/oracle-amendment/summary.json) |
| Полный isolated Q02 | **PASS**, 115,177 с, exact-candidate oracle/binding PASS | [Receipt](../artifacts/contract-validation/isolated-pilot/result.json); external oracle native-trusted |
| Фактически недоступный isolation backend | **PASS fail-closed**, 0 attempts/model/action intents | [Receipt](../artifacts/contract-validation/fail-closed/result.json) |
| Representative test-quality audit | **6 candidates**, 26 new test functions; isolated red/green controls и Q06 mutants | [Аудит](../artifacts/contract-validation/test-quality-audit.md) |
| Catalog consistency | **42 valid scenarios**, **73 existing mappings**, full scenarios **0** | [Receipt](../artifacts/contract-validation/catalog-consistency.json) |

129 tests: **85 library + 3 Codex CLI + 14 Codex protocol + 6 contracts + 7 end-to-end + 7 isolation + 1 parallel failure + 3 parent-death + 3 security regressions**. Восемь boundary checks состоят из семи integration isolation tests и одного library isolation test. Повторные отдельные исполнения этих же tests не увеличивают число уникальных tests.

Boundary fixtures проверяют workspace доступность без host home/credentials, protected source/parent replacement, отсутствующий protected prefix, hardlinks/symlinks, отсутствие host loopback/nested namespaces, offline Cargo и cleanup detached sandbox descendants. Synthetic credentials используются только как canaries. Это реальное исполнение Linux backend в данном окружении, а не полная сертификация против всех kernel/host attacks.

Официальный ChatGPT вход подтверждён историческим auth check: account chatgpt/plus, gpt-6.1-sol, usage 5000+45. Новая отдельная risk оценка проверяет исправленное разграничение readonly inference/Gateway. Остаток quota и будущая доступность модели из этих фактов не выводятся. [Настройка подписки](CHATGPT_SUBSCRIPTION.md).

Native pilot: **1 465 445 input + 46 879 output = 1 512 324 tokens**, **205 complete calls**, unknown/reserved **0**. Все 18 candidates имеют exact command checks и четыре reviews с валидными source references; binding не доказывает произвольное reviewer explanation. Q03 grader ошибочно включал private helper после public функции в её тело. Исправленный standalone AST grader принял **все три точных кандидата**, с unchanged behavior checks и protected contract; original 15/18 и FAIL receipts сохранены, новых model calls нет. Отдельные **8 Rust + 23 Python grader tests PASS** не входят в core 129. Isolated Q02: **71 937 tokens**, **10 complete calls**, unknown/reserved **0**, candidate **20443ec9e3a5d6bc6ad200e21574283fb9c19395**. Source, task и original HEAD preserved; publication отсутствует.

**Исторические результаты 0.1.1:** 91 tests и component checks PASS; основной enforced pilot 0/18 до candidate; отдельный Q02 shadow diagnostic прошёл внешний oracle. Эти trials относятся к старому binary и не являются результатом 0.1.2.

Warm CLI **0.1.2** p95: version **2,482 мс**, doctor **28,685 мс**, eval **14,011 мс**; measurements выполнены одновременно с одним live pilot. Это не full startup, full-run RSS или matched parallel speedup. [Raw samples](../artifacts/contract-validation/performance/summary.json).

Полный runner S01–S42, frozen 30-task dev/holdout benchmark с B0/B1/H и тремя повторениями, reviewer/Jev calibration, Windows/macOS runtime и complete performance gates не закрыты. [План оценивания](EVALUATION_PLAN.md), [каталог](../evals/scenarios.json), [Schema](../evals/scenarios.schema.json).

## Ограничения реализации

1. **Native имеет права пользователя.** Gateway grants не ограничивают filesystem/network effects subprocess; process cleanup не является sandbox. Deliberately detached native процессы могут выйти из process group.
2. **Isolated поддержан только на Linux после probe.** Требуются bubblewrap и разрешённые namespaces; иначе run отказывает без native downgrade. Workspace — ресурс команды, не syscall-level owner scope. Trusted system/toolchain mounts read-only; CPU/memory/disk quotas отсутствуют. Kernel vulnerabilities и hostile concurrent same-UID host races вне текущей границы.
3. **Protected mounts могут ограничить сборку.** Если path ещё не существует, freezes ближайший existing ancestor; Gateway edit и command write могут иметь разный effective доступ. Credential/.git masking может сделать legacy Cargo Git registry cache непригодным для offline build; проверьте dependencies заранее.
4. **Supervisor — часть packaged CLI.** Executable должен сохранять имя harness/harness.exe. Rust library host требует CLI companion в поддерживаемом расположении; произвольный executable не служит supervisor. Windows guard назначает Job Object до command header, но фактическое Windows исполнение не проверено.
5. **Resume восстанавливает node checkpoints, не незавершённый диалог.** Pending effects/usage требуют reconciliation. Завершённая команда незавершённого builder запрещает автоматический replay; нужны inspection и явно новая задача.
6. **Token/deadline accounting не является billing.** Денежные тарифы/лимиты отсутствуют. ChatGPT output cap мягкий; hidden context/retries могут превысить reservation estimate. Bounded usage drain не является final billing receipt. Unknown spending удерживает reservation.
7. **DecisionService не полноценный Jev router.** Typed subjects/bindings и полный enforced cycle проверены на public pilot, но нет отдельного SDK, calibration, multi-provider route switching или доказанного выигрыша neural routing.
8. **Source proofs не проверяют смысл.** Path/line/coverage guards не доказывают объяснение reviewer. Independent roles одной модели могут коррелированно ошибаться; нужна внешняя приёмка и calibration corpus.
9. **Trusted oracle — отдельная граница.** Protected task/check definitions и candidate diffs не создают полноценный hidden acceptance controller. Существующие public toy oracles не заменяют frozen holdout.
10. **Context/credential filtering не универсальная DLP.** Общая path policy и explicit redaction закрывают известные имена/секреты, но неизвестный секрет может находиться в обычном source. Нет автоматической L0 classification или encrypted state store. Административные memory/skills inputs сохраняют пользовательские данные.
11. **Context Shunt и autonomous knowledge/strategy частичны.** Нет cheap extraction tier, AST index, entailment validator, полного compaction pipeline или автоматического strategy/probe loop. Episode остаётся candidate до promotion.
12. **Нет полного replay/cache/GC и streaming UI.** Worktrees/receipts сохраняются для inspection; OpenAI transport возвращает финальный ответ, пользовательская live-token UI отсутствует.
13. **Codex protocol version-sensitive.** Проверенная версия 0.159.0-alpha.3; unexpected tool/server items fail closed. Credentials остаются в официальном CLI store. Future protocol compatibility, доступная подписка и quota требуют собственных проверок.
14. **Кроссплатформенность требует выполнения.** Source/workflow содержит OS branches; Windows/macOS в этой сессии NOT_EXECUTED. Готовый Linux binary требует glibc >=2.39; для старой среды нужна пересборка. Cold/full startup, RSS полного успешного цикла и matched serial/parallel speedup ещё требуют измерений.

Функциональный Linux core прошёл указанный pilot; расширение проверки включает полный benchmark, три-OS CI, crash/power-loss corpus, reviewer calibration и измерение context/strategy gains. [README](../README.md) содержит команды; [контракт DecisionService](DECISION_CONTRACT.md) и [knowledge/skills/strategy](knowledge-skills-strategy.md) описывают подсистемы.
