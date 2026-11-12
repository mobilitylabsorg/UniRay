package main

import "errors"

// OVPNConfig: see ovpn.go. On Windows uniray-ovpn's packet socket is not a
// Unix socketpair; that build is still to come.
type OVPNConfig struct {
	Binary   string `json:"binary"`
	Profile  string `json:"profile"`
	Username string `json:"username"`
	Password string `json:"password"`
	Server   string `json:"server"`
	Port     int    `json:"port"`
	Proto    string `json:"proto"`
}

func startOVPN(*OVPNConfig, string) (Upstream, error) {
	return nil, errors.New("OpenVPN is not supported on Windows yet")
}
