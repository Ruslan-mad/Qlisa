# Чеклист следующих исправлений / Follow-up regression checklist

These changes are in the source after the 1.5.11 baseline. This file records
manual checks to run; an unchecked item is not a pass. / Эти изменения внесены
в исходный код после версии 1.5.11. Здесь записаны ручные проверки; пустой
чекбокс не означает, что проверка пройдена.

## Manual checks / Ручные проверки

## Stop nested cue state / Состояния вложенных cues после STOP

- [ ] Stop a running Number that contains a nested Group with running Audio
  and paused Video cues. Confirm the parent and every descendant publish their
  real state transition to Standby, and the Cue List refresh removes stale
  active rows and timing indicators. Confirm an unrelated running cue stays
  active.
- [ ] Repeat STOP when the target is already in Standby. Confirm the command
  still refreshes the Cue List so stale frontend state clears.
- [ ] Use soft STOP All and hard STOP All with nested cues. Confirm each
  changed descendant receives a state event and no unrelated cue is reported
  as stopped.

- [ ] **Stage theme / Тема Stage.** Finish the profile migration, save status,
  output binding, transport, and column preference checks in [the Stage theme
  notes](flagship-theme.md). Browser UI QA passed for independent panel
  visibility/persistence and the 1280 px layout with a local IPC fixture. This
  does not validate physical transport, fullscreen, or window controls.

- [ ] **Output Monitor / Монитор выходов.** Select each physical output in
  turn, switch between outputs repeatedly, close and reopen the monitor, and
  confirm an older selection never replaces the current preview. Check frame,
  black, unchanged, and no-frame responses. Confirm the Canvas keeps the last
  image for unchanged responses, follows aspect ratio after output resolution
  changes, and clears on black. Check that polling has at most one invoke in
  flight and stops after closing the monitor. Run the native `off`, `4`, and
  `30` benchmark modes from `src-tauri` as described in
  [Output Monitor](output-monitor.md); record results before treating the
  performance target as verified. The native benchmark does not measure IPC,
  WebView, Canvas conversion, or physical display scanout.
- [ ] **Group and Number navigation / Группы и номера.** Double-click a Group
  or Number row to expand and collapse it. Use Left and Right for the same
  actions. Confirm a collapsed/expanded row consumes both keys, inline editing
  still works, and video preview frame buttons still step frames.
- [ ] **Preview transport and sound / Транспорт и звук предпросмотра.** Start
  from the editor cursor. Confirm shared Play/Pause controls both picture and
  sound, while the headphone button only mutes or unmutes sound. Seek at least
  ten times in quick succession and confirm the final cursor and audio position
  match without a full-file decode on each seek. Seek while paused and confirm
  audio stays paused; frame-step must also seek audio and remain paused. Check
  EOF recovery and a Number timeline with offset/trim actions. Restart at a
  nonzero cursor several times; confirm startup has no yellow underrun warning,
  the voice starts after its initial buffer is ready, and later real underruns
  are still reported.
- [ ] **Video thumbnail visibility / Видимость видео-превью.** Start video
  playback and scroll the Inspector thumbnail out of view, then back. Confirm
  Play/Pause and frame-step controls remain enabled, active video and headphone
  audio stay aligned, and the elapsed cursor keeps updating. Mute and unmute
  headphones during playback. Switch away from the editor or close it and
  confirm the preview stops. Trigger or select a video source error and confirm
  its controls stay disabled.
- [ ] **Cue List arrows and GO / Стрелки Cue List и GO.** With a row focused,
  move Up/Down through visible rows and confirm both selection and Playhead
  move. Repeat with no selection and with nested Group rows. Press Space after
  rapid arrow presses; GO must start the final selected cue. Confirm Shift
  extends the range and editable fields retain their input behavior.
- [ ] **Number creation / Создание Number.** Select several top-level cues in
  a non-list order and create a Number. Confirm it appears at the first
  selected cue's position, children follow Cue List order, and membership
  survives undo and redo.
- [ ] **Wait progress / Прогресс ожидания.** Check Pre-Wait and Post-Wait bars
  in the matching cells. Pause and resume a wait, edit each wait value, and
  confirm the other cell does not show the active phase.
- [ ] **Held video frame / Последний кадр видео.** Let a video reach its last
  frame while Post-Wait and Auto-Follow or Auto-Continue are configured.
  Confirm the next cue advances on schedule; seek back and confirm playback
  state remains correct.
- [ ] **Russian transition labels / Русские названия автопереходов.** Switch
  the interface to Russian. Check Auto-Continue, Auto-Follow, and Do Not
  Continue labels in Cue List and the other transition controls.

## Подтверждённые QA-результаты

- STOP Number: в mock-IPC fixture режим Legacy меняет 4 карточки на 3
  (карточка Number исчезает, две дочерние остаются). После Reset режим Fixed
  оставляет только независимое Audio. В Rust прошли 2 теста
  `stop_tree_notification_tests`. Fixture не подключает реальное воспроизведение.
- Stage: прошли 492 frontend-теста на момент проверки и `tsc`. Часы вычисляют
  `rgba(0, 0, 0, 0)` для прозрачного фона. Визуально подтверждены четыре цвета
  групп с долей цвета 45%; снимок: `tmp/monitor-validation/stage-colours-fixed.png`.
- Conversion: новая Windows debug-сборка `pnpm tauri:check` прошла; также прошли
  494 frontend-теста и один Rust-тест вложенных visual cues. На 3-секундном
  4K-фрагменте создан и назначен 1080p proxy (3 840×2 160, 9 376 332 байта →
  608×1 080, 1 022 511 байт). Команда «Вернуть оригинал» восстановила исходный
  путь и размер 3 840×2 160; проект сохранил это назначение после событий и
  опроса состояния. В отдельном project copy исходный 4K-файл также назначен
  обратно; история заданий конвертации хранится в памяти и не переносится через
  перезапуск.
