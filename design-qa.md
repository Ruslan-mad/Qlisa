# FullscreenControl visual QA

Проверено на временном Vite harness с реальным `FullscreenControl` и mock Tauri IPC.

- Механика reference: split-кнопка; основная часть переключает все физические окна; зелёный LED показывает aggregate visible; стрелка открывает выбор output и monitor.
- Стиль Qlisa: верхняя панель, центр заголовка, существующие `--wc-*` tokens, Phosphor icons, меню раскрывается вниз.
- Меню: все enabled `display` outputs раскрыты сразу; NDI/SRT и floating outputs отсутствуют.
- Мониторы: только подключённые физические экраны; занятый монитор показывает владельца и остаётся кликабельным для backend swap; фактический duplicate подсвечивается красным.
- Interaction: click-outside и Escape закрывают меню; toggle меняет LED; swap обновляет обе секции.
- Accessibility: button labels, `aria-pressed`, `aria-expanded`, `menuitemradio`, `aria-checked`.

Результат: PASS. P0/P1/P2 замечаний нет.

Остаётся release smoke-test в native Tauri app на Windows с реальными display windows и физическими мониторами.
