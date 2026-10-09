<div align="center">

<img src="src-tauri/icons/128x128@2x.png" width="112" alt="UniRay">

# UniRay

**Кроссплатформенный VPN-клиент на ядре [Xray](https://github.com/XTLS/Xray-core)**<br>
VLESS · VMess · Trojan · Shadowsocks · Hysteria2 · WireGuard · AmneziaWG · NaiveProxy

[![Build](https://github.com/delarun/UniRay/actions/workflows/build.yml/badge.svg)](https://github.com/delarun/UniRay/actions/workflows/build.yml)
![Tauri 2](https://img.shields.io/badge/Tauri-2-24C8DB?logo=tauri&logoColor=white)
![Rust](https://img.shields.io/badge/Rust-core-000000?logo=rust)
![Vue 3](https://img.shields.io/badge/Vue-3-4FC08D?logo=vuedotjs&logoColor=white)

Linux · Windows · macOS · Android · MikroTik

</div>

---

## Платформы

| | Прокси | Туннель | Пакет |
| --- | :---: | :---: | --- |
| 🐧 **Linux** | ✅ | ✅ TUN | AppImage |
| 🪟 **Windows** 10+ | ✅ | ✅ TUN (Wintun) | установщик и портативный zip |
| 🍎 **macOS** | ✅ | 🟡 TUN собирается, вживую не проверен | DMG (Apple Silicon и Intel) |
| 🤖 **Android** | — | ✅ VPN · ✅ Stealth (root) | APK (arm64) |
| 📡 **MikroTik** | ✅ | ✅ шлюз для сети | контейнер (armv7, arm64), [подробнее](docker/gateway/README.md) |
| 📱 iOS | — | ❌ туннеля пока нет | — |

✅ проверено вживую · 🟡 собирается, вживую не проверено · ❌ не работает

Контейнер для MikroTik проверен в Docker с лимитом памяти 64 МБ, как на роутере. Про iOS — [ниже](#ios).

## Протоколы

| | Как | Состояние |
| --- | --- | --- |
| VLESS, VMess, Trojan, Shadowsocks, Hysteria2 | Xray | ✅ |
| WireGuard, AmneziaWG 2.0 | `uniray-engine` за Xray | ✅ upload при потерях медленнее, [см. ниже](#upload-wireguard) |
| AmneziaWG 3.1 (RandomTrailers, DisableCookies, HeaderProtectionKey) | `uniray-engine` на amneziawg-go v3.1 | ✅ upload при потерях медленнее, [см. ниже](#upload-wireguard) |
| NaiveProxy (`naive+https`, `naive+quic`) | официальный `naive` за движком | ✅ |

Цепочка «Xray → движок → сервер AmneziaWG» проверяется в CI на каждый коммит: в network namespaces, в режимах прокси и TUN.

## Возможности

- 📥 **Импорт** — `vless://`, `vmess://`, `trojan://`, `ss://`, `hysteria2://`, `wireguard://`, `naive+https://`, ключи AmneziaVPN `vpn://`, файлы `.conf` и JSON-конфиги: текстом, из буфера, из файла или QR-кодом с картинки.
- 🔄 **Подписки** — base64, текст и JSON, трафик и срок действия, автообновление, заголовок `x-hwid` для панелей с лимитом устройств (Remnawave).
- 🧭 **Маршрутизация** — «всё через VPN», «локальная сеть напрямую», «RU напрямую» и свои правила по доменам и IP.
- 🌐 **DNS** — удалённый через VPN и прямой, в том числе DNS провайдера текущей сети.
- 📊 **Мониторинг** — проверка задержки, скорость и трафик в реальном времени, журнал ядра.
- 🕵️ **Stealth на Android** — работа без VpnService: сканеры VPN не находят ни `tun0`, ни VpnService, ни VPN-транспорта (нужен root).

## Установка

Готовые сборки — в [релизах](https://github.com/delarun/UniRay/releases) и в артефактах [запусков CI](https://github.com/delarun/UniRay/actions/workflows/build.yml).

- **Linux** — `chmod +x UniRay_*.AppImage` и запуск. Для TUN-режима нужен polkit-агент (права запрашиваются через `pkexec`). Системный прокси выставляется через `gsettings` (GNOME, GTK-приложения, Chromium); в окружениях без него используйте TUN или укажите прокси приложениям вручную.
- **Windows** — установщик или портативный zip. TUN-режим запрашивает права администратора (UAC).
- **macOS** — перетащите UniRay в «Программы». Без сертификата разработчика при первом запуске macOS не сможет проверить разработчика: откройте приложение через контекстное меню («Открыть») или разрешите в «Системные настройки → Конфиденциальность и безопасность». Либо снимите карантин:
  ```sh
  xattr -cr /Applications/UniRay.app
  ```
- **Android** — установите APK. Stealth-режим требует root.
- **MikroTik** — см. [docker/gateway/README.md](docker/gateway/README.md).

## Как это устроено

```
 приложения ──► Xray (TUN / прокси, маршрутизация, DNS, счётчики) ──► сервер VLESS, Trojan, …
                  │
                  └─ SOCKS5 127.0.0.1 ──► uniray-engine ──► сервер WireGuard / AmneziaWG
                                              └─► naive ──► сервер NaiveProxy
```

На десктопе Xray работает отдельным процессом. В TUN-режиме его запускает `uniray-helper` с правами администратора: на Linux через `pkexec`, на Windows через UAC, на macOS через системный запрос пароля. Helper останавливает Xray, когда приложение просит об этом или завершается.

Серверы, которых Xray не умеет, работают через `uniray-engine`, а Xray остаётся впереди. Порт движка закрыт трижды:
1. пароль, новый на каждое подключение (секреты передаются только файлами, которые движок удаляет, и никогда — аргументами);
2. проверка, что подключился именно процесс Xray (по таблицам сокетов ОС, на Android — по uid через `getConnectionOwnerUid`);
3. в stealth-режиме — ещё и правило iptables.

Свои пакеты движка к серверу уходят мимо туннеля по правилу `direct`, так что в TUN-режиме петли нет.

| Путь | Что там |
| --- | --- |
| `crates/uniray-core` | Логика без привязки к платформе: разбор ссылок и подписок, HWID, генерация конфига Xray. |
| `crates/uniray-helper` | Привилегированный супервизор Xray и движка для TUN-режима на десктопе. |
| `engine` | `uniray-engine` (Go): WireGuard/AmneziaWG на amneziawg-go и NaiveProxy через `naive`; e2e-тест в `engine/e2e`. |
| `crates/uniray-gateway` | Контейнер для роутеров: VPN-шлюз для сети и веб-интерфейс. |
| `src-tauri` | Приложение: запуск ядра, системный прокси, хранение состояния, команды для интерфейса. |
| `src` | Интерфейс на Vue 3 (Pinia, vue-router, vue-i18n, Tailwind). |

## Известные ограничения и планы

<a id="upload-wireguard"></a>
### Upload через WireGuard/AmneziaWG

**Что видно.** Download почти как у официального клиента AmneziaVPN (на 5–10% ниже), а upload в разы ниже: на реальном канале 5 Мбит/с против 70 у AmneziaVPN. У NaiveProxy этого нет: его upload совпадает с `naive` без UniRay.

**Почему.** AmneziaVPN отдаёт системный TUN прямо amneziawg-go. TCP-соединения приложений идут через туннель без изменений, данные отправляет TCP ядра ОС (CUBIC или BBR, современное восстановление после потерь). У нас впереди Xray (маршрутизация, DNS, счётчики): он принимает TCP приложений и передаёт его движку по SOCKS, а движок открывает соединение к сайту из userspace-стека [gVisor](https://gvisor.dev/) внутри amneziawg-go. На длинном участке данные отправляет gVisor, а он после потерь восстанавливается гораздо хуже ядра. При download отправляет сервер сайта со своим ядерным TCP, поэтому download почти не страдает.

**Замеры** (стенд: сервер amneziawg-go v3.1 в network namespace, UDP-ретранслятор с задержкой и потерями, RTT 60 мс, upload):

| | без потерь | 0,1% потерь | 0,5% потерь |
| --- | ---: | ---: | ---: |
| `uniray-engine` (gVisor, reno) | 142 Мбит/с | 6–16 Мбит/с | 4,5 Мбит/с |
| TCP ядра Linux, CUBIC (как у AmneziaVPN) | 162 Мбит/с | 95 Мбит/с | 3,6 Мбит/с |
| TCP ядра Linux, BBR | 163 Мбит/с | 69–91 Мбит/с | 74 Мбит/с |

Что уже пробовали: CUBIC и буферы побольше в gVisor почти не помогают. Другого userspace-стека лучше gVisor нет: lwIP и smoltcp слабее, BBR нет ни в одном из них, в том числе в свежем gVisor. gVisor также плохо переносит перестановку пакетов. Download и NaiveProxy в порядке.

**Что можно сделать:**

1. **TCP ядра через настоящий TUN** там, где есть root: TUN-режим на десктопе (helper уже запускает движок от администратора) и Stealth на Android. Движок поднимает TUN-интерфейс AmneziaWG (amneziawg-go это умеет, как в AmneziaVPN) и открывает соединения к сайтам системными сокетами, привязанными к этому интерфейсу (`SO_BINDTODEVICE` на Linux и Android, `IP_UNICAST_IF` на Windows, `IP_BOUND_IF` на macOS). Xray остаётся впереди со всей маршрутизацией. Скорость как у AmneziaVPN. Не помогает в обычном VPN-режиме Android (там один TUN, у Xray) и в режиме прокси без root.
2. **BBR в gVisor**: собирать движок с пропатченной копией gVisor (как `scripts/build-engine.sh` уже патчит amneziawg-go для образа MikroTik `-lowmem`). BBR почти не реагирует на случайные потери (см. таблицу), так что это работает во всех режимах, включая VPN-режим Android и прокси. Минусы: большая работа (BBR — сотни строк тонкого кода, в gVisor нет расчёта скорости доставки) и патч, который придётся поддерживать при обновлениях gVisor.
3. **Для AWG-серверов в VPN-режиме Android** отдавать TUN VpnService прямо amneziawg-go, без Xray, как это делает AmneziaVPN. Тогда для таких серверов останутся только правила по IP: правила по доменам и DNS Xray работать не будут.

Разумный порядок: сначала вариант 1 (без потерь функциональности, покрывает десктоп и Stealth), затем вариант 2 для остальных режимов.

## Разработка

Нужны Rust, Go 1.25+ (движок `engine/`), Node.js с pnpm и [системные зависимости Tauri](https://tauri.app/start/prerequisites/).

```sh
pnpm install
pnpm app:dev     # скачивает Xray и naive, собирает helper и движок, запускает приложение
pnpm app:build   # релизная сборка и пакеты
```

`scripts/fetch-xray.sh` и `scripts/fetch-naive.sh` скачивают закреплённые версии Xray-core и NaiveProxy и сверяют SHA-256 (при обновлении версии меняйте её вместе с хэшами), `scripts/build-engine.sh` собирает движок.

<details>
<summary><b>Тесты</b></summary>

```sh
cargo test
```

Часть тестов прогоняет сгенерированные конфиги через настоящий Xray, включая передачу трафика по loopback. Они включаются переменными окружения, без них пропускаются:

```sh
UNIRAY_XRAY_BIN=$PWD/src-tauri/binaries/uniray-xray-$(rustc -vV | sed -n 's/^host: //p') \
XRAY_LOCATION_ASSET=$PWD/src-tauri/resources cargo test
```

Движок проверяется против настоящего сервера amneziawg-go в network namespaces (нужен root): `sudo bash engine/e2e/run.sh`.

</details>

<details>
<summary><b>Windows: кросс-сборка с Linux</b></summary>

Нужны `cargo-xwin` (`cargo install cargo-xwin`; при первой сборке скачивает Windows SDK и CRT от Microsoft), `rustup target add x86_64-pc-windows-msvc` и `makensis` для установщика.

```sh
bash scripts/prepare-sidecars.sh --release x86_64-pc-windows-msvc
pnpm tauri build --runner cargo-xwin --target x86_64-pc-windows-msvc --bundles nsis
```

Установщик появится в `target/x86_64-pc-windows-msvc/release/bundle/nsis/`. Для портативной версии достаточно содержимого `target/x86_64-pc-windows-msvc/release/`: `uniray.exe`, `uniray-xray.exe`, `uniray-helper.exe`, `uniray-engine.exe`, `uniray-naive.exe`, `wintun.dll` и папка `resources`.

Скрипт установщика Tauri подключает `Win\RestartManager.nsh` из NSIS 3.09+. С более старым NSIS (например, 3.08 из Ubuntu 22.04) скопируйте `scripts/nsis/Win` в его папку `Include`.

</details>

<details>
<summary><b>CI и подпись</b></summary>

`.github/workflows/build.yml` собирает все пакеты на каждый push в `main`, на pull request и вручную; при push тега `v*` они же публикуются как релиз. Linux, Windows (кросс-сборкой), Android и MikroTik собираются на своём Linux-раннере (метки `self-hosted`, `uniray`), macOS — на раннере GitHub и только по тегу или вручную.

**Android.** Без секретов APK подписывается одноразовым отладочным ключом, и каждая сборка ставится только начисто. Чтобы обновления ставились поверх, добавьте в секреты репозитория `ANDROID_KEYSTORE` (keystore в base64), `ANDROID_KEYSTORE_PASSWORD` и `ANDROID_KEY_ALIAS`.

**macOS.** Без секретов приложение подписывается ad-hoc. Для подписи и нотаризации добавьте `APPLE_CERTIFICATE` (Developer ID Application в .p12, base64), `APPLE_CERTIFICATE_PASSWORD`, `APPLE_SIGNING_IDENTITY`, `APPLE_ID`, `APPLE_PASSWORD` (пароль приложения с appleid.apple.com) и `APPLE_TEAM_ID`. Нужен платный Apple Developer Program.

</details>

<details id="ios">
<summary><b>iOS</b></summary>

В CI iOS не собрать: нужны Mac с Xcode и аккаунт Apple. Сейчас приложение на iOS запускается, импортирует серверы и обновляет подписки, но **не подключается**: для VPN на iOS нужно расширение Network Extension (Packet Tunnel) с ядром Xray внутри, а его ещё нет. Для такого расширения обязателен платный Apple Developer Program ($99 в год).

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
3. Укажите Team ID (Xcode → Settings → Accounts или developer.apple.com → Membership): в `src-tauri/tauri.ios.conf.json` как `"bundle": { "iOS": { "developmentTeam": "ABCDE12345" } }` или переменной `APPLE_DEVELOPMENT_TEAM`.
4. Запуск:
   ```sh
   pnpm tauri ios dev                       # в симуляторе
   pnpm tauri ios dev --host                # на iPhone по кабелю («Режим разработчика» на телефоне)
   pnpm tauri ios build --export-method debugging   # .ipa в src-tauri/gen/apple/build/arm64/
   ```
   Готовый `.ipa` ставится на iPhone через Xcode (Window → Devices and Simulators).

</details>
