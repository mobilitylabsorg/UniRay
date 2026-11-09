# TUN-стек Xray за каналом с потерями

Стенд к замерам в [README](../../README.md#tun-bbr): клиент в своём network namespace → veth с `netem` в обе стороны (задержка, потери, перестановка) → namespace с Xray, который заворачивает трафик в свой TUN → freedom → локальный источник на 127.0.0.1. Root не нужен: всё в user namespace (`unshare -rn`).

```sh
# run.sh <xray> <down|up> <потоков> <секунд> [параметры netem...]
unshare -rn sh bench/tun/run.sh src-tauri/binaries/uniray-xray-x86_64-unknown-linux-gnu down 1 6 delay 0.2ms loss 2%
```

Выводит МБ/с и секунды CPU Xray на гигабайт (`GOMAXPROCS=2`). Лог Xray — в файл из `LOG`. Сценарии из таблицы:

| Сценарий | netem |
| --- | --- |
| LAN без потерь | `delay 0.2ms` |
| 0,5% / 2% потерь | `delay 0.2ms loss 0.5%` / `loss 2%` |
| перестановка | `delay 0.2ms 0.05ms` |
| Wi-Fi | `delay 2ms 1ms loss 1%` |
| RTT 20 мс, 1% потерь | `delay 10ms loss 1%` |
| 100 Мбит, очередь 100 пакетов | `rate 100mbit delay 5ms limit 100` |
