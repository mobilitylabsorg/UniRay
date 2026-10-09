# Стенд скорости движка

Замеры скорости `uniray-engine` (с BBRv3 из форка gVisor, см. `engine/tcp.go`) против эталона: для AmneziaWG/WireGuard — TCP ядра через TUN (как в AmneziaVPN), для NaiveProxy — `naive` без UniRay. Всё в network namespaces, нужен root. С этого стенда сняты цифры раздела «Upload через WireGuard/AmneziaWG» в корневом README.

```sh
bash engine/bench/build.sh /tmp/bench-bin      # движок, amneziawg-go, ретранслятор, Caddy, naive

# AmneziaWG: RTT 60 мс (30 мс в каждую сторону), 0,1% потерь
sudo LOSS=0.001 KERNEL_CC="cubic bbr" bash engine/bench/awg.sh /tmp/bench-bin 30
# с опциями AmneziaWG (в формате UAPI)
sudo bash engine/bench/awg.sh /tmp/bench-bin 30 "jc=4 jmin=10 jmax=50 s1=12 s2=12 s3=12 s4=12 h1=1 h2=2 h3=3 h4=4 random_trailers=true"

# NaiveProxy: через движок, напрямую, чистый TCP
sudo bash engine/bench/naive.sh /tmp/bench-bin
```

| Файл | Что делает |
| --- | --- |
| `awg.sh` | Сервер amneziawg-go и два клиента: движок (SOCKS → gVisor) и amneziawg-go с TUN в ядре. Переменные: `LOSS`, `MB`, `KERNEL_CC`, `ENGINE` (другой бинарник движка для сравнения, например собранный без `bbr.Register()`), `PCAP`. |
| `naive.sh` | Caddy с forwardproxy (форк klzgrad, TLS, padding) и три пути до приёмника: `naive` через движок, `naive` сам по себе, чистый TCP. На время прогона добавляет свой CA в `/etc/ssl/certs/ca-certificates.crt` и убирает его при любом выходе: `naive` доверяет только системным сертификатам. |
| `delay/` | UDP-ретранслятор перед сервером: задержка в каждую сторону и потери, **с сохранением порядка пакетов**. В ядре здесь нет `netem`, а WireGuard ходит только по UDP. |
| `client.py`, `sink.py` | Загрузка и скачивание N МиБ через SOCKS5 (или напрямую) до приёмника; приёмник подтверждает конец загрузки байтом, не полагаясь на полузакрытие через прокси. |
| `sniff.py` | Запись TCP-сегментов на интерфейсе (`PCAP=файл` в `awg.sh`): время, seq/ack, длина, SACK. Видно перестановки и дыры. |
| `build.sh` | Сборка всего этого в одну папку; Caddy — во временном модуле (версии закреплены в скрипте). |

Чему учит стенд:
- **Порядок пакетов важен.** gVisor очень плохо переносит перестановку: первая версия ретранслятора (таймер на каждый пакет) переставляла пакеты, и upload движка падал в 10 раз, хотя потерь не было. Перед выводами проверяйте `PCAP`: доля сегментов не по порядку должна быть около нуля.
- **Сравнивайте с тем же алгоритмом ядра.** В песочнице по умолчанию стоит BBR, а на Android и Windows обычно CUBIC. `KERNEL_CC="cubic bbr"` показывает оба.
- **Разброс большой**, особенно при потерях: делайте несколько прогонов.
