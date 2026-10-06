# Qlisa 1.5.12

## Русский

### Stage и управление шоу

- Добавлена тема Stage для работы во время шоу: компактный Cue List, независимо
  скрываемые панели, крупные кнопки транспорта, часы и локальные настройки
  раскладки и колонок.
- Панель Cue и меню физического вывода корректно располагаются в полноэкранном
  режиме. Удалён отдельный фон часов; повышен контраст цветов строк.
- Исправлены навигация стрелками и запуск GO после быстрого перемещения,
  размещение Number по первой выбранной строке и порядок его дочерних Cue.
- Группы и Number показывают линии иерархии. STOP обновляет состояние Number и
  его вложенных Cue.
- Исправлены прогресс Pre-Wait/Post-Wait, удержание последнего видеокадра во
  время перехода и русские подписи Auto-Continue, Auto-Follow и Do Not Continue.

### Предпросмотр медиа

- Play/Pause и перемотка управляют изображением и звуком предпросмотра вместе;
  кнопка наушников независимо включает и выключает звук.
- Частые перемотки остаются отзывчивыми. Предпросмотр запускает аудиопоток после
  готовности начального буфера. Кнопки видео остаются доступны, даже когда
  миниатюра прокручена за пределы Inspector.

### Монитор физического выхода

- Добавлен предпросмотр изображения с выбранного физического выхода с частотой
  до 30 кадров/с и разрешением до 640 × 360. Технические детали реализации и
  ограничения измерений приведены в [заметках Output Monitor](https://github.com/Ruslan-mad/Qlisa/blob/v1.5.12/docs/output-monitor.md).

### Конвертация и стабильность состояния

- Автозамена исходного медиа на результат конвертации применяется только по
  событию завершения работы конвертера. Восстановление состояния не запускает её
  повторно. Вложенные Video и Image находятся по UUID; запоздалый probe не
  перезаписывает уже изменённый источник.

### Проверки

- Прошли 494 frontend-теста, 3 сфокусированных Rust-теста и `pnpm tauri:check`.
- Локальные проверки интерфейса и рабочих сценариев зафиксированы в
  [чеклисте регрессии](https://github.com/Ruslan-mad/Qlisa/blob/v1.5.12/docs/BUGFIXES_NEXT.md).

## English

### Stage and show control

- Added the Stage theme for live operation, with a compact Cue List, independently
  collapsible panels, large transport controls, a clock, and saved layout and
  column preferences.
- Fixed Cue toolbar and physical-output menu layering in fullscreen. Removed the
  separate clock background and improved cue-row color contrast.
- Fixed arrow navigation and GO after rapid movement, Number placement at the
  first selected row, and child Cue ordering.
- Group and Number rows show hierarchy lines. STOP updates Number and nested Cue
  state.
- Fixed Pre-Wait/Post-Wait progress, held final video frames during transitions,
  and Russian labels for Auto-Continue, Auto-Follow, and Do Not Continue.

### Media preview

- Play/Pause and seek now control preview picture and sound together. The
  headphone button independently mutes or unmutes sound.
- Repeated seeks remain responsive. Preview starts its audio voice after the
  initial buffer is ready. Video controls stay active when the thumbnail is
  scrolled out of the Inspector viewport.

### Physical Output Monitor

- Added an image preview of the selected physical output at up to 30 frames per
  second and up to 640 × 360 resolution. See the [Output Monitor notes](https://github.com/Ruslan-mad/Qlisa/blob/v1.5.12/docs/output-monitor.md)
  for implementation details and measurement limits.

### Conversion and state stability

- Automatic replacement of source media with the converted output runs only on
  the converter's completion event. State restoration does not trigger it again.
  Nested Video and Image cues resolve by UUID, and a delayed probe cannot
  overwrite a source that has already changed.

### Checks

- 494 frontend tests, 3 focused Rust tests, and `pnpm tauri:check` passed.
- Local UI and workflow checks are recorded in the
  [regression checklist](https://github.com/Ruslan-mad/Qlisa/blob/v1.5.12/docs/BUGFIXES_NEXT.md).
