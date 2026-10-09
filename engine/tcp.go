package main

import (
	"errors"

	"github.com/amnezia-vpn/amneziawg-go/v3/tun/netstack"
	"gvisor.dev/gvisor/pkg/tcpip"
	"gvisor.dev/gvisor/pkg/tcpip/transport/tcp"
	"gvisor.dev/gvisor/pkg/tcpip/transport/tcp/bbr"
)

// The tunnel's userspace TCP is what sends a WireGuard/AmneziaWG upload over
// the long haul (a download is the server's to send), and gVisor's own Reno
// and Cubic give up most of it to random loss: at 0.1-0.5% loss over 60 ms,
// a few Mbit/s where a kernel's BBR keeps 75-100. BBRv3 does not take each
// loss for congestion. It comes from the gVisor fork the engine is built on
// (mobilitylabsorg/gvisor, branch uniray: apoxy-dev/ccsim's pluggable
// congestion control and BBRv3, AGPL-3.0). See engine/bench for the
// measurements.
func init() { bbr.Register() }

// tuneTCP selects BBRv3 for the tunnel's TCP, with a send buffer for
// BBR's window (gVisor sends from a fixed-size buffer; 1 MiB, its default,
// caps an upload at 80 Mbit/s over 100 ms).
func tuneTCP(tnet *netstack.Net) error {
	s := tnet.Stack()
	cc := tcpip.CongestionControlOption("bbr")
	send := tcpip.TCPSendBufferSizeRangeOption{Min: 4096, Default: tcpSendBuffer, Max: 2 * tcpSendBuffer}
	for _, option := range []tcpip.SettableTransportProtocolOption{&cc, &send} {
		if err := s.SetTransportProtocolOption(tcp.ProtocolNumber, option); err != nil {
			return errors.New(err.String())
		}
	}
	return nil
}
