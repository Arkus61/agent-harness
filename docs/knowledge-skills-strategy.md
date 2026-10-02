# Локальная память, навыки и выбор стратегии

Эти подсистемы работают локально: SQLite/FTS5, неизменяемые JSON-пакеты и программный выбор по заданным ограничениям. PostgreSQL, embeddings и отдельный сервер не нужны.

В примерах ниже `harness` — собранный executable в PATH. Для запуска из этого репозитория используйте `cargo run --bin harness --` перед аргументами команды. Путь `PROJECT` замените путём существующего Git-репозитория. Двойные кавычки вокруг путей подходят для Bash и PowerShell; в `cmd.exe` используйте обычные двойные кавычки.

## Память

Состояние расположено в `<git-common-dir>/harness/knowledge.sqlite`. Project scope привязан к common Git directory, поэтому worktree одного репозитория видят одну область. Другой репозиторий не получает её записи.

Сначала получите ревизию источника:

```text
git -C "PROJECT" rev-parse HEAD
```

Замените `SOURCE_SHA` полученным hash. Если утверждение зависит от другого артефакта, передайте fingerprint этого источника и сохраняйте его одинаковым при поиске и продвижении. CLI принимает fingerprint как декларацию пользователя; он не проверяет, что произвольный текст evidence действительно подтверждает claim.

```text
harness --repo "PROJECT" memory add --kind fact --claim "language: Rust" --evidence "Cargo.toml and the inspected source tree at SOURCE_SHA" --source "SOURCE_SHA"
harness --repo "PROJECT" memory list
harness --repo "PROJECT" memory promote "MEMORY_ID" --source "SOURCE_SHA"
harness --repo "PROJECT" memory search "Rust" --source "SOURCE_SHA"
```

`memory add` печатает `{ "id": "..." }`. Подставьте этот ID вместо `MEMORY_ID`. Запись изначально имеет статус `candidate`, и поиск её не возвращает. Продвижение требует непустого evidence и точного совпадения source hash. Пользователь выполняет его после независимой проверки источника. Частота повторений не активирует память автоматически.

Поиск возвращает только `active` для текущего project и переданного source hash, максимум 10 записей. Утверждение для другой ревизии не применяется автоматически. Начальная политика консервативна: долгоживущие утверждения не переносятся между ревизиями без отдельного подтверждения.

Типы: `fact`, `decision`, `episode`, `failure`, `procedure`, `constraint`, `security_assumption`, `preference`. Evidence и claim сохраняются локально как текст. Не заносите секреты через административный CLI: операции памяти не проходят через task-specific secret filter.

Утверждения вида `runtime: Rust` и `runtime: Python` имеют общий topic `runtime` и становятся `contested`; прежняя активная запись тоже исключается из поиска. Это консервативное обнаружение потенциального конфликта, а не автоматическое определение истины. Произвольный prose не имеет надёжного семантического конфликтного анализа.

Программный API позволяет явно задать topic и разрешить конфликт:

```rust
use agent_harness::knowledge::{Knowledge, MemoryStatus};

fn example(db: &Knowledge) -> anyhow::Result<()> {
let old = db.insert_scoped("project", "fact", "runtime", "Rust", "old manifest", "old-tree")?;
let replacement = db.insert_scoped("project", "fact", "runtime", "Python", "verified new manifest", "new-tree")?;
db.set_status(&old, MemoryStatus::Superseded)?;
db.promote(&replacement, "new-tree")?;
Ok(())
}
```

Статусы `rejected`, `superseded`, `archived` и `stale` доступны через `set_status`; административная CLI-команда для них пока не предоставлена. Переход в `active` возможен только через `promote`. Память — данные для рассмотрения, она не расширяет полномочия и не заменяет условия задачи.

## Навыки

[Пример пакета](../examples/skill.json) содержит навык `source-localization@1.0.0`. У него требуются только чтения `src/**` и `Cargo.toml`.

```text
harness --repo "PROJECT" skills install "examples/skill.json"
harness --repo "PROJECT" skills list
```

Путь файла относится к текущей директории CLI, а не к `PROJECT`; из другой директории передайте абсолютный путь к примеру. Install возвращает content hash. Пакет сохраняется в `<git-common-dir>/harness/skills/objects`, а SQLite registry связывает точный `id@version` с hash и статусом. Установка — явное локальное действие пользователя; она не доказывает безопасность инструкции.

