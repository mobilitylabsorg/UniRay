//go:build uniray_lowmem

package main

// uniray_lowmem (the MikroTik image's -lowmem engine): gVisor's default, as
// every upload holds its buffer; 80 Mbit/s over 100 ms is more than such a
// router's uplink usually gives.
const tcpSendBuffer = 1 << 20
