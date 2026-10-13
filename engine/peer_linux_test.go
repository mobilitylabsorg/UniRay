package main

import "testing"

func TestParseProcAddr(t *testing.T) {
	cases := map[string]string{
		"0100007F:1F90":                         "127.0.0.1:8080",
		"00000000:0035":                         "0.0.0.0:53",
		"00000000000000000000000001000000:0050": "[::1]:80",
		"0000000000000000FFFF00000100007F:01BB": "127.0.0.1:443", // v4-mapped
		"garbage":                               "invalid AddrPort",
	}
	for field, want := range cases {
		if got := parseProcAddr(field).String(); got != want {
			t.Errorf("parseProcAddr(%q) = %s, want %s", field, got, want)
		}
	}
}
