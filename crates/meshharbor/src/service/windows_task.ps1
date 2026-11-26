$ErrorActionPreference = 'Stop'
$operation = $env:MESHHARBOR_TASK_OPERATION
$sid = [Security.Principal.WindowsIdentity]::GetCurrent().User.Value
$name = 'MeshHarbor-' + $sid
$marker = 'Registered by meshharbor service install: '

$scheduler = New-Object -ComObject Schedule.Service
$scheduler.Connect()
$folder = $scheduler.GetFolder('\')
$task = $null
try {
    $task = $folder.GetTask($name)
} catch {
    $notFound = $false
    for ($failure = $_.Exception; $null -ne $failure; $failure = $failure.InnerException) {
        if ($failure.HResult -eq 0x80070002) { $notFound = $true; break }
    }
    if (-not $notFound) { throw }
}

$id = [guid]::Empty
if ($null -ne $task) {
    $definition = $task.Definition
    $description = [string]$definition.RegistrationInfo.Description
    if (-not $description.StartsWith($marker, [StringComparison]::Ordinal) -or
        -not [guid]::TryParseExact($description.Substring($marker.Length), 'D', [ref]$id)) {
        throw 'Refusing to modify a task not installed by MeshHarbor.'
    }
}

if ($operation -eq 'install') {
    $binary = $env:MESHHARBOR_TASK_BINARY
    $arguments = $env:MESHHARBOR_TASK_ARGUMENTS
    if ($null -ne $task) {
        if ($definition.Actions.Count -ne 1 -or
            $definition.Actions.Item(1).Path -ine $binary -or
            $definition.Actions.Item(1).Arguments -cne ($arguments + ' --background-id ' + $id)) {
            throw 'The installed service uses a different configuration. Run meshharbor service uninstall before reinstalling.'
        }
    } else {
        $id = [guid]::NewGuid()
        $definition = $scheduler.NewTask(0)
        $definition.RegistrationInfo.Description = $marker + $id
        $definition.Principal.UserId = $sid
        $interactiveToken = 3
        $definition.Principal.LogonType = $interactiveToken
        $definition.Principal.RunLevel = 0
        $logonTrigger = 9
        $trigger = $definition.Triggers.Create($logonTrigger)
        $trigger.UserId = $sid
        $settings = $definition.Settings
        $settings.DisallowStartIfOnBatteries = $false
        $settings.StopIfGoingOnBatteries = $false
        $settings.ExecutionTimeLimit = 'PT0S'
        $settings.RestartInterval = 'PT1M'
        $settings.RestartCount = 10
        $ignoreNewInstance = 2
        $settings.MultipleInstances = $ignoreNewInstance
        $execAction = 0
        $action = $definition.Actions.Create($execAction)
        $action.Path = $binary
        $action.Arguments = $arguments + ' --background-id ' + $id
        $createOnly = 2
        $null = $folder.RegisterTaskDefinition($name, $definition, $createOnly, $sid, $null, $interactiveToken, $null)
    }
    Write-Output ('Installed Windows scheduled task ' + $name + '. Run meshharbor service start to start it now.')
    return
}

if ($null -eq $task) {
    throw 'MeshHarbor service is not installed. Run meshharbor service install first.'
}

$queued = 2
$running = 4
switch ($operation) {
    'start' {
        if ($task.State -notin ($queued, $running)) { $null = $task.Run($null) }
    }
    'status' {
        if ($task.State -eq $running) {
            Write-Output 'MeshHarbor service is running.'
        } else {
            throw ('MeshHarbor service is not running. Task Scheduler last result: 0x{0:X8}. See %LOCALAPPDATA%\MeshHarbor\service\serve.log.' -f [int]$task.LastTaskResult)
        }
    }
    { $_ -in ('stop', 'uninstall') } {
        $deadline = [Diagnostics.Stopwatch]::StartNew()
        $signaled = $false
        while ($task.State -in ($queued, $running)) {
            if (-not $signaled) {
                try {
                    $event = [Threading.EventWaitHandle]::OpenExisting('Global\MeshHarbor.Stop.' + $id)
                    try {
                        $signaled = $event.Set()
                    } finally {
                        $event.Dispose()
                    }
                } catch [Threading.WaitHandleCannotBeOpenedException] {
                }
            }
            if ($deadline.Elapsed.TotalSeconds -ge 90) {
                throw 'Timed out waiting for MeshHarbor to stop. The task was left registered so active jobs are not forcefully terminated.'
            }
            Start-Sleep -Milliseconds 200
        }
        if ($operation -eq 'uninstall') {
            $folder.DeleteTask($name, 0)
            Write-Output 'Removed the Windows scheduled task. MeshHarbor data was not changed.'
        }
    }
    default { throw 'Unknown MeshHarbor service operation.' }
}
