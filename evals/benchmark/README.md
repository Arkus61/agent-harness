# Frozen benchmark: 30 различных Rust-задач

Корпус содержит Q01–Q10 × D1/D2/H1: 20 dev и 10 holdout-labelled задач.
Holdout виден разработчикам корпуса; это **не скрытый production holdout**.
Для каждой задачи есть отдельные baseline, requirements, внешний oracle и
исправленный контроль. Q08-D1 содержит настоящие 10 000 файлов; Q06 защищает
implementation и требует уничтожения трёх compiled mutants assertions.
Q03 имеет отдельный `syn` AST checker, не исполняющий candidate code.

```sh
python evals/benchmark/test_suite.py
python evals/benchmark/suite.py prepare --output artifacts/benchmark-fresh
```

Prepare не вызывает модель. Он создаёт committed seed, task JSON, manifest
и baseline/control receipts. Контроль должен проходить, baseline — проваливать
независимый критерий. Для каждого из 30 случаев итоговый локальный freeze
версии 0.1.3 подтвердил это условие. Исторические неуспешные подготовительные
прогоны сохранены отдельно.

Linux oracle использует настоящий bubblewrap: отдельные namespaces, закрытую
сеть, чистый env, read-only system/toolchain и candidate/oracle source mounts.
Writable только disposable sandbox и target dirs. Host credentials не
монтируются. Candidate checks выполняются до создания external oracle;
их failure не маскируется успешным external contract. Lockfile защищён,
build configuration/links/path escapes в archive отклоняются. Это не
формальное доказательство против произвольного вредоносного Rust-кода,
выполняемого внутри test process.

```sh
harness --repo artifacts/benchmark-fresh/seeds/Q02-D1 run \
  --task artifacts/benchmark-fresh/tasks/Q02-D1.json
python evals/benchmark/suite.py accept --task-id Q02-D1 \
  --repo artifacts/benchmark-fresh/seeds/Q02-D1 --candidate EXACT_SHA \
  --output artifacts/benchmark-fresh/acceptance.json
```

Для повторов нужны свежие seeds и три настоящих запуска на configuration.
Одинаковый ограниченный budget закреплён в TaskSpec. H доступен; B0/B1 ещё
требуют реальных ablation modes и runner. Их нельзя подменять тем же H.
Полный comparative protocol — 270 свежих runs; prepare этого не выполняет.

Q08 дополнительно требует context evidence; Q09 — настоящие memory/skill
setup hooks, Q10 — сравнимые измерения стратегий и выбор по hard constraints.
Пока hooks/evidence не выполнены, успешный code oracle сохраняет
`scenario_status=NOT_PROVED`. Поданные произвольные JSON evidence не повышают
вердикт. Test run и fixture controls не являются оценкой качества модели.

Требуются Rust 1.99.0, offline toolchain/cache и Linux bubblewrap. На других
ОС `prepare/accept` не понижают профиль до native. Cross-platform CI отдельно
проверяет portable controller и AST tests; Linux job проверяет все controls.
