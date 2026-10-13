package main

import (
	"context"
	"fmt"
	"net"
	"net/http"
	"strconv"
	"time"
)

// probe measures the server's delay the way the app's delay test does for
// Xray servers: the time of one HTTPS request through it. It needs no SOCKS
// port at all, so nothing listens while it runs.
func probe(upstream Upstream, url string, timeout time.Duration) int64 {
	transport := &http.Transport{
		DialContext: func(ctx context.Context, _, address string) (net.Conn, error) {
			host, portText, err := net.SplitHostPort(address)
			if err != nil {
				return nil, err
			}
			port, err := strconv.ParseUint(portText, 10, 16)
			if err != nil {
				return nil, err
			}
			return upstream.DialTCP(ctx, host, uint16(port))
		},
		DisableKeepAlives: true,
	}
	client := &http.Client{Transport: transport, Timeout: timeout}
	started := time.Now()
	response, err := client.Get(url)
	if err != nil {
		logf("Info", "delay test failed: %v", err)
		return -1
	}
	defer response.Body.Close()
	if response.StatusCode < 200 || response.StatusCode >= 300 {
		logf("Info", "delay test got HTTP %d", response.StatusCode)
		return -1
	}
	return time.Since(started).Milliseconds()
}

func printDelay(delay int64) {
	fmt.Printf("delay %d\n", delay)
}
