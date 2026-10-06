# Output Monitor

Output Monitor показывает итоговую композицию выбранного физического display
output. Она берётся из существующего renderer; монитор не запускает второй
decoder и не захватывает desktop. Максимальный кадр предпросмотра — 640 × 360.
Browser Cue не входит в OpenGL-композицию и может отсутствовать в этом окне.

**Статус:** ПО готово к локальному review после успешного 300-секундного прогона
и проверок UI/native окна. Это не подтверждает готовность к event use: реальное
аудио, многовыходной 60 FPS прогон и физический display scanout не проверены.
Изменения относятся к исходникам после 1.5.11 и не меняют статус публикации.

## Изменение пути кадра

| Часть | Предыдущий путь | Текущий путь |
| --- | --- | --- |
| IPC кадра | Rust упаковывал каждый новый кадр в BMP и base64 data URL внутри JSON. Это добавляло кодирование и временную копию данных. | Rust возвращает бинарный `ArrayBuffer` с фиксированным заголовком и BGRA payload. |
| UI | React сохранял data URL как frame state и отображал изображение. | Один Canvas сохраняет последний кадр. UI проверяет бинарный пакет и рисует в повторно используемый `ImageData`. |
| GPU readback | Уже использовался асинхронный PBO путь; он не менялся ради формата IPC. | Существующий трёхслотовый PBO/fence путь сохраняет readback вне блокирующего ожидания. |
| Selection | Устаревшие ответы могли пережить быстрое переключение источника. | Session token связывает выбор, кадр и Canvas; backend и frontend отбрасывают устаревшую сессию. |
| Измерения | Измерения только renderer/mailbox/encoder не включали Tauri IPC и UI. | Необязательный WebView прогон использует production окно, команды IPC, Canvas conversion и frontend sampling. |

## Файлы и границы ответственности

- `src/windows/OutputMonitorWindow.tsx` — окно, события видимости и выбора,
  один запрос кадра за раз, Canvas и сбор frontend метрик.
- `src/windows/outputMonitorModel.ts` — парсер пакета, session/sequence gates,
  cadence, метрики и преобразование BGRA в RGBA.
- `src/windows/outputMonitorModel.test.ts` — тесты пакета, cadence, устаревших
  ответов, метрик и Canvas.
- `src-tauri/src/commands/preferences_cmds.rs` — список физических выходов,
  session selection, бинарное кодирование и backend diagnostics.
- `src-tauri/src/engine/output_engine/mod.rs` — интерфейс кадра и mailbox.
- `src-tauri/src/engine/output_engine/render.rs` — финальный compositor FBO,
  PBO/fence readback, отбор кадра и публикация.
- `src-tauri/examples/output_monitor_bench.rs` — native и необязательный WebView
  benchmark на production окне, IPC-командах и Canvas.

## Текущий путь кадра

Renderer рисует в итоговую текстуру выбранного выхода, включая warp и fade
слои. Когда монитор включён, renderer захватывает уменьшенный финальный кадр с
целью до 30 FPS, в том числе при статичном выходе. Анимированный выход сохраняет
свои более частые пробуждения renderer. Когда монитор закрыт, действует обычный
idle timeout.

GPU readback пишет BGRA в один из трёх PBO и ставит GPU fence. Renderer не ждёт
fence: при следующем проходе он проверяет только готовые слоты, выбирает самый
новый кадр текущей сессии и отбрасывает более старые готовые кадры. За проход
мапится не более одного PBO. Если свободного слота нет, попытка пропускается;
renderer не задерживает показ кадра ради медленного монитора. Строки
переворачиваются в top-down при копировании CPU кадра в mailbox.

Mailbox хранит последний `Arc` snapshot. Публикация использует `try_lock`; если
mailbox занят, кадр пропускается вместо ожидания. Capture sequence нумерует
попытки, поэтому пропуски sequence допустимы. Номер сравнивается с учётом
переполнения `u64`.

