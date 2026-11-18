//go:build !linux && !windows

package main

import "os/exec"

// setChildAttrs: on macOS the engine stops its children itself.
func setChildAttrs(*exec.Cmd) {}
