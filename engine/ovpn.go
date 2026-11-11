//go:build !windows

package main

import (
	"bufio"
	"context"
	"errors"
	"fmt"
	"io"
	"net"
	"net/netip"
	"os"
	"os/exec"
	"path/filepath"
	"strconv"
	"strings"
	"sync"
	"sync/atomic"
	"syscall"
	"time"

	"github.com/amnezia-vpn/amneziawg-go/v3/tun"
	"github.com/amnezia-vpn/amneziawg-go/v3/tun/netstack"
)

// OVPNConfig runs an OpenVPN server through uniray-ovpn, the OpenVPN 3 core
// without a TUN device (ovpn/uniray-ovpn.cpp): it reads and writes the
// tunnel's IP packets on a socket whose other end feeds this engine's
// userspace stack, as for AmneziaWG.
type OVPNConfig struct {
	Binary string `json:"binary"`
	// The .ovpn profile as the user imported it.
	Profile  string `json:"profile"`
	Username string `json:"username"`
	Password string `json:"password"`
}

// OVPNSettings is what the server pushed: the tunnel's address and MTU, and
// the networks, DNS servers and domains reached through it.
type OVPNSettings struct {
	MTU       int
	Addresses []netip.Prefix
	Routes    []netip.Prefix
	DNS       []netip.Addr
	Domains   []string
}

const ovpnStartTimeout = 30 * time.Second

type ovpnTunnel struct {
	cmd      *exec.Cmd
	stdin    io.WriteCloser
	packets  *net.UnixConn
	device   tun.Device
	net      *netstack.Net
	settings OVPNSettings
	exited   chan struct{}
	once     sync.Once
	closing  atomic.Bool
}

// startOVPN starts uniray-ovpn with the profile in `dir` (private to the
// app; uniray-ovpn removes the file once read) and the credentials on its
// stdin, waits for the tunnel's settings, and brings the stack up with them.
// onExit is called if uniray-ovpn ends on its own (not through Close).
func startOVPN(config *OVPNConfig, dir string, onExit func(error)) (*ovpnTunnel, error) {
	if config.Binary == "" || config.Profile == "" {
		return nil, errors.New("the config has no uniray-ovpn binary or profile")
	}
	// Datagrams keep each packet whole, as a TUN does.
	fds, err := syscall.Socketpair(syscall.AF_UNIX, syscall.SOCK_DGRAM|syscall.SOCK_CLOEXEC, 0)
	if err != nil {
		return nil, fmt.Errorf("packet socket: %w", err)
	}
	ours, theirs := os.NewFile(uintptr(fds[0]), "ovpn-packets"), os.NewFile(uintptr(fds[1]), "ovpn-packets-child")
	defer theirs.Close()
	conn, err := net.FileConn(ours)
	ours.Close()
	if err != nil {
		return nil, fmt.Errorf("packet socket: %w", err)
	}
	packets := conn.(*net.UnixConn)

	profile := filepath.Join(dir, "ovpn-"+randomHex(8)+".ovpn")
	if err := os.WriteFile(profile, []byte(config.Profile), 0o600); err != nil {
		packets.Close()
		return nil, fmt.Errorf("profile: %w", err)
	}
	cmd := exec.Command(config.Binary, "--config", profile, "--packet-fd", "3")
	cmd.ExtraFiles = []*os.File{theirs}
	cmd.Stderr = os.Stderr
	stdin, err := cmd.StdinPipe()
	if err != nil {
		packets.Close()
		return nil, err
	}
	stdout, err := cmd.StdoutPipe()
	if err != nil {
		packets.Close()
		return nil, err
	}
	setChildAttrs(cmd)
	if err := cmd.Start(); err != nil {
		os.Remove(profile)
		packets.Close()
		return nil, fmt.Errorf("starting uniray-ovpn: %w", err)
	}
	t := &ovpnTunnel{cmd: cmd, stdin: stdin, packets: packets, exited: make(chan struct{})}
	if _, err := fmt.Fprintf(stdin, "%s\n%s\n", config.Username, config.Password); err != nil {
		t.Close()
		return nil, fmt.Errorf("credentials: %w", err)
	}

	lines := bufio.NewScanner(stdout)
	ready := make(chan error, 1)
	go func() {
		ready <- t.readSettings(lines)
		// After the settings: "connected"/"reconnecting" for the log, and
		// "error" when the session ends.
		for lines.Scan() {
			logf("Info", "ovpn: %s", lines.Text())
		}
	}()
	go func() {
		err := cmd.Wait()
		close(t.exited)
		os.Remove(profile)
		if !t.closing.Load() {
			onExit(err)
		}
	}()

	select {
	case err := <-ready:
		if err != nil {
			t.Close()
			return nil, err
		}
	case <-t.exited:
		return nil, errors.New("uniray-ovpn exited before the tunnel came up")
	case <-time.After(ovpnStartTimeout):
		t.Close()
		return nil, errors.New("the OpenVPN server did not answer in time")
	}

	addresses := make([]netip.Addr, 0, len(t.settings.Addresses))
	for _, prefix := range t.settings.Addresses {
		addresses = append(addresses, prefix.Addr())
	}
	dns := t.settings.DNS
	if len(dns) == 0 {
		for _, server := range DefaultDNS {
			dns = append(dns, netip.MustParseAddr(server))
		}
	}
	t.device, t.net, err = netstack.CreateNetTUN(addresses, dns, t.settings.MTU)
	if err != nil {
		t.Close()
		return nil, fmt.Errorf("network stack: %w", err)
	}
	if err := tuneTCP(t.net); err != nil {
		t.Close()
		return nil, fmt.Errorf("TCP of the network stack: %w", err)
	}
	go t.fromServer()
	go t.toServer()
	logf("Info", "ovpn: up, %v, routes %v, DNS %v, domains %v", t.settings.Addresses, t.settings.Routes, t.settings.DNS, t.settings.Domains)
	return t, nil
}

