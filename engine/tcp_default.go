//go:build !uniray_lowmem

package main

// 4 MiB: about 320 Mbit/s over 100 ms.
const tcpSendBuffer = 4 << 20
