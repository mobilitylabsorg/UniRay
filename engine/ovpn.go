package main

import (
	"bufio"
	"context"
	"encoding/json"
	"errors"
	"fmt"
	"io"
	"net"
	"net/netip"
	"os"
	"os/exec"
	"path/filepath"
	"slices"
	"strconv"
	"strings"
	"sync"
	"sync/atomic"
	"time"

	"github.com/amnezia-vpn/amneziawg-go/v3/tun"
	"github.com/amnezia-vpn/amneziawg-go/v3/tun/netstack"
)

// OVPNConfig runs an OpenVPN server through uniray-ovpn, the OpenVPN 3 core
// without a TUN device (ovpn/uniray-ovpn.cpp): it reads and writes the
// tunnel's IP packets on a socket whose other end feeds this engine's
// userspace stack, as for AmneziaWG. The socket is a socketpair on Unix and a
// loopback UDP port on Windows (see openPackets).
type OVPNConfig struct {
	Binary string `json:"binary"`
	// The .ovpn profile as the user imported it.
	Profile  string `json:"profile"`
	Username string `json:"username"`
	Password string `json:"password"`
	// Where to connect instead of the profile's remote, when set: the address
	// the app resolved it to (its rule sending these packets past the tunnel
	// matches the address), or Xray's loopback port that forwards to the
	// server through another server in front of it.
	Server string `json:"server"`
	Port   int    `json:"port"`
	// "udp" or "tcp", when Server is set.
	Proto string `json:"proto"`
}

// OVPNSettings is what the server pushed: the tunnel's address and MTU, and
// the networks, DNS servers and domains reached through it.
type OVPNSettings struct {
	MTU       int            `json:"mtu"`
	Addresses []netip.Prefix `json:"addresses"`
	Routes    []netip.Prefix `json:"routes"`
	DNS       []netip.Addr   `json:"dns"`
	Domains   []string       `json:"domains"`
}

const (
	// How long a connection waits for the tunnel to come up (again).
	ovpnDialWait = 20 * time.Second
	// Between restarts of a uniray-ovpn that ended without saying why.
	ovpnRetryMin = 2 * time.Second
	ovpnRetryMax = time.Minute
)

// ovpnUpstream keeps a session of uniray-ovpn up. It serves at once: the
// tunnel may need Xray, started after the engine is ready, to reach its
// server, so connections wait for the tunnel instead. The OpenVPN core
// reconnects on its own within a session; a session that ends on an error
// the server gave (wrong password, a bad profile) is not retried, and
// connections fail with that error.
//
// The app learns the tunnel's state from the log ("corp: up {settings}",
// "corp: down <reason>"), which it reaches by every route the engine runs.
type ovpnUpstream struct {
	child tunnelChild

	mu      sync.Mutex
	stack   *ovpnStack
	failure error
	// Closed and replaced whenever stack or failure changes.
	changed chan struct{}

	stop    chan struct{}
	once    sync.Once
	closing atomic.Bool
	done    chan struct{}
}

// tunnelChild is the program behind a corporate tunnel: uniray-ovpn, or
// uniray-cp (Check Point, see cp.go), which speaks the same protocol.
type tunnelChild struct {
	// Its name, for the log.
	name string
	// The tunnel's protocol, for errors the user sees.
	label string
	// command is its command line for one session on `channel`, what to
	// write to its stdin, and what to remove once the session is over.
	command func(channel *ovpnChannel) (cmd *exec.Cmd, input string, cleanup func(), err error)
}

// ovpnStack is the userspace network stack on the tunnel's address.
type ovpnStack struct {
	device   tun.Device
	net      *netstack.Net
	settings OVPNSettings
}

func startOVPN(config *OVPNConfig, dir string) (Upstream, error) {
	if config.Binary == "" || config.Profile == "" {
		return nil, errors.New("the config has no uniray-ovpn binary or profile")
	}
	child := tunnelChild{name: "uniray-ovpn", label: "OpenVPN", command: func(channel *ovpnChannel) (*exec.Cmd, string, func(), error) {
		// uniray-ovpn removes the profile once read; cleanup, if it never does.
		profile := filepath.Join(dir, "ovpn-"+randomHex(8)+".ovpn")
		if err := os.WriteFile(profile, []byte(config.Profile), 0o600); err != nil {
			return nil, "", nil, fmt.Errorf("profile: %w", err)
		}
		args := append([]string{"--config", profile}, channel.args...)
		if config.Server != "" {
			args = append(args, "--server", config.Server, "--port", strconv.Itoa(config.Port))
			if config.Proto != "" {
				args = append(args, "--proto", config.Proto)
			}
		}
		input := config.Username + "\n" + config.Password + "\n"
		if channel.token != "" {
			input += channel.token + "\n"
		}
		return exec.Command(config.Binary, args...), input, func() { os.Remove(profile) }, nil
	}}
	return startTunnel(child), nil
}