Для задачи добавьте поле:

```json
"skills": ["source-localization@1.0.0"]
```

Её `grants.read` должны включать `src/**` и `Cargo.toml`. Все остальные обязательные поля `TaskSpec`, включая реальные protected checks, остаются обязательными. Проверить разрешение пакетов можно без запуска модели:

```text
harness --repo "PROJECT" skills resolve --task "task-with-skill.json"
```

`resolve` проверяет hash, текущий статус, зависимости и требуемые capabilities. Зависимости возвращаются до зависимых пакетов. `latest`, незакреплённые ссылки, отсутствующие dependencies, циклы и слишком глубокие графы отклоняются. Runtime ToolGateway по-прежнему проверяет каждое реальное действие.

Для capabilities используется консервативная проверка включения scopes: точное совпадение, полный `**`, literal parent `src/**`, либо literal файл внутри разрешённого glob. Произвольное включение одного сложного glob в другой не угадывается. Command requirement совместим с grant только для того же program и допустимого argv prefix. Не расширяйте grants ради обхода ошибки разрешения; уменьшайте scope пакета до нужного.

Изменение тела под прежним `id@version` отклоняется. Чтобы изменить пакет, выпустите новую version и явно закрепите её в задаче.

```text
harness --repo "PROJECT" skills quarantine "source-localization@1.0.0"
harness --repo "PROJECT" skills resolve --task "task-with-skill.json"
```

Вторая команда после quarantine завершается ошибкой. Повторная установка прежнего пакета не снимает quarantine. Инструкции пакета не исполняются при install/resolve, и registry не запускает команды сам.

## SESE / StrategySelector

```text
harness strategy --input "examples/strategy.json"
```

[Пример сравнения](../examples/strategy.json) использует **синтетические значения** для демонстрации selector. Evidence помечено `SYNTHETIC FIXTURE`; это не benchmark SQLite, не доказательство кроссплатформенности и не live evaluation.

Ожидаемый результат примера:

- `remote-index` отклоняется из-за hard requirement `offline`, несмотря на выгодные числа;
- `linear-scan` и `sqlite-fts` входят в Pareto frontier;
- `sqlite-fts` выбирается по объявленным весам latency=3 и memory=1;
- `probes` и `unresolved` пусты.

Selector принимает одинаковый список hard requirements для всех кандидатов. `PASS`/`FAIL` без evidence рассматривается как неопределённость; неизвестное обязательное условие не считается выполненным. Программная подсистема проверяет наличие и применимость деклараций, но фактическую истинность evidence должен проверять внешний probe/оценщик.

Измените один hard status на `UNKNOWN`, удалите measurement либо задайте другую `environment_hash`: выбор будет отложен, `selected` станет `null`, и появятся probe requests. Значения с разными unit, неизвестной средой, отсутствующим evidence или нулевым sample size не участвуют в сравнении. Некорректный JSON отклоняется до выбора.

В standard profile максимум три кандидата и два probe requests. Программа не исполняет probes автоматически; их результаты нужно получить в одинаковой среде и передать новой версией входного JSON. Если unresolved кандидатов больше лимита probes, все остаются перечисленными в `unresolved`, а выполнение ограничено budget policy.

Измеренные допустимые кандидаты сначала фильтруются по Pareto. Затем используются заданные веса и min-max нормализация среди допустимых измерений; это воспроизводимое правило выбора, не вероятность успеха. Равная utility разрешается стабильным порядком ID. Без comparison metrics несколько допустимых стратегий не получают произвольного победителя.

Сейчас CLI `strategy` — отдельный вызов selector. Он не подменяет автоматически DAG или провайдера в `run`: выбранную стратегию нужно оформить в `TaskSpec`, scope и план исполнения. Значение этих модулей оценивается сквозными задачами, а не самим фактом успешного импорта JSON.

## Проверки

```text
cargo test --lib knowledge::
cargo test --lib skills::
cargo test --lib strategy::
```

Тесты проверяют source/project фильтрацию, prerequisites продвижения, конфликты и restart; неизменяемость, quarantine, зависимости, hashes и scopes; hard constraints, bounded probes, сопоставимость измерений и настоящий Pareto tradeoff. Они не оценивают качество модели, истинность произвольного claim или фактическую скорость поиска на пользовательском репозитории.
