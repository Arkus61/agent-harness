# Проверка Harness 0.1.2 — 2026-10-02

**129 локальных тестов прошли. Все 18 свежих native trials завершили полный enforced цикл с VERIFIED; отдельный Linux isolated Q02 также прошёл.** Независимая приёмка native кандидатов — **18/18: 15 исходных PASS + 3 supplemental Q03 PASS после исправления AST grader**. Исходные **15/18** и Q03 FAIL receipts сохранены. [Машиночитаемый результат](validation-summary-0.1.2-2026-10-02.json).

Исходники runtime заморожены в commit **`d532e47517460893a9e67047969d66c7524ea1c5`**; последующие изменения относятся к независимому grader, его контроллеру, CI и отчётам. Linux x86_64, Rust 1.99.0, Codex CLI 0.159.0-alpha.3, ChatGPT Plus, модель `gpt-6.1-sol`. Release **`harness 0.1.2`**: **11 820 328 байт**, SHA256 **`bbd2b6b461137ebdd7609d35c3a31d27ff40aee715eb54c2fbfe0b0a01f9044b`**. [Binary/source provenance](../artifacts/contract-validation/release/provenance.json).

Это проверка базового функционального Linux харнесса и public toy pilot. Она не подтверждает полное завершение архитектуры, frozen benchmark из 30 задач, все 42 системных сценария или Windows/macOS. [Исторический отчёт неизменного 0.1.1](VALIDATION_REPORT_2026-10-02.md) с **0/18** enforced success не переписан.

## Локальные проверки и исправления

| Проверка | Результат | Свидетельство |
|---|---|---|
| cargo test --locked --all-targets | **129 PASS**, 0 failures; 1 manual smoke ignored | [Полный лог](../artifacts/contract-validation/local/cargo-test.stdout.log) |
| fmt, clippy всех targets с -D warnings, release | **PASS** | [Команды, exits и время](../artifacts/contract-validation/local/summary.json) |
| Release eval/demo/status/report | **20/20** assertions PASS; demo FIXTURE_VERIFIED | Тот же summary; fixture не даёт production VERIFIED |
| Linux isolation probe | **available:true**, linux-bubblewrap | [Doctor](../artifacts/contract-validation/local/doctor.stdout.log) |
| Реальные Linux boundary tests | **8 PASS**, входят в 129 | Итоговый test log |
| Native/Codex owner SIGKILL, normal-exit cleanup | **PASS** ordinary descendant fixtures | Итоговый test log |
| Primary parallel failure | **PASS**, unknown sibling usage сохраняется | Итоговый test log |
| Один реальный risk-contract smoke | **PASS**, Gateway не исполнялся | [Receipt](../artifacts/contract-validation/subscription/live-risk-contract-smoke.json) |
| Fail-closed при фактически недоступном backend | **PASS**, run BLOCKED до attempts/model/action intents | [Receipt](../artifacts/contract-validation/fail-closed/result.json) |
| Catalog/Schema | **42 сценария**, **73 существующих mappings**, valid | [Audit](../artifacts/contract-validation/catalog-consistency.json); полных сценариев зачтено 0 |

129 tests: **85 library + 3 Codex CLI + 14 Codex protocol + 6 contracts + 7 end-to-end + 7 isolation + 1 parallel failure + 3 parent-death + 3 security regressions**. Manual subscription smoke выполнен отдельно; обычный suite не делал реальных model calls. Исторические 91 tests и повторные выборки не прибавляются к этому числу как новые уникальные tests.

V01 исправлен: model/tools/risk имеют отдельные subjects, UUID, BLAKE3 binding всего запроса, effective scope и runtime profile; статичную модель нельзя переключить, tools нельзя расширить. V02 исправлен: Codex оценивает JSON-предложение внешнему Gateway, сохраняя собственную read-only tool-free среду. V03 закрыт supervisor fixtures для обычных descendants после owner death; deliberately detached native процессы остаются вне process-group границы. V04 закрыт fixture, сохраняющим initiating error отдельно от secondary cancellation. [Точный контракт](DECISION_CONTRACT.md).

Восемь Linux boundary checks — семь integration tests и один library test. В данном прогоне `HARNESS_REQUIRE_LINUX_ISOLATION=1` требует доступный backend, исключая незаметный availability skip. Проверены workspace доступ без host home/credentials, protected source/parent replacement, absent protected prefix, hardlink/symlink denial, отсутствие host loopback/nested namespaces, offline Cargo, cleanup detached sandbox descendants и credential/.git masking toolchain cache.