// startTunnel keeps sessions of `child` up, see ovpnUpstream.
func startTunnel(child tunnelChild) *ovpnUpstream {
	u := &ovpnUpstream{child: child, changed: make(chan struct{}), stop: make(chan struct{}), done: make(chan struct{})}
	go u.run()
	return u
}

func (u *ovpnUpstream) run() {
	defer close(u.done)
	retry := ovpnRetryMin
	for !u.closing.Load() {
		started := time.Now()
		err := u.session()
		if u.closing.Load() {
			return
		}
		var refused *ovpnRefused
		if errors.As(err, &refused) {
			logf("Error", "%s: down %s", corpLabel, refused.message)
			u.update(func() { u.failure = errors.New(u.child.label + ": " + refused.message) })
			return
		}
		if time.Since(started) > ovpnRetryMax {
			retry = ovpnRetryMin
		}
		logf("Warning", "%s: down %v; again in %v", corpLabel, err, retry)
		select {
		case <-u.stop:
			return
		case <-time.After(retry):
		}
		retry = min(retry*2, ovpnRetryMax)
	}
}

// ovpnRefused is the child's "error" line: the server or the profile
// refused the session, which starting again would not change.
type ovpnRefused struct{ message string }

func (e *ovpnRefused) Error() string { return e.message }

// session runs the child until it exits.
func (u *ovpnUpstream) session() error {
	channel, err := openPackets()
	if err != nil {
		return fmt.Errorf("packet socket: %w", err)
	}
	packets := channel.conn
	defer packets.Close()
	defer channel.release()

	cmd, input, cleanup, err := u.child.command(channel)
	if err != nil {
		return err
	}
	defer cleanup()
	channel.prepare(cmd)
	cmd.Stderr = os.Stderr
	stdin, err := cmd.StdinPipe()
	if err != nil {
		return err
	}
	stdout, err := cmd.StdoutPipe()
	if err != nil {
		return err
	}
	setChildAttrs(cmd)
	err = cmd.Start()
	channel.release()
	if err != nil {
		return fmt.Errorf("starting %s: %w", u.child.name, err)
	}
	// Closing stdin stops it: on Close, or when this session is over.
	stopped := make(chan struct{})
	go func() {
		select {
		case <-u.stop:
		case <-stopped:
		}
		stdin.Close()
		select {
		case <-stopped:
		case <-time.After(5 * time.Second):
			cmd.Process.Kill()
		}
	}()
	if input != "" {
		io.WriteString(stdin, input)
	}

	go u.fromServer(packets)
	refused := u.follow(stdout, packets, func() { stdin.Close() })
	waitErr := cmd.Wait()
	close(stopped)
	u.update(func() { u.setStack(nil) })
	if refused != nil {
		return refused
	}
	if waitErr != nil {
		return fmt.Errorf("%s: %w", u.child.name, waitErr)
	}
	return fmt.Errorf("%s ended", u.child.name)
}

// follow reads uniray-ovpn's lines until it exits: the tunnel's settings,
// then "connected" each time the tunnel is up. Returns its "error", if any;
// after one it is told to stop with `quit`, should it not have already.
func (u *ovpnUpstream) follow(stdout io.Reader, packets ovpnPackets, quit func()) error {
	lines := bufio.NewScanner(stdout)
	var pending OVPNSettings
	var refused error
	for lines.Scan() {
		key, value, _ := strings.Cut(lines.Text(), " ")
		switch key {
		case "tun":
			pending = OVPNSettings{}
			pending.MTU, _ = strconv.Atoi(value)
		case "address":
			if prefix, err := netip.ParsePrefix(value); err == nil {
				pending.Addresses = append(pending.Addresses, prefix)
			}
		case "route":
			if prefix, err := netip.ParsePrefix(value); err == nil {
				pending.Routes = append(pending.Routes, prefix.Masked())
			}
		case "dns":
			if addr, err := netip.ParseAddr(value); err == nil && !slices.Contains(pending.DNS, addr) {
				pending.DNS = append(pending.DNS, addr)
			}
		case "domain":
			if !slices.Contains(pending.Domains, value) {
				pending.Domains = append(pending.Domains, value)
			}
		case "connected":
			if err := u.up(pending, packets); err != nil {
				logf("Error", "%s: down %v", corpLabel, err)
			}
		case "reconnecting":
			logf("Warning", "%s: reconnecting", corpLabel)
		case "error":
			refused = &ovpnRefused{message: value}
			quit()
		}
	}
	return refused
}

