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
    $Result = "$([char]0x1b)]7391;PS`a"
    $Result += $Global:__HiruzenOriginalPrompt.Invoke()
    $Result += "$([char]0x1b)]7391;PE`a"
    if ($pwd.Provider.Name -eq 'FileSystem') {
        $Result += "$([char]0x1b)]7391;CWD;$(__Hiruzen-Escape $pwd.ProviderPath)`a"
    }
    return $Result
}
