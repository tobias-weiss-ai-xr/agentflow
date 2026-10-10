#!/bin/sh

# Unity compile gate script: compiles LearningWorlds project
# with Unity version 6000.3.25f1 using Universal Render Pipeline

set -e

# Define Unity version and package path
UNITY_VERSION="6000.3.25f1"
UNITY_ROOT=""  # either user home or provided path

# Ensure Unity path is correct
if [ -z "$UNITY_ROOT" ]; then
    export UNITY_ROOT="$HOME/Library/Unity/Hub/Editor/$UNITY_VERSION/Editor"
    if [ ! -d "$UNITY_ROOT" ]; then
        echo "Error: Unity version not found: $UNITY_VERSION"
        exit 1
    fi
fi

# Build command with URP
echo "Building LearningWorlds using URP (Unity $UNITY_VERSION)..."

# Default project path: adjust if needed
PROJECT_PATH="/Users/Tobias/git/agentflow/state-lw/worktrees/lw-0-scaffold"

# Navigate to project and build
$UNITY_ROOT/Unity -batchmode -projectPaths "$PROJECT_PATH" -logFile "ci/unity_compile.log" \
    -executeMethod LearningWorlds.BuildPitchProgress.UnityUtility.CompilationGate

# Return compilation success status
return $?
