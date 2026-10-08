# UniRay

Кроссплатформенный VPN-клиент на ядре [Xray](https://github.com/XTLS/Xray-core): Tauri 2 (Rust) + Vue 3.

Состояние:
- проверено вживую: Linux в режиме прокси, подписки (в том числе с балансировщиками), контейнер для MikroTik — в Docker с лимитом 64 МБ;
- собирается, но вживую не проверено: TUN на Linux и Windows, Windows целиком (кросс-сборка с Linux), туннель на Android;
- собирается в CI, вживую не проверено: macOS (режим прокси и TUN);
- WireGuard, AmneziaWG и NaiveProxy: цепочка «Xray → движок → сервер AmneziaWG» проверяется в CI (network namespaces, режимы прокси и TUN), с настоящими серверами вживую ещё не проверено;
- iOS: интерфейс собирается, но туннеля ещё нет — подключение не работает (см. «iOS» ниже).

## Возможности

- Импорт `vless://`, `vmess://`, `trojan://`, `ss://`, `hysteria2://` — текстом, из буфера или QR-кодом с картинки.
- WireGuard и AmneziaWG (в том числе AWG 2.0): файлы `.conf`, ключи AmneziaVPN `vpn://`, ссылки `wireguard://`.
- NaiveProxy: ссылки `naive+https://` и `naive+quic://`.
- Подписки: base64/текст/JSON-конфиги, трафик и срок действия, автообновление, заголовок `x-hwid` для панелей с лимитом устройств (Remnawave).
- Маршрутизация: «всё через VPN», «локальная сеть напрямую», «RU напрямую» и свои правила по доменам и IP.
- Проверка задержки, скорость и трафик в реальном времени, журнал ядра.

## Разработка

Нужны Rust, Go 1.25+ (для движка `engine/`), Node.js с pnpm и [системные зависимости Tauri](https://tauri.app/start/prerequisites/).

```sh
pnpm install
pnpm app:dev     # скачивает Xray и naive, собирает helper и движок, запускает приложение
pnpm app:build   # релизная сборка и пакеты
```

`scripts/fetch-xray.sh` скачивает закреплённую версию Xray-core и сверяет SHA-256; при обновлении версии меняйте её вместе с хэшами. Так же `scripts/fetch-naive.sh` скачивает NaiveProxy, а `scripts/build-engine.sh` собирает движок.

### Windows (кросс-сборка с Linux)

Нужны `cargo-xwin` (`cargo install cargo-xwin`; при первой сборке скачивает Windows SDK и CRT от Microsoft), `rustup target add x86_64-pc-windows-msvc` и `makensis` для установщика (на Arch — пакет `nsis` из AUR).

```sh
bash scripts/prepare-sidecars.sh --release x86_64-pc-windows-msvc
pnpm tauri build --runner cargo-xwin --target x86_64-pc-windows-msvc --bundles nsis
```

Установщик появится в `target/x86_64-pc-windows-msvc/release/bundle/nsis/`. Для портативной версии достаточно содержимого `target/x86_64-pc-windows-msvc/release/`: `uniray.exe`, `uniray-xray.exe`, `uniray-helper.exe`, `uniray-engine.exe`, `uniray-naive.exe`, `wintun.dll` и папка `resources`. Нужна Windows 10 или новее.

### CI

`.github/workflows/build.yml` собирает AppImage для Linux, установщик и портативный zip для Windows, DMG для macOS (Apple Silicon и Intel), APK для Android (arm64) и образы контейнера для MikroTik (armv7, arm64) на каждый push в `main`, на pull request и вручную. Готовые файлы лежат в артефактах запуска; при push тега `v*` они же публикуются как релиз GitHub.

Без секретов APK подписывается одноразовым отладочным ключом, и каждая сборка ставится только начисто. Чтобы обновления ставились поверх, добавьте в секреты репозитория `ANDROID_KEYSTORE` (keystore в base64), `ANDROID_KEYSTORE_PASSWORD` и `ANDROID_KEY_ALIAS`.

Без секретов приложение для macOS подписывается ad-hoc (без сертификата разработчика), и при первом запуске macOS сообщит, что не может проверить разработчика. Перетащите UniRay в «Программы», затем откройте его через контекстное меню («Открыть») или разрешите в «Системные настройки → Конфиденциальность и безопасность → Всё равно открыть». Либо снимите карантин:

```sh
xattr -cr /Applications/UniRay.app
```

Для подписи и нотаризации добавьте секреты `APPLE_CERTIFICATE` (сертификат Developer ID Application в .p12, base64), `APPLE_CERTIFICATE_PASSWORD`, `APPLE_SIGNING_IDENTITY`, `APPLE_ID`, `APPLE_PASSWORD` (пароль приложения с appleid.apple.com) и `APPLE_TEAM_ID`. Нужен платный Apple Developer Program.

### iOS

В CI iOS не собрать: нужны Mac с Xcode и аккаунт Apple. Сейчас приложение на iOS запускается, импортирует серверы и обновляет подписки, но **не подключается**: для VPN на iOS нужно расширение Network Extension (Packet Tunnel) с ядром Xray внутри, а его ещё нет. Для такого расширения обязателен платный Apple Developer Program ($99 в год): бесплатный аккаунт не даёт нужного разрешения.

Сборка на Mac:

1. Установите Xcode из App Store, запустите его один раз и примите лицензию. Затем:
   ```sh
   xcode-select --install
   brew install cocoapods
   rustup target add aarch64-apple-ios aarch64-apple-ios-sim
   ```
2. В корне репозитория:
   ```sh
   pnpm install
   pnpm tauri ios init        # создаёт Xcode-проект в src-tauri/gen/apple
   ```
3. Укажите свою команду разработчика: Team ID из Xcode (Settings → Accounts) или developer.apple.com → Membership. Его можно записать в `src-tauri/tauri.ios.conf.json` как `"bundle": { "iOS": { "developmentTeam": "ABCDE12345" } }` или передать переменной `APPLE_DEVELOPMENT_TEAM`.
4. Запуск:
   ```sh
   pnpm tauri ios dev                       # в симуляторе (выбрать из списка)
   pnpm tauri ios dev --host                # на iPhone по кабелю; на телефоне включите «Режим разработчика»
   pnpm tauri ios build --export-method debugging   # .ipa в src-tauri/gen/apple/build/arm64/
   ```
   Готовый `.ipa` ставится на подключённый iPhone через Xcode (Window → Devices and Simulators, «+» под списком приложений).

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
| `engine` | `uniray-engine` (Go): WireGuard/AmneziaWG на `amneziawg-go` и NaiveProxy через официальный `naive`, за Xray; e2e-тест в `engine/e2e`. |
| `crates/uniray-gateway` | Контейнер для роутеров (MikroTik): VPN-шлюз для сети (Xray, AmneziaWG/WireGuard и NaiveProxy через `uniray-engine`) и веб-интерфейс, см. [docker/gateway/README.md](docker/gateway/README.md). |
| `src-tauri` | Приложение: запуск ядра, системный прокси, хранение состояния, команды для интерфейса. |
| `src` | Интерфейс на Vue 3 (Pinia, vue-router, vue-i18n, Tailwind). |

На десктопе Xray работает отдельным процессом. В TUN-режиме его запускает `uniray-helper` с правами администратора: на Linux через `pkexec` (нужен запущенный polkit-агент), на Windows через запрос UAC, на macOS через системный запрос пароля администратора. Helper останавливает Xray, когда приложение просит об этом или завершается.

Серверы, которых Xray не умеет (WireGuard, AmneziaWG, NaiveProxy), работают через `uniray-engine`: Xray остаётся впереди (TUN, маршрутизация, DNS, счётчики), а его исходящий `proxy` — SOCKS5 к движку на 127.0.0.1. Порт движка закрыт трижды: пароль, новый на каждое подключение (секреты передаются только файлами, которые движок удаляет, и никогда — аргументами); проверка, что подключился именно процесс Xray (по таблицам сокетов ОС, на Android — по uid через `getConnectionOwnerUid`); в stealth-режиме — ещё и правило iptables. Свои пакеты движка к серверу уходят мимо туннеля по правилу `direct`, так что в TUN-режиме петли нет.

Системный прокси на Linux выставляется через `gsettings` (GNOME-настройки, которым следуют GTK-приложения и Chromium). В окружениях без них используйте TUN-режим или укажите приложениям локальный прокси вручную.
