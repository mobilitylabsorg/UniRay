package main

import "os"

// terminate stops a child: Windows has no signal to ask with.
func terminate(process *os.Process) error {
	return process.Kill()
}