Отдельный fail-closed controller реально скрыл executable bubblewrap от доверенного координатора. Requested isolated run завершился BLOCKED: **0 attempts, 0 model intents, 0 action intents**, без native downgrade. Это проверка недоступного backend, а не симуляция разрешения модели.

## Свежий native enforced pilot

Шесть public Rust-задач выполнены по три раза на свежих committed baseline. Configured модель, requirements, grants, protected checks и original oracle закреплены. Ни scripted answers, ни shadow mode, ни прежние кандидаты не использовались. Новые source изменения харнесса во время trials отсутствовали.

| Задача | Внутренний enforced цикл | Исходный oracle PASS | Время trial, с |
|---|---:|---:|---:|
| Q01 — новая функциональность | 3/3 VERIFIED | 3/3 | 92,242–105,318 |
| Q02 — граничная ошибка | 3/3 VERIFIED | 3/3 | 84,678–98,757 |
| Q03 — структурный рефакторинг | 3/3 VERIFIED | 0/3 original; 3/3 supplemental AST PASS | 90,108–121,954 |
| Q04 — параллельные компоненты | 3/3 VERIFIED | 3/3 | 106,542–110,393 |
| Q05 — зависимость узлов | 3/3 VERIFIED | 3/3 | 237,689–254,074 |
| Q06 — качество тестов и mutants | 3/3 VERIFIED | 3/3 | 104,494–112,508 |

Каждый из **18** кандидатов прошёл configured command checks и четыре reviews. Независимый typed verifier пересчитал exact candidate manifest и проверил привязку TaskSpec, SHA/tree, generation, configured checks, roles и всех source proof references: **18 audited, binding failures 0**. Это программная проверка ссылок и привязки; истинность произвольного объяснения не выводится из наличия path/line.

Native model telemetry: **1 465 445 input + 46 879 output = 1 512 324 токена**, **205 complete calls**, unknown **0**, reserved **0**. Ledger соответствует наблюдаемым receipts; billing receipt не проверен. Время Q04/Q05 не измеряет speedup: matched serial comparison отсутствует.

[Original live summary: 15/18](../artifacts/contract-validation/live/summary.json) · [Exact-candidate binding audit](../artifacts/contract-validation/candidate-binding-audit.json) · [Supplemental Q03 и combined acceptance 18/18](../artifacts/contract-validation/oracle-amendment/summary.json).

### Q03: сохранённый FAIL и отдельный пересчёт

Frozen Q03 grader берёт за тело `shipping_rate` весь suffix файла после её объявления. Размещённый после public функции private helper `rate_table` с корректной таблицей регионов попадает в этот suffix и вызывает `shipping_no_region_literals` FAIL. Истинная граница функции и public API не соответствуют такому правилу. Source inspection и отдельная representative проверка подтвердили ошибку grader; его control с helper до public функции её не обнаруживал.

Все три исходных Q03 FAIL receipts, frozen oracle и original summary **15/18** сохранены. Отдельный standalone Rust AST grader проверил точные три candidate commits; corrected structure, неизменённые behavior checks и protected contract **PASS в 3/3**. Combined acceptance — **18/18 = 15 original + 3 supplemental**, stable tasks **6/6**. Новых model calls нет; исходный frozen controller не объявляется задним числом прошедшим.

| Trial | Exact candidate SHA | Supplemental oracle |
|---|---|---|
| 1 | 58fd9e9d76b3470bc36eb44d12065ef53636ffe5 | [PASS receipt](../artifacts/contract-validation/oracle-amendment/trial-1/oracle.json) |
| 2 | 34f04cafd9058ee6087ce489c1eaf3cfd13d5db9 | [PASS receipt](../artifacts/contract-validation/oracle-amendment/trial-2/oracle.json) |
| 3 | 29a84095c87c76eac9d1cd5745c9c60c3593801b | [PASS receipt](../artifacts/contract-validation/oracle-amendment/trial-3/oracle.json) |

AST checker прошёл отдельно **8 Rust + 23 Python tests**; они не входят в core 129. SHA256 corrected checker sources: **`5efd692ca40b8db4cf7507a25ee05b8fc95237e5bfee94a50c1cd54a6dac1228`**. [Amendment manifest](../artifacts/contract-validation/oracle-amendment/summary.json) фиксирует original/corrected controller hashes, exact candidate SHA/tree и сохранённые original receipts. Requirements, task fixtures и behavioral oracle не изменены.