Frontend выполняет один `invoke` за раз. После завершения запроса следующий
начинается после остатка интервала 33,33 ms или сразу, если запрос уже занял
больше времени. Canvas сохраняет последний нарисованный кадр на ответе
`unchanged`; ответ `black` очищает его в чёрный цвет.

## Почему пакет хранит BGRA

OpenGL readback запрашивает каналы в BGRA. Backend сохраняет этот порядок в
пакете и не делает ещё одну CPU перестановку каналов перед IPC. Canvas API
принимает RGBA, поэтому frontend переставляет красный и синий каналы при
подготовке `ImageData`. Буфер `ImageData` повторно используется для той же
разрешающей способности. Пиксели пакета всегда top-down.

## Session и выбор источника

Каждый выбор источника получает возрастающий token. Backend отклоняет команду
с более старым token и возвращает для кадра session. Frontend применяет пакет,
только если session совпадает с активным выбором и sequence не старше уже
нарисованного кадра. Generation token дополнительно инвалидирует результат
после смены вкладки или закрытия окна. Эти gates не дают запоздавшему ответу
предыдущего display заменить актуальный кадр.

## События и готовность окна

- `output-monitor-opened` включает окно и запускает перечитывание списка
  источников.
- `workspace-modified` и `preferences-updated` перечитывают список выходов.
- Смена видимости документа или выбранной вкладки останавливает текущий polling,
  сбрасывает generation и освобождает backend selection новым token.
- Закрытие окна останавливает timer и capture; повторное открытие создаёт новую
  сессию.
- Benchmark повторяет `output-monitor-opened` раз в 250 ms, пока не получит первый
  frontend diagnostic sample или пока не истекут 2 секунды. Так прогон не
  принимает отсутствие зарегистрированного UI listener за нулевую нагрузку.
- Во время активного benchmark frontend присылает диагностический sample раз в
  секунду. `activeSamples` — число активных секундных выборок, использованное
  для среднего FPS.

## Бинарный пакет версии 1

Команда возвращает `tauri::ipc::Response::new(Vec<u8>)`; frontend получает
`ArrayBuffer`. Все целые числа little-endian. Фиксированный header занимает 64
байта.

| Offset | Size | Поле |
| ---: | ---: | --- |
| 0 | 4 | ASCII magic `QLMF` |
| 4 | 2 | Версия: `1` |
| 6 | 2 | Размер header: `64` |
| 8 | 1 | Status: `0` нет кадра, `1` кадр, `2` без изменений, `3` чёрный кадр |
| 9 | 1 | Pixel format: `1` BGRA8 |
| 10 | 2 | Reserved; должен быть `0` |
| 12 | 4 | Ширина в пикселях |
| 16 | 4 | Высота в пикселях |
| 20 | 4 | Stride в байтах |
| 24 | 8 | Capture sequence |
| 32 | 8 | Selection session token |
| 40 | 8 | Время захвата, Unix microseconds |
| 48 | 8 | Время ответа, Unix microseconds |
| 56 | 4 | Время подготовки backend пакета, microseconds |
| 60 | 4 | Размер payload в байтах |

Payload начинается с offset 64. Status `frame` содержит плотно упакованные
top-down BGRA8 пиксели: `stride = width × 4`, `payload length = stride × height`.
Status `black` содержит размеры и stride без payload. Статусы `no frame` и
`unchanged` имеют нулевые размеры, stride и payload. `unchanged` сохраняет
последние capture sequence и timestamp. Парсер проверяет полный header и длину
payload до создания pixel view.

## Измерения WebView

Логи: `tmp/monitor-validation/monitor-video-off-webview.log`,
`monitor-video-4-webview.log`, `monitor-video-30-webview.log` и
`monitor-three-layer-smoke.log`. Прогоны использовали `CARGO_BUILD_JOBS=2`, два
логических CPU (affinity mask `0x3`) и priority `BelowNormal`. Видео для трёх
однослойных режимов: один активный 1280 × 720 источник, 30 FPS. RAM приведена
как native process working set, MiB; формат значений: начало / пик / конец.

