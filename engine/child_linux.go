package main

import (
	"os/exec"
	"syscall"
)

// setChildAttrs takes a child (naive) down with the engine, however the
// engine ends.
func setChildAttrs(cmd *exec.Cmd) {
	cmd.SysProcAttr = &syscall.SysProcAttr{Pdeathsig: syscall.SIGTERM}
}
