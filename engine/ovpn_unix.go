//go:build !windows

package main

import (
	"net"
	"os"
	"os/exec"
	"syscall"
)

// ovpnPackets carries the tunnel's IP packets to and from uniray-ovpn, one
// per Read or Write.
type ovpnPackets interface {
	Read([]byte) (int, error)
	Write([]byte) (int, error)
	Close() error
}

// ovpnChannel is one session's packet socket: ours, and how uniray-ovpn
// gets the other end.
type ovpnChannel struct {
	conn ovpnPackets
	// For uniray-ovpn's command line.
	args []string
	// A line for its stdin after the credentials, when not empty.
	token string
	// The child's end, until it has it.
	theirs *os.File
}

// openPackets: a socketpair; datagrams keep each packet whole, as a TUN
// does. The child inherits its end as descriptor 3.
func openPackets() (*ovpnChannel, error) {
	// Close-on-exec under ForkLock, as Go does where SOCK_CLOEXEC is missing
	// (macOS): no other child inherits either end.
	syscall.ForkLock.RLock()
	fds, err := syscall.Socketpair(syscall.AF_UNIX, syscall.SOCK_DGRAM, 0)
	if err == nil {
		syscall.CloseOnExec(fds[0])
		syscall.CloseOnExec(fds[1])
	}
	syscall.ForkLock.RUnlock()
	if err != nil {
		return nil, err
	}
	ours, theirs := os.NewFile(uintptr(fds[0]), "ovpn-packets"), os.NewFile(uintptr(fds[1]), "ovpn-packets-child")
	conn, err := net.FileConn(ours)
	ours.Close()
	if err != nil {
		theirs.Close()
		return nil, err
	}
	return &ovpnChannel{conn: conn.(*net.UnixConn), args: []string{"--packet-fd", "3"}, theirs: theirs}, nil
}

func (c *ovpnChannel) prepare(cmd *exec.Cmd) {
	cmd.ExtraFiles = []*os.File{c.theirs}
}

// release closes the child's end in this process: after the start, or when
// there is none. Once is enough; again does nothing.
func (c *ovpnChannel) release() {
	if c.theirs != nil {
		c.theirs.Close()
		c.theirs = nil
	}
}
