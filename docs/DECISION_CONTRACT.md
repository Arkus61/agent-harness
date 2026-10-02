# Контракт DecisionService

Контракт версии 1 разделяет три предмета оценки: выбранную модель, конфигурацию инструментов и риск конкретного действия. Оценка не выдаёт полномочий. Источник разрешений — пользовательский TaskSpec и программные проверки Harness ToolGateway.

Документ описывает текущие изменения для следующего выпуска 0.1.2. Результаты испытаний фиксируются отдельно в [отчёте проверки](VALIDATION_REPORT_0.1.2_2026-10-02.md); описание контракта само по себе не означает успешную live-приёмку.

## Запрос

Типы и проверки находятся в [src/decision.rs](../src/decision.rs). Запрос имеет строгую форму без неизвестных полей:

| Поле | Назначение |
|---|---|
| `schema_version` | Сейчас только `1` |
| `request_id` | Новый UUID для каждой оценки |
| `purpose` | `model`, `tools` или `risk` |
| `executor` | Только `harness_tool_gateway` |
| `effective_scope` | Полный контекст полномочий и runtime этой оценки |
| `subject` | Типизированный предмет, соответствующий `purpose` |
| `subject_hash` | BLAKE3 всего запроса, кроме самого hash |

`effective_scope` содержит `role`, `runtime_profile`, `grants`, `owned_paths`, `read_only` и `protected_paths`. Role определяется как `builder` либо `reviewer`; reviewer имеет `read_only:true`. Пустые command grants означают запрет `run_command`, а не недостаток сведений для оценки модели или набора инструментов.

Hash вычисляется через Rust `serde_json::to_vec` над tuple:

```text
(schema_version, request_id, purpose, executor, effective_scope, subject)
```

Это конкретное представление контракта, а не произвольная JSON canonicalization. При реализации другого клиента используйте совместимую сериализацию или предоставленный Rust API. Изменение UUID, purpose, scope, runtime profile или любого значения subject меняет hash. Оценка предыдущего запроса не подходит новому. Hash обеспечивает связь данных, но не является подписью или самостоятельным доказательством доверенного источника.

## Три предмета оценки

| Purpose | `subject.type` | Данные | Условие корректного approval |
|---|---|---|---|
| `model` | `model_selection` | `provider`, `requested_model`, `selection_source`, `allow_remote` | `choice` равен `requested_model`; `tools:[]` |
| `tools` | `tool_configuration` | Точный `enabled_tools` из effective scope | `choice:null`; tools содержат весь заданный набор без добавлений и дубликатов |
| `risk` | `action_risk` | Полный `proposed_action` | `choice:null`; `tools:[]`; оценивается именно данное действие |

Выбор модели статичен: configured provider и model задаются TaskSpec. Assessment может разрешить или остановить использование этой конфигурации, но не переключает backend или модель. Пустой model для ChatGPT представлен как `requested_model:"codex_account_default"`, `selection_source:"account_default"`; фактическую default модель выбирает официальный Codex для аккаунта. Для scripted fixture используется отдельное значение `scripted_fixture`. Это не автоматический neural router между несколькими моделями.

`enabled_tools` строится программно. Read grants включают `read_file` и `search`; write grants вместе с ownership и отсутствием read-only включают `write_file` и `edit_file`; command grants при отсутствии read-only включают `run_command`. Это перечень классов инструментов, а не разрешение любой операции этого класса. Конкретный path, owner, argv и ограничения всё равно проверяет Gateway.

Для `risk` в subject входит исходное действие целиком: например, path/content/expected_hash для записи либо program/args для команды. Предложенный текст — недоверенные данные. Embedded инструкции в файле, snippet или аргументе не изменяют policy. Запрос с несовпадающими purpose и subject, запрещённым действием или изменённым hash отклоняется до model call.

## Ответ

Ответ использует общий `ModelReply`, но обязан завершить только оценку:

```json
{
  "actions": [],
  "done": true,
  "summary": "Оценён конкретный subject",
  "verdict": null,
  "findings": [],
  "plan": [],
  "proofs": [],
  "decision": {
    "purpose": "risk",
    "subject_hash": "EXACT_HASH_FROM_REQUEST",
    "allow": true,
    "abstain": false,
    "reason": "Действие укладывается в предоставленный effective scope.",
    "choice": null,
    "tools": []
  }
}
```

Значение hash в примере — placeholder; в настоящем ответе оно должно буквально совпадать с запросом. Live backend не дополняет отсутствующий hash. Только явно обозначенный scripted fixture может связать пустой fixture placeholder с текущим запросом; fixture не получает production `VERIFIED`.

