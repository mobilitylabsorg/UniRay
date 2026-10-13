package main

/*
#include <errno.h>
#include <stdint.h>
#include <stdlib.h>
#include <string.h>
#include <arpa/inet.h>
#include <sys/proc_info.h>

// From libproc.h (part of libSystem).
int proc_pidinfo(int pid, int flavor, uint64_t arg, void *buffer, int buffersize);
int proc_pidfdinfo(int pid, int fd, int flavor, void *buffer, int buffersize);

typedef struct {
	int tcp;
	uint8_t laddr[16], raddr[16];
	uint16_t lport, rport;
	int v4;
} uniray_socket;

// uniray_sockets lists the internet sockets of a process: their count, or
// -1 with errno set when the process cannot be read.
static int uniray_sockets(int pid, uniray_socket *out, int max) {
	int size = proc_pidinfo(pid, PROC_PIDLISTFDS, 0, NULL, 0);
	if (size <= 0) return -1;
	struct proc_fdinfo *fds = malloc(size);
	if (fds == NULL) return -1;
	size = proc_pidinfo(pid, PROC_PIDLISTFDS, 0, fds, size);
	if (size <= 0) { free(fds); return -1; }
	int count = 0;
	for (int i = 0; i < size / (int)PROC_PIDLISTFD_SIZE && count < max; i++) {
		if (fds[i].proc_fdtype != PROX_FDTYPE_SOCKET) continue;
		struct socket_fdinfo info;
		if (proc_pidfdinfo(pid, fds[i].proc_fd, PROC_PIDFDSOCKETINFO, &info, sizeof info) < (int)sizeof info) continue;
		struct in_sockinfo *in;
		if (info.psi.soi_kind == SOCKINFO_TCP) in = &info.psi.soi_proto.pri_tcp.tcpsi_ini;
		else if (info.psi.soi_kind == SOCKINFO_IN) in = &info.psi.soi_proto.pri_in;
		else continue;
		uniray_socket *s = &out[count++];
		memset(s, 0, sizeof *s);
		s->tcp = info.psi.soi_kind == SOCKINFO_TCP;
		s->v4 = (in->insi_vflag & INI_IPV4) != 0;
		if (s->v4) {
			memcpy(s->laddr, &in->insi_laddr.ina_46.i46a_addr4, 4);
			memcpy(s->raddr, &in->insi_faddr.ina_46.i46a_addr4, 4);
		} else {
			memcpy(s->laddr, &in->insi_laddr.ina_6, 16);
			memcpy(s->raddr, &in->insi_faddr.ina_6, 16);
		}
		s->lport = ntohs((uint16_t)in->insi_lport);
		s->rport = ntohs((uint16_t)in->insi_fport);
	}
	free(fds);
	return count;
}
*/
import "C"

import (
	"errors"
	"fmt"
	"net/netip"
	"syscall"
	"unsafe"
)

// macOS: the process's own socket descriptors through libproc. Reading
// another process needs its uid or root, so the engine runs as Xray does.

type socketRow struct {
	tcp           bool
	local, remote netip.AddrPort
}

const maxSockets = 4096

func processSockets(pid int) ([]socketRow, error) {
	buf := (*[maxSockets]C.uniray_socket)(C.malloc(C.size_t(maxSockets) * C.size_t(unsafe.Sizeof(C.uniray_socket{}))))
	defer C.free(unsafe.Pointer(buf))
	count, err := C.uniray_sockets(C.int(pid), &buf[0], maxSockets)
	if count < 0 {
		if errors.Is(err, syscall.ESRCH) {
			return nil, errPeerGone
		}
		return nil, fmt.Errorf("process %d: %v", pid, err)
	}
	rows := make([]socketRow, 0, int(count))
	for _, s := range buf[:count] {
		addr := func(raw [16]C.uint8_t) netip.Addr {
			var b [16]byte
			for i := range b {
				b[i] = byte(raw[i])
			}
			if s.v4 != 0 {
				return netip.AddrFrom4([4]byte(b[:4]))
			}
			return netip.AddrFrom16(b).Unmap()
		}
		rows = append(rows, socketRow{
			tcp:    s.tcp != 0,
			local:  netip.AddrPortFrom(addr(s.laddr), uint16(s.lport)),
			remote: netip.AddrPortFrom(addr(s.raddr), uint16(s.rport)),
		})
	}
	return rows, nil
}

func tcpSocketOwnedBy(pid int, client, server netip.AddrPort) (bool, error) {
	rows, err := processSockets(pid)
	if err != nil {
		return false, err
	}
	for _, row := range rows {
		if row.tcp && row.local == client && row.remote == server {
			return true, nil
		}
	}
	return false, nil
}

func udpSocketOwnedBy(pid int, client netip.AddrPort) (bool, error) {
	rows, err := processSockets(pid)
	if err != nil {
		return false, err
	}
	// An unconnected socket is listed with the wildcard address it is bound to.
	for _, row := range rows {
		if !row.tcp && row.local.Port() == client.Port() && (row.local.Addr() == client.Addr() || row.local.Addr().IsUnspecified()) {
			return true, nil
		}
	}
	return false, nil
}

// Owner uids are an Android matter; there Xray's process is not ours to look
// into, here it is.
func tcpSocketUID(netip.AddrPort, netip.AddrPort) (int, error) { return -1, errNoOwnerUID }

func udpSocketUID(netip.AddrPort) (int, error) { return -1, errNoOwnerUID }
