# UniRay Gateway для MikroTik

Контейнер для RouterOS: подключается к VPN, работает шлюзом для выбранных устройств сети и даёт SOCKS/HTTP-прокси. Сервер переключается в веб-интерфейсе, подписка обновляется сама.

Протоколы: всё, что умеет Xray (VLESS, VMess, Trojan, Shadowsocks, Hysteria2), а также WireGuard и AmneziaWG (до 3.1) через `uniray-engine` и NaiveProxy через официальный `naive`. Движок и `naive` работают за Xray, как в приложении: Xray пускает в них только свой трафик, а сервер отправляет мимо туннеля.

Образы, оба для armv7 и arm64:

| Файл | Для чего |
| --- | --- |
| `uniray-gateway-armv7-lowmem.tar` | **Для лимита 64 МБ.** Движок AmneziaWG/WireGuard с ограниченными буферами: под нагрузкой ждёт, а не растёт. Скорость та же. |
| `uniray-gateway-armv7.tar` | amneziawg-go как есть. Под быстрой загрузкой движок может занять 50+ МБ: только при лимите от 128 МБ. |

- Размер: ~25 МБ (на диске ~58 МБ: Xray 37 МБ, движок 9 МБ, `naive` 10 МБ и несколько библиотек glibc для него).
- Память (замер в Docker с лимитом 64 МБ, AmneziaWG 3.1, `-lowmem`): ~12 МБ в простое, пик 40 МБ при четырёх параллельных загрузках ~30 МБ/с. Обычный образ в тех же условиях упирается в 64 МБ, и ядро убивает движок.
- NaiveProxy тяжелее: сам `naive` (сетевой стек Chromium) занимает ~10 МБ в простое и больше под нагрузкой. Для Naive-серверов дайте контейнеру от 96–128 МБ (`ram-high`).
- Раздельной маршрутизации нет: всё, что роутер отправит в контейнер, идёт через VPN. Если в подписке есть правила провайдера с прямым доступом к отдельным сайтам (например, RU-домены), в контейнере они отключаются. Блокировки провайдера (например, BitTorrent) остаются.

## Сборка

```sh
scripts/build-gateway.sh                 # linux/arm/v7 → build/uniray-gateway-armv7.tar
scripts/build-gateway.sh linux/arm64     # другие варианты: linux/arm64, linux/amd64
```

