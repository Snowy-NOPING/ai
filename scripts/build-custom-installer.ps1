$ErrorActionPreference = "Stop"

$projectRoot = Split-Path -Parent $PSScriptRoot
$tauriConfigPath = Join-Path $projectRoot "src-tauri\tauri.conf.json"
$installerRoot = Join-Path $projectRoot "custom-installer"
$installerHtmlSource = "C:\Users\santi\Downloads\xocat-installer.html"
$installerHtmlTarget = Join-Path $installerRoot "src\index.html"
$releaseRoot = Join-Path $projectRoot "release\gemma-local-chat"
$privateKeyPath = Join-Path $projectRoot "src-tauri\updater-private.key"

$config = Get-Content -LiteralPath $tauriConfigPath -Raw | ConvertFrom-Json
$targets = @($config.bundle.targets)
$blockedTargets = @("nsis", "msi", "wix")
$blocked = @($targets | Where-Object { $blockedTargets -contains $_ })
if ($blocked.Count -gt 0) {
  throw "tauri.conf.json bundle.targets must not include: $($blocked -join ', ')"
}

if (-not (Test-Path -LiteralPath $installerHtmlSource)) {
  throw "missing custom installer html at $installerHtmlSource"
}

if (Test-Path -LiteralPath $privateKeyPath) {
  $env:TAURI_SIGNING_PRIVATE_KEY = (Get-Content -LiteralPath $privateKeyPath -Raw).Trim()
  $env:TAURI_SIGNING_PRIVATE_KEY_PASSWORD = ""
}

$installerScript = @'
<script>
const installer = {
  name: 'Gemma Local Chat.exe',
};

const fillEl = document.getElementById('fill');
const pctEl = document.getElementById('pct');
const etaEl = document.getElementById('eta');
const speedEl = document.getElementById('speed');
const bytesEl = document.getElementById('bytes');
const filenameEl = document.getElementById('filename');
const subtitleEl = document.getElementById('subtitle');
const titleEl = document.getElementById('title');
const markEl = document.getElementById('mark');
const checkEl = document.getElementById('check');

function setFile(prefix, name) {
  filenameEl.innerHTML = '<span class="dim">' + prefix + '</span> ' + name;
}

function formatBytes(bytes) {
  if (!Number.isFinite(bytes) || bytes <= 0) return '0 MB';
  return (bytes / 1024 / 1024).toFixed(1) + ' MB';
}

function setProgress(percent, status, detail) {
  const safePercent = Math.max(0, Math.min(100, Number(percent) || 0));
  fillEl.style.width = safePercent + '%';
  pctEl.textContent = Math.floor(safePercent) + '%';
  etaEl.textContent = status || 'working';
  speedEl.textContent = 'local';
  if (detail) setFile(status || 'installing', detail);
}

function wireWindowControls(tauri) {
  document.querySelector('.dot.r')?.addEventListener('click', () => {
    tauri.core.invoke('close_installer');
  });
  document.querySelector('.dot.y')?.addEventListener('click', () => {
    tauri.core.invoke('minimize_installer');
  });
  document.querySelector('.dot.g')?.addEventListener('click', () => {
    tauri.core.invoke('toggle_maximize_installer');
  });
}

async function runInstaller() {
  const tauri = window.__TAURI__;
  if (!tauri?.core?.invoke) {
    throw new Error('installer bridge is not available');
  }
  wireWindowControls(tauri);

  const info = await tauri.core.invoke('installer_info');
  installer.name = info.payloadName || installer.name;

  setFile('payload', installer.name);
  subtitleEl.textContent = 'Installing local payload';
  bytesEl.textContent = '0 MB / ' + formatBytes(info.payloadSize);
  speedEl.textContent = 'local';
  etaEl.textContent = 'preparing';
  setProgress(5, 'preparing', installer.name);

  const unlisten = await tauri.event.listen('install-progress', (event) => {
    const progress = event.payload || {};
    setProgress(progress.percent, progress.status, progress.detail);
    if (info.payloadSize) {
      const received = Math.floor(info.payloadSize * ((Number(progress.percent) || 0) / 100));
      bytesEl.textContent = formatBytes(received) + ' / ' + formatBytes(info.payloadSize);
    }
  });

  try {
    const result = await tauri.core.invoke('install_app');
    setProgress(100, 'installed', installer.name);
    bytesEl.textContent = formatBytes(info.payloadSize) + ' / ' + formatBytes(info.payloadSize);

    window.setTimeout(() => {
      markEl.style.display = 'none';
      checkEl.classList.add('show');
      titleEl.textContent = 'Install complete';
      subtitleEl.textContent = 'Gemma Local Chat is ready';
      setFile('installed', result.appPath || installer.name);
    }, 500);
  } finally {
    unlisten();
  }
}

