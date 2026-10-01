# Локальное измерение CLI — 2026-10-01

Release executable `harness 0.1.0` занимает **11 202 440 байт**. На проверенной Linux x86_64 среде запуск нового процесса `harness --version` с прогретой файловой системой дал:

| Метрика | Миллисекунды |
|---|---:|
| p50 | 2,116 |
| p95 | 2,508 |
| max | 2,751 |

Метод: 20 warmup и 200 последовательных измеряемых процессов, монотонные часы `time.perf_counter_ns`, nearest-rank percentile. В wall time входят spawn, CLI parsing, capture stdout и exit. После warmup executable и библиотеки могут находиться в файловом кеше. CPU/power profile не контролировался; это локальный контейнер, а не измерение пользовательского ноутбука.

Полные machine metadata, SHA-256 executable/source и сырые samples находятся в [startup-baseline.json](startup-baseline.json). Для повторения после release build:

```text
python3 tools/measure_startup.py target/release/harness --output startup-local.json
```

На Windows используйте `python` и `target/release/harness.exe`. Python нужен только этому вспомогательному benchmark script.

`--version` выходит до создания agent runtime. Это измерение не включает SQLite/Git, dispatch, модель, сборку или тесты проекта и не закрывает весь сценарий S40. Cold-cache startup, idle coordinator RSS, cancellation latency и сравнение на Windows/macOS здесь не измерялись. Результат другой машины сохраняется отдельно и не объявляется ускорением относительно этой.
