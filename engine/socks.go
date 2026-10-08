package main

import (
	"context"
	"crypto/subtle"
	"encoding/binary"
	"errors"
	"io"
	"net"
	"net/netip"
	"strconv"
	"sync"
	"time"
)

// Upstream is the tunnel the SOCKS server hands connections to.
type Upstream interface {
	DialTCP(ctx context.Context, host string, port uint16) (net.Conn, error)
	DialUDP(ctx context.Context, host string, port uint16) (net.Conn, error)
	Close() error
}

// Server is a SOCKS5 server (RFC 1928) for one client, Xray: username and
// password (RFC 1929) required, CONNECT and UDP ASSOCIATE, and every
// connection - and every UDP sender - checked to come from Xray itself.
type Server struct {
	User, Pass string
	Upstream   Upstream
	Verifier   Verifier
}

const (
	socksVersion   = 5
	methodUserPass = 2
	methodNone     = 0xff
	cmdConnect     = 1
	cmdAssociate   = 3
	atypIPv4       = 1
	atypDomain     = 3
	atypIPv6       = 4

	replySucceeded   = 0
	replyFailure     = 1
	replyNotAllowed  = 2
	replyUnreachable = 4
	replyCmdUnknown  = 7
	replyAtypUnknown = 8

	handshakeTimeout = 10 * time.Second
	udpIdle          = 2 * time.Minute
)

func (s *Server) Serve(listener net.Listener) error {
	for {
		conn, err := listener.Accept()
		if err != nil {
			if errors.Is(err, net.ErrClosed) {
				return nil
			}
			logf("Warning", "accept: %v", err)
			continue
		}
		go s.handle(conn)
	}
}

func (s *Server) handle(conn net.Conn) {
	defer conn.Close()
	client := addrPort(conn.RemoteAddr())
	local := addrPort(conn.LocalAddr())
	// Before reading a byte: anything but Xray gets a closed connection.
	if !s.Verifier.AllowTCP(client, local) {
		logf("Warning", "refused a connection from %v: not Xray", client)
		return
	}
	_ = conn.SetDeadline(time.Now().Add(handshakeTimeout))
	if err := s.authenticate(conn); err != nil {
		logf("Warning", "refused a connection from %v: %v", client, err)
		return
	}
	cmd, host, port, err := readRequest(conn)
	if err != nil {
		if errors.Is(err, errAtyp) {
			writeReply(conn, replyAtypUnknown, netip.AddrPort{})
		}
		return
	}
	_ = conn.SetDeadline(time.Time{})

	switch cmd {
	case cmdConnect:
		s.connect(conn, host, port)
	case cmdAssociate:
		s.associate(conn, client)
	default:
		writeReply(conn, replyCmdUnknown, netip.AddrPort{})
	}
}

func (s *Server) authenticate(conn net.Conn) error {
	head := make([]byte, 2)
	if _, err := io.ReadFull(conn, head); err != nil {
		return err
	}
	if head[0] != socksVersion {
		return errors.New("not SOCKS5")
	}
	methods := make([]byte, head[1])
	if _, err := io.ReadFull(conn, methods); err != nil {
		return err
	}
	offered := false
	for _, method := range methods {
		offered = offered || method == methodUserPass
	}
	if !offered {
		_, _ = conn.Write([]byte{socksVersion, methodNone})
		return errors.New("no password offered")
	}
	if _, err := conn.Write([]byte{socksVersion, methodUserPass}); err != nil {
		return err
	}

	// RFC 1929: VER ULEN UNAME PLEN PASSWD.
	if _, err := io.ReadFull(conn, head[:2]); err != nil {
		return err
	}
	user := make([]byte, head[1])
	if _, err := io.ReadFull(conn, user); err != nil {
		return err
	}
	if _, err := io.ReadFull(conn, head[:1]); err != nil {
		return err
	}
	pass := make([]byte, head[0])
	if _, err := io.ReadFull(conn, pass); err != nil {
		return err
	}
	userOK := subtle.ConstantTimeCompare(user, []byte(s.User)) == 1
	passOK := subtle.ConstantTimeCompare(pass, []byte(s.Pass)) == 1
	if !userOK || !passOK {
		_, _ = conn.Write([]byte{1, 1})
		return errors.New("wrong credentials")
	}
	_, err := conn.Write([]byte{1, 0})
	return err
}

