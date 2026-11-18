package main

import (
	"os"
	"os/exec"
	"syscall"
)

// terminate stops a child: Windows has no signal to ask with.
func terminate(process *os.Process) error {
	return process.Kill()
}

// setChildAttrs: no console window for a child (naive, uniray-ovpn). The job
// object the engine runs in (the app's or the helper's) takes its children
// down with it.
func setChildAttrs(cmd *exec.Cmd) {
	const createNoWindow = 0x0800_0000
	cmd.SysProcAttr = &syscall.SysProcAttr{CreationFlags: createNoWindow, HideWindow: true}
}
