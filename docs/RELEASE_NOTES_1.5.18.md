# Qlisa 1.5.18

## Изменения

- Добавлены начальные настройки «Чёрные поля» по типу файла. Для видео и JPG/JPEG/BMP они включены; для PNG, WebP, GIF и SVG — выключены. Настройка применяется к новым видео и при первом назначении файла пустому изображению. Существующие значения cue сохраняются.
- Обновлена панель транспорта. Четырёхсекторный переключатель открывает панель cue, Active Cues, Inspector или нижний dock; центральная кнопка открывает и закрывает монитор выходов. Увеличены области нажатия.
- Диапазон главного регулятора расширен до +6 dB. Перетаскивание легко возвращает его к 0 dB; шаг клавиатуры остаётся 0,5 dB.
- Добавлен числовой peak readout для левого и правого каналов. Он показывает максимальный sample peak, сохраняет статус CLIP до ручного сброса и повторно фиксирует его при новых событиях, даже если уровень не изменился. Измеритель суммирует программу на основном устройстве после master gain и исключает preview. Он не суммирует auxiliary outputs и не измеряет true peak.
- Для кнопок FTB добавлена анимация нажатия. Поведение FTB не изменено.

## English summary

Qlisa 1.5.18 adds per-media-type Black Bars defaults, a transport panel switcher with an Output Monitor toggle, a +6 dB master range with pointer snap at unity, and latched per-channel sample-peak readouts. The meter reports summed post-master program output on the main device; it excludes preview audio, does not aggregate auxiliary outputs, and is not a true-peak meter. FTB buttons now show press feedback without changing their behavior.
