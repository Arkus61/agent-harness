# Проверка Harness 0.1.1 — 2026-10-02

**Основной режим `enforced` не прошёл live-приёмку: 0/18 успешных запусков.**
Локальные тесты и проверки отказов прошли. Отдельная диагностика `shadow`
выполнила полный цикл исправления ошибки с независимой приёмкой; она не
заменяет результат основного режима.

Проверен неизменный executable **0.1.1**, Linux x86_64, Rust 1.99.0,
Codex CLI 0.159.0-alpha.3, ChatGPT Plus, модель `gpt-6.1-sol`.
Исходный commit реализации: `1268c328c4c713eef706d30ff15f14349567aac5`.
SHA256 executable:
`b5ec508f863299b48cf3373adc09e2af0d92ccf7eb1f0c93cafe767fe51269fa`.
Код харнесса в ходе испытаний не изменялся; добавлены тестовые fixtures,
контроллеры и отчёты.

## Выполненные проверки

| Проверка | Результат | Свидетельство |
|---|---|---|
| Unit и integration tests | **91/91 PASS**, 0 failures | [Полный лог](../artifacts/full-validation/local/cargo-test.stdout.log) |
| fmt, clippy `-D warnings`, release build | **PASS** | [Команды, exits и время](../artifacts/full-validation/local/summary.json) |
| doctor, status, report | **PASS** | Тот же local summary |
| `harness eval` | **20/20 PASS**, component assertions | [JSON](../artifacts/full-validation/local/eval.stdout.log) |
| Scripted demo | **FIXTURE_VERIFIED** | [Кандидат и четыре fixture reviews](../artifacts/full-validation/local/mechanical-demo-report.json) |
| Повторная выборка проверок отказов | **22/22 PASS**, входит в исходные 91 тест | [Выборка и логи](../artifacts/full-validation/failures/selected-tests.json) |
| Дополнительные CLI-проверки отказов | **8/8 PASS** | [Отчёт](../artifacts/full-validation/failures/cli-failure-checks.json) |
| Каталог и JSON Schema | **PASS**: 42 описания, 131 уникальный критерий, 66 существующих mappings | Local summary; это проверка структуры каталога |
| Controls шести live-задач | **6 baseline FAIL / 6 valid controls PASS** | [Замороженный manifest](../evals/live/manifest.json) |
| Шесть live-задач × три свежих запуска, `enforced` | **0/18 PASS**, все BLOCKED, кандидатов нет | [Результаты](../artifacts/full-validation/live/summary.json) |
| Отдельный live-пилот Q02, `shadow` | **PASS диагностического цикла**, external oracle PASS | [Точный commit и audit](../artifacts/full-validation/diagnostic-shadow/candidate-binding-audit.json) |

Тестовые группы: 64 library, 2 Codex CLI, 12 Codex protocol, 6 contracts,
7 end-to-end. Выборка 22 тестов повторяет часть полного набора; её нельзя
прибавлять к 91 как новые уникальные тесты. Cached release build занял
0,125 с; это не измерение чистой сборки.

## Что проверили при отказах

Подтверждены запреты команд и чужих writes, readonly reviewer, protected paths,
stale hash, строгий JSON, обработка отсутствующего/задержанного usage, root
budget, timeout и ограниченный вывод, отмена процессов, отказ fixture merge,
durable intent после настоящего SIGKILL и отказ автоматического replay.

Дополнительный CLI trap подтвердил отсутствие запрещённого эффекта даже
при permissive model assessment. Неизвестный расход сохраняет reservation;
`resume` не принимает его за ноль. Отмена SIGINT очистила helper и private
directory в 5/5 измерений. Наблюдаемое SIGINT → выход CLI — 0,688–0,933 мс
на fixture в этом контейнере; это не подтверждение общего CI performance gate.

**Ограничение native runtime:** после SIGKILL координатора дочерняя команда
продолжила работать. Испытательный controller явно остановил её и проверил
прекращение heartbeat. Durable intent и HOLD предотвратили повтор эффекта,
но не обеспечили автоматическую остановку оставшегося процесса.

