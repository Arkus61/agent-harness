# Статус реализации — 0.1.1, 2026-10-02

Реализован локальный Rust CLI с native agent loop, DAG, Git worktrees, интеграцией, командными проверками и четырьмя независимыми reviewer-сессиями. Это рабочая исходная версия **`native-trusted`**, с OpenAI-compatible transport, подключением ChatGPT через официальный Codex app-server и отдельно обозначенными scripted fixtures. Свежий live pilot выявил блокирующие ошибки DecisionService; версия **не готова к рабочему использованию `decision_mode: "enforced"`**. Полное завершение всех этапов [архитектуры](ARCHITECTURE_PLAN.md) не заявляется. [Отчёт проверок 2026-10-02](VALIDATION_REPORT_2026-10-02.md).

## Реализовано

| Область | Фактическая реализация |
|---|---|
| CLI | `doctor`, `run`, `resume`, `status`, `inspect`, `report`, `cancel`, `reconcile`, `settle`, `merge`, `demo`, `eval`; административные команды memory/skills/strategy; `auth status` и `auth chatgpt` с login/device/check |
| TaskSpec | Типизированный JSON, неизвестные поля отклоняются; requirement/check/budget limits; explicit DAG либо typed model planner |
| DAG | Проверка покрытия, циклов, зависимостей и консервативного ownership overlap; bounded parallel builders; точные входные и выходные SHA |
| Git | Detached worktrees, committed baseline, интеграция дельт; отключение hooks/filters для служебных Git-команд; отдельная fast-forward публикация с expected ref |
| ToolGateway | Read/write scopes, ownership, command program/argv prefix, read-only reviewers; защита специальных путей и отказ links/junctions в файловых API |
| Existing-source write/edit | BLAKE3 `expected_hash` обязателен, stale hash отклоняется; `edit_file` заменяет ровно один непустой snippet с atomic CAS/recheck; partial-edit UTF-8 source/result ≤64 МиБ |
| Protected acceptance | `protected_paths` неизменяемы: Gateway отклоняет writes; native final diff и интегрированный diff относительно исходного baseline также проверяются; ordinary product tests могут меняться |
| Native processes | Ограниченный stdout/stderr, timeout/cancel; Unix process group либо Windows Job Object, cleanup при Drop future |
| Durability | SQLite WAL + synchronous FULL; state/event/outbox в транзакции; intents/receipts; BLAKE3 CAS с проверкой bytes/hash |
| Recovery | Durable budget/repair limits и generation fencing; сохранённый план/base и reuse завершённых node outputs с проверкой input tree; pending effects и неизвестный usage блокируют resume; reconcile/settle с evidence |
| Budget | Root token spent/reserved accounting, идемпотентные reserve/settle, wall-time deadline от исходного created_at; unknown spending сохраняется |
| Model transport | OpenAI-compatible: local loopback HTTP либо явно разрешённый внешний HTTPS, no redirects/proxy/automatic request retry, bounded body, строгий JSON и usage contract. ChatGPT: официальный Codex app-server stdio, fresh ephemeral thread, `outputSchema`, полный usage либо unknown/HOLD |
| ChatGPT authentication | Codex управляет login и credentials, ручного извлечения tokens нет; требуется ChatGPT auth и `allow_remote=true`, API fallback отключён; `environments:[]` и отключённые host tools сохраняют исполнение действий внутри harness Gateway |
| DecisionService | Typed оценки `model`/`tools`/`risk`, shadow/enforced; модель не отменяет программный запрет; action assessment для предложенного точного действия |
| Context | Scoped deterministic file discovery/selection, byte bounds, источники/hash и omissions; targeted read/search; read snapshot потоково получает full BLAKE3 при bounded returned prefix; explicit-secret redaction |
| Verification | Проверки точного интегрированного кандидата; requirements/code/tests/security reviews; источник и строка proof проверяются, requirements coverage обязательна |
| Repair | Один batch от интегрированного кандидата, generation advance, идемпотентный hook claim; обязательные проверки повторяются полностью |
| Fixture separation | Scripted run получает только `FIXTURE_VERIFIED`; fixture reviews не закрывают production gate; fixture merge запрещён |
| Memory | Project/source scoped SQLite FTS5, candidate/promotion, typed records и консервативные topic conflicts; builder получает до пяти active records для точного input SHA; после production VERIFIED episode сохраняется candidate, independent reviews память не получают |
| Skills | Immutable `id@version`, content hash, dependencies и capabilities; quarantine; pinned packages добавляются к task prompt и повторно проверяются перед действиями |
| StrategySelector | Hard constraints, bounded probe requests, сравнимость measurements, Pareto + weighted choice; отдельная CLI/API, данные не объявляются измерениями без evidence |
| CI | Workflow format/clippy/tests/release build на Ubuntu, macOS, Windows; upload executable artifacts |