// up puts the tunnel to use with `settings`: the stack it has, when the
// address is the same (connections survive a reconnect), else a new one.
func (u *ovpnUpstream) up(settings OVPNSettings, packets ovpnPackets) error {
	if len(settings.Addresses) == 0 {
		return fmt.Errorf("the %s server gave no tunnel address", u.child.label)
	}
	if settings.MTU <= 0 {
		settings.MTU = 1500
	}
	u.mu.Lock()
	current := u.stack
	u.mu.Unlock()
	if current == nil || !slices.Equal(current.settings.Addresses, settings.Addresses) || current.settings.MTU != settings.MTU {
		addresses := make([]netip.Addr, 0, len(settings.Addresses))
		for _, prefix := range settings.Addresses {
			addresses = append(addresses, prefix.Addr())
		}
		dns := settings.DNS
		if len(dns) == 0 {
			for _, server := range DefaultDNS {
				dns = append(dns, netip.MustParseAddr(server))
			}
		}
		device, tnet, err := netstack.CreateNetTUN(addresses, dns, settings.MTU)
		if err != nil {
			return fmt.Errorf("network stack: %w", err)
		}
		if err := tuneTCP(tnet); err != nil {
			device.Close()
			return fmt.Errorf("TCP of the network stack: %w", err)
		}
		current = &ovpnStack{device: device, net: tnet}
		go toServer(current.device, packets)
	}
	current.settings = settings
	u.update(func() {
		u.setStack(current)
		u.failure = nil
	})
	report, _ := json.Marshal(settings)
	logf("Info", "%s: up %s", corpLabel, report)
	return nil
}

// setStack, under mu: replaces the stack, closing the one it replaces.
func (u *ovpnUpstream) setStack(stack *ovpnStack) {
	if u.stack != nil && u.stack != stack {
		u.stack.device.Close()
	}
	u.stack = stack
}

func (u *ovpnUpstream) update(change func()) {
	u.mu.Lock()
	defer u.mu.Unlock()
	change()
	close(u.changed)
	u.changed = make(chan struct{})
}

// fromServer feeds the packets uniray-ovpn decrypted to the stack of the
// moment, until the session's socket closes.
func (u *ovpnUpstream) fromServer(packets ovpnPackets) {
	buf := make([]byte, 65535)
	for {
		n, err := packets.Read(buf)
		if err != nil {
			return
		}
		u.mu.Lock()
		stack := u.stack
		u.mu.Unlock()
		if stack == nil {
			continue
		}
		if _, err := stack.device.Write([][]byte{buf[:n]}, 0); err != nil && !u.closing.Load() {
			logf("Warning", "%s: to the stack: %v", corpLabel, err)
		}
	}
}

// toServer hands a stack's packets to uniray-ovpn to encrypt and send,
// until the stack or the socket closes.
func toServer(device tun.Device, packets ovpnPackets) {
	batch := device.BatchSize()
	bufs := make([][]byte, batch)
	for i := range bufs {
		bufs[i] = make([]byte, 65535)
	}
	sizes := make([]int, batch)
	for {
		n, err := device.Read(bufs, sizes, 0)
		if err != nil {
			return
		}
		for i := 0; i < n; i++ {
			if _, err := packets.Write(bufs[i][:sizes[i]]); err != nil {
				if errors.Is(err, net.ErrClosed) {
					return
				}
				// Else a full buffer: the packet is dropped, as a TUN would.
			}
		}
	}
}

// net waits for the tunnel to be up, at most ovpnDialWait.
func (u *ovpnUpstream) net(ctx context.Context) (*netstack.Net, error) {
	timeout := time.NewTimer(ovpnDialWait)
	defer timeout.Stop()
	for {
		u.mu.Lock()
		stack, failure, changed := u.stack, u.failure, u.changed
		u.mu.Unlock()
		switch {
		case stack != nil:
			return stack.net, nil
		case failure != nil:
			return nil, failure
		case u.closing.Load():
			return nil, net.ErrClosed
		}
		select {
		case <-changed:
		case <-ctx.Done():
			return nil, ctx.Err()
		case <-timeout.C:
			return nil, fmt.Errorf("the %s tunnel is not up", u.child.label)
		}
	}
}

func (u *ovpnUpstream) DialTCP(ctx context.Context, host string, port uint16) (net.Conn, error) {
	tnet, err := u.net(ctx)
	if err != nil {
		return nil, err
	}
	return tnet.DialContext(ctx, "tcp", net.JoinHostPort(host, strconv.Itoa(int(port))))
}

func (u *ovpnUpstream) DialUDP(ctx context.Context, host string, port uint16) (net.Conn, error) {
	tnet, err := u.net(ctx)
	if err != nil {
		return nil, err
	}
	return tnet.DialContext(ctx, "udp", net.JoinHostPort(host, strconv.Itoa(int(port))))
}

// Close ends the session: uniray-ovpn stops when its stdin closes.
func (u *ovpnUpstream) Close() error {
	u.once.Do(func() {
		u.closing.Store(true)
		close(u.stop)
		<-u.done
		u.update(func() { u.setStack(nil) })
	})
	return nil
}
