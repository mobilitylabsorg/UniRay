package main

import (
	"context"
	"crypto/rand"
	"encoding/binary"
	"encoding/hex"
	"encoding/json"
	"errors"
	"fmt"
	"io"
	"net"
	"net/netip"
	"os"
	"os/exec"
	"path/filepath"
	"strconv"
	"sync"
	"sync/atomic"
	"time"
)

// NaiveConfig runs a NaiveProxy server through the official `naive`: only
// Chromium's network stack makes its connections look like a browser's, so
// the engine does not reimplement it but starts naive as its child and
// passes CONNECTs on to it.
type NaiveConfig struct {
	Binary string `json:"binary"`
	// naive's config.json without "listen", which the engine adds: a SOCKS5
	// server on loopback with credentials only the engine knows.
	Config map[string]any `json:"config"`
}

const naiveStartTimeout = 15 * time.Second

type naiveUpstream struct {
	cmd        *exec.Cmd
	addr       string
	user, pass string
	exited     chan struct{}
	once       sync.Once
	closing    atomic.Bool
}

// startNaive starts naive with its config in `dir` (private to the app; the
// file holds the server's password and is removed once naive listens) and
// calls onExit if naive ends on its own (not through Close).
func startNaive(config *NaiveConfig, dir string, onExit func(error)) (Upstream, error) {
	if config.Binary == "" || config.Config == nil {
		return nil, errors.New("the config has no naive binary or naive config")
	}
	port, err := freePort()
	if err != nil {
		return nil, err
	}
	upstream := &naiveUpstream{
		addr:   net.JoinHostPort("127.0.0.1", strconv.Itoa(port)),
		user:   "uniray",
		pass:   randomHex(32),
		exited: make(chan struct{}),
	}
	naiveConfig := map[string]any{}
	for key, value := range config.Config {
		naiveConfig[key] = value
	}
	naiveConfig["listen"] = fmt.Sprintf("socks://%s:%s@%s", upstream.user, upstream.pass, upstream.addr)
	data, err := json.Marshal(naiveConfig)
	if err != nil {
		return nil, err
	}
	file, err := os.CreateTemp(dir, "naive-*.json")
	if err != nil {
		return nil, err
	}
	path := file.Name()
	defer os.Remove(path)
	_, err = file.Write(data)
	if closeErr := file.Close(); err == nil {
		err = closeErr
	}
	if err != nil {
		return nil, err
	}

	upstream.cmd = exec.Command(config.Binary, path)
	upstream.cmd.Dir = filepath.Dir(path)
	upstream.cmd.Stdout = os.Stderr // naive's log, into ours
	upstream.cmd.Stderr = os.Stderr
	setChildAttrs(upstream.cmd)
	if err := upstream.cmd.Start(); err != nil {
		return nil, fmt.Errorf("cannot start naive: %w", err)
	}
	go func() {
		err := upstream.cmd.Wait()
		close(upstream.exited)
		if onExit != nil && !upstream.closing.Load() {
			onExit(err)
		}
	}()

	deadline := time.Now().Add(naiveStartTimeout)
	for {
		conn, err := net.DialTimeout("tcp", upstream.addr, time.Second)
		if err == nil {
			conn.Close()
			return upstream, nil
		}
		select {
		case <-upstream.exited:
			return nil, errors.New("naive exited during startup (see its log above)")
		case <-time.After(100 * time.Millisecond):
		}
		if time.Now().After(deadline) {
			upstream.Close()
			return nil, errors.New("naive did not start listening in time")
		}
	}
}

func (n *naiveUpstream) DialTCP(ctx context.Context, host string, port uint16) (net.Conn, error) {
	var dialer net.Dialer
	conn, err := dialer.DialContext(ctx, "tcp", n.addr)
	if err != nil {
		return nil, err
	}
	if deadline, ok := ctx.Deadline(); ok {
		_ = conn.SetDeadline(deadline)
	}
	if err := socksConnect(conn, n.user, n.pass, host, port); err != nil {
		conn.Close()
		return nil, err
	}
	_ = conn.SetDeadline(time.Time{})
	return conn, nil
}

// errNoUDP: naive tunnels TCP only. Xray's config refuses UDP for such a
// server, so this is only a safety net.
var errNoUDP = errors.New("NaiveProxy carries no UDP")

func (n *naiveUpstream) DialUDP(context.Context, string, uint16) (net.Conn, error) {
	return nil, errNoUDP
}

func (n *naiveUpstream) Close() error {
	n.once.Do(func() {
		n.closing.Store(true)
		if n.cmd.Process == nil {
			return
		}
		if terminate(n.cmd.Process) != nil {
			_ = n.cmd.Process.Kill()
		}
		select {
		case <-n.exited:
		case <-time.After(3 * time.Second):
			_ = n.cmd.Process.Kill()
		}
	})
	return nil
}

// socksConnect does a SOCKS5 handshake with username/password and a CONNECT.
func socksConnect(conn net.Conn, user, pass, host string, port uint16) error {
	if _, err := conn.Write([]byte{5, 1, 2}); err != nil {
		return err
	}
	reply := make([]byte, 2)
	if _, err := io.ReadFull(conn, reply); err != nil {
		return err
	}
	if reply[0] != 5 || reply[1] != 2 {
		return errors.New("naive refused password authentication")
	}
	auth := []byte{1, byte(len(user))}
	auth = append(auth, user...)
	auth = append(auth, byte(len(pass)))
	auth = append(auth, pass...)
	if _, err := conn.Write(auth); err != nil {
		return err
	}
	if _, err := io.ReadFull(conn, reply); err != nil {
		return err
	}
	if reply[1] != 0 {
		return errors.New("naive refused the credentials")
	}

	request := []byte{5, 1, 0}
	if addr, err := netip.ParseAddr(host); err == nil {
		if addr.Is4() {
			request = append(request, 1)
		} else {
			request = append(request, 4)
		}
		request = append(request, addr.AsSlice()...)
	} else {
		if len(host) > 255 {
			return errors.New("host name too long")
		}
		request = append(request, 3, byte(len(host)))
		request = append(request, host...)
	}
	request = binary.BigEndian.AppendUint16(request, port)
	if _, err := conn.Write(request); err != nil {
		return err
	}
	header := make([]byte, 4)
	if _, err := io.ReadFull(conn, header); err != nil {
		return err
	}
	if header[1] != 0 {
		return fmt.Errorf("naive could not connect to %s (SOCKS reply %d)", net.JoinHostPort(host, strconv.Itoa(int(port))), header[1])
	}
	// Skip the bound address.
	var skip int
	switch header[3] {
	case 1:
		skip = 4
	case 4:
		skip = 16
	case 3:
		length := make([]byte, 1)
		if _, err := io.ReadFull(conn, length); err != nil {
			return err
		}
		skip = int(length[0])
	default:
		return errors.New("bad SOCKS reply from naive")
	}
	_, err := io.ReadFull(conn, make([]byte, skip+2))
	return err
}

func freePort() (int, error) {
	listener, err := net.Listen("tcp", "127.0.0.1:0")
	if err != nil {
		return 0, err
	}
	defer listener.Close()
	return listener.Addr().(*net.TCPAddr).Port, nil
}

func randomHex(n int) string {
	buf := make([]byte, n)
	if _, err := rand.Read(buf); err != nil {
		panic(err)
	}
	return hex.EncodeToString(buf)
}