Outbox здесь — durable журнал доставки и основа hook idempotency. Полноценного универсального event dispatcher с отдельными worker leases/retries ещё нет. Local knowledge и skills используют отдельные SQLite файлы с собственной авторитетностью; их обновления не изображаются одной транзакцией с core DB.

## Проверки и доказательства

Набор разбит на unit contracts, переносимые integration contracts и CLI end-to-end fixtures. `harness eval` сообщает только действительно исполненные component assertions, а не число исполненных полных сценариев S01–S42. [Полный план оценивания](EVALUATION_PLAN.md), [каталог](../evals/scenarios.json) и [JSON Schema](../evals/scenarios.schema.json) фиксируют критерии и отдельное planned/partial покрытие.

| Сценарии | Что проверяется сейчас | Что это не доказывает |
|---|---|---|
| S01/S13/S15/S16 | CLI demo, integrated candidate, четыре explicit fixture review, fixture gate и запрет публикации; local mock HTTP → engine с typed assessments, command receipt, source proofs и settled usage | Качество реального builder/reviewer или независимую истинность model proof; mock HTTP проверяет механизм, не LLM |
| S02/S03/S10 | Ошибки TaskSpec, scopes/commands/ownership, Unicode/CRLF, stale hash, traversal и links | Confinement произвольного native-кода либо устойчивость к hostile same-user races |
| S04/S05/S09/S11/S12 | Store reopen, транзакционные rollback, durable intent failpoint и reconciliation, budget/generation, missing/corrupt CAS | Все crash points, внешний exactly-once, power-loss certification и весь S04–S12 |
| S07/S08 | Helper process timeout, bounded stdout, cancel descendant heartbeat и Drop cleanup | Остановку намеренно detached процесса или гарантированный Windows pre-exec containment |
| S19 | Duplicate/stale hook claims и repair-batch foundation | Полный dispatcher, бесконечные adversarial event graphs и доставку после любого crash |
| S20/S21/S22 | Fixture publication rejection, explicit DAG validation, две независимые branches перед dependent builder | Live-model multiagent success rate, все Git conflicts и performance speedup |
| S26–S36/S42 | Component tests контекста/provider/memory/skills/selector; hashes, statuses, limits, source scopes и сравнимость | Все полные сценарии, истинность arbitrary evidence, live токен-экономию и корректность полноценных кешей/replay |
| S38 | `isolated` fail-closed до инструментов/model calls | Поддержку strict runtime S37 |

Свежий прогон **2026-10-02** для исходников **0.1.1** на **native Linux, Rust 1.99.0** прошёл: **64 library + 2 Codex CLI + 12 Codex protocol + 6 contracts + 7 end-to-end = 91 тест**. Новые Codex fixtures проверяют typed contract, tool rejection, delayed/invalid usage, протокольные границы и фактический SIGINT CLI с cleanup helper-процессов. Это проверка механизма transport, а не live качества модели.

| Проверка | Результат |
|---|---|
| `cargo test --locked --all-targets` | PASS для 0.1.1 — 91 тест |
| `cargo fmt --all -- --check` | PASS для финальных исходников 0.1.1 |
| `cargo clippy --locked --all-targets -- -D warnings` | PASS для финальных исходников 0.1.1 |
| `cargo build --locked --release` | PASS — executable `harness 0.1.1` |

Toolchain закреплён в `rust-toolchain.toml`. Release **0.1.1** выполнил свежие `eval`: **20/20** component assertions, и `demo` → **`FIXTURE_VERIFIED`** с одним настоящим Cargo check и четырьмя scripted reviews. Дополнительно прошли 22 повторные выбранные тестовые проверки и восемь CLI проверок отказов, включая пять SIGINT cleanup. Принудительный SIGKILL координатора выявил ограничение: native child может остаться жив, хотя неизвестный intent сохраняется и resume получает HOLD. [Полный отчёт](VALIDATION_REPORT_2026-10-02.md).

Для ChatGPT подключения на **Codex CLI 0.159.0-alpha.3** официальный device login завершён, и **2026-10-02** release `auth chatgpt --check` прошёл. `auth status` сообщает `authenticated:true`, тип `chatgpt`, план `plus` и default model `gpt-6.1-sol`. Настоящий запрос модели `gpt-6.1-sol` вернул ожидаемый структурированный ответ без actions и полный usage: **5000 input + 45 output tokens**. Успешный короткий live model turn подтверждён; последующий live pilot разработки описан ниже. Полный benchmark 30 задач не выполнен. [Сохранённый результат проверки](chatgpt-connection-check.json). [Настройка и ограничения подписки](CHATGPT_SUBSCRIPTION.md).

