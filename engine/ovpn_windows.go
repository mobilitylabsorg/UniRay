package main

import "errors"

// OVPNConfig: see ovpn.go. On Windows uniray-ovpn's packet socket is not a
// Unix socketpair; that build is still to come.
type OVPNConfig struct {
	Binary   string `json:"binary"`
	Profile  string `json:"profile"`
	Username string `json:"username"`
	Password string `json:"password"`
}

func startOVPN(*OVPNConfig, string, func(error)) (Upstream, error) {
	return nil, errors.New("OpenVPN is not supported on Windows yet")
}
