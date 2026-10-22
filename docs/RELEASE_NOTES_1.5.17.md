# Qlisa 1.5.17

## Изменения

- Исправлено обновление списка cue при запуске сохранённого проекта и при открытии, создании, импорте или восстановлении проекта. После замены проекта интерфейс получает актуальное состояние от backend. Выбор и указатель GO остаются согласованными при переходе между cue, а PAUSE доступна для работающих cue после загрузки проекта.
- Кнопка транспорта и клавиша P переключают паузу и продолжение выбранных активных cue. Удержание P не переключает состояние повторно; поля ввода сохраняют обычную обработку клавиш.

## English summary

Qlisa 1.5.17 fixes cue-list state after loading or replacing a project. Selection, GO position, and Pause availability stay in sync with the backend. The transport button and P key toggle Pause/Resume for selected active cues, and holding P does not repeat the action.
