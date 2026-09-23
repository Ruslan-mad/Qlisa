# Реестр источников stageq.ru

Проверка проведена 15.09.2026. Стартовые страницы пользователя, XML sitemap и связанные PDF были прочитаны. Веб-просмотрщик возвращал 429, поэтому публичные документы были получены напрямую с сайта; это не меняет их URL и содержание.

## Материалы, из которых извлечены требования

### Основные страницы и PDF

- [Главная](https://stageq.ru/) — позиционирование, лицензии, заявленные технологии.
- [Возможности](https://stageq.ru/about/) — структура CueList, Video Out, Windows-аудио, Installer/Portable, сценарии использования.
- [Руководство](https://stageq.ru/manual/) — актуальные правила CueList, управления, GO/STOP, NDI и настройки.
- [Полное руководство пользователя, RU PDF](https://stageq.ru/wp-content/uploads/2026/07/StageCue_User_Manual_RU.pdf) — 12 страниц, детальные свойства, hotkeys, troubleshooting.
- [Quick Start, RU PDF](https://stageq.ru/wp-content/uploads/2026/07/StageCue-Quick-Start-RUS.pdf) — 4 страницы, базовая раскладка и быстрые действия. Часть правил исторически устарела; см. release history.
- [Quick Start, EN PDF](https://stageq.ru/wp-content/uploads/2026/07/StageCue-Quick-Start.pdf) — англоязычная версия списка быстрых действий; в этой русской спецификации не использована как самостоятельный источник требований.

### Все публикации из `post-type-post-sitemap-1.xml`

- [3.2.1](https://stageq.ru/ver3-2-1/)
- [3.3.0](https://stageq.ru/stagecue-3-3-0/)
- [3.5.0](https://stageq.ru/stagecue-3-5/)
- [Лицензирование](https://stageq.ru/2111-2/)
- [3.6.0](https://stageq.ru/stagecue_3_6_0/)
- [NDI: объяснение схемы](https://stageq.ru/stage-cue-ndi/)
- [3.8.0](https://stageq.ru/stage-cue-3-8-0/)
- [QLab-аналог: обзор](https://stageq.ru/qlab-analog-windows/)
- [3.8.1](https://stageq.ru/stagecue-3-8-1/)
- [Два домена / позиционирование](https://stageq.ru/stagecue-analog-qlab-windows/)
- [3.8.3](https://stageq.ru/stagecue-3-8-3/)

## Остальные URL из публичного sitemap

| Группа | URL | Роль в этом исследовании |
| --- | --- | --- |
| Документация/витрина | `/manual/`, `/about/`, `/`, `/lessons/`, `/shop/`, `/contact/` | Релевантные продуктовые страницы внесены выше. `/lessons/` содержит нерелевантный шаблонный контент о guitar lessons. |
| Аккаунт и оформление | `/cart/`, `/checkout/`, `/my-account/`, `/login-customizer/`, `/form/` | Не описывают возможности StageCUE. Не выполнялись авторизация, оформление или отправка форм. |
| Правовые документы | `/privacy-policy/`, `/useragree-2/`, `/license-agree/`, `/personal-data/`, `/userdataagree/`, `/warrantyagree/`, `/commercial-use/`, `/cookie-agree/` | Влияют на лицензионные/операционные оговорки, но не расширяют набор функций приложения. |
| Товары | `/product/stage-cue-professional-license/`, `/product/week_license/`, `/product/mounth_license/`, `/product/corporate_license/` | Подтверждают модель сроков лицензии; функциональная комплектация, по главной странице, одинакова. |
| Категории/архивы | `/category/releases/`, `/category/news/`, `/author/esilenko/` | Индексы уже учтённых публикаций. |

## Sitemap, использованный для полноты

- `https://stageq.ru/wp-sitemap.xml`
- `https://stageq.ru/post-type-page-sitemap-1.xml`
- `https://stageq.ru/post-type-post-sitemap-1.xml`
- `https://stageq.ru/post-type-product-sitemap-1.xml`
- `https://stageq.ru/archives-sitemap-1.xml`

## Ограничение точности

Спецификация документирует только то, что публично заявлено сайтом и PDF. Точные пиксельные размеры, шрифтовое семейство, внутренние API, полный протокол OSC и фактическая совместимость драйверов не раскрыты — их нужно измерить или проверить в запущенной целевой сборке.

