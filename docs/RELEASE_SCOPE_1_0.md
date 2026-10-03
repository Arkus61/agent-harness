# Состав полнофункционального релиза Agent Harness 1.0

Дата: **2026-10-03**. Статус: **целевые требования, приёмка не выполнена**.

Этот документ определяет конечный состав 1.0. [План разработки и приёмки](superpowers/plans/2026-10-03-full-release.md) задаёт порядок работ; [машиночитаемая матрица](../evals/release/acceptance-matrix.json) хранит критерии. Исторические отчёты 0.1.2/0.1.3 сохраняют прежние границы доказательств.

## Что означает «полнофункциональный»

В 1.0 пользователь может установить готовый CLI, подключить модель и MCP-инструменты, поставить задачу, выбрать ограниченную стратегию, выполнить DAG, получить независимо проверенный кандидат, восстановиться после сбоя и явно опубликовать результат. Контекст, память, граф навыков и эксперименты встроены в этот путь и проверены по отдельным критериям. Нет конкурирующих координаторов внутри подсистем.

Релиз является полнофункциональным в указанном составе. Исследовательские алгоритмы и интерфейсы из раздела «После 1.0» имеют отдельный срок и не используются для обозначения готовности основного контура.

## Обязательные возможности

| Область | Обязательство 1.0 |
| :--- | :--- |
| Поставка | Rust CLI и библиотечное ядро; native-trusted на Linux/macOS/Windows; пять бинарных целей: Linux x64/ARM64, macOS x64/ARM64, Windows x64 |
| Модель | ChatGPT через официальный Codex CLI и OpenAI-compatible backend; совместимые версии и JSON/usage контракт закреплены; ошибки и неизвестный расход сохраняются |
| Агентский цикл | Планирование, проверка DAG/ownership, параллельные builders, интеграция, адресный repair от кандидата и четыре обязательных ревью |
| Доказательства | Полный набор checks и reviews, привязанный к задаче, плану, кандидату, grants, политике, среде и версиям инструментов; отдельный VerificationCertificate |
| Долговечность | Intent/receipt для эффектов, fencing, cancel/resume, budget ledger, outbox; публикация Git проходит тот же протокол неоднозначных эффектов |
| MCP | Клиент stdio и Streamable HTTP, tools/resources/prompts через общий Gateway; замороженные разрешения, bounded output, безопасное хранение credentials и OAuth/PKCE для поддерживаемых HTTP-серверов |
| Изоляция | Linux bubblewrap; управляемый guest backend для isolated на macOS/Windows; реальные probes и тесты границ; недоступная требуемая возможность блокирует задачу |
| Ресурсы | Tokens/deadline/output/concurrency и provider-capable monetary ledger с immutable tariff/currency/billing receipts; per-process capabilities; aggregate RAM/PIDs/disk в isolated backend; точная семантика CPU-rate и накопленного CPU-бюджета с опубликованным пределом overshoot |
| Контекст | Bounded Context Shunt, точные source refs, revalidation/redaction, структурированная compaction с сохранением обязательных условий и возможностью открыть источник |
| Память | Provenance/dependency applicability, lifecycle/conflicts/supersession, изоляция проектов и reviewer-контекста; durable асинхронная запись эпизодов и отдельное promotion |
| Навыки | Неизменяемые версии, полный минимальный lifecycle, ограниченный граф отношений и компиляция выбранного пути в общий DAG; карантин проверяется при запуске и resume |
| Стратегии | SIMPLE/STANDARD: до трёх существенно разных вариантов и двух probes; реальные эксперименты, общий бюджет, Pareto/constraints, runner-up и общий DagCompiler |
| DecisionService | Заменяемый backend, allow-listed routing/tool/skill/risk decisions, abstention и calibration; программная политика всегда обязательна |
| Обслуживание | Согласованные backup/restore, миграции, retention/GC с pins, read-only replay полного заявленного набора проекций, отчёт и экспорт доказательств |
| Приёмка | Полные применимые S01–S42, MCP/maintenance/provider дополнения, свежий benchmark и reviewer/decision calibration; packaged native smoke на пяти целях |

## Границы платформ и изоляции

`native-trusted` выполняет команды с правами пользователя. Изоляция инструментов модели в Gateway не превращает проектную команду или stdio MCP-сервер в sandbox.

`isolated` требует подтверждённого backend. Для macOS/Windows используется отдельная управляемая VM; настройка необязательна для native-пользователя, но обязательна при выборе isolated. Docker и WSL не являются обязательными зависимостями ядра. Доступ к host home, credentials, общему Git/state directory, docker socket и широким shared folders гостю не выдаётся.

Manifest всегда различает **host OS**, **guest OS**, архитектуру и toolchain. Linux-гость на Windows/macOS доказывает работу соответствующего профиля на этом хосте; он не доказывает совместимость Windows/macOS-зависимого кода с Linux. Задача с требованием нативной ОС получает совместимый backend либо `BLOCKED`; незаметной подмены платформы нет. Приёмка native-функций проводится на настоящей целевой ОС отдельно.