[Подробности отрицательных проверок](../artifacts/full-validation/failures/SUMMARY.md).

## Основная live-приёмка

Шесть маленьких Rust-проектов без внешних зависимостей проверяют разные классы:
Q01 — новая функциональность; Q02 — граничная ошибка; Q03 — рефакторинг;
Q04 — параллельные компоненты; Q05 — изменение с зависимостью узлов;
Q06 — содержательность тестов через mutation testing.

Каждый trial клонировал исходный committed baseline. Модель, требования,
oracle и бюджет закреплены; прежние ответы и кандидаты не использовались.
Grants и protected Cargo checks сохранились. Controls и external oracle
находятся вне репозитория агента; native execution имеет права пользователя
и не создаёт защищённой среды для враждебного кода.

| Задача | Успешных trials | Stable success |
|---|---|---|
| Q01 | 0/3 | FAIL |
| Q02 | 0/3 | FAIL |
| Q03 | 0/3 | FAIL |
| Q04 | 0/3 | FAIL |
| Q05 | 0/3 | FAIL |
| Q06 | 0/3 | FAIL |

Причины: **14 прямых DecisionService HOLD**, один model timeout и три
отмены вызова соседнего параллельного узла с неопределённым usage. Ни один
основной trial не дошёл до candidate, checks и reviews. Поэтому эти результаты
не измеряют способность модели исправить шесть задач или обнаруживать дефекты.

Получена телеметрия **202 147 токенов** по 37 завершённым model calls.
Ещё четыре calls имеют неизвестный расход; сохранены **82 785 reserved tokens**.
Reservation — оценка, а не подтверждённый расход. Итоговый billing не проверен,
неизвестные расходы вручную не обнулены.

## Отдельная диагностика shadow

Для Q02 создана отдельная копия TaskSpec с `decision_mode: shadow`.
Детерминированные grants, ownership, protected paths, бюджет и checks сохранились.
Нейронные HOLD в этом режиме наблюдались без блокирования действия. Этот
запуск исключён из 18 основных trials.

За **87,241 с** агент исправил clamp и добавил четыре regression tests.
Итоговый commit: `e38bc3995769a023cf075ac3ba31c8c049c8ce2f`.
Tree: `69e7e9819d35472bc2c860a7c87948ef21d5b41c`.

Protected `cargo test --locked --offline`, четыре reviews и два внешних
acceptance tests прошли на этом commit. Типизированный независимый verifier
пересчитал BLAKE3 candidate manifest и проверил связь проверок, reviews,
source proofs, generation и исходного TaskSpec с точным commit/tree.
Исходный HEAD сохранён, tracked файлы чистые, публикация отсутствует.

Source inspection подтвердил regression cases для relative positions,
границ, равных границ, отрицательных диапазонов и extreme i32. При временной
подстановке исходной ошибочной реализации с сохранением добавленных тестов
код скомпилировался и три теста упали на assertions; тест равных границ остался
PASS ожидаемо. На правильном кандидате все четыре теста PASS.

Все девять model calls имеют complete наблюдаемую телеметрию:
58 299 input + 1 896 output = **60 195 токенов**; reservations и unknown calls
равны нулю. Это не billing receipt.

[External oracle](../artifacts/full-validation/diagnostic-shadow/oracle.json) ·
[Binding audit](../artifacts/full-validation/diagnostic-shadow/candidate-binding-audit.json) ·
[Регрессионные тесты и negative control](../artifacts/full-validation/diagnostic-shadow/regression-test-coverage.json).

## Блокирующие находки и исправления

1. **V01 — неоднозначный контракт DecisionService.** Для оценки model/tools
   конфигурации передаётся `action: null`, а инструкция требует оценить exact
   action и воздержаться при отсутствии данных. Модель возвращает HOLD.
   Нужны отдельные типизированные subjects для выбора модели, конфигурации
   инструментов и риска конкретного действия. Их полноту надо проверять до
   model call; assessment должен ссылаться на тот же subject.
