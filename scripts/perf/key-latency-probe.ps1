<#
Key latency probe (runs on the Windows target machine). Pure ASCII on purpose:
the driver key-latency.sh uploads it with a UTF-8 BOM (Windows PowerShell 5
reads BOM-less files as GBK). Do not run it by hand unless you know why.

Talks to the input service over its bridge named pipe, exactly like the TSF DLL
(frame = 8-byte header + 18-byte KeyPayload, see wind-ipc protocol.rs), sends a
keydown for each letter of every code, then Esc to cancel the composition
(nothing is committed, no word-frequency update). Times each round trip.

Output CSV (appended to -OutFile):  phase,code,keyindex|esc,ms,0xRESPCMD
Side file -OutFile.meta: working set / page faults before and after, skip counts.

Examples:
  .\key-latency-probe.ps1 -Variant dev -Codes "ni,hao,nihao" -Phase warm -OutFile r.csv
  .\key-latency-probe.ps1 -CodesFile codes.txt -Phase trim -Cold trim -OutFile r.csv
  .\key-latency-probe.ps1 -CodesFile codes.txt -Phase purge -Cold purge -OutFile r.csv   (admin)
#>
param(
    [ValidateSet("dev", "release")][string]$Variant = "dev",
    [string]$CodesFile,
    [string]$Codes,
    [Parameter(Mandatory = $true)][string]$Phase,
    # none | trim (EmptyWorkingSet on the service) | purge (trim + purge system standby list, needs admin)
    [ValidateSet("none", "trim", "purge")][string]$Cold = "none",
    [int]$Rounds = 1,
    [int]$KeyDelayMs = 0,
    [int]$CodeDelayMs = 30,
    [Parameter(Mandatory = $true)][string]$OutFile
)
$ErrorActionPreference = "Stop"

function Fail([string]$msg) { [Console]::Error.WriteLine("ERROR: $msg"); exit 1 }

if ([bool]$CodesFile -eq [bool]$Codes) { Fail "give exactly one of -CodesFile / -Codes" }
if ($Rounds -lt 1) { Fail "-Rounds must be >= 1" }

# dev: wind_input_dev / pipe wind_input_dev_<SID>;  release: wind_input / pipe wind_input_<SID>
$suffix = if ($Variant -eq "dev") { "_dev" } else { "" }
$procName = "wind_input$suffix"
$pipeRe = '\\' + [regex]::Escape("wind_input${suffix}_S-1-") + '[0-9-]+$'

# Map one character to a Windows virtual-key code; $null = unsupported (skipped and counted).
# Extend here for digits / punctuation (e.g. '0'-'9' => 0x30-0x39, ',' => 0xBC).
function Get-VkForChar([char]$c) {
    if ($c -ge 'a' -and $c -le 'z') { return [uint32][char]::ToUpper($c) }
    return $null
}

Add-Type -TypeDefinition @"
using System;
using System.Runtime.InteropServices;
public static class Mem {
    [StructLayout(LayoutKind.Sequential)] struct LUID { public uint Lo; public int Hi; }
    [StructLayout(LayoutKind.Sequential)] struct TP { public uint Count; public LUID Luid; public uint Attr; }
    [DllImport("advapi32.dll", SetLastError=true)] static extern bool OpenProcessToken(IntPtr h, uint acc, out IntPtr tok);
    [DllImport("advapi32.dll", SetLastError=true, CharSet=CharSet.Unicode)] static extern bool LookupPrivilegeValue(string sys, string name, out LUID l);
    [DllImport("advapi32.dll", SetLastError=true)] static extern bool AdjustTokenPrivileges(IntPtr tok, bool dis, ref TP n, uint len, IntPtr p, IntPtr r);
    [DllImport("kernel32.dll")] static extern IntPtr GetCurrentProcess();
    [DllImport("kernel32.dll", SetLastError=true)] static extern IntPtr OpenProcess(uint acc, bool inh, int pid);
    [DllImport("kernel32.dll")] static extern bool CloseHandle(IntPtr h);
    [DllImport("psapi.dll", SetLastError=true)] static extern bool EmptyWorkingSet(IntPtr h);
    [DllImport("ntdll.dll")] static extern int NtSetSystemInformation(int cls, ref int info, int len);
    static void Enable(string name) {
        IntPtr tok; LUID l;
        if (!OpenProcessToken(GetCurrentProcess(), 0x28, out tok)) throw new Exception("OpenProcessToken");
        if (!LookupPrivilegeValue(null, name, out l)) throw new Exception("Lookup " + name);
        TP tp = new TP { Count = 1, Luid = l, Attr = 2 };
        AdjustTokenPrivileges(tok, false, ref tp, 0, IntPtr.Zero, IntPtr.Zero);
        int err = Marshal.GetLastWin32Error();
        CloseHandle(tok);
        if (err != 0) throw new Exception("AdjustTokenPrivileges " + name + " err=" + err);
    }
    public static bool Trim(int pid) {
        IntPtr h = OpenProcess(0x0100 | 0x0400, false, pid);
        if (h == IntPtr.Zero) return false;
        bool ok = EmptyWorkingSet(h);
        CloseHandle(h);
        return ok;
    }
    public static int PurgeStandby() {
        Enable("SeProfileSingleProcessPrivilege");
        int cmd = 4; // MemoryPurgeStandbyList
        return NtSetSystemInformation(80, ref cmd, 4); // SystemMemoryListInformation
    }
}
"@

