# Qlisa 1.5.11

## Русский

### Управление звуком в «Номере»

- Кнопка динамика на вкладке «Номер» переключает флаг `muted` у дочерних аудио- и видеокью. Повторное нажатие включает звук, не меняя громкость, матрицу выходов и позицию воспроизведения. Предпрослушивание в «Номере» учитывает тот же флаг, что и вкладка «Основные».
- Если действие «Номер» в версии 1.5.10 уже сохранило громкость дочернего кью на −60 дБ, восстановить прежний уровень невозможно. Если кью остаётся без звука, отрегулируйте громкость вручную.

## English

### Number cue speaker control

- The Number tab speaker quick action now toggles the actual mute state of Audio and Video children. Unmuting works again and does not change child volume, output matrix, or playback position. Number preview gating uses the same mute state as Basics.
- If a Number action in 1.5.10 already saved a child volume of −60 dB, its previous level cannot be inferred. Manually adjust the volume if that cue remains silent.
