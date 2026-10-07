# UniRay Gateway для MikroTik

Контейнер для RouterOS: подключается к VPN через Xray, работает шлюзом для выбранных устройств сети и даёт SOCKS/HTTP-прокси. Сервер переключается в веб-интерфейсе, подписка обновляется сама.

- Образ: `uniray-gateway-armv7.tar` (~15 МБ; на диске ~40 МБ, из них Xray 36 МБ).
- Память: ~23 МБ в простое, ~26 МБ под нагрузкой. Рассчитан на лимит 64 МБ.
- Раздельной маршрутизации нет: всё, что роутер отправит в контейнер, идёт через VPN. Если в подписке есть правила провайдера с прямым доступом к отдельным сайтам (например, RU-домены), в контейнере они отключаются. Блокировки провайдера (например, BitTorrent) остаются.

## Сборка

```sh
scripts/build-gateway.sh                 # linux/arm/v7 → build/uniray-gateway-armv7.tar
scripts/build-gateway.sh linux/arm64     # другие варианты: linux/arm64, linux/amd64
```

Нужны Docker с buildx и [cross](https://github.com/cross-rs/cross).

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

# Загрузите uniray-gateway-armv7.tar на роутер (Files), затем:
/container add file=usb1/uniray-gateway-armv7.tar interface=UNIRAY envlists=UNIRAY mountlists=UNIRAY_DATA \
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
| `SRV`, `SRV1`, `SRV_…` | — | Ссылки на серверы (`vless://`, `vmess://`, `trojan://`, `ss://`, `hysteria2://`) |
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
