$binary = $env:MESHHARBOR_UNINSTALL_BINARY
$deadline = [Diagnostics.Stopwatch]::StartNew()
do {
    try {
        [IO.File]::Delete($binary)
        return
    } catch {
        Start-Sleep -Milliseconds 200
    }
} while ($deadline.Elapsed.TotalSeconds -lt 30)
exit 1