Дополнительные **9 manifest/controller tests PASS** выполняют настоящий CLI в обычном Python и с `-O`: **20 вызовов**, без harness/model dispatch. Изменённые controller/AST/task/fixture/oracle/baseline и старый manifest без hash отклоняются до создания trial repository; существующие receipts не перезаписываются. `--skip-existing` отключён после отдельного RED, показавшего принятие старого PASS при изменённой задаче; текущие свежие 18 trials его не использовали. [Результаты](../artifacts/contract-validation/oracle-fix/manifest-validation-final.json). Эти тесты также не входят в core 129.

## Полный isolated Q02

Отдельная свежая Q02 task отличается только runtime profile **isolated**, при прежних requirements, grants, protected checks, модели и baseline. Run завершился **VERIFIED**, protected command check и четыре reviews PASS; все assessment scopes указывают `runtime_profile:isolated`. Exact candidate **`20443ec9e3a5d6bc6ad200e21574283fb9c19395`**, tree **`46128efd5bc4dfd75803ffd99988b30d1547e28c`** независимо связаны с evidence.

Время **115,177 с**, telemetry **69 961 input + 1 976 output = 71 937 токенов**, **10 complete calls**, unknown/reserved **0**. Original HEAD/files, canonical task и отсутствие publication подтверждены. External oracle принял точный candidate; **сам external oracle исполнялся native-trusted**, не внутри isolation. Это один полный public isolated trial, не весь multiagent/holdout corpus.

[Результат](../artifacts/contract-validation/isolated-pilot/result.json) · [Typed binding audit](../artifacts/contract-validation/isolated-pilot/audit-summary.json).

## Независимая проверка качества тестов

Проверено **шесть representative candidates** — по первому для каждой задачи, суммарно **26 добавленных test functions**. Exact Q01–Q05 прошли **21 test** в настоящем Linux isolated runtime. При шести подстановках исходных ошибочных модулей с сохранением новых tests каждый вариант скомпилировался и упал на assertions; compile error не засчитывался как выявленный дефект. Корректный Q03 baseline сохранил behavioral PASS ожидаемо: задача требовала refactor, а не изменения поведения.

Representative Q06 имеет **пять tests и 37 заданных inputs**; сохранённый external oracle подтверждает **3/3 killed mutants** (middle argument, minimum, maximum), с успешной компиляцией mutant variants. Q06 oracle исполнялся native-trusted. Этот аудит не распространяется автоматически на остальные 12 trial candidates или произвольные будущие задачи.

[Методика и exact commits](../artifacts/contract-validation/test-quality-audit.md) · [Полные receipts](../artifacts/contract-validation/test-quality-audit.json).

## Скорость

| Warm CLI | Samples / warmups | p50, мс | p95, мс | max, мс |
|---|---:|---:|---:|---:|
| --version | 200 / 20 | 2,014 | 2,482 | 3,783 |
| doctor | 30 / 3 | 24,398 | 28,685 | 31,659 |
| eval | 20 / 2 | 6,345 | 14,011 | 16,496 |

Измерен final binary на warm filesystem одновременно с одним live pilot. Doctor включает фактический isolation probe, eval — component assertions; эти значения не обозначают полный coordinator startup. Cold start, full-run/idle RSS, no-I/O dispatch, matched serial/parallel speedup и complete performance gates не измерены. [Raw samples и условия](../artifacts/contract-validation/performance/summary.json).

## Ограничения результата

Поддержанный isolation backend — Linux bubblewrap после реального probe. Workspace остаётся ресурсом команды, не syscall-level ownership каждого файла; trusted system/toolchain mounts read-only. Absent protected path может заморозить existing ancestor. CPU/memory/disk quotas отсутствуют; kernel vulnerabilities и hostile same-UID host races вне текущей границы. Credential/.git masking может нарушить offline работу legacy Cargo Git registry cache; неизвестные секреты в обычном source автоматически не обнаруживаются.

Supervisor требует trusted CLI с именем harness/harness.exe; Rust library host нужен CLI companion в поддерживаемом расположении. Native процессы имеют права пользователя; deliberately detached native descendants могут покинуть process group. Credentials остаются в официальном Codex store. Soft ChatGPT output cap, hidden context/retries и bounded usage drain не дают денежной квоты или final billing receipt. Остаток subscription quota — **UNKNOWN**; complete наблюдаемая telemetry pilot не означает безлимит.

Полный frozen **30-task benchmark**, hidden holdout, B0/B1/H и три повторения по конфигурации, все **42 системных сценария**, reviewer/Jev calibration, full crash/power-loss corpus, Windows/macOS и complete performance gates остаются незакрытыми. Валидность **73 mappings** не заменяет исполнения сценариев; полных сценариев зачтено **0**. Итог относится к функциональному Linux core и указанному pilot, не к универсальной production readiness.