Нужны Docker с buildx, Go 1.25+ (движок), [cargo-zigbuild](https://github.com/rust-cross/cargo-zigbuild) (`cargo install cargo-zigbuild`), Zig (`pip install ziglang` или пакет из дистрибутива), `readelf` (binutils) и цели Rust: `rustup target add armv7-unknown-linux-musleabihf aarch64-unknown-linux-musl`.

Каждая платформа даёт два образа: `build/uniray-gateway-<arch>.tar` и `build/uniray-gateway-<arch>-lowmem.tar`. `NAIVE=0 scripts/build-gateway.sh` собирает без `naive` и glibc (меньше на ~11 МБ, без Naive-серверов). Ограничение буферов в `-lowmem` задаёт `ENGINE_MAX_BUFFERS` (по умолчанию 384, см. `scripts/build-engine.sh`).

Готовые образы для armv7 и arm64 собирает CI (`.github/workflows/build.yml`): они лежат в артефакте `uniray-gateway` каждого запуска и в релизах.

## Установка

Пример: контейнер в отдельной сети `192.168.254.0/24`, файлы на `usb1`. Адреса и пути подставьте свои.

```routeros
# Сеть для контейнера
/interface/bridge add name=Bridge-Docker port-cost-mode=short
/ip/address add address=192.168.254.1/24 interface=Bridge-Docker network=192.168.254.0
/interface/veth add address=192.168.254.4/24 gateway=192.168.254.1 name=UNIRAY
/interface/bridge/port add bridge=Bridge-Docker interface=UNIRAY

# Подписка (или ссылки: SRV1="vless://...", SRV2=...)
/container envs add list=UNIRAY key=SUB value="https://..."
# Пароль на веб-интерфейс (по желанию)
/container envs add list=UNIRAY key=WEB_PASSWORD value="придумайте-пароль"

# Папка для выбранного сервера и копии подписки между перезапусками (по желанию)
/container mounts add list=UNIRAY_DATA src=/usb1/uniray dst=/data

# Загрузите uniray-gateway-armv7-lowmem.tar на роутер (Files), затем:
/container add file=usb1/uniray-gateway-armv7-lowmem.tar interface=UNIRAY envlists=UNIRAY mountlists=UNIRAY_DATA \
    root-dir=/usb1/docker/uniray dns=77.88.8.8,1.1.1.1 logging=yes start-on-boot=yes
/container start [find interface=UNIRAY]
```

Веб-интерфейс: `http://192.168.254.4:9090`. Прокси: `192.168.254.4:1080` (SOCKS5 и HTTP).

Без `mountlists` контейнер тоже работает, но после перезапуска выберет сервер заново (переменная `SERVER` или первый в списке) и скачает подписку с нуля. HWID при этом не меняется: он считается от MAC-адреса veth.

### DNS

Через `dns=` контейнера Xray находит адреса VPN-серверов. Эти DNS-серверы контейнер всегда опрашивает напрямую, мимо туннеля. **Если у роутера мобильный аплинк (LTE)**, укажите DNS оператора: в режиме «белых списков» другие серверы могут не отвечать. Всё остальное — DNS клиентов, трафик, DoH — идёт через VPN.

## Как направить устройства через контейнер

Роутер помечает трафик выбранных устройств и отправляет его на `192.168.254.4`, остальной идёт как обычно.

```routeros
/routing table add name=to-uniray fib
/ip route add dst-address=0.0.0.0/0 gateway=192.168.254.4 routing-table=to-uniray

# Кого пускать через VPN: отдельные устройства или вся сеть
/ip firewall address-list add list=via-uniray address=192.168.88.0/24
# Что никогда не отправлять в VPN: локальные сети
/ip firewall address-list
add list=not-via-uniray address=10.0.0.0/8
add list=not-via-uniray address=172.16.0.0/12
add list=not-via-uniray address=192.168.0.0/16

/ip firewall mangle
add chain=prerouting src-address-list=via-uniray dst-address-list=!not-via-uniray \
    connection-state=new action=mark-connection new-connection-mark=uniray passthrough=yes
add chain=prerouting connection-mark=uniray src-address-list=via-uniray \
    action=mark-routing new-routing-mark=to-uniray passthrough=no

# FastTrack пропускает mangle: без этого часть пакетов пойдёт мимо VPN
/ip firewall filter set [find action=fasttrack-connection] connection-mark=no-mark
```

Исходящий трафик самого контейнера к VPN-серверам идёт с адреса `192.168.254.4` и выходит в интернет через обычный masquerade.

> Настройки RouterOS выше составлены по аналогии с проверенными схемами, но на реальном MikroTik не проверялись. Сам контейнер проверен в Docker с лимитом 64 МБ: шлюз для соседнего контейнера, прокси-порт, балансировщики.

## Переменные

| Переменная | По умолчанию | Что делает |
| --- | --- | --- |
| `SUB`, `SUB1`, `SUB_…` | — | URL подписок |
| `SRV`, `SRV1`, `SRV_…` | — | Ссылки на серверы (`vless://`, `vmess://`, `trojan://`, `ss://`, `hysteria2://`, `wireguard://`, `vpn://` AmneziaVPN, `naive+https://`, `naive+quic://`). Конфиг AmneziaWG `.conf` можно отдать подпиской (`SUB`) |
| `SERVER` | первый | Сервер при первом запуске: часть названия |
| `WEB_PORT` | `9090` | Порт веб-интерфейса |
| `WEB_PASSWORD` | — | Пароль веб-интерфейса (HTTP Basic, любое имя) |
| `MIXED_PORT` | `1080` | Порт SOCKS5/HTTP-прокси |
| `TUN` | `true` | Режим шлюза; `false` — только прокси |
| `UPDATE_INTERVAL` | из подписки, иначе 12 | Часы между обновлениями подписки |
| `SEND_HWID` | `true` | Отправлять HWID (нужен для панелей с лимитом устройств, Remnawave) |
| `HWID` | от MAC veth | Задать HWID вручную |
| `DEVICE_MODEL` | `MikroTik` | Модель устройства для панели |
| `REMOTE_DNS` | `https://1.1.1.1/dns-query` | DNS через VPN |
| `DIRECT_DNS` | `localhost` (= `dns=` контейнера) | DNS для адресов VPN-серверов |
| `LOG_LEVEL` | `warning` | Уровень журнала Xray |
| `DATA_DIR` | `/data` | Куда сохранять выбор и подписку |
| `ENGINE_BIN`, `NAIVE_BIN` | `/usr/bin/uniray-engine`, `/usr/bin/naive` | Движок и `naive`; без них WireGuard/AmneziaWG и Naive-серверы пропускаются |
