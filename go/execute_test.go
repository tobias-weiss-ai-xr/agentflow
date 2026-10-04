package main

import (
	"reflect"
	"runtime"
	"testing"
)

// Default dispatch (no command) must stay pi-shaped:
// <cli> --provider P --model M -p @<abs-prompt-path>.
func TestAgentInvocationDefaultIsPiShaped(t *testing.T) {
	w := &Worker{Name: "glm", Provider: "zai", Model: "glm-4.6", CLI: "pi"}
	exe, args := agentInvocation(w, `/abs/prompts/A.md`)
	if exe != "pi" {
		t.Fatalf("exe = %q, want pi", exe)
	}
	want := []string{"--provider", "zai", "--model", "glm-4.6", "-p", "@/abs/prompts/A.md"}
	if !reflect.DeepEqual(args, want) {
		t.Fatalf("args = %v, want %v", args, want)
	}
}

// Command template: {prompt} substituted with the absolute prompt path,
// string run via the platform shell (cmd /C on Windows, sh -c otherwise).
func TestAgentInvocationCommandSubstitutesPrompt(t *testing.T) {
	w := &Worker{Name: "w", Command: "run-agent --file {prompt}"}
	exe, args := agentInvocation(w, `/abs/prompts/A.md`)
	shell, flag := "sh", "-c"
	if runtime.GOOS == "windows" {
		shell, flag = "cmd", "/C"
	}
	if exe != shell {
		t.Fatalf("exe = %q, want %q", exe, shell)
	}
	want := []string{flag, "run-agent --file /abs/prompts/A.md"}
	if !reflect.DeepEqual(args, want) {
		t.Fatalf("args = %v, want %v", args, want)
	}
}