var errAtyp = errors.New("unknown address type")

func readRequest(conn net.Conn) (byte, string, uint16, error) {
	head := make([]byte, 4)
	if _, err := io.ReadFull(conn, head); err != nil {
		return 0, "", 0, err
	}
	if head[0] != socksVersion {
		return 0, "", 0, errors.New("not SOCKS5")
	}
	host, port, err := readAddress(conn, head[3])
	return head[1], host, port, err
}

func readAddress(reader io.Reader, atyp byte) (string, uint16, error) {
	var host string
	switch atyp {
	case atypIPv4, atypIPv6:
		size := 4
		if atyp == atypIPv6 {
			size = 16
		}
		raw := make([]byte, size)
		if _, err := io.ReadFull(reader, raw); err != nil {
			return "", 0, err
		}
		addr, _ := netip.AddrFromSlice(raw)
		host = addr.Unmap().String()
	case atypDomain:
		length := make([]byte, 1)
		if _, err := io.ReadFull(reader, length); err != nil {
			return "", 0, err
		}
		raw := make([]byte, length[0])
		if _, err := io.ReadFull(reader, raw); err != nil {
			return "", 0, err
		}
		host = string(raw)
	default:
		return "", 0, errAtyp
	}
	portBytes := make([]byte, 2)
	if _, err := io.ReadFull(reader, portBytes); err != nil {
		return "", 0, err
	}
	return host, binary.BigEndian.Uint16(portBytes), nil
}

func writeReply(conn net.Conn, code byte, bound netip.AddrPort) {
	reply := []byte{socksVersion, code, 0}
	reply = appendAddress(reply, bound)
	_, _ = conn.Write(reply)
}

func appendAddress(buf []byte, addr netip.AddrPort) []byte {
	ip := addr.Addr().Unmap()
	if !ip.IsValid() {
		ip = netip.IPv4Unspecified()
	}
	if ip.Is4() {
		buf = append(buf, atypIPv4)
	} else {
		buf = append(buf, atypIPv6)
	}
	buf = append(buf, ip.AsSlice()...)
	return binary.BigEndian.AppendUint16(buf, addr.Port())
}

func (s *Server) connect(conn net.Conn, host string, port uint16) {
	ctx, cancel := context.WithTimeout(context.Background(), 15*time.Second)
	upstream, err := s.Upstream.DialTCP(ctx, host, port)
	cancel()
	if err != nil {
		logf("Info", "connect %s:%d failed: %v", host, port, err)
		writeReply(conn, replyUnreachable, netip.AddrPort{})
		return
	}
	defer upstream.Close()
	writeReply(conn, replySucceeded, netip.AddrPort{})
	relay(conn, upstream)
}

// relay copies both ways until either side is done.
func relay(a, b net.Conn) {
	done := make(chan struct{}, 2)
	pipe := func(dst, src net.Conn) {
		_, _ = io.Copy(dst, src)
		// Pass the end on, but let the other direction finish.
		if closer, ok := dst.(interface{ CloseWrite() error }); ok {
			_ = closer.CloseWrite()
		} else {
			_ = dst.SetReadDeadline(time.Now())
		}
		done <- struct{}{}
	}
	go pipe(a, b)
	go pipe(b, a)
	<-done
	<-done
}

// associate serves UDP ASSOCIATE: a relay socket on loopback for as long as
// the TCP connection lasts. The first datagram fixes the sender, which must
// be a socket of Xray's too; datagrams from anything else are dropped.
func (s *Server) associate(conn net.Conn, client netip.AddrPort) {
	relaySocket, err := net.ListenUDP("udp", &net.UDPAddr{IP: net.IPv4(127, 0, 0, 1)})
	if err != nil {
		writeReply(conn, replyFailure, netip.AddrPort{})
		return
	}
	defer relaySocket.Close()
	writeReply(conn, replySucceeded, addrPort(relaySocket.LocalAddr()))

	association := &association{server: s, socket: relaySocket, flows: map[string]net.Conn{}}
	go func() {
		// The association lives as long as its TCP connection.
		_, _ = io.Copy(io.Discard, conn)
		_ = relaySocket.Close()
	}()
	association.run()
	association.close()
}

