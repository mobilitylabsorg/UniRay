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
	// AllowUDP: the client's socket is bound to `client` and sends to our
	// `relay`.
	AllowUDP(client, relay netip.AddrPort) bool
	AddPeer(pid int)
}

func newVerifier(config *Config) (Verifier, error) {
	if config.OwnerUID != nil {
		return &ownerVerifier{socket: config.OwnerSocket, uid: *config.OwnerUID}, nil
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

var errNoOwnerUID = errors.New("socket owner uids are not available on this system")

// processVerifier finds the process owning the client socket through the
// OS (socketOwnedBy, per platform) and accepts it only if it is a peer.
type processVerifier struct {
	peers *peerSet
}

func (v *processVerifier) AddPeer(pid int) { v.peers.AddPeer(pid) }

func (v *processVerifier) AllowTCP(client, server netip.AddrPort) bool {
	return v.owned(func(pid int) (bool, error) { return tcpSocketOwnedBy(pid, client, server) })
}

func (v *processVerifier) AllowUDP(client, _ netip.AddrPort) bool {
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

// ownerVerifier accepts the connections of one uid. Android runs Xray
// inside the app (VPN mode) or as root (stealth), so its uid tells it from
// other apps, while its process is not one we could look into.
//
// The owner of a connection comes from /proc/net, which Android 10+ hides
// from apps. There the VPN service asks the system instead
// (ConnectivityManager.getConnectionOwnerUid) and answers over `socket`: one
// line per question, "tcp <client> <server>" or "udp <client> <relay>",
// answered with the uid or -1.
type ownerVerifier struct {
	socket string
	uid    int
	mu     sync.Mutex
}

func (v *ownerVerifier) AddPeer(int) {}

func (v *ownerVerifier) AllowTCP(client, server netip.AddrPort) bool {
	if v.socket != "" {
		return v.ask(fmt.Sprintf("tcp %s %s", client, server))
	}
	return v.fromProc(tcpSocketUID(client, server))
}

func (v *ownerVerifier) AllowUDP(client, relay netip.AddrPort) bool {
	if v.socket != "" {
		return v.ask(fmt.Sprintf("udp %s %s", client, relay))
	}
	return v.fromProc(udpSocketUID(client))
}

func (v *ownerVerifier) fromProc(uid int, err error) bool {
	if err != nil {
		logf("Warning", "cannot tell who owns a connection: %v", err)
		return false
	}
	return uid == v.uid
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
