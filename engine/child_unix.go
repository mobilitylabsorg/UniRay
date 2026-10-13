//go:build !windows

package main

import (
	"os"
	"syscall"
)

// terminate asks a child to stop. SIGTERM, not SIGINT: a background job of
// a script starts with SIGINT ignored, and its children inherit that.
func terminate(process *os.Process) error {
	return process.Signal(syscall.SIGTERM)
}
