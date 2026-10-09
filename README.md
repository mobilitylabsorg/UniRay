<div align="center">

<img src="src-tauri/icons/128x128@2x.png" width="112" alt="UniRay">

# UniRay

**Кроссплатформенный VPN-клиент на ядре [Xray](https://github.com/XTLS/Xray-core)**<br>
VLESS · VMess · Trojan · Shadowsocks · Hysteria2 · WireGuard · AmneziaWG · NaiveProxy

[![Build](https://github.com/mobilitylabsorg/UniRay/actions/workflows/build.yml/badge.svg)](https://github.com/mobilitylabsorg/UniRay/actions/workflows/build.yml)
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
| WireGuard, AmneziaWG 2.0 | `uniray-engine` за Xray | ✅ upload с BBRv3, [см. ниже](#upload-wireguard) |
| AmneziaWG 3.1 (RandomTrailers, DisableCookies, HeaderProtectionKey) | `uniray-engine` на amneziawg-go v3.1 | ✅ upload с BBRv3, [см. ниже](#upload-wireguard) |
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

Готовые сборки — в [релизах](https://github.com/mobilitylabsorg/UniRay/releases) и в артефактах [запусков CI](https://github.com/mobilitylabsorg/UniRay/actions/workflows/build.yml).

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
| `engine` | `uniray-engine` (Go): WireGuard/AmneziaWG на amneziawg-go и NaiveProxy через `naive`; форки gVisor (BBRv3) и amneziawg-go — в [mobilitylabsorg](https://github.com/mobilitylabsorg); e2e-тест в `engine/e2e`, стенд скорости в `engine/bench`. |
| `crates/uniray-gateway` | Контейнер для роутеров: VPN-шлюз для сети и веб-интерфейс. |
| `src-tauri` | Приложение: запуск ядра, системный прокси, хранение состояния, команды для интерфейса. |
| `src` | Интерфейс на Vue 3 (Pinia, vue-router, vue-i18n, Tailwind). |

## Известные ограничения и планы

<a id="upload-wireguard"></a>
### Upload через WireGuard/AmneziaWG: BBRv3 в gVisor

**Что было.** Download почти как у официального клиента AmneziaVPN, а upload в разы ниже: на реальном канале 5 Мбит/с против 70 у AmneziaVPN. У NaiveProxy этого не было: его upload совпадает с `naive` без UniRay.

**Почему.** AmneziaVPN отдаёт системный TUN прямо amneziawg-go, и данные приложений отправляет TCP ядра ОС. У нас впереди Xray (маршрутизация, DNS, счётчики): он принимает TCP приложений и передаёт его движку по SOCKS, а движок открывает соединение к сайту из userspace-стека [gVisor](https://gvisor.dev/) внутри amneziawg-go. На длинном участке данные отправляет gVisor, а его reno/cubic после случайных потерь восстанавливаются очень плохо. При download отправляет сервер сайта своим TCP, поэтому download почти не страдал.

**Что сделано.** Движок собирается с форками, которые лежат в организации:
- [mobilitylabsorg/gvisor](https://github.com/mobilitylabsorg/gvisor/tree/uniray) — патч из [apoxy-dev/ccsim](https://github.com/apoxy-dev/ccsim) (подключаемый congestion control, оценка скорости доставки, pacing, исправления RACK/SACK), реализация **BBRv3** оттуда же и ограничение числа SACK-блоков;
- [mobilitylabsorg/amneziawg-go](https://github.com/mobilitylabsorg/amneziawg-go/tree/uniray) — доступ к стеку gVisor (`Net.Stack()`) и тег сборки `uniray_lowmem` с ограниченными пулами буферов для образа MikroTik `-lowmem`.

`engine/go.mod` подключает их через `replace` на закреплённые коммиты, движок включает BBR и буфер отправки 4 МиБ (1 МиБ в `uniray_lowmem`), см. `engine/tcp.go`. BBR почти не реагирует на случайные потери, поэтому помогает во всех режимах: TUN и прокси на десктопе, VPN и Stealth на Android, MikroTik.

**Замеры** (стенд [`engine/bench`](engine/bench): сервер amneziawg-go v3.1 в network namespace, UDP-ретранслятор с задержкой и потерями, RTT 60 мс, upload; при потерях разброс между прогонами большой):

| | без потерь | 0,1% потерь | 0,5% потерь |
| --- | ---: | ---: | ---: |
| `uniray-engine` до (gVisor, reno) | 142 Мбит/с | 6–42 Мбит/с | 3–4,5 Мбит/с |
| **`uniray-engine` с BBRv3** | **191 Мбит/с** | **89–159 Мбит/с** | **34–79 Мбит/с** |
| TCP ядра Linux, CUBIC (как у AmneziaVPN) | 148–162 Мбит/с | 8–95 Мбит/с | 3,4–5,4 Мбит/с |
| TCP ядра Linux, BBR | 163–185 Мбит/с | 69–128 Мбит/с | 65–80 Мбит/с |

Download с BBR не изменился. NaiveProxy это не касается: `naive` отправляет данные своим TCP ядра, движок только передаёт их по loopback.

**Что осталось:**
- проверить на живом канале против AmneziaVPN (тестовые сборки — в артефактах CI);
- перенести патч на свежий gVisor: в новых версиях у `congestionControl.Update` появился аргумент `ackTime`, поэтому форк пока на gVisor от июля 2026 (его же требует amneziawg-go v3.1);
- при желании — TCP ядра через настоящий TUN там, где есть root (TUN-режим на десктопе, Stealth на Android): движок поднимает TUN AmneziaWG и открывает соединения системными сокетами, привязанными к нему.

## Разработка

Нужны Rust, Go 1.26+ (движок `engine/`; `GOTOOLCHAIN=auto` скачает нужную версию сам), Node.js с pnpm и [системные зависимости Tauri](https://tauri.app/start/prerequisites/).

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

## Лицензия

UniRay распространяется под [GNU General Public License v3.0](LICENSE) (`GPL-3.0-only`): код можно использовать, изменять и распространять, в том числе за деньги, но изменённые версии и производные работы должны оставаться под GPL-3.0 с открытыми исходниками.

**Движок `uniray-engine` распространяется на условиях [GNU AGPL v3.0](https://www.gnu.org/licenses/agpl-3.0.html).** В него входит код под AGPL-3.0 из [apoxy-dev/ccsim](https://github.com/apoxy-dev/ccsim): BBRv3 и патч gVisor в форке [mobilitylabsorg/gvisor](https://github.com/mobilitylabsorg/gvisor/tree/uniray). GPL-3.0 (раздел 13) разрешает такое объединение: исходники UniRay остаются под GPL-3.0, а собранный движок, где они соединены с кодом под AGPL, — под AGPL-3.0. Для пользователей приложения разницы нет; условие AGPL о сетевом доступе касается только тех, кто изменит движок и даст им пользоваться по сети.

Сторонние компоненты сохраняют свои лицензии: Xray-core — MPL-2.0, amneziawg-go — MIT, gVisor — Apache-2.0 (патч и BBRv3 в нашем форке — AGPL-3.0), NaiveProxy — BSD-3-Clause, Wintun — свои условия (`LICENSE-Wintun` рядом с `wintun.dll`).
