# Работа с задачами и результатами

Практический справочник CLI: инструменты, проверка кандидата, восстановление запуска и публикация результата. Для установки и первого запуска откройте [README](../README.md).

## Инструменты и приёмка

Model response использует типизированный контракт. Пример последовательности для существующего файла:

```json
{"actions":[{"type":"read_file","path":"src/lib.rs"}],"done":false}
```

Для небольшого файла следующий ответ содержит **полный** новый UTF-8 файл и `expected_hash` из квитанции исходного snapshot:

```json
{"actions":[{"type":"write_file","path":"src/lib.rs","content":"FULL_NEW_FILE_CONTENT","expected_hash":"HASH_FROM_COMPLETE_READ_RECEIPT"}],"done":true}
```

Чтобы исправить небольшой участок без передачи всего нового файла, используйте `edit_file`:

```json
{"actions":[{"type":"edit_file","path":"src/lib.rs","old":"value.min(low).max(high)","new":"value.max(low).min(high)","expected_hash":"HASH_FROM_SOURCE_SNAPSHOT_RECEIPT"}],"done":true}
```

`old` должен быть непустым и встречаться **ровно один раз**, включая пересекающиеся совпадения. Нет совпадения, несколько совпадений или stale hash — отказ без изменения файла. Edit применяется атомарно с повторной проверкой source hash; действует тот же write scope и ownership.

Запись существующего файла без hash и с устаревшим hash отклоняется. `read_file` возвращает ограниченный prefix с явным маркером усечения, но full-source BLAKE3 вычисляется потоковым чтением одного snapshot. Hash целостности не означает, что модель видела пропущенный код; перед edit нужно получить точный изменяемый snippet. Исходный UTF-8 файл и результат partial edit ограничены 64 МиБ. Поиск — ограниченный по scope и объёму; отсутствие совпадений не доказывает отсутствие вне рассмотренной области.

Gateway проверяет read/write grants, ownership и запрет writes/commands для reviewers. `run_command` разрешается только указанным `program` и `args_prefix`; shell не подставляется автоматически. Пустой `commands` в примере запрещает builder запускать команды, но координатор исполняет пользовательские protected `checks`. Сам executable и `checks` входят в доверенную конфигурацию пользователя: объявление `cargo test` не делает тестируемый код безопасным.

`protected_paths` задаёт неизменяемые acceptance-источники, например `acceptance/**`; по умолчанию список пуст. Такие paths нельзя изменить через файловый Gateway. Изменения native-команды дополнительно проверяются перед завершением builder и на объединённом кандидате относительно исходного baseline. Продуктовые тесты в разрешённом `src/**` можно исправлять и дополнять; защищённые acceptance-тесты остаются прежними. Это проверка целостности результата, а не ограничение прав native-процесса: недоверенный код мог выполнить внешний эффект до отказа. Для adversarial hidden evaluation нужен отдельный доверенный oracle-controller и ограниченная среда исполнения.

DecisionService получает оценки `model`, `tools` и `risk` через configured provider. Каждый запрос содержит отдельный typed subject, UUID, effective scope с runtime profile и BLAKE3 binding; ответ обязан вернуть тот же hash. Model choice статичен, assessment не включает дополнительные tools. В `enforced` отказ или воздержание блокируют действие; в `shadow` корректная оценка наблюдается, но не расширяет grants. Некорректный assessment не становится разрешением. Это typed neural assessment, а не отдельный Jev SDK или автоматическая маршрутизация между моделями. [Точный контракт](DECISION_CONTRACT.md).

После объединения всех дельт проверки выполняются на точном кандидате. Command check требует успешный exit, отсутствие timeout/cancellation и неизменённый candidate. Четыре reviewer-сессии получают исходные требования, candidate, diff и command receipts; private reasoning и самооценка builder не передаются.

Production `PASS` требует `proofs` вида `{ "requirement": 0, "path": "src/lib.rs", "line": 1, "explanation": "..." }`. Харнесс проверяет существование допустимого пути, строки и покрытие всех требований в requirements-review. **Существование ссылки не доказывает истинность объяснения**; семантическое качество остаётся предметом независимой оценки. `UNKNOWN`, `ERROR`, отсутствие доказательства или blocker не дают `VERIFIED`.

Repair начинается от объединённого кандидата и повторяет все обязательные проверки. Для одного repair используется общий владелец разрешённых write scopes. Публикация выполняется отдельной командой после production `VERIFIED`.

## Управление запуском и восстановление

