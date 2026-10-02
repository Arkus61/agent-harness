# Локальные контракты версии 0.1.3

## Ограничения процессов

TaskSpec принимает необязательное `command_resource_limits`; CommandSpec для
обязательной проверки — `resource_limits`. Значения положительные, не больше
`i64::MAX`. Отсутствие поля сохраняет прежнюю сериализацию и поведение.

```json
{
  "command_resource_limits": {
    "cpu_seconds": 30,
    "address_space_bytes": 2147483648,
    "file_size_bytes": 268435456,
    "processes": 4096
  }
}
```

Это фрагмент полного TaskSpec. Для каждого поля применяется меньший предел
из task default и explicit check. Ограничения входят в DecisionScope и binding
проверки. Supervisor устанавливает hard/soft rlimits перед запуском target;
Linux также устанавливает `no_new_privs`. Supervisor сохраняет возможность
выполнить cleanup. Наследуемый hard limit нельзя поднять обычным дочерним процессом.

`doctor.resource_limits` показывает результат ограниченного syscall probe в
отдельном ребёнке и ограничения backend. Root/CAP_SYS_RESOURCE, а для NPROC
также CAP_SYS_ADMIN, отклоняются. CPU/AS — пределы каждого процесса, FSIZE —
каждого файла; Linux NPROC считает процессы реального UID, включая посторонние.
Это не суммарные CPU/RAM/disk квоты дерева. Поля `aggregate_cpu_seconds`,
`aggregate_memory_bytes`, `aggregate_disk_bytes` распознаются и отклоняются
до model calls/attempts, пока не настроен настоящий aggregate backend.
Windows resource limits пока недоступны; JobObject здесь отвечает за lifecycle.

## Контекст и источники

Engine использует общий для одного запуска ContextCache: до 64 записей и
8 MiB сериализованного redacted payload; metadata и временное чтение источников
учитываются отдельно. Ключ включает project/root, role, grants, prompt,
requirements, budget, версии политики redaction и актуальные bytes источников.
Каждый hit повторно проверяет разрешённую карту файлов и содержимое источников.
Кэш сокращает повторный rendering; сокращение source I/O и измеренный speedup
не заявляются. Worktrees и роли имеют отдельные ключи.

`context.compiled` содержит ключ, hit, hashes и EvidencePacket. SourceExcerpt
содержит полный BLAKE3 исходного файла, исходный byte range, completeness и
redacted text. Границы не раскрывают часть известного секрета. API
`expand_source` возвращает явные Current/StaleSource/AccessDenied/InvalidRange
и другие статусы; не расширяет grants. Семантический `claim_status` остаётся
UNKNOWN. Entailment и полноценная context compaction пока не реализованы.
Файловая работа вынесена в blocking worker; отмена прекращает ожидание, но
уже начатое ограниченное чтение может завершиться в фоне.

## Outbox

SQLite schema v3 сохраняет атомарность projection/event/original outbox и
добавляет immutable registrations и delivery на пару event/handler. Есть
lease/fence, bounded retry/backoff, dead letter, UNKNOWN и reconciliation.
Для неидемпотентного backend неоднозначный результат запрещает автоматический
повтор. NoEffect требует trusted attestation, точного ключа и attempt fence;
простое отсутствие эффекта на момент запроса не доказывает, что старый
attempt уже не сможет завершиться. Adapters — доверенный код.

CLI предлагает только безопасный local journal. Его эффект — SQLite запись
с unique idempotency key; event payload не превращается в команды или grants.
Journal размещается внутри state directory; внешние сообщения не отправляются.

```json
{
  "handler": "journal",
  "version": 1,
  "event_kinds": ["run_created"],
  "max_attempts": 3,
  "lease_ms": 10000,
  "backoff_ms": 100,
  "max_backoff_ms": 1000,
  "max_chain_depth": 2
}
```

```sh
harness outbox dispatch --policy policy.json --limit 100
harness outbox list journal
harness outbox reconcile --policy policy.json --event 123
```

Один tick ограничен 100 попытками. После регистрации policy/capabilities
не меняются; новая policy требует нового handler identity. Backend должен
сам доказать idempotency/reconciliation. Общее exactly-once для внешних
сервисов, capability protocol внешних plugins и fencing изменяющих run state
обработчиков этим local journal не подтверждаются.

## Исторический replay и планировщик

`harness replay RUN_ID` читает один согласованный SQLite snapshot через
READ_ONLY/no-follow connection. Он не создаёт state, не мигрирует schema,
не меняет permissions, не публикует CAS и не запускает provider/tools/outbox.
SQLite WAL может использовать shared-memory sidecars. Допускаются schema 2/3;
другие версии требуют отдельной миграции.

Replay проверяет state/candidate/generation projection и task hash, выводит
redacted journal и snapshot hash. Исторический VERIFIED не является новой
приёмкой. Нет authenticated event chain, полного восстановления всех
projections, повторной проверки исходников, snapshots/restore или GC.

Для некорректного свежего model-generated DAG записывается `plan.rejected`,
затем создаётся один fallback node в прежних write grants, covering все
исходные requirements. Accepted revision связывается с rejected plan hash.
Некорректный explicit или сохранённый DAG блокируется. Ошибки цикла и
отсутствующей зависимости диагностируются отдельно; права не расширяются.
