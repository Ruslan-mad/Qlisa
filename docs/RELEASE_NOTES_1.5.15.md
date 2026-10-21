# Qlisa 1.5.15

## Изменения

- Исправлено чтение коротких аудиопакетов: остаток пакета сохраняется между чтениями и предотвращает щелчки на границе буфера.
- Стерео-аудио сохраняет уровень каналов при центральном балансе. Моно и live-входы сохраняют прежнюю equal-power панораму.
- PNG сохраняет альфа-канал в слотах слоёв.
- Фоновая заставка Number защищена от обычной остановки. Кнопка «Паника» и Escape мягко останавливают текущий список; второй клик или нажатие Esc в течение 500 мс останавливает cue во всех списках сразу. Фоновая заставка сохраняется. Кнопка выбранного STOP и клавиша S останавливают один и тот же набор выбранных cue.
- GO выполняется через очередь. После успешного GO выбор и указатель GO переходят к следующему cue по результату запуска; проигнорированный, отменённый или неудачный запрос не меняет выбор.
- Active Cues компактно показывает Group и Number. Preview и Live — отдельные вкладки; Live следует последнему запущенному обычному медиа-cue. Фоновые cue доступны вверху списка выбора. GO и выбор cue не переключают вкладку.
- Устаревшая колонка «Выход» удалена. Колонка «Выходы», Pre-Wait и Post-Wait включены по умолчанию.
- Перетаскивание за заголовок окна работает; кнопка полноэкранного режима остаётся интерактивной. Полоса прогресса Active Cues стала заметнее. Кнопка «Паника» не меняет положение при изменении счётчика активных cue. Шкала геометрии центрируется на 100% и привязывает указатель к этому значению. Кнопки транспорта и панели cue получили общую анимацию нажатия.

## Проверки

- `pnpm test`: 543 теста в 75 файлах; `pnpm build` прошёл.
- `cargo test --features asio-support`: 1 047 тестов прошли, 12 пропущены.
- Браузерная проверка интерфейса подтвердила состояния Live, Panic, геометрии и Active Cues.

## English summary

Qlisa 1.5.15 fixes short audio packet reads, preserves centered stereo levels and PNG alpha, and protects background Number screensavers from ordinary stops. Panic and Escape use a soft stop followed by an immediate workspace stop on a second press within 500 ms. The selected Stop button matches the S shortcut. Queued GO advances selection only after a successful result. Active Cues, the separate Preview and Live tabs, cue-list columns, title bar, geometry scale, and transport buttons receive usability improvements.