type association struct {
	server *Server
	socket *net.UDPConn
	sender netip.AddrPort

	mu    sync.Mutex
	flows map[string]net.Conn
}

func (a *association) run() {
	buf := make([]byte, 64*1024)
	for {
		n, from, err := a.socket.ReadFromUDPAddrPort(buf)
		if err != nil {
			return
		}
		from = netip.AddrPortFrom(from.Addr().Unmap(), from.Port())
		if !a.sender.IsValid() {
			if !a.server.Verifier.AllowUDP(from) {
				logf("Warning", "dropped a datagram from %v: not Xray", from)
				continue
			}
			a.sender = from
		} else if from != a.sender {
			continue
		}
		a.forward(buf[:n])
	}
}

// forward sends one SOCKS UDP datagram (RSV RSV FRAG ATYP ADDR PORT DATA)
// to its destination through the tunnel.
func (a *association) forward(datagram []byte) {
	if len(datagram) < 4 || datagram[2] != 0 { // fragments are not supported
		return
	}
	reader := &sliceReader{data: datagram[4:]}
	host, port, err := readAddress(reader, datagram[3])
	if err != nil {
		return
	}
	payload := reader.data
	key := net.JoinHostPort(host, strconv.Itoa(int(port)))

	a.mu.Lock()
	flow := a.flows[key]
	a.mu.Unlock()
	if flow == nil {
		ctx, cancel := context.WithTimeout(context.Background(), 10*time.Second)
		flow, err = a.server.Upstream.DialUDP(ctx, host, port)
		cancel()
		if err != nil {
			logf("Info", "udp %s failed: %v", key, err)
			return
		}
		a.mu.Lock()
		a.flows[key] = flow
		a.mu.Unlock()
		go a.receive(flow, host, port)
	}
	_ = flow.SetDeadline(time.Now().Add(udpIdle))
	_, _ = flow.Write(payload)
}

// receive wraps the replies of one destination for the sender.
func (a *association) receive(flow net.Conn, host string, port uint16) {
	defer flow.Close()
	source := netip.AddrPortFrom(netip.IPv4Unspecified(), port)
	if ip, err := netip.ParseAddr(host); err == nil {
		source = netip.AddrPortFrom(ip, port)
	} else if remote := addrPort(flow.RemoteAddr()); remote.IsValid() {
		source = remote
	}
	buf := make([]byte, 64*1024)
	for {
		n, err := flow.Read(buf)
		if err != nil {
			return
		}
		_ = flow.SetDeadline(time.Now().Add(udpIdle))
		packet := appendAddress([]byte{0, 0, 0}, source)
		packet = append(packet, buf[:n]...)
		if _, err := a.socket.WriteToUDPAddrPort(packet, a.sender); err != nil {
			return
		}
	}
}

func (a *association) close() {
	a.mu.Lock()
	defer a.mu.Unlock()
	for _, flow := range a.flows {
		_ = flow.Close()
	}
}

type sliceReader struct{ data []byte }

func (r *sliceReader) Read(p []byte) (int, error) {
	if len(r.data) == 0 {
		return 0, io.EOF
	}
	n := copy(p, r.data)
	r.data = r.data[n:]
	return n, nil
}

func addrPort(addr net.Addr) netip.AddrPort {
	switch a := addr.(type) {
	case *net.TCPAddr:
		ip, _ := netip.AddrFromSlice(a.IP)
		return netip.AddrPortFrom(ip.Unmap(), uint16(a.Port))
	case *net.UDPAddr:
		ip, _ := netip.AddrFromSlice(a.IP)
		return netip.AddrPortFrom(ip.Unmap(), uint16(a.Port))
	}
	if parsed, err := netip.ParseAddrPort(addr.String()); err == nil {
		return parsed
	}
	return netip.AddrPort{}
}
