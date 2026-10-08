//go:build !linux

package main

import "os/exec"

// setChildAttrs: on Windows the job object the engine runs in (the app's or
// the helper's) takes its children down with it; on macOS the engine stops
// naive itself.
func setChildAttrs(*exec.Cmd) {}
