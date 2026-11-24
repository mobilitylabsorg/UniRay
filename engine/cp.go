package main

import (
	"encoding/base64"
	"errors"
	"fmt"
	"os"
	"os/exec"
	"path/filepath"
	"strings"
)

// CPConfig runs a Check Point gateway through uniray-cp: snx-rs (IPsec, IKEv1,
// ESP over UDP in userspace) patched to hand the tunnel's IP packets to this
// engine on the same packet socket as uniray-ovpn, and to print the same
// settings (cp/snxcore.patch). The rest is ovpnUpstream's.
type CPConfig struct {
	Binary string `json:"binary"`
	// The gateway's name, which its TLS certificate names.
	Gateway string `json:"gateway"`
	// Its IPv4 address to connect to, when set: the one the app resolved it to
	// and sends past the VPN.
	Address string `json:"address"`
	// The user's certificate and key, PKCS#12 in base64; none for a login
	// without a certificate.
	Cert         string `json:"cert"`
	CertPassword string `json:"certPassword"`
	Username     string `json:"username"`
	Password     string `json:"password"`
	// The gateway's login option; empty picks the one that fits what is
	// given (a certificate, a password).
	LoginType string `json:"loginType"`
}

func startCP(config *CPConfig, dir string) (Upstream, error) {
	if config.Binary == "" || config.Gateway == "" {
		return nil, errors.New("the config has no uniray-cp binary or gateway")
	}
	for _, value := range []string{config.Gateway, config.Address, config.CertPassword, config.Username, config.LoginType} {
		if strings.ContainsAny(value, "\r\n") {
			return nil, errors.New("a Check Point setting has a line break")
		}
	}
	var cert []byte
	if config.Cert != "" {
		var err error
		if cert, err = base64.StdEncoding.DecodeString(config.Cert); err != nil {
			return nil, fmt.Errorf("the certificate: %w", err)
		}
	}

	child := tunnelChild{name: "uniray-cp", label: "Check Point", command: func(channel *ovpnChannel) (*exec.Cmd, string, func(), error) {
		var files []string
		cleanup := func() {
			for _, file := range files {
				os.Remove(file)
			}
		}
		write := func(suffix string, data []byte) (string, error) {
			path := filepath.Join(dir, "cp-"+randomHex(8)+suffix)
			files = append(files, path)
			return path, os.WriteFile(path, data, 0o600)
		}

		lines := []string{
			"server-name=" + config.Gateway,
			"tunnel-type=ipsec",
			"ike-version=1",
			"transport-type=udp",
			// The Endpoint Security flow: the certificate in IKE, then the
			// password over XAUTH. A gateway asks the default (SecureClient)
			// for a user name and then drops the answer.
			"client-mode=endpoint_security",
			"ike-persist=false",
			"no-dns=true",
			"log-level=info",
		}
		if config.Address != "" {
			lines = append(lines, "server-address="+config.Address)
		}
		if config.LoginType != "" {
			lines = append(lines, "login-type="+config.LoginType)
		}
		if config.Username != "" {
			lines = append(lines, "user-name="+config.Username)
		}
		if config.Password != "" {
			lines = append(lines, "password="+base64.StdEncoding.EncodeToString([]byte(config.Password)))
		}
		if cert != nil {
			path, err := write(".p12", cert)
			if err != nil {
				cleanup()
				return nil, "", nil, fmt.Errorf("certificate: %w", err)
			}
			lines = append(lines, "cert-type=pkcs12", "cert-path="+path)
			if config.CertPassword != "" {
				lines = append(lines, "cert-password="+config.CertPassword)
			}
		}
		// The packet socket: --packet-fd 3, or --packet-udp <port> and its
		// token, as config keys.
		for i := 0; i+1 < len(channel.args); i += 2 {
			lines = append(lines, strings.TrimPrefix(channel.args[i], "--")+"="+channel.args[i+1])
		}
		if channel.token != "" {
			lines = append(lines, "packet-token="+channel.token)
		}

		path, err := write(".conf", []byte(strings.Join(lines, "\n")+"\n"))
		if err != nil {
			cleanup()
			return nil, "", nil, fmt.Errorf("config: %w", err)
		}
		return exec.Command(config.Binary, "-m", "standalone", "-c", path), "", cleanup, nil
	}}
	return startTunnel(child), nil
}
