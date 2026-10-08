package main

import (
	"encoding/binary"
	"fmt"
	"net/netip"
	"unsafe"

	"golang.org/x/sys/windows"
)

// Windows: the owning PID comes with each row of the system's TCP and UDP
// tables (GetExtendedTcpTable / GetExtendedUdpTable).

var (
	iphlpapi            = windows.NewLazySystemDLL("iphlpapi.dll")
	getExtendedTcpTable = iphlpapi.NewProc("GetExtendedTcpTable")
	getExtendedUdpTable = iphlpapi.NewProc("GetExtendedUdpTable")
)

const (
	tcpTableOwnerPidAll = 5
	udpTableOwnerPid    = 1
)

type socketRow struct {
	local, remote netip.AddrPort
	pid           int
}

func tcpSocketOwnedBy(pid int, client, server netip.AddrPort) (bool, error) {
	rows, err := socketTable(getExtendedTcpTable, tcpTableOwnerPidAll, true)
	if err != nil {
		return false, err
	}
	for _, row := range rows {
		if row.local == client && row.remote == server {
			return row.pid == pid, nil
		}
	}
	return false, nil
}

func udpSocketOwnedBy(pid int, client netip.AddrPort) (bool, error) {
	rows, err := socketTable(getExtendedUdpTable, udpTableOwnerPid, false)
	if err != nil {
		return false, err
	}
	// An unbound-address socket is listed as 0.0.0.0/:: with its port.
	owner := -1
	for _, row := range rows {
		if row.local == client {
			return row.pid == pid, nil
		}
		if owner < 0 && row.local.Port() == client.Port() && row.local.Addr().IsUnspecified() {
			owner = row.pid
		}
	}
	return owner == pid, nil
}

// socketTable reads the IPv4 and IPv6 tables into rows.
func socketTable(proc *windows.LazyProc, class uint32, tcp bool) ([]socketRow, error) {
	var rows []socketRow
	for _, family := range []uint32{windows.AF_INET, windows.AF_INET6} {
		buf, err := readTable(proc, family, class)
		if err != nil {
			return nil, err
		}
		if len(buf) < 4 {
			continue
		}
		count := int(binary.LittleEndian.Uint32(buf))
		data := buf[4:]
		size := rowSize(family, tcp)
		for i := 0; i < count && (i+1)*size <= len(data); i++ {
			rows = append(rows, parseRow(data[i*size:(i+1)*size], family, tcp))
		}
	}
	return rows, nil
}

func readTable(proc *windows.LazyProc, family, class uint32) ([]byte, error) {
	size := uint32(16 * 1024)
	for attempt := 0; attempt < 4; attempt++ {
		buf := make([]byte, size)
		ret, _, _ := proc.Call(uintptr(unsafe.Pointer(&buf[0])), uintptr(unsafe.Pointer(&size)), 0,
			uintptr(family), uintptr(class), 0)
		switch windows.Errno(ret) {
		case 0:
			return buf[:size], nil
		case windows.ERROR_INSUFFICIENT_BUFFER:
			size += 4 * 1024 // the table may grow between the calls
			continue
		default:
			return nil, fmt.Errorf("%s: %w", proc.Name, windows.Errno(ret))
		}
	}
	return nil, fmt.Errorf("%s: the table keeps growing", proc.Name)
}

// Row layouts (all fields are DWORDs unless noted, ports in network order
// in the first two bytes of their DWORD):
//
//	MIB_TCPROW_OWNER_PID   state, laddr, lport, raddr, rport, pid
//	MIB_TCP6ROW_OWNER_PID  laddr[16], lscope, lport, raddr[16], rscope, rport, state, pid
//	MIB_UDPROW_OWNER_PID   laddr, lport, pid
//	MIB_UDP6ROW_OWNER_PID  laddr[16], lscope, lport, pid
func rowSize(family uint32, tcp bool) int {
	switch {
	case tcp && family == windows.AF_INET:
		return 24
	case tcp:
		return 56
	case family == windows.AF_INET:
		return 12
	default:
		return 28
	}
}

func parseRow(row []byte, family uint32, tcp bool) socketRow {
	port := func(at int) uint16 { return binary.BigEndian.Uint16(row[at:]) }
	pid := func(at int) int { return int(binary.LittleEndian.Uint32(row[at:])) }
	v4 := func(at int) netip.Addr { return netip.AddrFrom4([4]byte(row[at : at+4])) }
	v6 := func(at int) netip.Addr { return netip.AddrFrom16([16]byte(row[at : at+16])).Unmap() }
	switch {
	case tcp && family == windows.AF_INET:
		return socketRow{netip.AddrPortFrom(v4(4), port(8)), netip.AddrPortFrom(v4(12), port(16)), pid(20)}
	case tcp:
		return socketRow{netip.AddrPortFrom(v6(0), port(20)), netip.AddrPortFrom(v6(24), port(44)), pid(52)}
	case family == windows.AF_INET:
		return socketRow{local: netip.AddrPortFrom(v4(0), port(4)), pid: pid(8)}
	default:
		return socketRow{local: netip.AddrPortFrom(v6(0), port(20)), pid: pid(24)}
	}
}

// Owner uids are an Android matter; there Xray's process is not ours to look
// into, here it is.
func tcpSocketUID(netip.AddrPort, netip.AddrPort) (int, error) { return -1, errNoOwnerUID }

func udpSocketUID(netip.AddrPort) (int, error) { return -1, errNoOwnerUID }
