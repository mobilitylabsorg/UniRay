package main

import (
	"crypto/rand"
	"encoding/base64"
	"encoding/hex"
	"fmt"

	"golang.org/x/crypto/curve25519"
)

// keygen is the e2e test's key generator. It prints "<private b64> <public b64> <private hex> <public hex>".
func main() {
	var priv [32]byte
	_, _ = rand.Read(priv[:])
	priv[0] &= 248
	priv[31] = (priv[31] & 127) | 64
	pub, _ := curve25519.X25519(priv[:], curve25519.Basepoint)
	fmt.Println(base64.StdEncoding.EncodeToString(priv[:]), base64.StdEncoding.EncodeToString(pub), hex.EncodeToString(priv[:]), hex.EncodeToString(pub))
}