Для **Linux x86_64 `--version` выпуска 0.1.1** в локальном контейнере с прогретой файловой системой выполнены 20 warmups и 200 новых процессов: p50 **2,119 мс**, p95 **2,417 мс**, max **2,693 мс**. Размер executable — **11 604 136 байт**. [Методика baseline](BASELINE.md) и [raw samples 0.1.1](startup-baseline.json) фиксируют условия измерения; [измерения 0.1.0 сохранены отдельно](startup-baseline-0.1.0.json). Этот короткий путь CLI не запускает весь координатор; полный startup, RSS, dispatch и весь S40 не измерены и не закрыты этим результатом.

Свежий [pilot на шести публичных Rust-задачах](../evals/live/README.md) с моделью **`gpt-6.1-sol`** и тремя свежими повторениями каждой завершился: **18/18 `BLOCKED`, 0 кандидатов** в `enforced`. Из них 14 прямых DecisionService HOLD при assessment с `action:null`, один timeout и три sibling cancellations; последние маскируют первичную причину и оставляют unknown usage. Наблюдаемый расход — **202 147 токенов**; отдельно удержано **82 785 оценочных reserved tokens** по четырём вызовам с неизвестным расходом. Эти резервации не являются фактическим расходом или billing receipt.

Отдельный **Q02 `shadow` diagnostic** завершил полный цикл: **`VERIFIED`**, exact candidate **`e38bc3995769a023cf075ac3ba31c8c049c8ce2f`**, четыре reviews и command checks; независимый oracle и привязка evidence к кандидату — **PASS**, также сохранены red/green доказательства regression tests. Время **87,241 с**, наблюдаемый расход **60 195 токенов**, девять полных usage receipts, reserved **0**. Этот запуск исключён из `enforced` результатов. Risk evaluator в нём ошибочно воспринимал собственную read-only среду как запрет разрешённого редактирования через внешний Gateway. Требуется исправить typed subjects конфигурационных assessments и явно задать полномочия evaluator/Gateway. [Отчёт и доказательства](VALIDATION_REPORT_2026-10-02.md).

Полный benchmark из **30 задач** Q01–Q10, hidden holdout, три повторения по каждой benchmark-конфигурации, B0/B1/H comparison и reviewers/Jev calibration **не исполнялись**. Исполнение на всех трёх ОС в текущей локальной сессии не подтверждено. Настройка CI отличается от фактически завершённых matrix runs.

## Ограничения, важные для пользователя

