# Output Monitor

Output Monitor показывает итоговую композицию выбранного физического display
output. Она берётся из существующего renderer; монитор не запускает второй
decoder и не захватывает desktop. Максимальный кадр предпросмотра — 640 × 360.
Browser Cue не входит в OpenGL-композицию и может отсутствовать в этом окне.

**Статус:** реализация и предварительные измерения доступны в исходниках после
1.5.11. Проверка не имеет статуса READY: финальные результаты ещё ожидаются.
Таблица ниже фиксирует отдельные прогоны, включая короткий трёхслойный smoke.
Она не подтверждает работу физического display scanout или аппаратуры.

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

## Предварительные измерения WebView

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

## Как запустить необязательный WebView benchmark

Запускайте из `src-tauri` с локальным видеофайлом. Аргументы: файл, длительность
в секундах, capture mode (`off`, `4` или `30`), число compositor layers (`1`–`3`)
и финальный маркер `webview`.

```powershell
$env:CARGO_BUILD_JOBS = "2"
cargo run --release --example output_monitor_bench --features asio-support -- "C:\media\test.mp4" 30 off 1 webview
cargo run --release --example output_monitor_bench --features asio-support -- "C:\media\test.mp4" 30 4 1 webview
cargo run --release --example output_monitor_bench --features asio-support -- "C:\media\test.mp4" 30 30 1 webview
```

Для сопоставимых Windows прогонов установите priority процесса в `BelowNormal`
и ограничьте его двумя логическими CPU; записывайте эти условия вместе с
результатом. `off` нужен как baseline с тем же видео и открытым WebView.
Повторите прогон несколько раз на каждой машине и отдельно запишите разрешение
источника, FPS, duration и количество слоёв. Не считайте один короткий smoke
тест финальным acceptance result. На других ОС опустите
`--features asio-support`.

JSON summary выводится одной строкой; лог WebView может также содержать текст
при завершении окна. Сохраните обе части лога. Benchmark создаёт временный
профиль Tauri, не используя обычный профиль оператора.

## Тесты и ограничения

В логах текущей проверки зафиксированы 488 frontend tests, успешный TypeScript
type check и 22 сфокусированных Rust tests для packet, monitor selection,
PBO/session ordering, cadence и mailbox. Rust прогон также содержит тесты
назначения физических выходов. Тесты покрывают формат, malformed payload,
channel conversion, текущую сессию, Canvas и ограничения layout. Это не
заменяет нагрузочный прогон или ручной осмотр на целевых Windows дисплеях.

Benchmark проходит через production renderer, mailbox, packet encoder, Tauri
IPC, WebView polling, Canvas conversion и frontend metrics. Он не измеряет
физический scanout, LED latency, UI presentation timestamp или GPU usage.
Browser Cue не является частью OpenGL композиции. CPU/RAM таблицы не включают
WebView process. Output Monitor не создаёт второй decoder, но пример может
запускать несколько media slots для измерения композиции.

Во время сборки 6 октября 2026 около 02:03 произошёл Windows BSOD `0x101`.
Точная причина неизвестна. Последующие ограниченные проверки завершились без
новой перезагрузки. Этот факт не устанавливает причинную связь между сборкой и
BSOD.

Проверка остаётся **WIP / не READY** до получения и просмотра финальных
результатов. Табличные измерения — наблюдения конкретных прогонов, не обещание
производительности и не готовность выпуска.
