// delay: a UDP relay that holds every packet for a while, both ways, in
// order (a FIFO per direction), optionally losing some.
// delay <listen addr> <server addr> <one-way delay> [loss, 0.005 for 0.5%]
package main

import (
	"math/rand"
	"net"
	"os"
	"strconv"
	"sync"
	"time"
)

type packet struct {
	due  time.Time
	data []byte
}

// line delivers packets after the delay, in the order they came.
func line(d time.Duration, send func([]byte)) chan<- packet {
	ch := make(chan packet, 1<<16)
	go func() {
		for p := range ch {
			if wait := time.Until(p.due); wait > 0 {
				time.Sleep(wait)
			}
			send(p.data)
		}
	}()
	return ch
}

func main() {
	listen, _ := net.ResolveUDPAddr("udp", os.Args[1])
	server, _ := net.ResolveUDPAddr("udp", os.Args[2])
	d, _ := time.ParseDuration(os.Args[3])
	loss := 0.0
	if len(os.Args) > 4 {
		loss, _ = strconv.ParseFloat(os.Args[4], 64)
	}
	front, err := net.ListenUDP("udp", listen)
	if err != nil {
		panic(err)
	}
	_ = front.SetReadBuffer(8 << 20)
	_ = front.SetWriteBuffer(8 << 20)
	var mu sync.Mutex
	type peer struct {
		back *net.UDPConn
		up   chan<- packet
	}
	peers := map[string]peer{}
	for {
		buf := make([]byte, 65536)
		n, client, err := front.ReadFromUDP(buf)
		if err != nil {
			panic(err)
		}
		mu.Lock()
		p, ok := peers[client.String()]
		if !ok {
			back, _ := net.DialUDP("udp", nil, server)
			_ = back.SetReadBuffer(8 << 20)
			_ = back.SetWriteBuffer(8 << 20)
			client := client
			down := line(d, func(b []byte) { _, _ = front.WriteToUDP(b, client) })
			p = peer{back: back, up: line(d, func(b []byte) { _, _ = back.Write(b) })}
			peers[client.String()] = p
			go func() {
				for {
					b := make([]byte, 65536)
					n, err := back.Read(b)
					if err != nil {
						return
					}
					if rand.Float64() >= loss {
						down <- packet{time.Now().Add(d), b[:n]}
					}
				}
			}()
		}
		mu.Unlock()
		if rand.Float64() >= loss {
			p.up <- packet{time.Now().Add(d), buf[:n]}
		}
	}
}
