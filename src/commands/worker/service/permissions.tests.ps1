$ErrorActionPreference = 'Stop'
$root = $env:PSF_GUARD_ACL_TEST_FOLDER
$user = [Security.Principal.WindowsIdentity]::GetCurrent().User.Value
$expected = @('S-1-5-18', 'S-1-5-32-544', $user) | Sort-Object
$paths = @((Get-Item -LiteralPath $root)) + @(Get-ChildItem -LiteralPath $root -Recurse -Force)
foreach ($item in $paths) {
    $acl = Get-Acl -LiteralPath $item.FullName
    $isRoot = $item.FullName -eq $root
    if ($acl.AreAccessRulesProtected -ne $isRoot) { throw "Wrong inheritance: $($item.FullName)" }
    $actual = @($acl.Access | ForEach-Object { $_.IdentityReference.Translate([Security.Principal.SecurityIdentifier]).Value }) | Sort-Object
    if (Compare-Object $expected $actual) { throw "Unexpected access list: $($item.FullName)" }
    foreach ($ace in $acl.Access) {
        if ($ace.AccessControlType -ne 'Allow' -or $ace.IsInherited -eq $isRoot) { throw "Wrong ACE: $($item.FullName)" }
        $sid = $ace.IdentityReference.Translate([Security.Principal.SecurityIdentifier]).Value
        $rights = if ($sid -eq $user) { [Security.AccessControl.FileSystemRights]::Modify } else { [Security.AccessControl.FileSystemRights]::FullControl }
        if (($ace.FileSystemRights -band $rights) -ne $rights) { throw "Missing rights for $sid" }
    }
}
if ((Get-Content -LiteralPath (Join-Path $root 'worker.json') -Raw).Trim() -ne 'test token') { throw 'Cannot read the settings' }
$probe = Join-Path $root 'tasks/write-probe'
[IO.File]::WriteAllText($probe, 'writable')
Remove-Item -LiteralPath $probe
