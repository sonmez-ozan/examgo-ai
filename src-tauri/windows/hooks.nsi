; ExamGo AI installer hooks
; After the app itself is installed, silently install the Ollama local-AI
; runtime so the app is ready to generate questions on first launch.
; If this fails (no winget, offline, user cancels), the app repeats the
; setup automatically on first run, so failure here is non-fatal.

!macro NSIS_HOOK_POSTINSTALL
  DetailPrint "Installing local AI runtime (Ollama)…"
  nsExec::ExecToLog 'winget install -e --id Ollama.Ollama --silent --accept-package-agreements --accept-source-agreements'
  Pop $0
  DetailPrint "Ollama installer finished (code $0)"
!macroend