Команды печатают JSON. `run`/`resume` возвращают exit 0 для `VERIFIED` или `FIXTURE_VERIFIED`, exit 2 для непрошедшего запуска; CLI/validation error — exit 1. Получите `RUN_ID` из `run.id` либо `status`.

```text
harness --repo "PROJECT" status
harness --repo "PROJECT" status "RUN_ID"
harness --repo "PROJECT" inspect "RUN_ID"
harness --repo "PROJECT" report "RUN_ID" --output "report.json"
harness --repo "PROJECT" cancel "RUN_ID"
harness --repo "PROJECT" resume "RUN_ID"
```

`cancel` сохраняет запрос в SQLite; активный координатор наблюдает его. Ctrl+C также запрашивает отмену. Уже проверенный результат неизменяем. Это best-effort остановка native process tree, не ограничение намеренно отделившегося недоверенного процесса.

После потерянного receipt `resume` блокируется: действие могло выполниться. Найдите `ACTION_ID` в событиях `action_intent`, проверьте состояние реального эффекта и явно сохраните доказательство:

```text
harness --repo "PROJECT" reconcile "RUN_ID" --action "ACTION_ID" --status not_executed --evidence "Описание независимой проверки отсутствия эффекта"
harness --repo "PROJECT" reconcile "RUN_ID" --action "ACTION_ID" --status completed --evidence "Идентификатор и результат проверки выполненного эффекта"
```

Выбирается **один** подтверждённый статус. Административное evidence — заявление пользователя, CLI не проверяет произвольный внешний сервис.

Если провайдер мог начислить usage, резервация сохраняется. Найдите `CALL_ID` в `model.intent`/`model.unknown`; получите точные токены из провайдера и запишите их:

```text
harness --repo "PROJECT" settle "RUN_ID" --call "CALL_ID" --input-tokens 123 --output-tokens 45 --evidence "Ссылка или идентификатор квитанции провайдера"
```

Числа в примере заменяются проверенными значениями. Не используйте нули для освобождения неизвестных расходов. `resume` сохраняет исходный root budget, deadline и расход repair rounds, повышает generation и восстанавливает последний сохранённый план и его baseline. Завершённые node outputs переиспользуются после проверки совместимости входного дерева зависимостей; незавершённые nodes строятся заново. Диалог модели с точного шага не восстанавливается. Выполненная команда незавершённого builder attempt блокирует автоматический replay; такой checkpoint требует инспекции и явно новой задачи. Cancelled и verified запуски не возобновляются.

Для публикации создайте/выберите существующую локальную ветку, не открытую ни в одном worktree; получите её текущий SHA:

```text
git -C "PROJECT" branch harness-result "BASE_SHA"
git -C "PROJECT" rev-parse refs/heads/harness-result
harness --repo "PROJECT" merge "RUN_ID" --target refs/heads/harness-result --expected "CURRENT_TARGET_SHA"
```

Подставьте реальные SHA. `merge` выполняет только fast-forward через compare-and-swap target ref. Команда не делает push, не меняет checked-out ветку и отказывает scripted результатам.

## Память, навыки и стратегии

Подсистемы доступны через `memory add/promote/search/list`, `skills install/list/quarantine/resolve` и `strategy --input`. Память требует явного source fingerprint и promotion. Builder получает до пяти применимых **active** записей для своего project и точного входного commit; independent reviewers эти записи не получают. После production `VERIFIED` сохраняется episode в статусе **candidate**, требующем отдельного подтверждения; ошибка записи памяти не отменяет результат задачи.

Skills закрепляются `id@version`, проверяются на capabilities и добавляются в task prompt. StrategySelector сравнивает hard constraints, сопоставимые measurements и Pareto frontier; probes автоматически не исполняются. Стратегия выбирается отдельной командой и явно оформляется в TaskSpec; automatic strategy/probe loop ещё не включён. [Контракты, примеры и ограничения этих подсистем](knowledge-skills-strategy.md).

## Где хранится состояние

Состояние находится в `<git-common-dir>/harness/`: SQLite WAL/FULL, события, action intents/receipts, резервации, content-addressed объекты и worktrees. Блокировки привязаны к общему Git directory. Используйте локальный диск; работа на сетевых файловых системах и восстановление после отключения питания отдельно не сертифицированы. Служебные worktrees сохраняются для инспекции; автоматического GC пока нет.

Подробности: [локальные контракты runtime](LOCAL_RUNTIME_CONTRACTS.md), [контракт решений](DECISION_CONTRACT.md), [память, навыки и стратегии](knowledge-skills-strategy.md).
