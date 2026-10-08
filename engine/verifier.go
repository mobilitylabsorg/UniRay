package main

import (
	"bufio"
	"errors"
	"fmt"
	"net"
	"net/netip"
	"strconv"
	"strings"
	"sync"
	"time"
)

// Verifier says whether a connection to the engine comes from Xray. The
// engine refuses everything else, password or not: the SOCKS port is on
// loopback, which every local process can reach.
type Verifier interface {
	// AllowTCP: the client's socket is `client`, connected to our `server`.
	AllowTCP(client, server netip.AddrPort) bool
	// AllowUDP: the client's socket is bound to `client`.
	AllowUDP(client netip.AddrPort) bool
	AddPeer(pid int)
}

func newVerifier(config *Config) (Verifier, error) {
	if config.OwnerSocket != "" {
		return &ownerVerifier{socket: config.OwnerSocket, uid: config.OwnerUID}, nil
	}
	peers := &peerSet{}
	for _, pid := range config.Peers {
		peers.AddPeer(pid)
	}
	return &processVerifier{peers: peers}, nil
}

// peerSet holds the processes allowed to connect: Xray's.
type peerSet struct {
	mu   sync.Mutex
	pids []int
}

func (p *peerSet) AddPeer(pid int) {
	p.mu.Lock()
	defer p.mu.Unlock()
	for _, known := range p.pids {
		if known == pid {
			return
		}
	}
	p.pids = append(p.pids, pid)
}

func (p *peerSet) remove(pid int) {
	p.mu.Lock()
	defer p.mu.Unlock()
	for i, known := range p.pids {
		if known == pid {
			p.pids = append(p.pids[:i], p.pids[i+1:]...)
			return
		}
	}
}

func (p *peerSet) list() []int {
	p.mu.Lock()
	defer p.mu.Unlock()
	return append([]int(nil), p.pids...)
}

// errPeerGone: the process has exited; it is dropped from the peers (a new
// Xray registers itself with its own pid).
var errPeerGone = errors.New("the process has exited")

// processVerifier finds the process owning the client socket through the
// OS (socketOwnedBy, per platform) and accepts it only if it is a peer.
type processVerifier struct {
	peers *peerSet
}

func (v *processVerifier) AddPeer(pid int) { v.peers.AddPeer(pid) }

func (v *processVerifier) AllowTCP(client, server netip.AddrPort) bool {
	return v.owned(func(pid int) (bool, error) { return tcpSocketOwnedBy(pid, client, server) })
}

func (v *processVerifier) AllowUDP(client netip.AddrPort) bool {
	return v.owned(func(pid int) (bool, error) { return udpSocketOwnedBy(pid, client) })
}

func (v *processVerifier) owned(check func(pid int) (bool, error)) bool {
	for _, pid := range v.peers.list() {
		owned, err := check(pid)
		if errors.Is(err, errPeerGone) {
			v.peers.remove(pid)
			continue
		}
		if err != nil {
			logf("Warning", "cannot check process %d: %v", pid, err)
			continue
		}
		if owned {
			return true
		}
	}
	return false
}

// ownerVerifier asks the app who owns a connection: on Android an app may
// not read /proc/net, but the VPN app may ask the system
// (ConnectivityManager.getConnectionOwnerUid). One line per question,
// "tcp <client> <server>" or "udp <client>", answered with the uid or -1.
// Only connections of the app's own uid - Xray runs inside it - are allowed.
type ownerVerifier struct {
	socket string
	uid    int
	mu     sync.Mutex
}

func (v *ownerVerifier) AddPeer(int) {}

func (v *ownerVerifier) AllowTCP(client, server netip.AddrPort) bool {
	return v.ask(fmt.Sprintf("tcp %s %s", client, server))
}

func (v *ownerVerifier) AllowUDP(client netip.AddrPort) bool {
	return v.ask(fmt.Sprintf("udp %s", client))
}

func (v *ownerVerifier) ask(question string) bool {
	v.mu.Lock()
	defer v.mu.Unlock()
	conn, err := net.DialTimeout("unix", v.socket, 2*time.Second)
	if err != nil {
		logf("Warning", "cannot ask who owns a connection: %v", err)
		return false
	}
	defer conn.Close()
	_ = conn.SetDeadline(time.Now().Add(2 * time.Second))
	if _, err := fmt.Fprintln(conn, question); err != nil {
		return false
	}
	answer, err := bufio.NewReader(conn).ReadString('\n')
	if err != nil {
		return false
	}
	uid, err := strconv.Atoi(strings.TrimSpace(answer))
	return err == nil && uid >= 0 && uid == v.uid
}
