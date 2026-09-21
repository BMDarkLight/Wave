@echo off
rem  Wave CLI shim  --  Copyright (C) 2025 BMDarkLight, AGPL-3.0.
rem
rem  Only this directory goes on PATH, never the install root. wave.exe is a
rem  GUI-subsystem binary, so an interactive prompt would not wait for it and
rem  would redraw over its output. Routing through a .cmd makes cmd.exe wait
rem  and propagate the exit code, which is what a CLI is expected to do.
rem
rem  PATHEXT resolves .EXE before .CMD, so this file must not sit next to
rem  wave.exe or the shim would never be picked.
"%~dp0..\wave.exe" %*
exit /b %ERRORLEVEL%
