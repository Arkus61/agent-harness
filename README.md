# Agent Harness 0.1.3

Локальный харнесс для разработки на **Rust**: задача проходит через DAG, исполнителей в Git worktree, объединение изменений, командные проверки и независимые reviews требований, кода, тестов и безопасности. Есть CLI, сохранение состояния в SQLite, учёт токенов, отмена и явное разрешение неоднозначных результатов действий.

Версия **0.1.3**: **191 локальный Rust-тест PASS**, bounded context cache с проверкой источников, process limits, durable local outbox и исторический replay. **30/30 benchmark baseline/control пар** прошли независимые Linux-isolated проверки. Реальное ChatGPT-подключение работает; свежий H pilot остановился с `BLOCKED` после Codex timeout и unknown usage. **Полная готовность не подтверждена.** [Текущий отчёт](docs/VALIDATION_REPORT_0.1.3_2026-10-02.md), [JSON](docs/validation-summary-0.1.3-2026-10-02.json), [новые контракты](docs/LOCAL_RUNTIME_CONTRACTS.md). Результаты успешного live pilot **0.1.2** сохранены [отдельно](docs/VALIDATION_REPORT_0.1.2_2026-10-02.md) и не сертифицируют новую версию.

## Сборка и быстрый пример

Если вы используете готовый Linux-архив, распакуйте его и запускайте executable из каталога пакета как `./bin/harness`. Например, `./bin/harness auth status` и `./bin/harness auth chatgpt --check`. Для команд ниже добавьте каталог `bin` пакета в `PATH` либо используйте полный путь к executable. Codex CLI устанавливается отдельно и тоже должен быть доступен в `PATH`.

Нужны **Rust 1.99.0**, Cargo и Git в `PATH`. Версия toolchain закреплена в `rust-toolchain.toml`; rustup выбирает её для сборки этого репозитория. SQLite включён в сборку; отдельный сервер, Python, Node.js и Docker для native runtime не требуются.

```text
cargo build --locked --release
cargo test --locked --all-targets
```

Executable: `target/release/harness` на Linux/macOS, `target/release/harness.exe` на Windows. Все команды ниже выполняются из корня проекта. На Linux/macOS:

```bash
# Эти существующие каталоги нужны rustup/Cargo внутри очищенного окружения checks.
export CARGO_HOME="${CARGO_HOME:-$HOME/.cargo}"
export RUSTUP_HOME="${RUSTUP_HOME:-$HOME/.rustup}"
./target/release/harness doctor
./target/release/harness demo --dir ../harness-demo
./target/release/harness --repo ../harness-demo status
```

На Windows для Rust/MSVC нужны Visual Studio Build Tools с компонентами C++ и Windows SDK. Запускайте команды из **Developer PowerShell** или **Developer Command Prompt** Visual Studio, чтобы MSVC linker и SDK были доступны проверкам проекта. В Developer PowerShell:

```powershell
if (-not $env:CARGO_HOME) { $env:CARGO_HOME = Join-Path $env:USERPROFILE ".cargo" }
if (-not $env:RUSTUP_HOME) { $env:RUSTUP_HOME = Join-Path $env:USERPROFILE ".rustup" }
.\target\release\harness.exe doctor
.\target\release\harness.exe demo --dir ..\harness-demo
.\target\release\harness.exe --repo ..\harness-demo status
```

Если Rust установлен в других каталогах, укажите их. Native command runtime сохраняет `PATH`, необходимые системные переменные и `CARGO_HOME`/`RUSTUP_HOME`/`RUSTUP_TOOLCHAIN`; остальные переменные окружения не передаются автоматически. Сохраните имя executable `harness` (`harness.exe` на Windows): он запускает собственный доверенный supervisor. При использовании Rust library этот CLI должен быть доступен в поддерживаемом расположении; произвольный host executable не заменяет supervisor.