`subject_hash` обязателен. Purpose должен совпадать. `allow:true` вместе с `abstain:true`, незавершённый ответ, пустая или превышающая 8192 байта причина, смешивание assessment с actions/review/plan/proofs, смена модели, добавление недоступного инструмента и дубликаты отклоняются. Для tool approval порядок tools несущественен, но состав должен совпасть полностью. При отказе допустим только subset разрешённых tools; он не меняет runtime configuration.

Approval требует `allow:true, abstain:false` в режиме `enforced`. Корректный deny или abstain создаёт HOLD. В `shadow` корректная оценка журналируется без семантического блокирования действия; программные запреты остаются обязательными. Некорректный структурированный assessment не превращается в approval. Неизвестный model usage удерживает reservation и блокирует продолжение даже в shadow.

## Порядок применения и журнал

Перед builder attempt выполняются model и tools assessments. На текущем пути [engine.rs](../src/engine.rs) request/reply validation и enforced HOLD происходят до создания builder worktree и записи attempt. Для мутации или команды deterministic validation и risk assessment выполняются до durable action intent и dispatch. Отказ risk assessment не исполняет предложенное действие; уже созданный builder attempt сохраняется для диагностики.

Gateway независимо проверяет grants, ownership, read-only, защищённые пути, формат path, symlinks/junctions, expected source hash, cancellation и актуальную generation. Model approval не расширяет ни одно из этих разрешений и не заменяет проверку файлового состояния. Hash запроса связывает ожидаемые данные операции, но не является полным filesystem snapshot; для изменения существующего файла требуется отдельный `expected_hash`.

После проверки сохраняется `decision.assessment` с request, assessment, mode, fixture flag и hash. Model call имеет собственные budget reservation, intent и usage settlement. Ошибка транспорта не предполагает нулевой расход; неизвестный расход остаётся в HOLD до reconciliation.

## Codex и внешний Gateway

Codex app-server остаётся движком JSON inference с собственным read-only sandbox, без среды исполнения и разрешённых host tools. В thread/turn применяются `environments:[]`, пустые workspace roots и запрет native tool execution. Реальный tool item либо неожиданный server request продолжает блокировать вызов.

JSON `write_file` или `run_command` описывает предложение внешнему Harness ToolGateway. Оценщик не выполняет его. Read-only ограничение самого Codex не запрещает описание допустимой Gateway записи: её исполнимость оценивается по переданным effective grants и runtime profile. Эта граница явно задана в [Codex developer instructions](../src/codex.rs) и системной инструкции DecisionService. Инструкция не снимает read-only sandbox и не выдаёт Codex инструменты.

`runtime_profile` описывает именно Gateway. В `native-trusted` разрешённая команда получает права пользователя и доступ к host filesystem/network; file scopes ограничивают файловые API Gateway, а не внутренние эффекты процесса. Для `isolated` необходим отдельно реализованный и проверенный runtime. Ни hash, ни approval не подтверждают наличие изоляции; недоступный профиль должен завершаться отказом без downgrade.

## Общая политика файлов и credentials

[src/policy.rs](../src/policy.rs) задаёт общую границу для автоматического ContextCompiler и явных файловых операций Gateway. Sensitive компоненты распознаются без учёта ASCII-регистра: служебные `.git`/`.harness`, каталоги известных credential stores, `.env` и `.env.*`, `auth.json`, `credentials`/`credentials.*`, `secrets`/`secrets.*`, SSH private-key имена и расширения ключей/сертификатов. Даже broad read grant не отменяет эти исключения.

ContextCompiler проверяет paths относительно корня конкретного worktree. Служебные `.git/harness/worktrees` в его абсолютных предках не исключают обычные `src/**` файлы. Явный path Gateway должен быть относительным, использовать `/` и не содержать parent traversal, drive prefix/ADS, NUL или переносимые запрещённые имена. Общая path policy исключает различие, при котором initial context отправлял файл, запрещённый explicit read, или explicit read обходил sensitive exclusion контекста.

Это список известных путей, не универсальный детектор секретов. Явные task secrets и API credentials дополнительно redacted в transport/export. Native subprocess не ограничен этой path policy и может обращаться к файлам с правами пользователя. Credentials официального Codex не читаются харнессом и не помещаются в task, memory, отчёты или архивы.

## Что должно подтверждаться проверками

Отдельные tests проверяют completeness subjects, UUID/hash binding, изменение runtime/scope, неверные purpose/hash, смешанные роли, deny/abstain, статичную модель, tools expansion/duplicates, запреты Gateway и нормализацию protected paths. HTTP transport проверяется против missing/wrong hash и model/tool expansion без fixture assistance. Codex protocol fixtures проверяют tool-free sandbox, JSON proposals и отказ реальных transport tools; security fixtures используют только synthetic canaries.

Результаты final build/test и повторного enforced pilot относятся к отдельному отчёту. Они не выводятся из наличия этих tests в source и не заменяют 42 полных системных сценария или замороженный benchmark из 30 задач.
