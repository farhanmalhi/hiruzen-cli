# hiruzen: dot-sourced by PowerShell at startup. Wraps the prompt in markers
# so hiruzen can tell which text on screen is the command being typed.
$Global:__HiruzenOriginalPrompt = $function:Prompt

function Global:__Hiruzen-Escape([string]$value) {
    [regex]::Replace($value, "[$([char]0x1b)$([char]0x07)\\\n;]", { param($match)
            -Join (
                [System.Text.Encoding]::UTF8.GetBytes($match.Value) | ForEach-Object { '\x{0:x2}' -f $_ }
            )
        })
}

function Global:Prompt() {
    # How the command just run ended. This must be the first statement:
    # anything before it would replace the value of $?.
    $Succeeded = $?
    $ExitStatus = if ($Succeeded) { 0 } elseif ($LASTEXITCODE) { $LASTEXITCODE } else { 1 }
    $Result = "$([char]0x1b)]7391;EXIT;$ExitStatus`a"
    $Result += "$([char]0x1b)]7391;PS`a"
    $Result += $Global:__HiruzenOriginalPrompt.Invoke()
    $Result += "$([char]0x1b)]7391;PE`a"
    if ($pwd.Provider.Name -eq 'FileSystem') {
        $Result += "$([char]0x1b)]7391;CWD;$(__Hiruzen-Escape $pwd.ProviderPath)`a"
    }
    return $Result
}

# Tell hiruzen the aliases and functions this shell defines, so it can
# suggest them. Names only: built-in aliases such as `ls` stand for cmdlets
# whose options differ from the tools hiruzen has specs for.
if (-not $env:HIRUZEN_REMOTE) {
    $__HiruzenNames = @(Get-Alias | ForEach-Object Name) + @(Get-Command -CommandType Function | ForEach-Object Name) |
        Where-Object { $_ -match '^[A-Za-z0-9][A-Za-z0-9._-]*$' }
    [Console]::Write("$([char]0x1b)]7391;NAMES;$($__HiruzenNames -join ' ')$([char]7)")
    Remove-Variable __HiruzenNames
}
