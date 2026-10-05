# Чеклист следующих исправлений / Follow-up regression checklist

These changes are in the source after the 1.5.11 baseline. This file records
manual checks to run; an unchecked item is not a pass. / Эти изменения внесены
в исходный код после версии 1.5.11. Здесь записаны ручные проверки; пустой
чекбокс не означает, что проверка пройдена.

## Manual checks / Ручные проверки

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