1. **`native-trusted` не является sandbox.** Granted commands, build scripts и тестируемый код получают права пользователя на файлы и сеть. Файловый Gateway не ограничивает произвольный subprocess. IPC/SQLite принадлежность текущему пользователю не защищают от враждебного процесса того же пользователя.
2. **`isolated` не реализован.** Doctor честно сообщает отсутствие confinement; требуемый профиль блокируется, автоматического downgrade нет. Отдельных OS/kernel probes strict isolation ещё нет.
3. **Windows Job Object назначается после spawn.** Есть окно гонки до назначения. Unix process groups тоже не сдерживают намеренное отделение процессов. Cleanup подходит для управляемых native-процессов, не для недоверенного adversarial code.
4. **Resume восстанавливает node checkpoints, но не диалог модели.** Используются сохранённый plan/base и завершённые outputs с совместимым input tree. Незавершённые nodes строятся заново. Pending effects/usage требуют ручного подтверждения. Выполненная команда незавершённого builder запрещает автоматический replay; для такого checkpoint нужна инспекция и явно новая задача.
5. **Учитываются токены и deadline, но не деньги.** Provider tariffs, currency, monetary budget/reservations и billing receipts пока не реализованы. Token reservation — byte-based оценка, не tokenizer-specific quota guarantee; фактический overspend фиксируется и блокирует продолжение. У ChatGPT/Codex нет жёсткого output cap: дополнительная оценка 16 КиБ не ограничивает hidden context или внутренние retries. `max_output_tokens` здесь мягкая квота. После завершения fresh thread выполняются `account/read` fence и bounded drain 200 мс; учитывается последний наблюдаемый cumulative total, без гарантии финального billing receipt. Missing/invalid/partial usage оставляет HOLD. Лимиты Codex для ChatGPT и отдельный API billing не взаимозаменяемы.
6. **Нет live token UI.** OpenAI-compatible HTTP использует `stream: false`; ChatGPT backend читает события app-server, но возвращает один финальный typed response. Command output bounded; полноценный пользовательский streaming UI ещё не предоставлен.
7. **DecisionService блокирует рабочий `enforced` pilot и пока не является полноценным Jev backend.** Конфигурационные `model`/`tools` assessments передают `action:null` при требовании оценить точное действие; evaluator воздерживается. Risk assessment может смешивать права evaluator и внешнего Gateway. Он использует configured model с typed JSON. Нет отдельного Choice/Score/Noul adapter, калибровки, нескольких model providers/route switching или доказанного выигрыша neural routing.
8. **Evidence references не проверяют смысл.** Path/line/coverage guards предотвращают пустые и неверные ссылки, но не доказывают правильность объяснения reviewer. Общая точность одной модели и коррелированные ошибки четырёх ролей требуют live evaluation.
9. **Protected command definition и paths не создают произвольный trusted oracle.** Checks и `protected_paths` неизменяемы в TaskSpec; final source diffs проверяются, ordinary repository tests остаются продуктовым кодом. Это результатные guards, не confinement исполнения и не защита от временного изменения и восстановления native-кодом. Полноценный hidden acceptance controller и отдельный evaluator security boundary ещё не предоставлены.
10. **Context Shunt реализован частично.** Есть deterministic/targeted bounded context; нет отдельного cheap extraction tier, AST/symbol index, entailment validator, context compaction pipeline и автоматически доказанной экономии токенов.
11. **Memory интегрирована консервативно, SESE пока выбирается отдельно.** Builder получает active records лишь для точного project/input SHA; independent reviewers их не получают. Успешный production run создаёт candidate episode, автоматической активации claim нет. Autonomous conflict resolution и strategy/probe execution loop ещё не включены. Skills интегрированы как pinned инструкции; install или declaration evidence сами по себе не доказывают безопасность.
12. **Secret filtering имеет явные границы.** Явные task secrets и API key redaction поддерживаются в transport/export; нет надёжного определения неизвестных секретов, L0/local data classification или encrypted state store. Административные memory/skill inputs сохраняются как пользовательские данные; не записывайте туда секреты. `allow_remote` — осознанное разрешение отправки контекста выбранному endpoint.
13. **Нет полноценных cache/replay/GC.** Service worktrees и receipts сохраняются для инспекции; есть reuse завершённых node outputs, но нет автоматической уборки, продолжения незавершённой model session и replay engine.
14. **Кроссплатформенность ещё требует выполнения CI.** Есть conditional Unix/Windows code и три-OS workflow. Это не заменяет реальные результаты на Windows/macOS и измерения скорости на закреплённом hardware. Готовый Linux x86_64 executable требует glibc ≥2.39 и системные libc/libm/libgcc_s; для старой glibc нужна пересборка в целевой среде.
15. **Подключение ChatGPT зависит от версии Codex.** Используются app-server и экспериментальное поле `environments:[]`; проверенная при разработке версия — 0.159.0-alpha.3. Неожиданные tool items/server requests блокируются, arbitrary CLI compatibility не заявляется. Credentials остаются в официальном CLI store; concrete paid plan и успешная генерация определяются отдельной проверкой, а не одним login status. Ошибки показывают публичный класс, например `unauthorized`, без raw body, credentials и private provider details.

## Соответствие этапам архитектуры

| Этап | Состояние |
|---|---|
| Контракты и native foundation | Native Linux tests/build/release smoke подтверждены; измерен warm-FS `--version`; три-OS CI задан, Windows/macOS и остальные performance baselines требуют выполнения |
| Один устойчивый агент | Есть transport/loop/recovery guards; свежий `enforced` pilot 0/18 с блокировкой до кандидата; отдельный Q02 `shadow` diagnostic прошёл полный цикл с независимым oracle |
| Независимая приёмка | Четыре роли/exact-candidate checks; Q02 `shadow` подтверждён внешним oracle и regression evidence; hidden acceptance и общая точность ролей остаются открыты |
| Многоагентный MVP | DAG/worktrees проверены механически; Q04/Q05 включены в live pilot, но `enforced` блокировался до кандидата; live multiagent success пока не подтверждён |
| Context/memory/neural routing | Базовый context, assessment, curated builder memory и postrun candidate episode в loop; полноценный shunt/route optimization и memory verification pipeline ещё не завершены |
| Skills и SESE | Skill registry + runtime integration; selector CLI/API; автоматический experimental search/probe orchestration ещё не реализован |
| Release validation | Свежие local/failure checks PASS; readiness `enforced` FAIL; 42 полных сценария, benchmark 30 задач/holdout и все OS/performance gates ещё не закрыты |

Следующий цикл разработки: исправить конфигурационные subjects и разграничение полномочий DecisionService, сохранить первичную причину при sibling cancellation и повторить свежий pilot. Далее — исполнить три-OS CI, измерить full startup/RSS/dispatch, добавить constrained isolated runtime и evaluator boundary; подготовить полный benchmark, model calibration и измерение context/strategy gains. Описание вызовов и примеры находятся в [README](../README.md) и [документации knowledge/skills/strategy](knowledge-skills-strategy.md).
