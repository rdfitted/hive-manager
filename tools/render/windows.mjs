import { execFileSync } from 'node:child_process';
import path from 'node:path';
export const quote = value => `'${String(value).replaceAll("'", "''")}'`;
export const ps = code => execFileSync('powershell.exe', ['-NoProfile', '-Command', code], { encoding: 'utf8', windowsHide: true, maxBuffer: 8 * 1024 * 1024 }).trim();
export const sleep = ms => new Promise(resolve => setTimeout(resolve, ms));
export const plainPath = value => value.replace(/^\\\\\?\\/, '');
export function inside(root, value) {
  if (!value) return false;
  const relative = path.relative(root, plainPath(value));
  return relative !== '..' && !relative.startsWith(`..${path.sep}`) && !path.isAbsolute(relative);
}
export function identity(pid) {
  if (!Number.isInteger(pid) || pid <= 0) throw new Error('Invalid owned process PID');
  const raw = ps(`$p=Get-CimInstance Win32_Process -Filter 'ProcessId=${pid}';if($p){@{pid=$p.ProcessId;exe=$p.ExecutablePath;started=$p.CreationDate.ToFileTimeUtc().ToString();commandLine=$p.CommandLine}|ConvertTo-Json -Compress}`);
  return raw ? JSON.parse(raw) : null;
}
export function sameProcess(expected, current) {
  return !!expected && !!current && typeof expected.exe === 'string' && typeof current.exe === 'string' && expected.pid === current.pid && expected.started === current.started && expected.exe.toLowerCase() === current.exe.toLowerCase();
}
export function tree(pid) {
  const raw = ps(`$all=@(Get-CimInstance Win32_Process);$ids=@(${pid});do{$next=@($all|Where-Object{$_.ParentProcessId -in $ids -and $_.ProcessId -notin $ids}|Select-Object -ExpandProperty ProcessId);$ids+=$next}while($next.Count);@($all|Where-Object{$_.ProcessId -in $ids}|Select-Object @{n='pid';e={$_.ProcessId}},@{n='exe';e={$_.ExecutablePath}},@{n='started';e={$_.CreationDate.ToFileTimeUtc().ToString()}},CommandLine)|ConvertTo-Json -Depth 3`);
  return raw ? [JSON.parse(raw)].flat() : [];
}
export async function killOwned(expected) {
  const current = identity(expected.pid);
  if (!current) return { alreadyExited: true };
  if (!sameProcess(expected, current)) throw new Error('Refusing to kill a PID whose executable or start time changed');
  const children = tree(expected.pid);
  let warning;
  try { ps(`& taskkill.exe /PID ${expected.pid} /T /F|Out-Null`); } catch (error) { warning = error.message; }
  await sleep(250);
  const remaining = children.filter(item => sameProcess(item, identity(item.pid)));
  // A child can outlive a concurrently exiting parent; reap only recorded identities.
  for (const item of remaining) {
    if (sameProcess(item, identity(item.pid))) {
      try { ps(`& taskkill.exe /PID ${item.pid} /T /F|Out-Null`); } catch { /* Verify below. */ }
    }
  }
  await sleep(250);
  const leftovers = children.filter(item => sameProcess(item, identity(item.pid))).map(item => item.pid);
  if (leftovers.length) throw new Error(`Owned process cleanup failed for PID ${leftovers.join(',')}`);
  return { stopped: expected.pid, warning, leftovers };
}
export async function liveState() {
  const listeners = ps('@(Get-NetTCPConnection -State Listen -LocalPort 18800 -ErrorAction SilentlyContinue|Select-Object -ExpandProperty OwningProcess -Unique)-join ","');
  if (!listeners) return { running: false, pids: [], health: null };
  const pids = listeners.split(',').map(Number);
  const response = await fetch('http://127.0.0.1:18800/health', { signal: AbortSignal.timeout(3000) });
  if (!response.ok) throw new Error('Live health check failed before/after render run');
  return { running: true, pids, processes: pids.map(identity), health: await response.text() };
}
export function liveUnchanged(before, after) {
  return !!before && !!after && before.running === after.running && before.health === after.health && [...before.pids].sort().join(',') === [...after.pids].sort().join(',') && (before.processes ?? []).every(item => sameProcess(item, (after.processes ?? []).find(other => other.pid === item.pid)));
}
export function desktopState() {
  return JSON.parse(ps(`Add-Type -TypeDefinition 'using System;using System.Text;using System.Runtime.InteropServices;public class RenderDesktop { [DllImport("user32.dll")]public static extern IntPtr GetForegroundWindow();[DllImport("user32.dll")]static extern IntPtr OpenInputDesktop(uint f,bool i,uint a);[DllImport("user32.dll")]static extern bool CloseDesktop(IntPtr h);[DllImport("user32.dll",CharSet=CharSet.Unicode)]static extern bool GetUserObjectInformation(IntPtr h,int n,StringBuilder b,int l,out int z);public static string Name(){var h=OpenInputDesktop(0,false,1);if(h==IntPtr.Zero)return "unavailable";try{var b=new StringBuilder(256);int n;return GetUserObjectInformation(h,2,b,512,out n)?b.ToString():"unavailable";}finally{CloseDesktop(h);}}}';@{inputDesktop=[RenderDesktop]::Name();foreground=[RenderDesktop]::GetForegroundWindow().ToInt64()}|ConvertTo-Json -Compress`));
}
export function environment(expected, repo) {
  return JSON.parse(ps(`& ${quote(path.join(repo, 'tools/render/environment.ps1'))} -ProcessId ${expected.pid} -ExpectedExe ${quote(expected.exe)} -ExpectedStart ${quote(expected.started)}`));
}