Для Linux isolation установите `bubblewrap` и проверьте `harness doctor`: `runtime_profiles.isolated.available` становится `true` только после реального namespace probe. Задайте `"profile":"isolated"` в task. Команды получают отдельные filesystem/PID/network namespaces, пустой home, скрытые credential paths и read-only trusted toolchain mounts. Workspace остаётся ресурсом команды; это не syscall ownership каждого файла. `protected_paths` замораживаются mounts, а отсутствующий prefix может заморозить ближайший существующий родительский каталог. CPU/RAM/disk quotas пока не реализованы. Cache с legacy Cargo Git registry может не работать offline после скрытия `.git` metadata; заранее проверьте зависимости в выбранном профиле.

Готовый Linux x86_64 executable требует **glibc ≥ 2.39** и системные `libc`, `libm`, `libgcc_s`. Для старой glibc пересоберите исходники в целевой среде. Сборки Linux/macOS/Windows и результаты проверок публикуются в [GitHub Actions](https://github.com/Arkus61/agent-harness/actions/workflows/ci.yml); бинарные артефакты доступны после успешного завершения соответствующего job. Локальные отчёты выше относятся к указанным в них замороженным исходникам и executable.

`demo` требует **новый, ещё не существующий каталог**. Он создаёт маленький Git/Rust-проект, исправляет ошибку `clamp`, запускает настоящие `cargo test --offline` и четыре scripted reviews. Ожидаемое состояние — **`FIXTURE_VERIFIED`**. Это демонстрация механики: scripted ответы не подтверждают качество модели, не получают production `VERIFIED` и не допускаются к `merge`. Исходный `HEAD` демо-проекта остаётся на baseline; кандидат находится в report и worktree.

Далее для удобства добавьте executable в `PATH` либо заменяйте `harness` полным путём к нему. Из исходников эквивалентный префикс: `cargo run --locked --bin harness --`.

## Запуск с настоящей моделью

Поддерживаются **ChatGPT через Codex CLI** и **OpenAI-compatible Chat Completions**. Оба backend возвращают типизированный `ModelReply`; файловые действия и команды выполняет ToolGateway харнесса.

### Подписка ChatGPT

Установите [официальный Codex CLI](https://developers.openai.com/codex/cli) и добавьте `codex` в `PATH`. Затем:

```text
harness auth status
harness auth chatgpt --device --check
harness --repo "../harness-demo" run --task "examples/task-chatgpt.json"
```

`auth chatgpt` использует существующий вход ChatGPT либо запускает официальный `codex login`. Для headless-среды `--device` выбирает `codex login --device-auth`: пользователь проходит вход в браузере, Codex сохраняет и обновляет credentials. `--check` делает настоящий короткий запрос модели и проверяет JSON-ответ и usage. Без этого флага проверяется вход, а не генерация. Для установки вне `PATH` есть `--codex-program "/path/to/codex"`; тот же путь задаётся в task provider.

Официальный вход ChatGPT подтверждён: аккаунт сообщает **Plus**, модель **`gpt-6.1-sol`** отвечает структурированным JSON с полной usage telemetry. На **0.1.2** все 18 native trials дошли до VERIFIED, checks и четырёх reviews; exact-candidate bindings проверены, unknown calls и reservations равны нулю. Внешняя приёмка приняла **15 кандидатов исходным oracle + 3 Q03 отдельным исправленным AST grader**, без новых model calls или замены original FAIL receipts. Отдельный полный isolated Q02 также прошёл внешний oracle. [Подробный отчёт](docs/VALIDATION_REPORT_0.1.2_2026-10-02.md); полный 30-task benchmark не выполнен. [Результат подключения](docs/chatgpt-connection-check.json). Исторические **0/18** относятся к [выпуску 0.1.1](docs/VALIDATION_REPORT_2026-10-02.md).

[Готовая задача](examples/task-chatgpt.json) содержит `kind: "chatgpt"` и обязательное `allow_remote: true`. Пустой `model` использует default Codex для аккаунта; доступные модели показывает `auth status`, выбранную можно задать в task и проверить через `--model MODEL`. Контекст отправляется сервису Codex. Credentials остаются в хранилище официального CLI; не помещайте их в проект, чат или архив.

Используются лимиты Codex, доступные текущему ChatGPT-аккаунту. API оплачивается отдельно; это подключение не создаёт API-кредитов и не означает безлимит. У Codex backend нет жёсткого output-token cap: `max_output_tokens` здесь мягкая квота, а резервация содержит дополнительную оценку 16 КиБ. Фактический расход сохраняется; превышение root budget блокирует продолжение. [Настройка, протокол и ограничения](docs/CHATGPT_SUBSCRIPTION.md).

### OpenAI-compatible backend

Провайдер должен принимать `response_format: {"type":"json_object"}`, возвращать один JSON-ответ с `finish_reason: "stop"` и полные `usage.prompt_tokens`/`usage.completion_tokens`. Markdown вокруг JSON, неизвестные поля, усечённый ответ и malformed JSON отклоняются. Возможность модели выполнять этот контракт определяется выбранным backend и вашей проверкой, а не именем провайдера.

Перед запуском нужен Git-репозиторий с хотя бы одним commit и чистыми **tracked** файлами. В worktree переносится committed baseline; произвольные untracked файлы исходной рабочей директории не копируются. Зависимости для `--offline` должны быть доступны заранее, а lockfile — закоммичен.

Для запуска на проекте `harness-demo` используйте [готовый task-local.json](examples/task-local.json) либо сохраните следующий JSON как отдельный `task-local.json`. В другом проекте замените описание, scopes и checks под его контракт.

```json
{
  "schema_version": 1,
  "prompt": "Исправь clamp в src/lib.rs для low <= high и добавь regression tests.",
  "requirements": [
    "Значение ниже low возвращает low, выше high — high, внутри интервала сохраняется.",
    "Добавлены regression tests для всех трёх случаев."
  ],
  "grants": {
    "read": ["src/**", "Cargo.toml", "Cargo.lock"],
    "write": ["src/**"],
    "commands": []
  },
  "checks": [
    {"program": "cargo", "args": ["test", "--locked", "--offline"], "timeout_secs": 120}
  ],
  "protected_paths": ["acceptance/**"],
  "provider": {
    "kind": "open_ai",
    "base_url": "http://127.0.0.1:11434/v1",
    "model": "qwen2.5-coder:7b",
    "api_key_env": "",
    "allow_remote": false
  },
  "nodes": [
    {"id": "fix", "prompt": "Исправь src/lib.rs и тесты.", "requirements": [0, 1], "depends_on": [], "owned_paths": ["src/**"]}
  ],
  "profile": "native-trusted",
  "decision_mode": "enforced",
  "budget": {"max_tokens": 250000, "max_output_tokens": 4096, "deadline_secs": 1800},
  "concurrency": 2,
  "max_steps": 20,
  "max_repairs": 2,
  "context_bytes": 48000,
  "secrets": [],
  "skills": []
}
```

Локальный Ollama должен быть запущен и иметь выбранную модель; замените `model` своей установленной моделью. Этот пример конфигурации не означает, что конкретная модель уже прошла live benchmark харнесса.

```text
harness --repo "../harness-demo" doctor
harness --repo "../harness-demo" run --task "examples/task-local.json"
```

Чтобы модель составляла DAG сама, уберите `nodes` или задайте `[]`. Явный план проверяется на покрытие требований, циклы, зависимости и пересечение ownership параллельных узлов. Индексы требований начинаются с нуля. `concurrency` ограничивает builders и одновременные model calls.

Для удалённого backend замените только объект `provider`:

```json
{
  "kind": "open_ai",
  "base_url": "https://api.openai.com/v1",
  "model": "YOUR_COMPATIBLE_MODEL",
  "api_key_env": "HARNESS_MODEL_API_KEY",
  "allow_remote": true
}
```

Задайте `HARNESS_MODEL_API_KEY` через своё окружение или менеджер секретов. Ключ не помещается в task JSON. Для внешних адресов обязательны `allow_remote: true` и HTTPS; URL с credentials/query/fragment отклоняется, redirects не выполняются. Явное разрешение remote означает разрешение отправлять этому endpoint доступный контекст задачи. Автоматического классификатора неизвестных секретов или ограничения сети произвольных native-команд пока нет.

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

DecisionService получает оценки `model`, `tools` и `risk` через configured provider. Каждый запрос содержит отдельный typed subject, UUID, effective scope с runtime profile и BLAKE3 binding; ответ обязан вернуть тот же hash. Model choice статичен, assessment не включает дополнительные tools. В `enforced` отказ или воздержание блокируют действие; в `shadow` корректная оценка наблюдается, но не расширяет grants. Некорректный assessment не становится разрешением. Это typed neural assessment, а не отдельный Jev SDK или автоматическая маршрутизация между моделями. [Точный контракт](docs/DECISION_CONTRACT.md).

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

Skills закрепляются `id@version`, проверяются на capabilities и добавляются в task prompt. StrategySelector сравнивает hard constraints, сопоставимые measurements и Pareto frontier; probes автоматически не исполняются. Стратегия выбирается отдельной командой и явно оформляется в TaskSpec; automatic strategy/probe loop ещё не включён. [Контракты, примеры и ограничения этих подсистем](docs/knowledge-skills-strategy.md).

## Хранилище и проверки

Состояние хранится в `<git-common-dir>/harness/`: SQLite WAL/FULL, события, action intents/receipts, резервации, content-addressed объекты и worktrees. Locks привязаны к общему Git directory. Используйте локальный диск; сохранность после отключения питания и работа на сетевых файловых системах отдельно не сертифицированы. Служебные worktrees сохраняются для инспекции; автоматического GC пока нет.

```text
cargo fmt --all -- --check
cargo clippy --locked --all-targets -- -D warnings
cargo test --locked --all-targets
harness eval
```

`eval` выполняет ограниченный набор **component fixture assertions** с отдельными критериями и доказательствами. Он не означает прохождение всех 42 сценариев, 30 live-задач или cross-platform certification. [План оценивания и критерии](docs/EVALUATION_PLAN.md) связывает существующие проверки с [машиночитаемым каталогом сценариев](evals/scenarios.json). CI настроен для Linux/macOS/Windows; результат настройки CI отличается от фактически завершённых запусков. [Матрица реализованного и оставшаяся работа](docs/IMPLEMENTATION_STATUS.md).

Свежий прогон **2026-10-02** для финального source **0.1.2** на Linux подтвердил **129 тестов**: 85 library, 3 Codex CLI, 14 Codex protocol, 6 contracts, 7 end-to-end, 7 isolation, 1 parallel failure, 3 parent-death и 3 security regressions. Также прошли fmt, clippy всех targets с `-D warnings` и release build. Восемь реальных Linux boundary checks входят в это число; доступность backend обязательна в данном прогоне. Отдельный реальный subscription smoke не входит в 129 обычных тестов.

Release **0.1.2** прошёл `eval` и `demo`: **20/20** component assertions и `FIXTURE_VERIFIED`. Supervisor tests подтвердили остановку ordinary native descendants и Codex helper после SIGKILL владельца. Warm CLI p95: **2,482 мс** для `--version`, **28,685 мс** для `doctor`, **14,011 мс** для `eval`; измерения шли одновременно с одним live pilot и не означают full startup/RSS benchmark. [Методика и результаты](docs/VALIDATION_REPORT_0.1.2_2026-10-02.md).

В [live pilot](evals/live/README.md) исполнены шесть Rust-задач × три fresh trials: **18 VERIFIED**, independent acceptance **18/18 = 15 original + 3 supplemental PASS**, binding failures **0**. [Q03 amendment](artifacts/contract-validation/oracle-amendment/summary.json) сохраняет исходные FAIL receipts и фиксирует исправление границы функции grader. Отдельный isolated Q02 прошёл за **115,177 с**; сам внешний oracle выполнялся native-trusted. Аудит тестов шести representative candidates подтвердил assertions и negative controls; он не распространяется на остальные варианты. Полные 42 сценария, frozen benchmark 30 задач и Windows/macOS остаются незакрытыми. [Подробный отчёт](docs/VALIDATION_REPORT_0.1.2_2026-10-02.md).

Архитектурные основания: [единый план](docs/ARCHITECTURE_PLAN.md) и [сравнение предложений](docs/ARCHITECTURE_COMPARISON.md).