| Режим | Время, слоёв | Renderer capture FPS | UI displayed FPS | Активных UI samples | Frame age среднее / максимум, ms | Skipped PBO | CPU native process среднее / максимум | RAM MiB: начало / пик / конец |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| OFF, baseline | 30 s, 1 | 0.00 | — | 0 | — | 0 | 0.93% / 1.33% | 343.9 / 347.4 / 347.4 |
| 4 FPS | 30 s, 1 | 4.02 | 3.99 | 29 | 39.1 / 73.0 | 0 | 0.92% / 1.56% | 406.6 / 411.1 / 410.4 |
| 30 FPS | 30 s, 1 | 30.01 | 26.52 | 29 | 38.6 / 75.0 | 0 | 1.53% / 2.26% | 348.6 / 357.9 / 354.2 |
| 30 FPS, 3 слоя | 15 s smoke | 30.04 | 29.29 | 15 | 24.4 / 47.0 | 0 | 3.28% / 4.06% | 684.4 / 732.0 / 710.5 |

OFF — baseline: окно и один источник видео остаются запущены, но monitor capture
выключен. Поэтому capture FPS равен нулю, а активных UI samples нет; UI FPS и
frame age для baseline не измерялись. В режимах 4 и 30 монитор получил 120 и
793 кадра соответственно. Средняя частота показа ниже capture частоты на 30 FPS
прогоне.

CPU и RAM относятся только к native benchmark process. Они не включают дочерний
WebView процесс. CPU нормализован по доступной процессу параллельности.
Использование GPU не измерялось. Frame age — время от timestamp capture внутри
Qlisa до обработки пакета frontend, не задержка до физического экрана. Renderer
capture FPS и UI displayed FPS — отдельные счётчики.

На однослойных прогонах video runtime на снимках начала и конца показывал один
загруженный слот, 1280 × 720 и 30 FPS. Для трёх слоёв было три слота 1920 × 1080
и 60 FPS. Счётчики mpv — снимки, а не гарантированная сумма за весь запуск:
трёхслойный прогон показывает 10 dropped frames на старте и 0 в конце после
сброса счётчика. Ноль в конечном снимке не означает ноль dropped frames за весь
прогон. Трёхслойный результат — 15-секундный smoke test, не длительный стресс
тест и не критерий готовности.

## Native и browser проверки

**Native validation harness:** журнал `monitor-native-validation-final.log`
фиксирует успешные проверки RGB red/green/blue, white и black; master fade и
FTB; 10 циклов A→B→A; 10 циклов disable/re-enable; отклонение stale selection
token. Output A измерен как 480 × 320 с ratio 1.5. Для B запрашивался portrait,
но фактически получено 640 × 350 (`geometryMatch=false`). Проверка portrait
layout в browser UI пройдена выше; native portrait geometry остаётся
непроверенной.

**Native window reopen:** `monitor-native-window-reopen.log` записывает два
повторных открытия окна за 90 секунд: одно через GUI close, второе через
Escape. За тест показано 83 кадра (`displayedFpsAverage=0.9725`). Каждый IPC
ответ намеренно задерживался на 1000 ms, поэтому частота около 1 FPS вызвана
тестовой задержкой и не является оценкой производительности. Harness
использовал настоящий `OutputMonitorWindow` и native engine, но IPC commands
были test adapters, разделявшие production packet encoder. Это не полный
`AppState`, не реальный audio path и не проверка физической аппаратуры.

Native floating geometry не прошла portrait assertion. Для A запрос 320 × 180
упирался в минимальную высоту окна 240; фактический client area был примерно
320 × 212. Для B запрошенная portrait geometry отобразилась landscape. Здесь
учтены фактические размеры; исправления engine не вносились.