2. **V02 — неверная область действия read-only ограничения.** В shadow risk
   assessment модель отклонила разрешённый edit из-за read-only среды самого
   оценщика Codex. Требуется явно задать исполнителя — Harness ToolGateway —
   и оценивать предложенную операцию относительно его effective grants.
   Оценщик получает данные операции и не выполняет её. Это отдельная причина
   HOLD, которая останется после исправления V01.
3. **V03 — дочерний процесс после SIGKILL.** Для гарантии автоматического cleanup
   нужен supervisor/parent-death механизм каждой ОС или supervised isolated
   runtime с отдельными испытаниями.
4. **V04 — потеря первичной причины при parallel cancellation.** На Q04 итоговая
   ошибка отменённого model call скрывает initiating HOLD соседнего узла.
   Следует сохранять первичный отказ отдельно от secondary cancellations,
   unknown usage и cleanup результатов.

[Независимый обзор](../artifacts/full-validation/oracle-review.json).

## Скорость и память

| Наблюдение | Samples | p50 | p95 | max |
|---|---:|---:|---:|---:|
| Warm `--version` | 200 после 20 warmups | 1,975 мс | 2,268 мс | 2,520 мс |
| `doctor` | 30 после 3 warmups | 12,605 мс | 13,188 мс | 13,537 мс |
| Component `eval` | 20 после 2 warmups | 5,637 мс | 6,262 мс | 6,292 мс |

`strace --version` наблюдал 0 network syscalls и 0 дочерних процессов в одном
контрольном запуске. Doctor выполняет Git discovery, но не весь coordinator.

Во время Q02 trial-2 получено 100 RSS samples через 200 мс: coordinator
p95/max **9,531 MiB**, дерево `harness + codex + bwrap` p95 **77,961 MiB**,
sampled max **78,488 MiB**. Trial остановился на early HOLD; builder/checks/reviews
не наблюдались. Сумма RSS учитывает общие страницы несколько раз и может
пропустить краткий пик. Это не idle RSS и не peak полного успешного цикла.

Контейнерная нагрузка и power profile не закреплены. Cold start, no-I/O
dispatch и matched successful serial/parallel comparison не измерены;
ускорение относительно прежнего baseline не заявляется.

[Raw samples и методика](../artifacts/full-validation/performance/performance-results.json).

## Незакрытые проверки

Полный runner S01–S42 отсутствует: fresh tests подтверждают facets 34 сценариев,
восемь требуют реализации дополнительных fixtures; **полных сценариев зачтено 0**.
Системный аудит перечисляет ограничения каждой строки. Шесть public toy tasks
не заменяют 30 frozen dev/holdout задач и сравнение B0/B1/H.

Windows/macOS не исполнялись: в workspace есть только Linux executor.
Профиль `isolated` не реализован и корректно отказывает. Не выполнены полный
crash/power-loss corpus, защищённая hidden приёмка, monetary billing,
полные performance gates и calibration reviewer corpus.

[Матрица всех 42 сценариев и 30 descriptors](../artifacts/full-validation/scenario-audit.json) ·
[Общий JSON-результат](validation-summary-2026-10-02.json).

## Воспроизведение

Локальные механические проверки описаны в [README](../README.md).
Для шести свежих live-задач с новым output directory:

```sh
python3 evals/live/prepare.py --output artifacts/recheck-seeds --manifest artifacts/recheck-manifest.json
python3 evals/live/run.py --manifest artifacts/recheck-manifest.json --output artifacts/recheck-live --repeats 3 --workers 3
```

Нужны Rust/Cargo, Git, Python 3.12+ и официальный Codex с ChatGPT login;
`CARGO_HOME`/`RUSTUP_HOME` должны указывать на существующую установку.
Контроллеры и helpers относятся к испытательному набору; Python не является
зависимостью native runtime харнесса.

[Fixtures, controls, contracts и ограничения oracle](../evals/live/README.md).