// readSettings takes uniray-ovpn's lines up to the first "connected".
func (t *ovpnTunnel) readSettings(lines *bufio.Scanner) error {
	for lines.Scan() {
		key, value, _ := strings.Cut(lines.Text(), " ")
		switch key {
		case "tun":
			t.settings = OVPNSettings{}
			t.settings.MTU, _ = strconv.Atoi(value)
		case "address":
			if prefix, err := netip.ParsePrefix(value); err == nil {
				t.settings.Addresses = append(t.settings.Addresses, prefix)
			}
		case "route":
			if prefix, err := netip.ParsePrefix(value); err == nil {
				t.settings.Routes = append(t.settings.Routes, prefix.Masked())
			}
		case "dns":
			if addr, err := netip.ParseAddr(value); err == nil {
				t.settings.DNS = append(t.settings.DNS, addr)
			}
		case "domain":
			t.settings.Domains = append(t.settings.Domains, value)
		case "connected":
			if len(t.settings.Addresses) == 0 {
				return errors.New("the OpenVPN server gave no tunnel address")
			}
			if t.settings.MTU <= 0 {
				t.settings.MTU = 1500
			}
			return nil
		case "error":
			return fmt.Errorf("OpenVPN: %s", value)
		}
	}
	return errors.New("uniray-ovpn ended before the tunnel came up")
}

// fromServer feeds the packets uniray-ovpn decrypted to the stack.
func (t *ovpnTunnel) fromServer() {
	buf := make([]byte, 65535)
	for {
		n, err := t.packets.Read(buf)
		if err != nil {
			return
		}
		if _, err := t.device.Write([][]byte{buf[:n]}, 0); err != nil && !t.closing.Load() {
			logf("Warning", "ovpn: to the stack: %v", err)
		}
	}
}

// toServer hands the stack's packets to uniray-ovpn to encrypt and send.
func (t *ovpnTunnel) toServer() {
	batch := t.device.BatchSize()
	bufs := make([][]byte, batch)
	for i := range bufs {
		bufs[i] = make([]byte, 65535)
	}
	sizes := make([]int, batch)
	for {
		n, err := t.device.Read(bufs, sizes, 0)
		if err != nil {
			return
		}
		for i := 0; i < n; i++ {
			if _, err := t.packets.Write(bufs[i][:sizes[i]]); err != nil {
				if !t.closing.Load() {
					logf("Warning", "ovpn: to the server: %v", err)
				}
				return
			}
		}
	}
}

func (t *ovpnTunnel) DialTCP(ctx context.Context, host string, port uint16) (net.Conn, error) {
	return t.net.DialContext(ctx, "tcp", net.JoinHostPort(host, strconv.Itoa(int(port))))
}

func (t *ovpnTunnel) DialUDP(ctx context.Context, host string, port uint16) (net.Conn, error) {
	return t.net.DialContext(ctx, "udp", net.JoinHostPort(host, strconv.Itoa(int(port))))
}

// Close ends the session: uniray-ovpn stops when its stdin closes.
func (t *ovpnTunnel) Close() error {
	t.once.Do(func() {
		t.closing.Store(true)
		t.stdin.Close()
		select {
		case <-t.exited:
		case <-time.After(5 * time.Second):
			if t.cmd.Process != nil {
				t.cmd.Process.Kill()
			}
		}
		t.packets.Close()
		if t.device != nil {
			t.device.Close()
		}
	})
	return nil
}