**Browser UI с IPC mock:** проверялось реальное React окно Output Monitor при
эмулированных ответах IPC. Portrait источник 202 × 360 в viewport 1280 × 720
получил Canvas 350.125 × 624; в viewport 400 × 300 — 114.45 × 204. Кадр
160 × 90 правильно масштабировался к максимуму 640 × 360. `black` очистил
предыдущий blue кадр, `no_frame` очистил Canvas, вкладка Preview отключила
backend source. Console errors: 0. Это подтверждает обработку UI-пакетов, не
portrait geometry renderer или реальные кадры устройства.

**Production app IPC smoke:** debug `qlisa.exe` был запущен с отдельным
`APPDATA` profile `tmp/stage-native-qa-profile`; общий Runtime cache оставался
доступен. Output Monitor открыли через View menu, выбрали source Main;
реальный AppState и production commands вернули black status, а окно показало
«На выходе чёрный экран». Скрытие окна через GUI сработало. Этот smoke проверил
интеграцию production IPC и состояние black; он не запускал media playback и
не подтверждает реальное физическое видео или аудио.

**Трёхслойный стресс-прогон:** 3 × 1920 × 1080 при 60 FPS, два логических CPU,
priority `BelowNormal`. OFF длился 30 s; ON длился 60 s. Измерения относятся к
запущенному benchmark process; GPU usage не измерялось.

| Режим | Длительность | CPU process | RSS начало → конец, MB | RSS пик, MB | Preview FPS | Frame age среднее / максимум, ms | Skipped PBO | Dropped stale | Максимум droppedFrames mpv |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| OFF | 30 s | 2.665% | 728 → 778 | — | — | — | — | — | 630 |
| ON | 60 s | 3.388% | 729 → 798 | 808 | 29.515 | 39.676 / 61 | 0 | 1 | 677 |

В OFF baseline droppedFrames уже достигал 630. ON зафиксировал максимум 677;
поэтому эти прогоны не подтверждают нулевые drops и не дают оснований
объявлять safe event use. PBO не пропускались; один кадр был отброшен как stale.
Это отдельные ограниченные прогоны, не длительная event-ready проверка.

**Длительный прогон:** проверка 3 × 1280 × 720 при 30 FPS длительностью
300 секунд завершилась с exit code 0. Нагрузка шла на двух логических CPU с
priority `BelowNormal`; pressure guard не сработал, новых перезагрузок не было.

| Длительность, layers | Renderer capture FPS | Canvas displayed FPS | Active samples | Frame age average / max, ms | Conversion average / max, ms | IPC request average / max, ms | Published / displayed frames | PBO skips / stale / mailbox drops | mpv dropped snapshots: start / max observed / end | CPU average / max | Native RSS: start / peak / end, MB |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| 300 s, 3 × 1280 × 720 @ 30 FPS | 30.001 | 29.449 | 297 | 39.457 / 62 | 0.507 / 2.6 | 7.788 / 23 | 9028 / 8876 | 0 / 0 / 0 | 2 / 2 / 0 | 2.796% / 3.513% | 645.2 / 678.8 / 673.7 |

Показания счётчика пропущенных кадров mpv — моментальные снимки, а не
накопительный итог. Счётчик показывал 2 в начале, достигал 2 во время прогона
и после сброса показывал 0 в конце. Отложенных кадров не зафиксировано. Эти
значения не доказывают, что за всё время воспроизведения пропусков не было.

Отдельный журнал процессов фиксировал память после прогрева. RSS основного
процесса вырос с 664,9 MB на 30,6-й секунде до 673,6 MB на 302-й секунде
(+8,7 MB); выделенная частная память снизилась с 2029,1 до 2017,3 MB. Шесть
дочерних процессов WebView использовали 411,4 MB RSS на 30,6-й секунде,
достигли пика 438,4 MB и завершили прогон с 392,2 MB. Их общая выделенная
частная память составляла 215,8 MB на 30,6-й секунде, достигала 241,8 MB и
завершила прогон на 192,2 MB. Пятиминутный прогон не заменяет длительную
проверку перед мероприятием и не подтверждает работу реального аудио или
выходных устройств.

