package main

import (
	"bufio"
	"encoding/binary"
	"encoding/hex"
	"errors"
	"fmt"
	"io/fs"
	"net/netip"
	"os"
	"strconv"
	"strings"
)

// Linux (and Android with root): the socket's inode from /proc/net, then
// whether the process holds a descriptor for it. Reading another process's
// /proc/<pid>/fd needs its uid or root, so the engine runs as Xray does.

func tcpSocketOwnedBy(pid int, client, server netip.AddrPort) (bool, error) {
	inode, err := findInode([]string{"/proc/net/tcp", "/proc/net/tcp6"}, client, server)
	if err != nil || inode == "" {
		return false, err
	}
	return holdsSocket(pid, inode)
}

func udpSocketOwnedBy(pid int, client netip.AddrPort) (bool, error) {
	inode, err := findInode([]string{"/proc/net/udp", "/proc/net/udp6"}, client, netip.AddrPort{})
	if err != nil || inode == "" {
		return false, err
	}
	return holdsSocket(pid, inode)
}

// findInode returns the inode of the socket bound to `local` (and connected
// to `remote`, when given), or "" when there is none. An unconnected UDP
// socket is listed with the wildcard address it is bound to, though its
// datagrams arrive from 127.0.0.1, so a wildcard socket on the same port
// counts when no exact match exists.
func findInode(tables []string, local, remote netip.AddrPort) (string, error) {
	var lastErr error
	read := 0
	wildcard := ""
	for _, table := range tables {
		file, err := os.Open(table)
		if err != nil {
			lastErr = err
			continue
		}
		read++
		scanner := bufio.NewScanner(file)
		scanner.Scan() // header
		for scanner.Scan() {
			// sl local_address rem_address st tx:rx tr:when retrnsmt uid timeout inode
			fields := strings.Fields(scanner.Text())
			if len(fields) < 10 {
				continue
			}
			bound := parseProcAddr(fields[1])
			if bound != local {
				if !remote.IsValid() && wildcard == "" && bound.Port() == local.Port() && bound.Addr().IsUnspecified() {
					wildcard = fields[9]
				}
				continue
			}
			if remote.IsValid() && parseProcAddr(fields[2]) != remote {
				continue
			}
			file.Close()
			return fields[9], nil
		}
		file.Close()
	}
	if read == 0 {
		return "", lastErr
	}
	return wildcard, nil
}

// parseProcAddr reads "0100007F:1F90": the address as 32-bit words in host
// (little-endian) order, then the port in hex.
func parseProcAddr(field string) netip.AddrPort {
	hexAddr, hexPort, ok := strings.Cut(field, ":")
	if !ok {
		return netip.AddrPort{}
	}
	raw, err := hex.DecodeString(hexAddr)
	if err != nil || (len(raw) != 4 && len(raw) != 16) {
		return netip.AddrPort{}
	}
	for word := 0; word < len(raw); word += 4 {
		binary.BigEndian.PutUint32(raw[word:], binary.LittleEndian.Uint32(raw[word:]))
	}
	port, err := strconv.ParseUint(hexPort, 16, 16)
	if err != nil {
		return netip.AddrPort{}
	}
	addr, _ := netip.AddrFromSlice(raw)
	return netip.AddrPortFrom(addr.Unmap(), uint16(port))
}

func holdsSocket(pid int, inode string) (bool, error) {
	dir := fmt.Sprintf("/proc/%d/fd", pid)
	entries, err := os.ReadDir(dir)
	if errors.Is(err, fs.ErrNotExist) {
		return false, errPeerGone
	}
	if err != nil {
		return false, err
	}
	want := "socket:[" + inode + "]"
	for _, entry := range entries {
		if target, err := os.Readlink(dir + "/" + entry.Name()); err == nil && target == want {
			return true, nil
		}
	}
	return false, nil
}
