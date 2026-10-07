# UniRay

Кроссплатформенный VPN-клиент на ядре [Xray](https://github.com/XTLS/Xray-core): Tauri 2 (Rust) + Vue 3.

Состояние:
- проверено вживую: Linux в режиме прокси, подписки (в том числе с балансировщиками), контейнер для MikroTik — в Docker с лимитом 64 МБ;
- собирается, но вживую не проверено: TUN на Linux и Windows, Windows целиком (кросс-сборка с Linux), туннель на Android;
- в планах: macOS и iOS.

## Возможности

- Импорт `vless://`, `vmess://`, `trojan://`, `ss://`, `hysteria2://` — текстом, из буфера или QR-кодом с картинки.
- Подписки: base64/текст/JSON-конфиги, трафик и срок действия, автообновление, заголовок `x-hwid` для панелей с лимитом устройств (Remnawave).
- Маршрутизация: «всё через VPN», «локальная сеть напрямую», «RU напрямую» и свои правила по доменам и IP.
- Проверка задержки, скорость и трафик в реальном времени, журнал ядра.

## Разработка

Нужны Rust, Node.js с pnpm и [системные зависимости Tauri](https://tauri.app/start/prerequisites/).

```sh
pnpm install
pnpm app:dev     # скачивает Xray, собирает helper и запускает приложение
pnpm app:build   # релизная сборка и пакеты
```

`scripts/fetch-xray.sh` скачивает закреплённую версию Xray-core и сверяет SHA-256; при обновлении версии меняйте её вместе с хэшами.

### Windows (кросс-сборка с Linux)

Нужны `cargo-xwin` (`cargo install cargo-xwin`; при первой сборке скачивает Windows SDK и CRT от Microsoft), `rustup target add x86_64-pc-windows-msvc` и `makensis` для установщика (на Arch — пакет `nsis` из AUR).

```sh
bash scripts/prepare-sidecars.sh --release x86_64-pc-windows-msvc
pnpm tauri build --runner cargo-xwin --target x86_64-pc-windows-msvc --bundles nsis
```

Установщик появится в `target/x86_64-pc-windows-msvc/release/bundle/nsis/`. Для портативной версии достаточно содержимого `target/x86_64-pc-windows-msvc/release/`: `uniray.exe`, `uniray-xray.exe`, `uniray-helper.exe`, `wintun.dll` и папка `resources`. Нужна Windows 10 или новее.

### Тесты

```sh
cargo test
```

Часть тестов прогоняет сгенерированные конфиги через настоящий Xray, включая передачу трафика по loopback. Они включаются переменными окружения (без них пропускаются):

```sh
UNIRAY_XRAY_BIN=$PWD/src-tauri/binaries/uniray-xray-$(rustc -vV | sed -n 's/^host: //p') \
XRAY_LOCATION_ASSET=$PWD/src-tauri/resources cargo test
```

## Устройство

| Путь | Что там |
| --- | --- |
| `crates/uniray-core` | Вся логика без привязки к платформе: разбор ссылок и подписок, HWID, генерация конфига Xray. |
| `crates/uniray-helper` | Привилегированный супервизор Xray для TUN-режима на десктопе. |
| `crates/uniray-gateway` | Контейнер для роутеров (MikroTik): Xray-шлюз для сети и веб-интерфейс, см. [docker/gateway/README.md](docker/gateway/README.md). |
| `src-tauri` | Приложение: запуск ядра, системный прокси, хранение состояния, команды для интерфейса. |
| `src` | Интерфейс на Vue 3 (Pinia, vue-router, vue-i18n, Tailwind). |

На десктопе Xray работает отдельным процессом. В TUN-режиме его запускает `uniray-helper` с правами администратора: на Linux через `pkexec` (нужен запущенный polkit-агент), на Windows через запрос UAC. Helper останавливает Xray, когда приложение просит об этом или завершается.

Системный прокси на Linux выставляется через `gsettings` (GNOME-настройки, которым следуют GTK-приложения и Chromium). В окружениях без них используйте TUN-режим или укажите приложениям локальный прокси вручную.