# ---- input list ----
if ($CodesFile) {
    if (-not (Test-Path -LiteralPath $CodesFile)) { Fail "codes file not found: $CodesFile" }
    $rawCodes = @(Get-Content -LiteralPath $CodesFile)
} else {
    $rawCodes = @($Codes -split ',')
}
$rawCodes = @($rawCodes | ForEach-Object { $_.Trim() } | Where-Object { $_ -ne "" })
$codeList = New-Object Collections.Generic.List[string]
$skipped = 0
foreach ($c in $rawCodes) {
    $ok = $true
    foreach ($ch in $c.ToCharArray()) { if ($null -eq (Get-VkForChar $ch)) { $ok = $false; break } }
    if ($ok) { $codeList.Add($c) } else { $skipped++ }
}
if ($codeList.Count -eq 0) { Fail "no usable codes (only a-z supported; $skipped skipped)" }

# ---- service process / pipe ----
$proc = Get-Process -Name $procName -ErrorAction SilentlyContinue | Select-Object -First 1
if (-not $proc) { Fail "service process '$procName' is not running (variant=$Variant)" }
$svcPid = $proc.Id

$outDir = Split-Path -Parent $OutFile
if ($outDir -and -not (Test-Path -LiteralPath $outDir)) { New-Item -ItemType Directory -Force -Path $outDir | Out-Null }
$meta = "$OutFile.meta"

if ($Cold -ne "none") {
    if (-not [Mem]::Trim($svcPid)) { Fail "EmptyWorkingSet failed on pid $svcPid (access denied?)" }
    $purgeStatus = 0
    if ($Cold -eq "purge") {
        try { $purgeStatus = [Mem]::PurgeStandby() }
        catch { Fail "purge needs an elevated (admin) session with SeProfileSingleProcessPrivilege: $($_.Exception.Message)" }
        if ($purgeStatus -ne 0) { Fail ("NtSetSystemInformation(purge standby) status=0x{0:X8}; run elevated" -f $purgeStatus) }
    }
    Start-Sleep -Milliseconds 500
    "cold=$Cold pid=$svcPid purgeStatus=$purgeStatus" | Out-File -Append -Encoding ascii $meta
}

$pipeFull = [IO.Directory]::GetFiles('\\.\pipe\') | Where-Object { $_ -match $pipeRe } | Select-Object -First 1
if (-not $pipeFull) { Fail "bridge pipe wind_input${suffix}_<SID> not found (is the service for variant=$Variant running?)" }
$pipeName = $pipeFull.Substring(9)

$proc.Refresh()
$faultsBefore = (Get-CimInstance Win32_Process -Filter "ProcessId=$svcPid").PageFaults
"phase=$Phase variant=$Variant pid=$svcPid wsMB=$([int]($proc.WorkingSet64/1MB)) faults_before=$faultsBefore codes=$($codeList.Count) rounds=$Rounds skipped_codes=$skipped" | Out-File -Append -Encoding ascii $meta

$pipe = New-Object IO.Pipes.NamedPipeClientStream('.', $pipeName, [IO.Pipes.PipeDirection]::InOut)
try { $pipe.Connect(3000) } catch { Fail "cannot connect to pipe $pipeName within 3s: $($_.Exception.Message)" }
$pipe.ReadMode = [IO.Pipes.PipeTransmissionMode]::Message

$script:seq = 0
$buf = New-Object byte[] 65536
$sw = New-Object Diagnostics.Stopwatch
# One KEY_EVENT keydown round trip; returns @(ms, response command).
# Frame: version u16=0x1001, command u16=0x0101, length u32=18, then KeyPayload
# (key_code u32 @0, scan_code @4, modifiers @8, event_type u8 @12, toggles u8 @13, event_seq u16 @14, prev_char u16 @16).
function Send-Key([uint32]$vk) {
    $script:seq = ($script:seq + 1) % 65536
    $msg = New-Object byte[] 26
    [BitConverter]::GetBytes([uint16]0x1001).CopyTo($msg, 0)
    [BitConverter]::GetBytes([uint16]0x0101).CopyTo($msg, 2)
    [BitConverter]::GetBytes([uint32]18).CopyTo($msg, 4)
    [BitConverter]::GetBytes($vk).CopyTo($msg, 8)
    [BitConverter]::GetBytes([uint16]$script:seq).CopyTo($msg, 22)
    $sw.Restart()
    $pipe.Write($msg, 0, 26)
    $pipe.Flush()
    $n = 0
    do { $n += $pipe.Read($buf, $n, $buf.Length - $n) } while (-not $pipe.IsMessageComplete)
    $sw.Stop()
    return @($sw.Elapsed.TotalMilliseconds, [BitConverter]::ToUInt16($buf, 2))
}

$lines = New-Object Collections.Generic.List[string]
try {
    for ($round = 1; $round -le $Rounds; $round++) {
        foreach ($code in $codeList) {
            $i = 0
            foreach ($ch in $code.ToCharArray()) {
                $r = Send-Key (Get-VkForChar $ch)
                $lines.Add(("{0},{1},{2},{3:F3},0x{4:X4}" -f $Phase, $code, $i, $r[0], $r[1]))
                $i++
                if ($KeyDelayMs -gt 0) { Start-Sleep -Milliseconds $KeyDelayMs }
            }
            $r = Send-Key 0x1B   # Esc: cancel composition, nothing committed
            $lines.Add(("{0},{1},esc,{2:F3},0x{3:X4}" -f $Phase, $code, $r[0], $r[1]))
            if ($CodeDelayMs -gt 0) { Start-Sleep -Milliseconds $CodeDelayMs }
        }
    }
} finally {
    $pipe.Dispose()
    # keep what we measured even if the pipe broke half way
    [IO.File]::AppendAllLines($OutFile, $lines)
}
$proc.Refresh()
"phase=$Phase done rows=$($lines.Count) faults_after=$((Get-CimInstance Win32_Process -Filter "ProcessId=$svcPid").PageFaults) wsMB=$([int]($proc.WorkingSet64/1MB))" | Out-File -Append -Encoding ascii $meta
