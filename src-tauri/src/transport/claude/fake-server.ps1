# Synthetic protocol fixture. Never reads Claude config, credentials, or transcripts.
$ErrorActionPreference = 'Stop'
$mode = [IO.File]::ReadAllText((Join-Path $PSScriptRoot 'mode'))
$sid = [IO.File]::ReadAllText((Join-Path $PSScriptRoot 'session-id'))
$child = Start-Process -FilePath "$PSHOME\powershell.exe" -ArgumentList '-NoProfile -NonInteractive -Command Start-Sleep -Seconds 300' -WindowStyle Hidden -PassThru
[IO.File]::WriteAllText((Join-Path $PSScriptRoot 'child.pid'), [string]$child.Id)
[IO.File]::WriteAllText((Join-Path $PSScriptRoot 'environment.json'), (@{parent_marker_present=[bool]$env:CLAUDECODE;agent=$env:HIVE_AGENT_ID}|ConvertTo-Json -Compress))
function Emit($value) { [Console]::Out.WriteLine(($value | ConvertTo-Json -Compress -Depth 20)) }
function Reply($m,$value) { Emit @{type='control_response';response=@{subtype='success';request_id=$m.request_id;response=$value}} }
function Finish { Emit @{type='result';session_id=$sid;parent_tool_use_id=$null;user_message_uuids=@($script:active);subtype='success'} }
while ($null -ne ($line = [Console]::ReadLine())) {
    $m = $line | ConvertFrom-Json
    [IO.File]::AppendAllText((Join-Path $PSScriptRoot 'requests.jsonl'), $line+"`n")
    if ($m.type -eq 'control_request') {
        if ($m.request.subtype -eq 'initialize') {
            if ($mode -eq 'cancel') { continue }
            $account = @{subscriptionType='Claude Ultra'}
            if ($mode -eq 'api-account') { $account.apiKeySource='apiKeyHelper' }
            $version='2.1.316';if ($mode -eq 'version') { $version='future-version' }
            Reply $m @{account=$account;claude_code_version=$version}
            if ($mode -eq 'closed-stdin') {
                Add-Type -TypeDefinition 'using System;using System.Runtime.InteropServices;public class ClaudePipeClose{[DllImport("kernel32.dll")]public static extern IntPtr GetStdHandle(int n);[DllImport("kernel32.dll")]public static extern bool CloseHandle(IntPtr h);}'
                [ClaudePipeClose]::CloseHandle([ClaudePipeClose]::GetStdHandle(-10)) | Out-Null
                [IO.File]::WriteAllText((Join-Path $PSScriptRoot 'input-closed'),'yes')
                Start-Sleep -Seconds 300
            }
        } elseif ($m.request.subtype -eq 'interrupt') {
            if ($mode -eq 'exit') { exit 0 }
            Reply $m @{still_queued=@()};Finish
        }
    } elseif ($m.type -eq 'user') {
        $script:active=$m.uuid
        $source='none';if ($mode -eq 'api-source') { $source='apiKeyHelper' }
        if (!$script:sentInit) {
            Emit @{type='system';subtype='init';session_id=$sid;apiKeySource=$source;permissionMode='bypassPermissions';claude_code_version='2.1.316'}
            $script:sentInit=$true
        }
        Emit @{type='future_unknown';session_id=$sid}
        Emit @{type='user';session_id='foreign';uuid=$m.uuid;isReplay=$true;message=@{role='user';content='foreign'}}
        Emit @{type='stream_event';session_id='foreign';event=@{type='message_start';message=@{id='foreign-message'}}}
        Emit @{type='user';session_id=$sid;uuid=$m.uuid;parent_tool_use_id='child';isReplay=$true;message=@{role='user';content='child'}}
        Emit @{type='user';session_id=$sid;uuid=$m.uuid;parent_tool_use_id=$null;isReplay=$true;message=$m.message}
        Emit @{type='stream_event';session_id=$sid;parent_tool_use_id=$null;event=@{type='message_start';message=@{id='synthetic-message'}}}
        Emit @{type='assistant';session_id=$sid;parent_tool_use_id=$null;message=@{id='synthetic-message';content=@(@{type='text';text='Synthetic answer'},@{type='tool_use';id='tool';name='Read';input=@{file_path='fixture.txt'}})}}
    }
}
