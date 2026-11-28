# Keeps builds and test runs at below-normal CPU priority, so the PC stays responsive
# (and a game keeps its frame rate) while agents build: Windows gives the CPU to
# normal-priority programs first, and builds take what is left.
#
#   powershell -NoProfile -ExecutionPolicy Bypass -File tools/lowprio.ps1
#
# Every 2 s it lowers cargo, rustc, the linkers, nextest and every test or server
# binary that runs from a build folder (a path with \target or \lucy-wt\target-).
# A program started from elsewhere (the user's release server in D:\Temp) keeps its priority,
# and so does the app itself (`lucy.exe`: an agent's game session serves a live client from
# its build folder and must keep its 62.5 Hz tick; test binaries are named lucy_<crate>-<hash>).
param([int]$Seconds = 2)
$tools = 'cargo', 'rustc', 'rust-lld', 'lld-link', 'link', 'cargo-nextest', 'cargo-clippy', 'clippy-driver', 'rustfmt', 'build-script-build'
while ($true) {
  foreach ($p in Get-Process -ErrorAction SilentlyContinue) {
    try {
      if ($p.PriorityClass -eq 'BelowNormal' -or $p.PriorityClass -eq 'Idle') { continue }
      $path = $p.Path
      $build = $tools -contains $p.ProcessName -or ($p.ProcessName -ne 'lucy' -and $path -and ($path -match '\\target(-[^\\]+)?\\(debug|release|gametest)\\'))
      if ($build) { $p.PriorityClass = 'BelowNormal' }
    } catch { }
  }
  Start-Sleep -Seconds $Seconds
}
