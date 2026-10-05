# Qlisa 1.5.10

## English

### Playback and Number timeline

- Enabling Loop for Audio or Video in the Time tab now starts infinite looping. The Time tab's infinity control switches between infinite looping and one finite extra repeat; the repeat count remains configurable. The compact Basics Loop control toggles between infinite looping and off. Infinite Audio and Video cues show `∞` in the cue-list duration column. Finite loops keep their calculated duration.
- Number timelines include finite Audio and Video repeats. A `loop_count` of N means N extra plays, for N + 1 total passes; infinite loops fill the remaining Number duration. Finite repeats of an Audio or Video master extend the Number duration.
- Trimmed Number waveforms map bins against the full source duration and use absolute source time. This keeps silent source tails from stretching the visible crop. A partial final pass displays the matching prefix of the source crop. The Number waveform continues to reuse full-file peaks when trim changes.

### Outputs and Inspector

- Preferences → Display can select enabled physical displays for newly created Video, Image, Camera, and Text cues. Browser cues use only the first selected display because they use one exclusive WebView. If no selected display is enabled, new cues use the regular default output.
- Video, Image, and Camera cues can store separate geometry per output destination. An output without an override uses the cue-level geometry. Overrides remain saved when routing is unchecked; newly selected destinations use cue-level geometry until edited. Fit changes preserve that output's crop and position.
- The Basics Inspector has compact Loop, mute, Continue, and per-output Fit controls. The same controls support single and multi-cue selection. Multi-cue actions appear only when all selected cues support them; mixed values are shown. Loop edits require all selected cues to be safe to rebuild. Fit applies to cues routed to that output and preserves their other geometry.
- Mute is saved on Audio, Video, and Camera cues. It silences the cue while preserving its authored levels, fades, and playback position.
- Inspector edits save on blur, including when a background click clears selection. Memo text in Basics, the Memo tab, and the cue-list Notes column uses the same `memo_text` value.
- Panic Stop clears cue state across every cue list.

### Preview cache

- Waveform, thumbnail, and video filmstrip previews persist in a project-scoped sidecar cache. Unsaved workspaces use a private cache; valid entries are copied to the project cache on save, Save As, and Collect and Save. Cache data is disposable and does not contain source media or decoded PCM. Cache transfer is best-effort and does not block saving.
- Cache keys include source identity, file size, modification time, cache format version, and request parameters. Missing, damaged, stale-version, or unwritable entries are treated as cache misses. Each cache scope prunes its own `.qcache` entries above 256 MiB toward 224 MiB; unrelated files are left alone.

### Validation

- All 436 frontend tests passed.
- `pnpm tauri:check` passed.
- Regression coverage includes finite Video repeats in the Number timeline.
- The latest build was reviewed manually.

## Русский

### Воспроизведение и таймлайн Number

- При включении Loop для Audio или Video во вкладке Time теперь запускается бесконечный цикл. Кнопка `∞` во вкладке Time переключает бесконечный цикл и один конечный дополнительный повтор; число повторов можно настроить отдельно. Компактная кнопка Loop в Basics переключает бесконечный цикл и выключенное состояние. В колонке длительности Cue для бесконечного Audio или Video показывается `∞`. Для конечных циклов отображается рассчитанная длительность.
- Таймлайн Number учитывает конечные повторы Audio и Video. Значение `loop_count` N означает N дополнительных повторов, то есть всего N + 1 проходов; бесконечный цикл заполняет оставшуюся длительность Number. Конечные повторы Audio- или Video-мастера увеличивают длительность Number.
- Waveform обрезанного медиа в Number сопоставляет bins с полной длительностью исходного файла и использует абсолютное время источника. Поэтому тихий хвост файла не растягивает видимую обрезку. В неполном последнем проходе показывается соответствующее начало обрезанного фрагмента. При изменении обрезки Number продолжает использовать уже загруженные peaks полного файла.

### Выходы и Inspector

- В Preferences → Display можно выбрать включённые физические дисплеи для новых Cue Video, Image, Camera и Text. Browser использует только первый выбранный дисплей, потому что для него создаётся одно эксклюзивное окно WebView. Если ни один выбранный дисплей не включён, новый Cue использует обычный выход по умолчанию.
- Для Video, Image и Camera можно задать отдельную геометрию для каждого выходного назначения. Если для выхода нет переопределения, используется геометрия Cue. Переопределения сохраняются после отключения маршрутизации; новый выход использует геометрию Cue, пока оператор её не изменит. Изменение Fit сохраняет обрезку и позицию этого выхода.
- В Basics Inspector добавлены компактные настройки Loop, звука, Continue и Fit по выходам. Они доступны для одного или нескольких выбранных Cue. Настройка для нескольких Cue появляется, только если действие поддерживают все выбранные Cue; смешанные значения отмечаются отдельно. Изменение Loop доступно, только если все выбранные Cue можно безопасно пересоздать. Fit применяется к Cue, направленным в этот выход, и сохраняет остальные параметры геометрии.
- Mute сохраняется в Audio, Video и Camera Cue. Он отключает звук Cue, сохраняя заданные уровни, затухания и позицию воспроизведения.
- Изменения Inspector сохраняются при потере фокуса, в том числе когда щелчок по фону снимает выделение. Текст Memo в Basics, вкладке Memo и колонке Notes списка Cue использует одно поле `memo_text`.
- Panic Stop очищает состояние Cue во всех списках.

### Кэш предпросмотра

- Waveform, миниатюры и видеокадры для filmstrip сохраняются в отдельном кэше проекта. Для несохранённого проекта используется приватный кэш; действующие записи копируются в кэш проекта при сохранении, Save As и Collect and Save. Кэш можно удалить: в нём нет исходных медиафайлов и декодированного PCM. Копирование кэша не блокирует сохранение и может не выполниться.
- Ключи кэша учитывают идентификатор источника, размер файла, время изменения, версию формата и параметры запроса. Отсутствующая, повреждённая, устаревшая или недоступная для записи запись считается промахом кэша. Каждый кэш удаляет только собственные `.qcache` записи при превышении 256 МиБ, сокращая объём примерно до 224 МиБ; остальные файлы не затрагиваются.

### Проверки

- Пройдены все 436 frontend-тестов.
- `pnpm tauri:check` завершилась успешно.
- Добавлена регрессионная проверка конечных повторов Video в таймлайне Number.
- Последняя сборка проверена вручную.
