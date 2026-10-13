// uniray-engine runs a tunnel that UniRay's Xray core cannot - AmneziaWG -
// and serves it to that Xray, and to nothing else, as a SOCKS5 proxy on
// loopback. Xray stays in front (TUN, routing, DNS, stats); for such a server
// its proxy outbound is SOCKS to this engine.
//
// Usage: uniray-engine awg|naive --config <file> [--probe <url> [--timeout <duration>]]
//
// With --probe it serves nothing: it fetches the URL through the server,
// prints "delay <ms>" (-1 on failure) and exits - the app's delay test.
//
// awg runs WireGuard/AmneziaWG itself (amneziawg-go, userspace); naive starts
// the official NaiveProxy client as its child and passes connections on.
//
// The config file holds secrets, so it is read once and removed. Nothing
// secret goes on the command line, which other users can read. The engine
// prints "ready <port>" once it listens (its only line on stdout; the log
// goes to stderr), then takes commands on stdin:
//
//	peer <pid>    accept connections from this process (Xray) from now on
//
// It exits when stdin closes: the app that started it is gone - unless the
// config says "detached" (Android stealth mode, where a script started it as
// root and the teardown kills it).
package main

import (
	"bufio"
	"encoding/json"
	"flag"
	"fmt"
	"net"
	"os"
	"path/filepath"
	"strconv"
	"strings"
	"time"
)

// Config is what the app writes for one run.
type Config struct {
	// SOCKS5 credentials Xray uses; random per run.
	User string `json:"user"`
	Pass string `json:"pass"`
	// Loopback port to listen on; 0 picks a free one (see "ready").
	Port int `json:"port"`
	// Processes allowed to connect from the start (more come over stdin).
	Peers []int `json:"peers"`
	// Accept the connections of this uid instead of given processes: Android,
	// where Xray runs inside the app (VPN mode) or as root (stealth mode).
	OwnerUID *int `json:"ownerUid"`
	// Android in VPN mode: an app may not read /proc/net (Android 10+), so
	// the VPN service answers who owns a connection over this unix socket
	// ("@name" for the abstract namespace). Without one, or when it does not
	// answer, the owner comes from /proc/net.
	OwnerSocket string `json:"ownerSocket"`
	// Keep running when stdin closes.
	Detached bool `json:"detached"`

	AWG   *AWGConfig   `json:"awg"`
	Naive *NaiveConfig `json:"naive"`
}

func main() {
	if len(os.Args) < 2 {
		fatalf("usage: uniray-engine awg|naive --config <file>")
	}
	mode := os.Args[1]
	flags := flag.NewFlagSet(mode, flag.ExitOnError)
	configPath := flags.String("config", "", "config file, removed once read")
	probeURL := flags.String("probe", "", "measure the delay of one request to this URL, then exit")
	probeTimeout := flags.Duration("timeout", 8*time.Second, "how long the probe may take")
	_ = flags.Parse(os.Args[2:])
	if *configPath == "" {
		fatalf("--config is required")
	}

	config, err := readConfig(*configPath)
	if err != nil {
		fatalf("cannot read the config: %v", err)
	}
	if config.User == "" || config.Pass == "" {
		fatalf("the config has no SOCKS credentials")
	}

	verifier, err := newVerifier(config)
	if err != nil {
		fatalf("cannot check connecting processes: %v", err)
	}

	var upstream Upstream
	switch mode {
	case "awg":
		if config.AWG == nil {
			fatalf("the config has no awg section")
		}
		upstream, err = startAWG(config.AWG)
	case "naive":
		if config.Naive == nil {
			fatalf("the config has no naive section")
		}
		upstream, err = startNaive(config.Naive, filepath.Dir(*configPath), func(err error) {
			fatalf("naive exited: %v", err)
		})
	default:
		fatalf("unknown mode %q", mode)
	}
	if err != nil {
		if *probeURL != "" {
			printDelay(-1)
		}
		fatalf("cannot start the tunnel: %v", err)
	}
	defer upstream.Close()

	if *probeURL != "" {
		printDelay(probe(upstream, *probeURL, *probeTimeout))
		upstream.Close()
		return
	}

	listener, err := net.Listen("tcp", net.JoinHostPort("127.0.0.1", strconv.Itoa(config.Port)))
	if err != nil {
		fatalf("cannot listen: %v", err)
	}
	server := &Server{User: config.User, Pass: config.Pass, Upstream: upstream, Verifier: verifier}
	fmt.Printf("ready %d\n", listener.Addr().(*net.TCPAddr).Port)

	go func() {
		readCommands(verifier)
		if config.Detached {
			return
		}
		// The app is gone; take the tunnel down with it.
		upstream.Close()
		os.Exit(0)
	}()
	if err := server.Serve(listener); err != nil {
		fatalf("serving: %v", err)
	}
}

func readConfig(path string) (*Config, error) {
	data, err := os.ReadFile(path)
	if err != nil {
		return nil, err
	}
	// Secrets do not linger on disk past the start.
	_ = os.Remove(path)
	var config Config
	if err := json.Unmarshal(data, &config); err != nil {
		return nil, err
	}
	return &config, nil
}

func readCommands(verifier Verifier) {
	scanner := bufio.NewScanner(os.Stdin)
	for scanner.Scan() {
		fields := strings.Fields(scanner.Text())
		if len(fields) == 2 && fields[0] == "peer" {
			if pid, err := strconv.Atoi(fields[1]); err == nil && pid > 0 {
				verifier.AddPeer(pid)
			}
		}
	}
}

// Logs in the shape of Xray's own lines, so the app files them by level.
func logf(level, format string, args ...any) {
	fmt.Fprintf(os.Stderr, "[%s] engine: %s\n", level, fmt.Sprintf(format, args...))
}

func fatalf(format string, args ...any) {
	logf("Error", format, args...)
	os.Exit(1)
}