## Как запустить необязательный WebView benchmark

Запускайте из `src-tauri` с локальным видеофайлом. Аргументы: файл, длительность
в секундах, capture mode (`off`, `4` или `30`), число compositor layers (`1`–`3`)
и маркер режима (`webview`, `checks` или `webview-checks`).

```powershell
$env:CARGO_BUILD_JOBS = "2"
$benchProcess = [System.Diagnostics.Process]::GetCurrentProcess()
$benchProcess.ProcessorAffinity = [IntPtr]3
$benchProcess.PriorityClass = [System.Diagnostics.ProcessPriorityClass]::BelowNormal
cargo build --example output_monitor_bench --features asio-support
$bench = ".\target\debug\examples\output_monitor_bench.exe"
& $bench "C:\media\test.mp4" 30 off 1 webview
& $bench "C:\media\test.mp4" 30 4 1 webview
& $bench "C:\media\test.mp4" 30 30 1 webview
& $bench "C:\media\test.mp4" 30 30 1 checks
& $bench "C:\media\test.mp4" 30 30 1 webview-checks
& $bench "C:\media\stress-720p.mp4" 300 30 3 webview
```

Команды задают priority `BelowNormal`, affinity mask `3` (два логических CPU) и
`CARGO_BUILD_JOBS=2` до запуска CLI. Сначала собирается debug example один раз;
затем исполняемый файл запускается напрямую для OFF/4/30 FPS и native checks.
Эти PowerShell ограничения действуют только в текущем процессе/session; они не
меняют постоянные настройки машины. Не используйте `--release` для этой
ограниченной проверки. `off` нужен как baseline с тем же видео и открытым
WebView. Записывайте условия вместе с результатом.
Повторите прогон несколько раз на каждой машине и отдельно запишите разрешение
источника, FPS, duration и количество слоёв. Не считайте один короткий smoke
тест финальным acceptance result. На других ОС опустите
`--features asio-support`.

JSON summary выводится одной строкой; лог WebView может также содержать текст
при завершении окна. Сохраните обе части лога. Benchmark создаёт временный
профиль Tauri, не используя обычный профиль оператора.

## Тесты и ограничения

В логах текущей проверки зафиксированы 488 frontend tests, успешный TypeScript
type check, итоговый `pnpm tauri:check` и 22 сфокусированных Rust tests для
packet, monitor selection, PBO/session ordering, cadence и mailbox. Rust прогон также содержит тесты
назначения физических выходов. Тесты покрывают формат, malformed payload,
channel conversion, текущую сессию, Canvas и ограничения layout. Это не
заменяет нагрузочный прогон или ручной осмотр на целевых Windows дисплеях.

Benchmark проходит через production renderer, mailbox, packet encoder, Tauri
IPC, WebView polling, Canvas conversion и frontend metrics. В минимальном
benchmark App команды являются test adapters; production encoder и renderer
используются, но полный AppState и реальный audio path не поднимаются. Он не
измеряет физический scanout, LED latency, UI presentation timestamp или GPU usage.
Browser Cue не является частью OpenGL композиции. CPU/RAM таблицы не включают
WebView process. Output Monitor не создаёт второй decoder, но пример может
запускать несколько media slots для измерения композиции.

Во время сборки 6 октября 2026 около 02:03 произошёл Windows BSOD `0x101`.
Точная причина неизвестна. Последующие ограниченные проверки и 300-секундный
прогон завершились без новой перезагрузки. Эти факты не указывают причину сбоя.

ПО: **READY для локального review** по имеющимся UI/native validation и
300-секундному трёхслойному прогону. Event/hardware readiness: **NOT CONFIRMED**.
Для этого нужен отдельный запуск с реальным аудио, несколькими видеовыходами при
60 FPS и обычными настройками машины. Результаты не являются готовностью выпуска
или обещанием производительности.