Aggregate resource caps относятся к **одной root task**, включая одновременные builders/checks/probes/stdio MCP workers и все их worktrees. Repair/resume не создаёт новую квоту. Gateway writes и snapshot transfer используют тот же ограниченный workspace volume либо отдельный явно учтённый staging cap; coordinator/state/remote services исключены явно и ограничиваются своими budgets. Aggregate resource caps применяются в backend, который реально умеет их ограничивать. На native-хосте отсутствие конкретного hard cap явно отражается в capabilities; обязательный неподдерживаемый предел отклоняется до вызовов модели. `cpu.max` означает скорость CPU, `cpu.stat` — накопленное потребление; watchdog накопленного бюджета имеет явный предел задержки и не обозначается абсолютным hard cap без overshoot. `RLIMIT_FSIZE` не заменяет суммарную дисковую квоту. Для sampled CPU budget замораживаются polling **100 ms**, тестовая нагрузка **2 CPU cores**, измеренный overshoot **≤1 CPU-second** на ≥100 stress runs; это corpus gate, а не абсолютная гарантия scheduler. Требуемый hard `aggregate_cpu_seconds` остаётся unavailable при одном watchdog; soft sampled budget объявляется отдельным полем.

## Совместимость и скорость

- Ядро: **Rust 1.99.0**, Tokio, локальные SQLite/FTS5 и Git; Python допустим в оценщике, но не требуется установленному CLI.
- Предлагаемый ABI выпуска: **Linux glibc ≥ 2.35**, **macOS ≥ 14**, **Windows 11 x64**. Это целевые floors; текущий Linux artifact требует glibc ≥ 2.39. ADR и испытания на минимальных версиях обязательны до RC; выбранные значения нельзя менять после испытания без новой версии scope.
- Native performance gates: прогретый `--version` **p95 ≤ 100 ms**, idle coordinator **RSS ≤ 40 MiB**, чистый dispatch без I/O **p95 ≤ 5 ms**, остановка управляемого helper **≤ 2 s** на закреплённых стендах.
- Время VM, MCP handshake, дисковых операций, модели, компиляции и тестов измеряется отдельно и входит в end-to-end отчёт. Разогретое ядро не выдаётся за полное время решения задачи.
- Результаты публикуются с machine manifest, release binary hash, cold/warm definition, raw samples и denominator. Быстродействие на одном хосте не сертифицирует остальные.

## Нормативные B0/B1/H

Определения ниже разрешают разночтение между историческим evaluation plan и stub в benchmark controller. Изменение конфигураций оформляется новой версией протокола до запусков.

| Режим | Состав |
| :--- | :--- |
| B0 | Один builder, базовый контекст, общие программные guards и protected checks. Без нейронного DecisionService, внутреннего review gate, памяти, графа, SESE и multiagent. Выдаёт только candidate; успех определяет внешний oracle; production VERIFIED не выдаётся |
| B1 | Один builder, базовый контекст, обязательный DecisionService, protected checks и четыре независимых ревью. Без multiagent, advanced compaction, памяти, графа и SESE. Использует production gate |
| H | Полный **применимый** контур 1.0: DAG, Context Shunt, память, граф, SESE и DecisionService; protected checks и четыре ревью. Неактивность неприменимой функции объясняется заранее заданным правилом |

Effective grants, внешние oracles, доступные модели и общий vector budget одинаковы. Стоимость всех решений, summaries, probes, reviews, retries и failed trials учитывается. Для Codex max_output_tokens остаётся мягкой квотой; скрытые upstream retries/контекст нельзя считать жёстко ограниченными. Неизвестный расход не равен нулю. Денежные значения отделяют подтверждённый provider usage, версию тарифа и расчётную стоимость; подписка ChatGPT не превращается в измеренные API-доллары.

Для Q08–Q10 общий функциональный oracle и denominator одинаковы у B0/B1/H. H-only context/memory/graph/probe proof проверяется отдельно и обязателен для приёмки возможностей H; отключённые функции не делают behavioral baseline автоматически FAIL.

Основной release benchmark: **20 dev + 10 новых закрытых задач × 3 режима × 3 fresh repeats = 270 trials**. Публичные H1 остаются development stress-корпусом; для закрытой приёмки создаются новые независимые задачи. Stable success **3/3** считается только по трём заранее назначенным primary trial IDs; ERROR — неуспех, diagnostic retry не заменяет исходный trial. Скрытые результаты не передаются агенту для repair внутри trial. Исходники тестов, исполняемых в процессе кандидата, не объявляются секретными; скрытая приёмка использует внешний black-box controller там, где это необходимо.

## Что блокирует выпуск

В любом испытании: наблюдённый запрещённый эффект, false/unjustified VERIFIED, потеря durable committed state, необоснованный повтор эффекта, утечка тестовых credential canaries или silent isolation/platform downgrade. UNKNOWN/ERROR/пропущенное обязательное доказательство не дают PASS. Отсутствие нарушений в ограниченном корпусе не доказывает отсутствие риска во всех будущих запусках.

Функциональные пороги сохраняют ранее предложенные значения: **6 разных dev-задач успешны 3/3** для рабочего агента; **≥16/20 dev** успешны 3/3 для полного H; **≥8/10 новых hidden** успешны 3/3 и **H ≥ B1 по stable successes** на тех же hidden задачах для release.

## После 1.0

GUI/TUI, MCP-server интерфейс самого харнесса, IDE/API adapters, legacy MCP HTTP+SSE; embeddings/ANN и AST/LSP-поиск при доказанном выигрыше; DEEP/MCTS и learned search; model-response cache; Windows ARM64; централизованное hosted-приложение. Сторонний Jev SDK необязателен: обязательна функция откалиброванного Decision backend. Универсальная exactly-once гарантия для любых внешних сервисов и защита от злонамеренного администратора хоста не заявляются.
