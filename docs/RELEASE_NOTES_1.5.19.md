# Qlisa 1.5.19

## Исправления

- Исправлена первая установка Windows runtime. Ссылка на прежний FFmpeg-архив возвращала HTTP 404. Qlisa теперь загружает FFmpeg и ffprobe 9.0.2 из закреплённого релиза Gyan и проверяет SHA-256 архива и исполняемых файлов перед установкой. Загрузка libmpv не менялась.

## English summary

Fixed first-run Windows runtime setup. The previous pinned FFmpeg archive URL returned HTTP 404. Qlisa now downloads FFmpeg and ffprobe 9.0.2 from a versioned Gyan release and verifies the archive and executable SHA-256 hashes before installation. The libmpv download is unchanged.
