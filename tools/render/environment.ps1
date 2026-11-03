param(
  [Parameter(Mandatory = $true)][int]$ProcessId,
  [Parameter(Mandatory = $true)][string]$ExpectedExe,
  [Parameter(Mandatory = $true)][string]$ExpectedStart
)
$ErrorActionPreference = 'Stop'
$target = Get-CimInstance Win32_Process -Filter "ProcessId=$ProcessId"
if (!$target -or $target.ExecutablePath -ne $ExpectedExe -or $target.CreationDate.ToFileTimeUtc().ToString() -ne $ExpectedStart) {
  throw 'Process identity changed before environment inspection'
}
# Read only the owned x64 process. Return the isolation roots, never the complete environment.
Add-Type -TypeDefinition @'
using System;
using System.Text;
using System.Collections.Generic;
using System.Runtime.InteropServices;
public static class RenderEnvironment {
  [StructLayout(LayoutKind.Sequential)] struct Basic { public IntPtr reserved,peb,a,b,pid,c; }
  [DllImport("kernel32.dll",SetLastError=true)] static extern IntPtr OpenProcess(uint access,bool inherit,int pid);
  [DllImport("kernel32.dll",SetLastError=true)] static extern bool ReadProcessMemory(IntPtr process,IntPtr address,byte[] buffer,int size,out IntPtr read);
  [DllImport("kernel32.dll")] static extern bool CloseHandle(IntPtr handle);
  [DllImport("ntdll.dll")] static extern int NtQueryInformationProcess(IntPtr process,int kind,ref Basic info,int length,out int returned);
  static IntPtr Pointer(IntPtr process,IntPtr address) {
    var b=new byte[8];IntPtr read;
    if(!ReadProcessMemory(process,address,b,8,out read)||read.ToInt64()!=8)throw new Exception("Unable to read owned process pointer");
    return new IntPtr(BitConverter.ToInt64(b,0));
  }
  public static Dictionary<string,string> Read(int pid) {
    if(IntPtr.Size!=8)throw new Exception("Environment probe requires x64");
    var process=OpenProcess(0x410,false,pid);
    if(process==IntPtr.Zero)throw new Exception("Cannot inspect owned process");
    try {
      var info=new Basic();int returned;
      if(NtQueryInformationProcess(process,0,ref info,Marshal.SizeOf(info),out returned)!=0)throw new Exception("Cannot read owned process information");
      var parameters=Pointer(process,IntPtr.Add(info.peb,0x20));
      var environment=Pointer(process,IntPtr.Add(parameters,0x80));
      var text=new StringBuilder();bool ended=false;
      for(int offset=0;offset<262144&&!ended;offset+=128) {
        var buffer=new byte[128];IntPtr count;
        var success=ReadProcessMemory(process,IntPtr.Add(environment,offset),buffer,buffer.Length,out count);
        var bytes=(int)count.ToInt64();
        if(bytes<=0)break;
        text.Append(Encoding.Unicode.GetString(buffer,0,bytes-(bytes%2)));
        ended=text.ToString().IndexOf("\0\0",StringComparison.Ordinal)>=0;
        if(!success)break;
      }
      if(!ended)throw new Exception("Owned environment terminator not found within bound");
      var result=new Dictionary<string,string>(StringComparer.OrdinalIgnoreCase);
      var allowed=new HashSet<string>(new[]{"USERPROFILE","HOME","APPDATA","LOCALAPPDATA","TEMP","TMP","CODEX_HOME","CLAUDE_CONFIG_DIR","WEBVIEW2_USER_DATA_FOLDER"},StringComparer.OrdinalIgnoreCase);
      var block=text.ToString();
      block=block.Substring(0,block.IndexOf("\0\0",StringComparison.Ordinal));
      foreach(var entry in block.Split('\0')) {
        var equal=entry.IndexOf('=');if(equal<=0)continue;
        var key=entry.Substring(0,equal);if(allowed.Contains(key))result[key]=entry.Substring(equal+1);
      }
      return result;
    } finally {CloseHandle(process);}
  }
}
'@
[RenderEnvironment]::Read($ProcessId) | ConvertTo-Json -Compress
