# Подключение ChatGPT через Codex

В версии 0.1.1 backend `chatgpt` использует официальный **Codex CLI app-server** по локальному stdio. Codex управляет входом, credentials и их обновлением. Харнесс не читает `auth.json`, не извлекает OAuth-токены и не обращается к приватным HTTP endpoints самостоятельно.

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

Thread и turn используют `environments: []`, что отключает доступ Codex к среде исполнения. Унаследованные hooks, уведомления, навыки, plugins/apps, shell, multi-agent инструменты и MCP отключаются конфигурацией адаптера. Код принимает только ожидаемые сообщения и завершение; появление tool item либо серверного запроса блокирует вызов. Codex не выполняет действия из `ModelReply`: их проверяет и исполняет наш ToolGateway с grants, ownership, журналом intents/receipts и cancellation.

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

Deadline, ограничение объёма протокола и отмена действуют в адаптере. Cleanup helper-процессов при настоящем SIGINT CLI проверен отдельным тестом; это не подтверждает confinement произвольного native-кода. При прерывании нельзя предполагать отсутствие начисленного usage. `allow_remote: true` обязательно, а redaction явных task secrets не обеспечивает автоматическое обнаружение неизвестных секретов в исходниках. Ошибки показывают публичный класс, например `unauthorized`, без исходного provider body или credentials.

На 2026-10-02 первоначально были подтверждены `account/read` с типом `chatgpt` и `model/list` с default `gpt-6-astra`; `planType` сервис не сообщил. **Два реальных model checks завершились `unauthorized`**. Последний `auth status` из release сообщает `authenticated:false`, `account_type:null`; текущее подключение не установлено. Новый официальный device login запущен и ожидает завершения пользователем в браузере. Одноразовые коды и credentials не записываются в документацию или архивы. Успешный model turn и live pipeline не заявляются.

Для финальных исходников 0.1.1 прошёл **91 тест**, включая Codex protocol и CLI fixtures, а также fmt, clippy и release build. Release выполнил 20 component assertions, все PASS. [Общий статус реализации](IMPLEMENTATION_STATUS.md) отличает проверки механики от полного benchmark.

Профиль `native-trusted` остаётся исполнением с правами пользователя. Подключение подписки не реализует строгую изоляцию, не закрывает hidden acceptance и не заменяет evaluation четырёх reviewer-ролей, 30 задач и трёх повторений.
