package main

import (
	"context"
	"errors"
	"fmt"
	"net"
	"net/netip"
	"strconv"
	"strings"
	"sync"
	"time"

	"github.com/amnezia-vpn/amneziawg-go/v3/conn"
	"github.com/amnezia-vpn/amneziawg-go/v3/device"
	"github.com/amnezia-vpn/amneziawg-go/v3/tun/netstack"
)

// AWGConfig is the tunnel: the UAPI text the app generated (keys, the
// AmneziaWG options, peers with resolved endpoints) and the interface side
// the userspace network stack needs.
type AWGConfig struct {
	UAPI      string   `json:"uapi"`
	Addresses []string `json:"addresses"`
	DNS       []string `json:"dns"`
	MTU       int      `json:"mtu"`
	// "error" (default), "verbose" for debugging the handshake.
	LogLevel string `json:"logLevel"`
}

// DefaultDNS resolves names inside the tunnel when the config names no
// server. Queries go through the tunnel, never around it.
var DefaultDNS = []string{"1.1.1.1", "2606:4700:4700::1111"}

type awgTunnel struct {
	device *device.Device
	net    *netstack.Net
	stop   chan struct{}
	once   sync.Once
}

func startAWG(config *AWGConfig) (Upstream, error) {
	addresses, err := parsePrefixAddrs(config.Addresses)
	if err != nil {
		return nil, err
	}
	dnsList := config.DNS
	if len(dnsList) == 0 {
		dnsList = DefaultDNS
	}
	var dns []netip.Addr
	for _, server := range dnsList {
		if addr, err := netip.ParseAddr(strings.TrimSpace(server)); err == nil {
			dns = append(dns, addr)
		}
	}
	mtu := config.MTU
	if mtu <= 0 {
		mtu = 1420
	}

	tunDevice, tnet, err := netstack.CreateNetTUN(addresses, dns, mtu)
	if err != nil {
		return nil, fmt.Errorf("network stack: %w", err)
	}
	if err := tuneTCP(tnet); err != nil {
		return nil, fmt.Errorf("TCP of the network stack: %w", err)
	}
	level := device.LogLevelError
	if config.LogLevel == "verbose" {
		level = device.LogLevelVerbose
	}
	dev := device.NewDevice(tunDevice, conn.NewDefaultBind(), device.NewLogger(level, "[Info] awg: "))
	if err := dev.IpcSet(config.UAPI); err != nil {
		dev.Close()
		return nil, fmt.Errorf("configuration: %w", err)
	}
	if err := dev.Up(); err != nil {
		dev.Close()
		return nil, fmt.Errorf("bringing up: %w", err)
	}
	tunnel := &awgTunnel{device: dev, net: tnet, stop: make(chan struct{})}
	go tunnel.reportHandshakes()
	return tunnel, nil
}

// Addresses come as "10.8.1.2/32"; the stack wants the bare address.
func parsePrefixAddrs(list []string) ([]netip.Addr, error) {
	var addrs []netip.Addr
	for _, item := range list {
		item = strings.TrimSpace(item)
		if prefix, err := netip.ParsePrefix(item); err == nil {
			addrs = append(addrs, prefix.Addr())
			continue
		}
		addr, err := netip.ParseAddr(item)
		if err != nil {
			return nil, fmt.Errorf("bad tunnel address %q", item)
		}
		addrs = append(addrs, addr)
	}
	if len(addrs) == 0 {
		return nil, errors.New("no tunnel address")
	}
	return addrs, nil
}

func (t *awgTunnel) DialTCP(ctx context.Context, host string, port uint16) (net.Conn, error) {
	return t.net.DialContext(ctx, "tcp", net.JoinHostPort(host, strconv.Itoa(int(port))))
}

func (t *awgTunnel) DialUDP(ctx context.Context, host string, port uint16) (net.Conn, error) {
	return t.net.DialContext(ctx, "udp", net.JoinHostPort(host, strconv.Itoa(int(port))))
}

func (t *awgTunnel) Close() error {
	t.once.Do(func() {
		close(t.stop)
		t.device.Close()
	})
	return nil
}

// reportHandshakes logs the first handshake, so the log tells a tunnel
// that came up from one that never did.
func (t *awgTunnel) reportHandshakes() {
	ticker := time.NewTicker(time.Second)
	defer ticker.Stop()
	for {
		select {
		case <-t.stop:
			return
		case <-ticker.C:
		}
		state, err := t.device.IpcGet()
		if err != nil {
			continue
		}
		for _, line := range strings.Split(state, "\n") {
			if value, ok := strings.CutPrefix(line, "last_handshake_time_sec="); ok && value != "0" {
				logf("Info", "handshake with the server done")
				return
			}
		}
	}
}
