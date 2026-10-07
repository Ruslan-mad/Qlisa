# Qlisa 1.5.13

## Русский

### Команды между Cue List

Подробное описание модели целей и сценарий проверки: [руководство по командам
между Cue List](https://github.com/Ruslan-mad/Qlisa/blob/v1.5.13/docs/cross-list-commands.md).

- Команды Start, Pause, Resume, Load, Reset, Goto, Stop, Fade, Devamp, Arm и
  Disarm могут ссылаться на один или несколько Cue из разных Cue List по UUID.
- Переключение цели не меняет видимую вкладку. Goto перемещает Playhead списка
  цели. Start запускает Cue без первоначального перемещения Playhead; последующие
  Auto-Continue и Auto-Follow работают по обычной последовательности списка-владельца.
- Active Cues показывает удалённые Cue и позволяет управлять их состоянием.
  Ссылки хранятся как UUID в существующих полях проекта. Старые ссылки только по
  номеру остаются привязаны к Cue List исходной команды.
- Пустая цель Stop и обычный Soft Stop All сохраняют область действия исходного
  или активного списка. Hard Stop All остаётся глобальным.

### Рабочий интерфейс

- Media Dock закреплён внизу справа отдельно от Inspector и выровнен с Timeline
  и Slice. Он остаётся виден при скрытом Inspector и смене его вкладок, используя
  один существующий preview.
- Компактный поиск закреплён справа в строке Cue List и остаётся доступен при
  прокрутке или скрытых вкладках.
- Настройки синхронизации TC для каждого Cue List находятся в Preferences →
  Network и сохраняются в проекте. Настройки приёма TC и выбора устройства
  остаются глобальными для компьютера.
- Числа в колонке `#` центрируются по горизонтали и вертикали, в том числе у
  вложенных Cue.

### Измерения и проверки

- Измерения задержки GO для Audio, Image и Video описаны в [исследовании задержки
  GO](https://github.com/Ruslan-mad/Qlisa/blob/v1.5.13/docs/go-start-latency.md).
  Это результаты конкретных измерений, а не новая
  гарантия производительности.
- Прошли 503 frontend-теста в 72 файлах, 1,010 Rust-тестов; 11 Rust-тестов
  пропущены. `pnpm tauri:check` завершился успешно.
- Browser review с mock IPC проверил UI, но не реальную связанную IPC-команду,
  воспроизведение или физическое устройство. Нативный GUI smoke test и cross-list
  end-to-end playback test не выполнялись.

## English

### Cross-list commands

See the [cross-list command guide](https://github.com/Ruslan-mad/Qlisa/blob/v1.5.13/docs/cross-list-commands.md)
for target ownership and a manual-check scenario.

- Start, Pause, Resume, Load, Reset, Goto, Stop, Fade, Devamp, Arm, and Disarm
  can target one or more Cues in different Cue Lists by UUID.
- Resolving a target does not switch the visible tab. Goto moves the target
  list's Playhead. Start begins the Cue without initially moving that Playhead;
  later Auto-Continue and Auto-Follow follow the owning list's normal sequence.
- Active Cues shows remote Cues and allows their state to be controlled. Targets
  remain UUID arrays in existing project fields. Legacy number-only references
  stay scoped to the source command's Cue List.
- An empty Stop target and ordinary Soft Stop All keep their source or active
  list scope. Hard Stop All remains workspace-wide.

### Workspace UI

- The Media Dock stays at the lower right, separate from Inspector and aligned
  with Timeline and Slice. It remains visible when Inspector is hidden or its
  tab changes, using one existing preview.
- Compact search stays at the right of the Cue List tab row while tabs scroll or
  are hidden.
- Per-list timecode sync settings are in Preferences → Network and are saved in
  the project. Timecode receiver and device settings remain global to the
  computer.
- Values in the `#` column are centered horizontally and vertically, including
  nested Cues.

### Measurements and checks

- GO latency measurements for Audio, Image, and Video are documented in the
  [GO latency study](https://github.com/Ruslan-mad/Qlisa/blob/v1.5.13/docs/go-start-latency.md).
  They are results from specific
  measurements, not a new performance guarantee.
- 503 frontend tests across 72 files and 1,010 Rust tests passed; 11 Rust tests
  were ignored. `pnpm tauri:check` passed.
- Browser review with mock IPC checked UI behavior, but not real linked-command
  IPC, playback, or physical devices. No native GUI smoke test or cross-list
  end-to-end playback test was run.