runInstaller().catch((error) => {
  fillEl.style.width = '100%';
  fillEl.style.background = 'linear-gradient(90deg, #ff5f56, #ffbd2e)';
  pctEl.textContent = 'error';
  etaEl.textContent = 'failed';
  speedEl.textContent = '--';
  bytesEl.textContent = '0 MB / -- MB';
  setFile('failed', installer.name);
  titleEl.textContent = 'Install failed';
  subtitleEl.textContent = error.message || 'Could not install app';
});
</script>
'@

$sourceHtml = Get-Content -LiteralPath $installerHtmlSource -Raw
$generatedHtml = [regex]::Replace($sourceHtml, '(?s)<script>.*?</script>\s*</body>', "$installerScript`r`n</body>")
$generatedHtml = $generatedHtml.Replace('.dot{', '.dot{cursor:pointer;')
Set-Content -LiteralPath $installerHtmlTarget -Value $generatedHtml -Encoding UTF8

npm run tauri build

$payloadExe = Join-Path $projectRoot "src-tauri\target\release\gemma4-chat.exe"
if (-not (Test-Path -LiteralPath $payloadExe)) {
  throw "main app exe was not built at $payloadExe"
}

cargo build --release --manifest-path (Join-Path $installerRoot "Cargo.toml")

$customInstallerExe = Join-Path $installerRoot "target\release\gemma-local-chat-installer.exe"
if (-not (Test-Path -LiteralPath $customInstallerExe)) {
  throw "custom installer exe was not built at $customInstallerExe"
}

New-Item -ItemType Directory -Force -Path $releaseRoot | Out-Null

Get-ChildItem -LiteralPath $releaseRoot -File -ErrorAction SilentlyContinue |
  Where-Object { $_.Name -like "Gemma Local Chat_*_x64*" -or $_.Extension -eq ".msi" } |
  Remove-Item -Force

foreach ($staleBundle in @("nsis", "msi", "wix")) {
  $stalePath = Join-Path $projectRoot "src-tauri\target\release\bundle\$staleBundle"
  if (Test-Path -LiteralPath $stalePath) {
    Remove-Item -LiteralPath $stalePath -Recurse -Force
  }
}

Copy-Item -LiteralPath $customInstallerExe -Destination (Join-Path $releaseRoot "Gemma Local Chat Setup.exe") -Force
$webInstallerHtml = (Get-Content -LiteralPath $installerHtmlSource -Raw).Replace(
  "Gemma Local Chat_0.1.0_x64-setup.exe",
  "Gemma Local Chat Setup.exe"
).Replace(
  "Gemma Local Chat_1.0.0_x64-setup.exe",
  "Gemma Local Chat Setup.exe"
)
Set-Content -LiteralPath (Join-Path $releaseRoot "installer.html") -Value $webInstallerHtml -Encoding UTF8

$releaseInstaller = Join-Path $releaseRoot "Gemma Local Chat Setup.exe"
$releaseSignature = Join-Path $releaseRoot "Gemma Local Chat Setup.exe.sig"

if (Test-Path -LiteralPath $privateKeyPath) {
  $signOutput = npx tauri signer sign --password= $releaseInstaller | Out-String
  if ($signOutput -match 'Public signature:\s*([A-Za-z0-9+/=]+)') {
    Set-Content -LiteralPath $releaseSignature -Value $matches[1] -Encoding UTF8
  } elseif (Test-Path -LiteralPath $releaseSignature) {
    $signature = Get-Content -LiteralPath $releaseSignature -Raw
    Set-Content -LiteralPath $releaseSignature -Value $signature.Trim() -Encoding UTF8
  } else {
    throw "could not extract updater signature for custom installer"
  }

  $package = Get-Content -LiteralPath (Join-Path $projectRoot "package.json") -Raw | ConvertFrom-Json
  $signature = (Get-Content -LiteralPath $releaseSignature -Raw).Trim()
  $manifest = [ordered]@{
    version = $package.version
    notes = "custom installer release"
    pub_date = (Get-Date).ToUniversalTime().ToString("yyyy-MM-ddTHH:mm:ssZ")
    platforms = [ordered]@{
      "windows-x86_64" = [ordered]@{
        signature = $signature
        url = "https://cdn.xocat.online/apps/gemma-local-chat/setup.exe"
      }
    }
  } | ConvertTo-Json -Depth 5
  [System.IO.File]::WriteAllText(
    (Join-Path $releaseRoot "latest.json"),
    $manifest,
    [System.Text.UTF8Encoding]::new($false)
  )
}

Write-Host ""
Write-Host "custom installer built:"
Write-Host (Join-Path $releaseRoot "Gemma Local Chat Setup.exe")
