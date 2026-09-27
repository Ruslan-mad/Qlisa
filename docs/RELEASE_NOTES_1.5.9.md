# Qlisa 1.5.9

## English

### Playback and outputs

- Physical display output windows no longer appear in the Windows taskbar.
- Image media probing skips audio decoding, including WebP: its RIFF container is no longer sent through the audio decoder or libmpv audio fallback. Failed audio decoding now logs the fallback failure with its reason.
- Audio playback now keeps whole-file, trimmed, and sliced WAV/MP3 loops continuous. Trimmed stream refills retain the packet tail, and start-only loops use the decoded source end.
- Video loops preserve the trim start and end on every repeat through the existing libmpv A–B loop path. Full-file loops and sliced loops keep their existing behavior.
- Number timeline waveforms and duration follow the effective audio trim range. Clearing trim restores the full-file waveform.
- A seek rebuffer is no longer classified as an audio underrun, so it does not show the yellow underrun warning. Genuine underruns remain reported.
- Streaming Audio Cue GO waits for the existing source-readiness threshold before starting its playback and action clocks. The callback does not wait for decoding.
- The main App mount waits for backend startup readiness. The readiness flag is registered before engine startup and set when setup finishes; initial project opens continue through the existing pending queue. If readiness takes over 60 seconds, the bootstrap screen offers Retry. This startup change is separate from Audio Cue stream readiness. Cold launch and second-instance project-open checks during setup passed without a `state-not-managed` error.

### SRT diagnostics

- SRT audio queue diagnostics now include queue depth, producer/consumer rates, blocked pipe-write time, drop counts, and sample-clock context. Reports help distinguish an unconnected receiver from a sender pacing problem.
- A connected local FFmpeg receiver test completed 1,800.269 seconds with concurrent video and audio. The app logged zero audio underruns; the cumulative queue drop count did not increase during the connected run, and the receiver decoded audio and video through clean shutdown. The real-app run used virtual Windows WASAPI CABLE-A at 48 kHz. It did not test physical audio hardware. Drops accumulated before the receiver connected remain in the lifetime counter.
- The test harness now keeps a live caller connected through FFmpeg input probing. This fixes a test-harness forced-disconnect failure; it does not establish that production sender pacing was fixed.

### Project and runtime files

- New projects and Save As continue to use `.qlisa`; existing `.qlisa` and `.inkue` projects remain supported.
- Runtime components are fetched from their pinned upstream sources and verified by SHA-256. The signed updater continues to use GitHub Releases.

## Русский

### Воспроизведение и выходы

- Окна физических видео-выходов больше не отображаются на панели задач Windows.
- При анализе изображений приложение пропускает декодирование аудио. Контейнер WebP RIFF больше не попадает в аудиодекодер и fallback через libmpv. Если декодирование аудио не удалось, журнал теперь содержит причину сбоя fallback.
- Циклическое воспроизведение WAV и MP3 сохраняет непрерывность для полного файла, обрезанных фрагментов и нарезанных сегментов. При повторном чтении обрезанного потока сохраняется конец декодированного пакета, а цикл с заданным началом использует фактический конец декодированных данных.
- Циклы Video сохраняют начало и конец обрезки при каждом повторе через существующий цикл A–B в libmpv. Поведение циклов всего файла и нарезанных сегментов не менялось.
- Waveform и длительность аудио в таймлайне Number учитывают границы обрезки. Сброс обрезки возвращает Waveform всего файла.
- Буферизация после перемотки больше не считается аудио underrun и не включает жёлтое предупреждение. Настоящие underrun по-прежнему регистрируются.
- При запуске потокового Audio Cue команда GO ждёт готовности источника по существующему порогу. От этого момента отсчитываются воспроизведение и длительность Cue; аудиопоток не ждёт декодирования.
- Основной интерфейс App монтируется после сигнала готовности backend. Флаг готовности регистрируется до запуска движков и устанавливается после завершения setup; начальное открытие проекта продолжает использовать существующую очередь. Если готовность не наступила за 60 секунд, экран запуска предлагает повторить попытку. Это исправление отдельно от ожидания готовности аудиопотока для Audio Cue. Проверки холодного запуска и открытия проекта через второй процесс во время setup прошли без ошибки `state-not-managed`.

### Диагностика SRT

- Диагностика очереди аудио SRT показывает её размер, скорость заполнения и чтения, время блокировки записи в канал, число потерянных кадров и позиции аудиосчётчика. Это помогает отличать неподключённый приёмник от проблемы с темпом отправки.
- Локальный тест с подключённым приёмником FFmpeg завершился после 1 800,269 секунды одновременного видео и аудио. Приложение зарегистрировало ноль аудио underrun; счётчик потерь очереди не увеличивался, а приёмник декодировал аудио и видео до штатного завершения. Реальное приложение использовало виртуальное устройство Windows WASAPI CABLE-A с частотой 48 кГц. Физическое аудиооборудование не проверялось. Потери, накопленные до подключения приёмника, остаются в общем счётчике.
- Тестовый сценарий теперь сохраняет подключение работающего клиента на время анализа входа FFmpeg. Это исправляет принудительное отключение клиента самим тестом; результат не доказывает исправление темпа работы production-отправителя.

### Проекты и runtime

- Новые проекты и команда Save As используют `.qlisa`; проекты `.qlisa` и `.inkue` по-прежнему поддерживаются.
- Компоненты runtime загружаются из закреплённых upstream-источников и проверяются по SHA-256. Подписанные обновления приложения по-прежнему распространяются через GitHub Releases.
