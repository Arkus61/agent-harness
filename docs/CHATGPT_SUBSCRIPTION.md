# Подключение ChatGPT через Codex

В версии **0.1.2** backend `chatgpt` использует официальный **Codex CLI app-server** по локальному stdio. Codex управляет входом, credentials и их обновлением. Харнесс не читает `auth.json`, не извлекает OAuth-токены и не обращается к приватным HTTP endpoints самостоятельно. [Текущая проверка выпуска](VALIDATION_REPORT_0.1.2_2026-10-02.md).

Это подключение для локального харнесса. Согласно [официальной документации app-server](https://developers.openai.com/codex/app-server), app-server authentication поддерживается для локальных и open-source приложений; коммерческие или hosted сервисы используют отдельный [Sign in with ChatGPT](https://developers.openai.com/cookbook/articles/sign-in-with-chatgpt).

## Вход и проверка

Установите [Codex CLI](https://developers.openai.com/codex/cli) и проверьте доступность команды:

```text
codex --version
harness auth status
harness auth chatgpt --device --check
```

`harness auth status` запрашивает account/model metadata через app-server. Он показывает тип входа, доступный план, если сервис его сообщил, список моделей и default. Отсутствующее значение `planType` не позволяет определить конкретную платную подписку. Сам факт входа не доказывает, что выбранная модель сейчас может генерировать ответ.

`harness auth chatgpt` повторно использует существующий вход **ChatGPT**. Если его нет, запускается официальный `codex login`; флаг `--device` выбирает `codex login --device-auth`. В headless-среде откройте показанный Codex адрес в своём браузере, войдите и введите одноразовый код. Device-code login необходимо разрешить в настройках безопасности личного аккаунта или разрешениях workspace; подробности — в [Authentication](https://developers.openai.com/codex/auth).

`--check` выполняет настоящий короткий model turn без файлов проекта. Успех требует ожидаемого структурированного ответа без actions и полной телеметрии usage. Это проверка соединения, а не качества разработки, reviews или прохождения 30 evaluation tasks. Выбор модели можно проверить явно:

```text
harness auth chatgpt --check --model MODEL_FROM_AUTH_STATUS
```

Если executable установлен вне `PATH`:

```text
harness auth status --codex-program "/path/to/codex"
harness auth chatgpt --codex-program "/path/to/codex" --device --check
```

На Windows можно указать полный путь к `codex.exe`. Реальное выполнение интеграции на Windows/macOS ещё требует проверки; наличие переносимого stdio-протокола не заменяет результаты CI на этих ОС.

## Конфигурация задачи

В [examples/task-chatgpt.json](../examples/task-chatgpt.json) сохранены scopes, checks и DAG примера `task-local.json`; изменён только provider:

```json
{
  "kind": "chatgpt",
  "codex_program": "codex",
  "model": "",
  "allow_remote": true
}
```

`codex_program` по умолчанию равен `codex`. Пустой `model` использует default Codex для текущего аккаунта; для воспроизводимого сравнения укажите доступную модель явно. `base_url` и `api_key_env` не используются этим backend. Подключение требует `allow_remote: true`, поскольку инструкции и доступный контекст задачи отправляются сервису Codex.

```text
harness --repo "../harness-demo" run --task "examples/task-chatgpt.json"
```

Пример рассчитан на Git/Rust-проект, созданный `harness demo`. Для другого проекта задайте свои requirements, grants, ownership и checks. Репозиторий должен иметь commit и чистые tracked файлы, а зависимости и закоммиченный lockfile — быть подготовлены для указанного `cargo test --locked --offline`.

Учётная запись и credentials остаются в хранилище официального CLI. Не помещайте `auth.json`, access/refresh tokens или API-ключи в task JSON, memory, skill packages, отчёты, репозиторий, архивы или чат. При утрате или истечении входа используйте официальный login повторно; отсутствие backend не переключает задачу автоматически на API.

## Как проходит запрос

Для каждого model call запускается официальный `codex app-server --stdio`, выполняется `initialize` с идентификатором `agent_harness` и `experimentalApi: true`, затем `initialized`. Харнесс проверяет `account/read` и создаёт **новый ephemeral thread**. Запрос `turn/start` задаёт `outputSchema` контракта `ModelReply`; финальный JSON проходит строгую Rust-десериализацию.

Thread и turn используют `environments: []`, что отключает доступ Codex к среде исполнения. Унаследованные hooks, уведомления, навыки, plugins/apps, shell, multi-agent инструменты и MCP отключаются конфигурацией адаптера. Код принимает только ожидаемые сообщения и завершение; появление tool item либо серверного запроса блокирует вызов. Codex не выполняет действия из `ModelReply`: их проверяет и исполняет наш ToolGateway с grants, ownership, журналом intents/receipts и cancellation. Собственный read-only sandbox оценщика не запрещает описывать допустимое действие внешнему Gateway; предложенное действие оценивается по supplied effective scope и runtime profile. [Контракт model/tools/risk](DECISION_CONTRACT.md) связывает оценку с конкретным запросом, не выдавая разрешений.

Сам протокол app-server и поле `environments` чувствительны к версии CLI. Исследование и переговоры протокола выполнены с **Codex CLI 0.159.0-alpha.3**. При обновлении Codex проверяйте `auth status`, `auth chatgpt --check` и тесты адаптера; несовместимый протокол должен завершаться ошибкой. Совместимость с произвольными будущими версиями не заявляется.

Для диагностики можно получить официальную схему установленного executable без входа и генерации:

```text
codex app-server generate-json-schema --experimental --out codex-protocol
```

Событие `thread/tokenUsage/updated` содержит cumulative `tokenUsage.total`. После завершения turn адаптер выполняет `account/read` как протокольный fence и читает задержанные события ещё **200 мс**. Используется последний наблюдаемый cumulative total свежего thread. Это ограниченное наблюдение, а не гарантия получения окончательного billing receipt или всех будущих событий.

Харнесс учитывает весь наблюдаемый `totalTokens`: в input записывается `inputTokens`, в output — остаток общего расхода, чтобы не потерять сообщённые сервисом дополнительные токены. Отсутствующая, некорректная или частично полученная телеметрия, а также прерванный результат оставляют расход неопределённым и удержанную резервацию в HOLD. Для reconciliation используется `settle`; нулевой расход не предполагается автоматически.

## Лимиты и приёмка

Используется доступ ChatGPT-аккаунта к Codex и соответствующие ограничения сервиса. Подключение не подтверждает конкретный платный план, не даёт API-кредитов и не означает безлимит. [Вход с API-ключом оплачивается отдельно](https://developers.openai.com/codex/auth), по правилам OpenAI Platform; этот backend требует именно вход ChatGPT.

App-server не предоставляет жёсткий `max_output_tokens`. Для `chatgpt` значение `budget.max_output_tokens` задаёт **мягкую квоту**, а reservation — оценку байтов отправленных инструкций и контекста с дополнительными **16 КиБ** для внутреннего контекста Codex плюс output quota. Codex может добавить скрытый контекст и выполнить внутренние retries. Харнесс не обещает ровно один upstream request или гарантированное предотвращение token overspend: фактический расход сохраняется, а превышение root budget блокирует продолжение. Денежные тарифы и billing receipts не учитываются.

Deadline, ограничение объёма протокола и отмена действуют в адаптере. Отдельный trusted supervisor очищает Codex process group после owner death; остановка app-server helper и descendant heartbeat при настоящем SIGKILL владельца подтверждена тестом 0.1.2. Это process lifecycle, не confinement произвольного native-кода. При прерывании нельзя предполагать отсутствие начисленного usage. `allow_remote: true` обязательно, а redaction явных task secrets не обеспечивает автоматическое обнаружение неизвестных секретов в исходниках. Ошибки показывают публичный класс, например `unauthorized`, без исходного provider body или credentials.

Исторический auth check **2026-10-02**, до выпуска 0.1.2, подтвердил официальный device login: **`authenticated:true`**, тип **`chatgpt`**, план **`plus`**, default **`gpt-6.1-sol`**. Настоящий `auth chatgpt --check` вернул `actions:[]`, `done:true`, `summary:"ChatGPT connection OK"` и complete usage — **5000 input + 45 output tokens**. [Сохранённый результат](chatgpt-connection-check.json). Этот receipt подтверждает тот вход и model turn, не остаток subscription quota. Одноразовые коды и credentials в документацию или архивы не помещаются.

С новым контрактом отдельно прошла одна реальная risk оценка `gpt-6.1-sol`: ожидаемый subject hash, `allow:true`, `abstain:false`, complete usage **5388 input + 137 output tokens**. **Gateway не исполнялся** — это проверка JSON inference, не готовая задача разработки. [Sanitized receipt](../artifacts/contract-validation/subscription/live-risk-contract-smoke.json).

После неё **18 native trials 0.1.2** завершили полный enforced цикл с **VERIFIED**, command checks и четырьмя reviews. Native usage — **1 512 324 tokens / 205 complete calls**, unknown/reserved **0**. Внешняя приёмка — **18/18: 15 original + 3 supplemental Q03 PASS** с исправленным AST grader, без новых model calls; исходные 15/18 и FAIL receipts сохранены. [Amendment receipt](../artifacts/contract-validation/oracle-amendment/summary.json). Отдельный полный **isolated Q02 PASS**: **115,177 с**, **71 937 tokens / 10 complete calls**, exact-candidate audit PASS; внешний oracle исполнялся native-trusted. Полный frozen 30-task benchmark не выполнен. [Отчёт и границы результата](VALIDATION_REPORT_0.1.2_2026-10-02.md), [JSON summary](validation-summary-0.1.2-2026-10-02.json).

Основной pilot **0.1.1** получил **0/18** enforced PASS из-за прежнего DecisionService; отдельный Q02 shadow diagnostic прошёл полный цикл с exact-candidate oracle. Эти исторические результаты сохранены в [отчёте 0.1.1](VALIDATION_REPORT_2026-10-02.md) и не присваиваются новому executable. [Воспроизводимые live fixtures](../evals/live/README.md).

Для final source **0.1.2** прошли **129 обычных тестов**, fmt, clippy и release; отдельный real smoke не входит в этот итог. Release выполнил **20/20** component assertions и scripted demo FIXTURE_VERIFIED. Linux isolation через probed bubblewrap проверен boundary fixtures, а native runtime остаётся исполнением с правами пользователя. Подключение модели не закрывает hidden acceptance, evaluation reviewer-ролей, benchmark 30 задач и три повторения. [Общий статус реализации](IMPLEMENTATION_STATUS.md).
