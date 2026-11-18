package main

import (
	"bytes"
	"net"
	"net/netip"
	"os/exec"
	"strconv"
	"sync/atomic"
	"syscall"
	"unsafe"
)

// ovpnPackets carries the tunnel's IP packets to and from uniray-ovpn, one
// per Read or Write.
type ovpnPackets interface {
	Read([]byte) (int, error)
	Write([]byte) (int, error)
	Close() error
}

// ovpnChannel is one session's packet socket: ours, and how uniray-ovpn
// reaches the other end.
type ovpnChannel struct {
	conn ovpnPackets
	// For uniray-ovpn's command line.
	args []string
	// A line for its stdin after the credentials.
	token string
}

// openPackets: Windows has no socketpair a child could inherit, so a UDP
// port on 127.0.0.1 (one packet per datagram, as on a TUN). uniray-ovpn's
// first datagram is the token, which it reads from stdin; packets come only
// from the sender of the latest such greeting, and go only to it.
func openPackets() (*ovpnChannel, error) {
	conn, err := net.ListenUDP("udp4", &net.UDPAddr{IP: net.IPv4(127, 0, 0, 1)})
	if err != nil {
		return nil, err
	}
	// Else a datagram to a port nobody listens on any more (the previous
	// connection's) makes the next read fail with WSAECONNRESET.
	if err := disableConnReset(conn); err != nil {
		conn.Close()
		return nil, err
	}
	_ = conn.SetReadBuffer(4 << 20)
	_ = conn.SetWriteBuffer(4 << 20)
	token := randomHex(16)
	port := conn.LocalAddr().(*net.UDPAddr).Port
	return &ovpnChannel{
		conn:  &udpPeer{conn: conn, hello: []byte(token)},
		args:  []string{"--packet-udp", strconv.Itoa(port)},
		token: token,
	}, nil
}

func (c *ovpnChannel) prepare(*exec.Cmd) {}

func (c *ovpnChannel) release() {}

// udpPeer is the UDP socket as a connection to whoever greeted it last.
type udpPeer struct {
	conn  *net.UDPConn
	hello []byte
	peer  atomic.Pointer[netip.AddrPort]
}

func (p *udpPeer) Read(b []byte) (int, error) {
	for {
		n, from, err := p.conn.ReadFromUDPAddrPort(b)
		if err != nil {
			return 0, err
		}
		if bytes.Equal(b[:n], p.hello) {
			p.peer.Store(&from)
			continue
		}
		if peer := p.peer.Load(); peer != nil && *peer == from {
			return n, nil
		}
	}
}

func (p *udpPeer) Write(b []byte) (int, error) {
	peer := p.peer.Load()
	if peer == nil {
		// Not greeted yet: dropped, as a TUN that is not up would.
		return len(b), nil
	}
	return p.conn.WriteToUDPAddrPort(b, *peer)
}

func (p *udpPeer) Close() error { return p.conn.Close() }

// SIO_UDP_CONNRESET: off.
func disableConnReset(conn *net.UDPConn) error {
	raw, err := conn.SyscallConn()
	if err != nil {
		return err
	}
	const sioUDPConnReset = syscall.IOC_IN | syscall.IOC_VENDOR | 12
	var ioErr error
	err = raw.Control(func(fd uintptr) {
		off := uint32(0)
		var returned uint32
		ioErr = syscall.WSAIoctl(syscall.Handle(fd), sioUDPConnReset, (*byte)(unsafe.Pointer(&off)), uint32(unsafe.Sizeof(off)), nil, 0, &returned, nil, 0)
	})
	if err != nil {
		return err
	}
	return ioErr
}
