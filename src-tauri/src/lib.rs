use encoding_rs::GBK;
use regex::Regex;
use serde::{Deserialize, Serialize};
use std::cmp::Ordering;
use std::env;
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::mpsc::{self, Sender};
use std::thread;
use std::time::Duration;
use tauri::Emitter;

const SETTINGS_FILE: &str = "settings.json";
const DEFAULT_PROXY_URL: &str = "http://10.20.34.92:7890";
const MANAGED_BLOCK_START: &str = "# >>> CX-Manager managed profile";
const MANAGED_BLOCK_END: &str = "# <<< CX-Manager managed profile";
const LEGACY_HELPERS_HEADER: &str = "# CX-Manager default terminal helpers";
const TOOL_PROGRESS_EVENT: &str = "tool-progress";
#[cfg(target_os = "windows")]
const CREATE_NO_WINDOW: u32 = 0x08000000;

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum TargetShell {
    Auto,
    Pwsh,
    Powershell,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default, rename_all = "camelCase")]
pub struct AppSettings {
    pub target_shell: TargetShell,
    pub proxy_url: String,
    pub use_proxy_for_tools: bool,
    pub project_roots: Vec<String>,
}

impl Default for AppSettings {
    fn default() -> Self {
        Self {
            target_shell: TargetShell::Auto,
            proxy_url: DEFAULT_PROXY_URL.to_string(),
            use_proxy_for_tools: true,
            project_roots: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ShellStatus {
    pub target_shell: TargetShell,
    pub command: String,
    pub version: Option<String>,
    pub profile_path: String,
    pub message: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct AppState {
    pub settings: AppSettings,
    pub shell_status: ShellStatus,
    pub profile_status: ProfileStatus,
    pub toolchain_status: ToolchainStatus,
    pub codex_status: CodexStatus,
}

fn workspace_settings_path() -> Result<PathBuf, String> {
    env::current_dir()
        .map(|dir| dir.join(SETTINGS_FILE))
        .map_err(|err| format!("无法获取当前目录: {err}"))
}

fn dirs_home() -> PathBuf {
    env::var_os("USERPROFILE")
        .map(PathBuf::from)
        .or_else(|| env::var_os("HOME").map(PathBuf::from))
        .unwrap_or_else(|| PathBuf::from("."))
}

fn normalized_project_roots(existing_roots: &[String]) -> Vec<String> {
    let mut roots = Vec::new();
    for root in existing_roots
        .iter()
        .map(|root| root.trim())
        .filter(|root| !root.is_empty())
    {
        if !roots
            .iter()
            .any(|existing: &String| existing.eq_ignore_ascii_case(root))
        {
            roots.push(root.to_string());
        }
    }
    roots
}

fn default_project_roots_for(home: &Path) -> Vec<String> {
    let pycharm_projects = home.join("PycharmProjects");
    if pycharm_projects.is_dir() {
        vec![pycharm_projects.to_string_lossy().to_string()]
    } else {
        Vec::new()
    }
}

fn apply_project_root_defaults(settings: &mut AppSettings) {
    if settings.project_roots.is_empty() {
        settings.project_roots = default_project_roots_for(&dirs_home());
    }
    settings.project_roots = normalized_project_roots(&settings.project_roots);
}

fn load_settings_from_disk() -> Result<AppSettings, String> {
    let path = workspace_settings_path()?;
    if !path.exists() {
        let mut settings = AppSettings::default();
        apply_project_root_defaults(&mut settings);
        return Ok(settings);
    }

    let raw = fs::read_to_string(&path).map_err(|err| format!("读取 settings.json 失败: {err}"))?;
    let mut settings: AppSettings =
        serde_json::from_str(&raw).map_err(|err| format!("解析 settings.json 失败: {err}"))?;
    if settings.proxy_url.trim().is_empty() {
        settings.proxy_url = DEFAULT_PROXY_URL.to_string();
    } else {
        settings.proxy_url = settings.proxy_url.trim().to_string();
    }
    settings.project_roots = normalized_project_roots(&settings.project_roots);
    Ok(settings)
}

fn save_settings_to_disk(settings: &AppSettings) -> Result<(), String> {
    let path = workspace_settings_path()?;
    let content =
        serde_json::to_string_pretty(settings).map_err(|err| format!("序列化设置失败: {err}"))?;
    fs::write(&path, content).map_err(|err| format!("写入 settings.json 失败: {err}"))
}

fn resolve_profile_path_for(target_shell: TargetShell, home: &Path) -> PathBuf {
    let folder = match target_shell {
        TargetShell::Powershell => "WindowsPowerShell",
        TargetShell::Auto | TargetShell::Pwsh => "PowerShell",
    };
    home.join("Documents")
        .join(folder)
        .join("Microsoft.PowerShell_profile.ps1")
}

fn shell_command(target_shell: TargetShell, pwsh_command: Option<&str>) -> String {
    match target_shell {
        TargetShell::Powershell => "powershell.exe".to_string(),
        TargetShell::Auto | TargetShell::Pwsh => pwsh_command.unwrap_or("pwsh").to_string(),
    }
}

fn host_hint_target(host_hint: Option<&str>) -> Option<TargetShell> {
    let hint = host_hint?.to_ascii_lowercase();
    if hint.contains("pwsh") {
        Some(TargetShell::Pwsh)
    } else if hint.contains("powershell") {
        Some(TargetShell::Powershell)
    } else {
        None
    }
}

fn host_hint_from_psmodule_path(value: &str) -> Option<String> {
    let lower = value.to_ascii_lowercase();
    let has_pwsh_paths = lower.contains("\\documents\\powershell\\modules")
        || lower.contains("\\program files\\powershell\\modules")
        || lower.contains("\\program files\\powershell\\7\\modules");
    let has_windows_powershell_paths = lower.contains("windowspowershell");

    match (has_pwsh_paths, has_windows_powershell_paths) {
        (true, false) => Some("pwsh".to_string()),
        (false, true) => Some("powershell".to_string()),
        _ => None,
    }
}

fn detect_target_shell_with_pwsh_command(
    settings: &AppSettings,
    host_hint: Option<&str>,
    pwsh_command: Option<&str>,
) -> ShellStatus {
    let target_shell = match settings.target_shell {
        TargetShell::Pwsh | TargetShell::Powershell => settings.target_shell,
        TargetShell::Auto if pwsh_command.is_some() => TargetShell::Pwsh,
        TargetShell::Auto => host_hint_target(host_hint).unwrap_or(TargetShell::Powershell),
    };
    let command = shell_command(target_shell, pwsh_command);
    let profile_path = resolve_profile_path_for(target_shell, &dirs_home());
    ShellStatus {
        target_shell,
        command: command.clone(),
        version: read_shell_version(&command).ok(),
        profile_path: profile_path.to_string_lossy().to_string(),
        message: format!("当前目标 Shell: {command}"),
    }
}

fn default_pwsh_candidate_paths(home: &Path) -> Vec<PathBuf> {
    let mut paths = Vec::new();
    if let Some(program_files) = env::var_os("ProgramFiles") {
        let program_files = PathBuf::from(program_files);
        paths.push(program_files.join("PowerShell").join("7").join("pwsh.exe"));
        paths.push(
            program_files
                .join("PowerShell")
                .join("7-preview")
                .join("pwsh.exe"),
        );
    }
    if let Some(local_app_data) = env::var_os("LOCALAPPDATA") {
        paths.push(
            PathBuf::from(local_app_data)
                .join("Microsoft")
                .join("PowerShell")
                .join("7")
                .join("pwsh.exe"),
        );
    } else {
        paths.push(
            home.join("AppData")
                .join("Local")
                .join("Microsoft")
                .join("PowerShell")
                .join("7")
                .join("pwsh.exe"),
        );
    }
    paths
}

fn resolve_pwsh_command_from_candidates(
    path_command_available: bool,
    candidates: &[PathBuf],
) -> Option<String> {
    if path_command_available {
        return Some("pwsh".to_string());
    }
    candidates
        .iter()
        .find(|path| path.is_file())
        .map(|path| path.to_string_lossy().to_string())
}

fn resolve_pwsh_command() -> Option<String> {
    resolve_pwsh_command_from_candidates(
        command_exists("pwsh"),
        &default_pwsh_candidate_paths(&dirs_home()),
    )
}

fn current_host_hint() -> Option<String> {
    env::var("PSModulePath")
        .ok()
        .and_then(|value| host_hint_from_psmodule_path(&value))
}

fn command_exists(command: &str) -> bool {
    let mut process = Command::new(command);
    process.args([
        "-NoProfile",
        "-Command",
        "$PSVersionTable.PSVersion.ToString()",
    ]);
    apply_hidden_window(&mut process);
    process
        .output()
        .map(|output| output.status.success())
        .unwrap_or(false)
}

fn decode_command_output_raw(bytes: &[u8]) -> String {
    if bytes.is_empty() {
        return String::new();
    }
    if let Ok(text) = std::str::from_utf8(bytes) {
        return text.to_string();
    }
    let (decoded, _, _) = GBK.decode(bytes);
    decoded.to_string()
}

fn decode_command_output(bytes: &[u8]) -> String {
    decode_command_output_raw(bytes).trim().to_string()
}

fn decode_stream_pending(pending: &mut Vec<u8>, final_chunk: bool) -> Option<String> {
    if pending.is_empty() {
        return None;
    }

    match std::str::from_utf8(pending) {
        Ok(text) => {
            let text = text.to_string();
            pending.clear();
            Some(text)
        }
        Err(err) if err.error_len().is_none() && !final_chunk => {
            let valid_up_to = err.valid_up_to();
            if valid_up_to == 0 {
                return None;
            }
            let text = String::from_utf8_lossy(&pending[..valid_up_to]).to_string();
            let rest = pending[valid_up_to..].to_vec();
            *pending = rest;
            Some(text)
        }
        Err(_) => {
            let text = decode_command_output_raw(pending);
            pending.clear();
            Some(text)
        }
    }
}

fn read_shell_version(command: &str) -> Result<String, String> {
    let mut process = Command::new(command);
    process.args([
        "-NoProfile",
        "-Command",
        "$PSVersionTable.PSVersion.ToString()",
    ]);
    apply_hidden_window(&mut process);
    let output = process
        .output()
        .map_err(|err| format!("调用 {command} 失败: {err}"))?;
    let stdout = decode_command_output(&output.stdout);
    let stderr = decode_command_output(&output.stderr);
    if output.status.success() && !stdout.is_empty() {
        Ok(stdout)
    } else if !stderr.is_empty() {
        Err(stderr)
    } else {
        Err(format!("{command} 未返回有效版本信息"))
    }
}

fn ps_escape(value: &str) -> String {
    value.replace('`', "``").replace('"', "`\"")
}

fn ps_single_quote_escape(value: &str) -> String {
    value.replace('\'', "''")
}

fn has_powershell_function(content: &str, function_name: &str) -> bool {
    let escaped = regex::escape(function_name);
    let pattern = format!(r"(?im)^\s*function\s+{escaped}\b");
    Regex::new(&pattern)
        .expect("valid generated regex")
        .is_match(content)
}

fn build_proxy_block(proxy_url: &str) -> String {
    format!(
        "$CX_MANAGER_PROXY_URL = \"{}\"",
        ps_escape(proxy_url.trim())
    )
}

fn build_project_roots_block(project_roots: &[String]) -> String {
    let mut lines = vec!["$CX_MANAGER_PROJECT_ROOTS = @(".to_string()];
    for root in normalized_project_roots(project_roots) {
        lines.push(format!("    \"{}\"", ps_escape(&root)));
    }
    lines.push(")".to_string());
    lines.join("\n")
}

fn build_tool_paths_block() -> &'static str {
    r#"function Add-CXManagerToolPath {
    param([string]$Path)
    if ([string]::IsNullOrWhiteSpace($Path)) { return }
    if (-not (Test-Path -LiteralPath $Path)) { return }
    $normalizedPath = $Path.TrimEnd('\')
    $existingPaths = @($env:Path -split ';' | Where-Object { -not [string]::IsNullOrWhiteSpace($_) })
    foreach ($existingPath in $existingPaths) {
        if ($existingPath.TrimEnd('\') -ieq $normalizedPath) { return }
    }
    if ([string]::IsNullOrWhiteSpace($env:Path)) {
        $env:Path = $Path
    } else {
        $env:Path = "$Path;$env:Path"
    }
}

$cxNpmPrefix = $null
try {
    $cxNpmCommand = Get-Command npm -CommandType Application -ErrorAction Stop
    $cxNpmPrefix = & $cxNpmCommand.Source prefix -g
    if ($LASTEXITCODE -ne 0) { $cxNpmPrefix = $null }
} catch {
    $cxNpmPrefix = $null
}

if ($cxNpmPrefix) {
    $cxNpmPrefix = @($cxNpmPrefix | Select-Object -First 1)[0].ToString().Trim()
    Add-CXManagerToolPath $cxNpmPrefix
    Add-CXManagerToolPath (Join-Path $cxNpmPrefix "bin")
}
if ($env:ProgramFiles) {
    Add-CXManagerToolPath (Join-Path $env:ProgramFiles "nodejs")
}
if (${env:ProgramFiles(x86)}) {
    Add-CXManagerToolPath (Join-Path ${env:ProgramFiles(x86)} "nodejs")
}
if ($env:APPDATA) {
    Add-CXManagerToolPath (Join-Path $env:APPDATA "npm")
}
if ($env:LOCALAPPDATA) {
    Add-CXManagerToolPath (Join-Path $env:LOCALAPPDATA "Programs\nodejs")
}"#
}

fn default_function_source(function_name: &str) -> Option<&'static str> {
    match function_name {
        "proxy" => Some(
            r#"function proxy {
    if (-not $CX_MANAGER_PROXY_URL) {
        Write-Host "CX-Manager 代理地址为空。" -ForegroundColor Yellow
        return
    }
    $env:HTTP_PROXY = $CX_MANAGER_PROXY_URL
    $env:HTTPS_PROXY = $CX_MANAGER_PROXY_URL
    $env:ALL_PROXY = $CX_MANAGER_PROXY_URL
    $env:http_proxy = $CX_MANAGER_PROXY_URL
    $env:https_proxy = $CX_MANAGER_PROXY_URL
    $env:all_proxy = $CX_MANAGER_PROXY_URL
    Write-Host "已启用代理: $CX_MANAGER_PROXY_URL" -ForegroundColor Green
}"#,
        ),
        "unproxy" => Some(
            r#"function unproxy {
    $env:HTTP_PROXY = ""
    $env:HTTPS_PROXY = ""
    $env:ALL_PROXY = ""
    $env:http_proxy = ""
    $env:https_proxy = ""
    $env:all_proxy = ""
    Write-Host "已清空当前 PowerShell 进程代理环境变量。" -ForegroundColor Green
}"#,
        ),
        "Show-CXMenu" => Some(
            r#"function Show-CXMenu {
    param([string[]]$Items, [string]$Title)
    if (-not $Items -or $Items.Count -eq 0) { return -1 }
    $selectedIndex = 0
    [Console]::CursorVisible = $false
    try {
        while ($true) {
            Clear-Host
            Write-Host $Title -ForegroundColor Cyan
            Write-Host ""
            for ($i = 0; $i -lt $Items.Count; $i++) {
                if ($i -eq $selectedIndex) {
                    Write-Host "> " -NoNewline -ForegroundColor Green
                    Write-Host $Items[$i] -ForegroundColor Green
                } else {
                    Write-Host "  " -NoNewline
                    Write-Host $Items[$i]
                }
            }
            $key = [Console]::ReadKey($true)
            switch ($key.Key) {
                "UpArrow" { if ($selectedIndex -gt 0) { $selectedIndex-- } }
                "DownArrow" { if ($selectedIndex -lt ($Items.Count - 1)) { $selectedIndex++ } }
                "Enter" { Clear-Host; return $selectedIndex }
                "Escape" { Clear-Host; return -1 }
            }
        }
    } finally {
        [Console]::CursorVisible = $true
    }
}"#,
        ),
        "Get-CXManagerProjectFolders" => Some(
            r#"function Get-CXManagerProjectFolders {
    $roots = @($CX_MANAGER_PROJECT_ROOTS | Where-Object { $_ -and (Test-Path -LiteralPath $_) })
    foreach ($root in $roots) {
        Get-ChildItem -LiteralPath $root -Directory | Sort-Object Name | ForEach-Object {
            [PSCustomObject]@{
                Name = $_.Name
                FullName = $_.FullName
                DisplayName = "$($_.Name)  [$root]"
            }
        }
    }
}"#,
        ),
        "Resolve-CXManagerCodexCommand" => Some(
            r#"function Resolve-CXManagerCodexCommand {
    foreach ($command in @("codex.cmd", "codex.exe", "codex.bat", "codex.com", "codex")) {
        try {
            $resolved = Get-Command $command -CommandType Application -ErrorAction Stop
            if ($resolved.Source) { return $resolved.Source }
        } catch {
        }
    }
    return "codex"
}"#,
        ),
        "Invoke-CXManagerCodex" => Some(
            r#"function Invoke-CXManagerCodex {
    $command = Resolve-CXManagerCodexCommand
    & $command @args
}"#,
        ),
        "cx" => Some(
            r#"function cx {
    $folders = @(Get-CXManagerProjectFolders)
    if ($folders.Count -eq 0) {
        Write-Host "没有找到项目目录，请先在 CX-Manager 中添加项目根目录。" -ForegroundColor Yellow
        return
    }
    $selectedIndex = Show-CXMenu -Items @($folders | ForEach-Object DisplayName) -Title "选择项目目录"
    if ($selectedIndex -eq -1) {
        Write-Host "`n已取消选择" -ForegroundColor Yellow
        return
    }

    $selectedFolder = $folders[$selectedIndex]
    Set-Location $selectedFolder.FullName
    Write-Host "`n已切换到: $($selectedFolder.FullName)" -ForegroundColor Green

    $modeIndex = Show-CXMenu -Items @("codex 自动模式", "codex 正常模式") -Title "选择启动模式"
    if ($modeIndex -eq -1) {
        Write-Host "`n已取消启动" -ForegroundColor Yellow
        return
    }

    switch ($modeIndex) {
        0 { Invoke-CXManagerCodex -s danger-full-access -a never @args }
        1 { Invoke-CXManagerCodex @args }
    }
}"#,
        ),
        _ => None,
    }
}

const DEFAULT_TERMINAL_FUNCTION_NAMES: &[&str] = &[
    "proxy",
    "unproxy",
    "Show-CXMenu",
    "Get-CXManagerProjectFolders",
    "Resolve-CXManagerCodexCommand",
    "Invoke-CXManagerCodex",
    "cx",
];

fn build_managed_profile_block(
    proxy_url: &str,
    project_roots: &[String],
    user_content: &str,
) -> String {
    let mut sections = vec![
        MANAGED_BLOCK_START.to_string(),
        build_proxy_block(proxy_url),
        build_project_roots_block(project_roots),
        build_tool_paths_block().to_string(),
    ];

    for function_name in DEFAULT_TERMINAL_FUNCTION_NAMES {
        if !has_powershell_function(user_content, function_name) {
            if let Some(source) = default_function_source(function_name) {
                sections.push(source.to_string());
            }
        }
    }

    sections.push(MANAGED_BLOCK_END.to_string());
    sections.join("\n\n")
}

fn strip_marked_managed_blocks(content: &str) -> String {
    let pattern = format!(
        r"(?s)(?:\r?\n)?{}\r?\n.*?\r?\n{}(?:\r?\n)?",
        regex::escape(MANAGED_BLOCK_START),
        regex::escape(MANAGED_BLOCK_END)
    );
    Regex::new(&pattern)
        .expect("valid regex")
        .replace_all(content, "\n")
        .to_string()
}

fn strip_legacy_managed_values(content: &str) -> String {
    let proxy_re =
        Regex::new(r#"(?m)^\s*\$CX_MANAGER_PROXY_URL\s*=\s*(?:"(?:`.|[^"])*"|[^\r\n]*)(?:\r?\n)?"#)
            .expect("valid regex");
    let without_proxy = proxy_re.replace_all(content, "").to_string();
    let roots_re =
        Regex::new(r"(?s)(?:\r?\n)?\$CX_MANAGER_PROJECT_ROOTS\s*=\s*@\((?:.*?)\)(?:\r?\n)?")
            .expect("valid regex");
    roots_re.replace_all(&without_proxy, "\n").to_string()
}

fn strip_legacy_helper_block(content: &str) -> String {
    if let Some(index) = content.find(LEGACY_HELPERS_HEADER) {
        content[..index].trim_end().to_string()
    } else {
        content.to_string()
    }
}

fn strip_existing_cxmanager_blocks(content: &str) -> String {
    let without_marked = strip_marked_managed_blocks(content);
    let without_legacy_values = strip_legacy_managed_values(&without_marked);
    strip_legacy_helper_block(&without_legacy_values)
}

fn sanitize_profile_content(content: &str) -> String {
    content.chars().filter(|ch| *ch != '\0').collect()
}

fn build_profile_content(content: &str, proxy_url: &str, project_roots: &[String]) -> String {
    let sanitized = sanitize_profile_content(content);
    let user_content = strip_existing_cxmanager_blocks(&sanitized);
    let managed_block = build_managed_profile_block(proxy_url, project_roots, &user_content);
    let trimmed_user_content = user_content.trim_start_matches(['\r', '\n']);
    if trimmed_user_content.trim().is_empty() {
        format!("{managed_block}\n")
    } else {
        format!("{managed_block}\n\n{trimmed_user_content}")
    }
}

fn read_profile(path: &Path) -> Result<String, String> {
    if path.exists() {
        fs::read_to_string(path)
            .map(|content| content.trim_start_matches('\u{feff}').to_string())
            .map_err(|err| format!("读取 Profile 失败: {err}"))
    } else {
        Ok(String::new())
    }
}

fn profile_file_bytes(content: &str) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(3 + content.len());
    bytes.extend_from_slice(&[0xEF, 0xBB, 0xBF]);
    bytes.extend_from_slice(content.as_bytes());
    bytes
}

fn validate_powershell_file_syntax(path: &Path) -> Result<(), String> {
    if !cfg!(target_os = "windows") {
        return Ok(());
    }

    let path = ps_single_quote_escape(&path.to_string_lossy());
    let command = format!(
        "$errors = $null; $null = [System.Management.Automation.Language.Parser]::ParseFile('{path}', [ref]$null, [ref]$errors); if ($errors) {{ $errors | ForEach-Object {{ Write-Output $_.ToString() }} }} else {{ Write-Output 'OK' }}"
    );

    let mut process = Command::new("powershell");
    process.args(["-NoProfile", "-Command", &command]);
    apply_hidden_window(&mut process);
    let output = process
        .output()
        .map_err(|err| format!("调用 PowerShell 失败: {err}"))?;
    let stdout = decode_command_output(&output.stdout);
    let stderr = decode_command_output(&output.stderr);

    if output.status.success() && stdout == "OK" {
        Ok(())
    } else if !stderr.is_empty() {
        Err(stderr)
    } else if !stdout.is_empty() {
        Err(stdout)
    } else {
        Err("PowerShell 语法检查失败，但没有输出错误详情".to_string())
    }
}

fn validate_powershell_syntax(content: &str) -> Result<(), String> {
    if !cfg!(target_os = "windows") {
        return Ok(());
    }

    let temp_dir = tempfile::tempdir().map_err(|err| format!("创建临时目录失败: {err}"))?;
    let temp_path = temp_dir.path().join("profile.ps1");
    fs::write(&temp_path, profile_file_bytes(content))
        .map_err(|err| format!("写入临时 Profile 失败: {err}"))?;
    validate_powershell_file_syntax(&temp_path)
}

fn backup_profile_path(path: &Path) -> PathBuf {
    let file_name = path
        .file_name()
        .map(|name| name.to_string_lossy().to_string())
        .unwrap_or_else(|| "Microsoft.PowerShell_profile.ps1".to_string());
    path.with_file_name(format!("{file_name}.cxmanager.bak"))
}

fn temp_profile_path(path: &Path) -> PathBuf {
    let file_name = path
        .file_name()
        .map(|name| name.to_string_lossy().to_string())
        .unwrap_or_else(|| "Microsoft.PowerShell_profile.ps1".to_string());
    path.with_file_name(format!(".{file_name}.cxmanager.{}.tmp", std::process::id()))
}

fn write_validated_profile(path: &Path, content: &str) -> Result<(), String> {
    validate_powershell_syntax(content)?;
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|err| format!("创建 Profile 目录失败: {err}"))?;
    }
    let temp_path = temp_profile_path(path);
    if temp_path.exists() {
        fs::remove_file(&temp_path).map_err(|err| format!("清理临时 Profile 失败: {err}"))?;
    }
    fs::write(&temp_path, profile_file_bytes(content))
        .map_err(|err| format!("写入临时 Profile 失败: {err}"))?;
    validate_powershell_file_syntax(&temp_path)?;

    let backup_path = backup_profile_path(path);
    let had_existing_profile = path.exists();
    if had_existing_profile {
        fs::copy(path, &backup_path).map_err(|err| format!("备份 Profile 失败: {err}"))?;
        fs::remove_file(path).map_err(|err| format!("准备替换 Profile 失败: {err}"))?;
    }

    fs::rename(&temp_path, path).map_err(|err| {
        if had_existing_profile && !path.exists() && backup_path.exists() {
            let _ = fs::copy(&backup_path, path);
        }
        format!("替换 Profile 失败: {err}")
    })
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ProfileStatus {
    pub profile_path: String,
    pub exists: bool,
    pub has_proxy_block: bool,
    pub has_project_roots_block: bool,
    pub missing_helpers: Vec<String>,
    pub is_complete: bool,
    pub message: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct CodexStatus {
    pub executable_paths: Vec<String>,
    pub local_version: Option<String>,
    pub latest_version: Option<String>,
    pub update_available: bool,
    pub warning: Option<String>,
    pub message: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ToolStatus {
    pub installed: bool,
    pub version: Option<String>,
    pub executable_paths: Vec<String>,
    pub warning: Option<String>,
    pub message: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ToolchainStatus {
    pub node: ToolStatus,
    pub npm: ToolStatus,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct RuntimeStatus {
    pub toolchain_status: ToolchainStatus,
    pub codex_status: CodexStatus,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct SaveResult {
    pub message: String,
    pub shell_status: ShellStatus,
    pub profile_status: ProfileStatus,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct RepairResult {
    pub message: String,
    pub changed: bool,
    pub profile_status: ProfileStatus,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct UpdateResult {
    pub message: String,
    pub stdout: String,
    pub stderr: String,
    pub toolchain_status: ToolchainStatus,
    pub codex_status: CodexStatus,
}

fn has_proxy_block(content: &str) -> bool {
    Regex::new(r#"(?m)^\s*\$CX_MANAGER_PROXY_URL\s*="#)
        .expect("valid regex")
        .is_match(content)
}

fn has_project_roots_block(content: &str) -> bool {
    Regex::new(r"(?s)\$CX_MANAGER_PROJECT_ROOTS\s*=\s*@\((?:.*?)\)")
        .expect("valid regex")
        .is_match(content)
}

fn profile_status_for(path: &Path, content: &str) -> ProfileStatus {
    let missing_helpers = DEFAULT_TERMINAL_FUNCTION_NAMES
        .iter()
        .filter(|function_name| !has_powershell_function(content, function_name))
        .map(|function_name| (*function_name).to_string())
        .collect::<Vec<_>>();
    let has_proxy_block = has_proxy_block(content);
    let has_project_roots_block = has_project_roots_block(content);
    let is_complete = has_proxy_block && has_project_roots_block && missing_helpers.is_empty();
    ProfileStatus {
        profile_path: path.to_string_lossy().to_string(),
        exists: path.exists(),
        has_proxy_block,
        has_project_roots_block,
        missing_helpers,
        is_complete,
        message: if is_complete {
            "Profile 已包含 CX-Manager 必要配置".to_string()
        } else {
            "Profile 缺少 CX-Manager 必要配置".to_string()
        },
    }
}

fn write_profile(path: &Path, content: &str, settings: &AppSettings) -> Result<String, String> {
    let final_content =
        build_profile_content(content, &settings.proxy_url, &settings.project_roots);
    write_validated_profile(path, &final_content)?;
    Ok(final_content)
}

fn initialize_profile_if_needed(
    path: &Path,
    content: &str,
    settings: &AppSettings,
) -> Result<(bool, String), String> {
    let final_content =
        build_profile_content(content, &settings.proxy_url, &settings.project_roots);
    if final_content == content {
        return Ok((false, content.to_string()));
    }
    write_validated_profile(path, &final_content)?;
    Ok((true, final_content))
}

fn parse_codex_version(output: &str) -> Option<String> {
    let re = Regex::new(r"(\d+\.\d+\.\d+(?:[-+][0-9A-Za-z.\-]+)?)").expect("valid regex");
    re.captures(output)
        .and_then(|capture| capture.get(1))
        .map(|value| value.as_str().to_string())
}

fn semver_parts(version: &str) -> Vec<u64> {
    version
        .split(|ch: char| !ch.is_ascii_digit())
        .filter(|part| !part.is_empty())
        .take(3)
        .map(|part| part.parse::<u64>().unwrap_or(0))
        .chain(std::iter::repeat(0))
        .take(3)
        .collect()
}

fn compare_semver(left: &str, right: &str) -> Ordering {
    semver_parts(left).cmp(&semver_parts(right))
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct CommandInvocation {
    program: String,
    args: Vec<String>,
    env: Vec<(String, String)>,
    hide_window: bool,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct ToolProgressEvent {
    action: String,
    stream: String,
    message: String,
    done: bool,
    success: Option<bool>,
}

fn should_hide_command_windows() -> bool {
    cfg!(target_os = "windows")
}

fn apply_hidden_window(process: &mut Command) {
    if should_hide_command_windows() {
        apply_windows_hidden_window(process);
    }
}

#[cfg(target_os = "windows")]
fn apply_windows_hidden_window(process: &mut Command) {
    use std::os::windows::process::CommandExt;
    process.creation_flags(CREATE_NO_WINDOW);
}

#[cfg(not(target_os = "windows"))]
fn apply_windows_hidden_window(_process: &mut Command) {}

fn build_invocation_command(invocation: &CommandInvocation) -> Command {
    let mut process = Command::new(&invocation.program);
    process.args(&invocation.args);
    for (key, value) in &invocation.env {
        process.env(key, value);
    }
    if invocation.hide_window {
        apply_hidden_window(&mut process);
    }
    process
}

fn command_invocation_with_proxy(
    command: &str,
    args: &[&str],
    proxy_url: Option<&str>,
) -> CommandInvocation {
    let proxy_env = proxy_url
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(|value| {
            [
                "HTTP_PROXY",
                "HTTPS_PROXY",
                "ALL_PROXY",
                "http_proxy",
                "https_proxy",
                "all_proxy",
            ]
            .into_iter()
            .map(|key| (key.to_string(), value.to_string()))
            .collect::<Vec<_>>()
        })
        .unwrap_or_default();

    if cfg!(target_os = "windows") && !command.to_ascii_lowercase().ends_with(".exe") {
        let mut shell_args = vec!["/C".to_string(), command.to_string()];
        shell_args.extend(args.iter().map(|arg| (*arg).to_string()));
        CommandInvocation {
            program: "cmd.exe".to_string(),
            args: shell_args,
            env: proxy_env,
            hide_window: should_hide_command_windows(),
        }
    } else {
        CommandInvocation {
            program: command.to_string(),
            args: args.iter().map(|arg| (*arg).to_string()).collect(),
            env: proxy_env,
            hide_window: should_hide_command_windows(),
        }
    }
}

fn tool_proxy_url(settings: &AppSettings) -> Option<&str> {
    if settings.use_proxy_for_tools {
        let proxy_url = settings.proxy_url.trim();
        if proxy_url.is_empty() {
            None
        } else {
            Some(proxy_url)
        }
    } else {
        None
    }
}

fn install_codex_invocation(proxy_url: Option<&str>) -> CommandInvocation {
    command_invocation_with_proxy("npm", &["install", "-g", "@openai/codex"], proxy_url)
}

fn install_nodejs_invocation(proxy_url: Option<&str>) -> CommandInvocation {
    command_invocation_with_proxy(
        "winget",
        &[
            "install",
            "--id",
            "OpenJS.NodeJS.LTS",
            "-e",
            "--accept-package-agreements",
            "--accept-source-agreements",
        ],
        proxy_url,
    )
}

fn run_invocation_output(
    invocation: &CommandInvocation,
    command_label: &str,
) -> Result<(String, String), String> {
    let output = build_invocation_command(invocation)
        .output()
        .map_err(|err| format!("调用 {command_label} 失败: {err}"))?;
    let stdout = decode_command_output(&output.stdout);
    let stderr = decode_command_output(&output.stderr);
    if output.status.success() {
        Ok((stdout, stderr))
    } else if !stderr.is_empty() {
        Err(stderr)
    } else if !stdout.is_empty() {
        Err(stdout)
    } else {
        Err(format!("{command_label} 执行失败，但没有输出错误详情"))
    }
}

fn emit_tool_progress(
    app: Option<&tauri::AppHandle>,
    action: &str,
    stream: &str,
    message: impl Into<String>,
    done: bool,
    success: Option<bool>,
) {
    if let Some(app) = app {
        let _ = app.emit(
            TOOL_PROGRESS_EVENT,
            ToolProgressEvent {
                action: action.to_string(),
                stream: stream.to_string(),
                message: message.into(),
                done,
                success,
            },
        );
    }
}

fn read_process_stream<R: Read + Send + 'static>(
    mut reader: R,
    stream: &'static str,
    sender: Sender<(String, String)>,
) -> thread::JoinHandle<()> {
    thread::spawn(move || {
        let mut buffer = [0_u8; 4096];
        let mut pending = Vec::new();
        loop {
            match reader.read(&mut buffer) {
                Ok(0) => break,
                Ok(read) => {
                    pending.extend_from_slice(&buffer[..read]);
                    if let Some(text) = decode_stream_pending(&mut pending, false) {
                        let _ = sender.send((stream.to_string(), text));
                    }
                }
                Err(err) => {
                    let _ = sender.send((
                        "stderr".to_string(),
                        format!("读取 {stream} 输出失败: {err}"),
                    ));
                    break;
                }
            }
        }
        if let Some(text) = decode_stream_pending(&mut pending, true) {
            let _ = sender.send((stream.to_string(), text));
        }
    })
}

fn collect_stream_message(
    app: Option<&tauri::AppHandle>,
    action: &str,
    stream: &str,
    text: &str,
    stdout: &mut String,
    stderr: &mut String,
) {
    if stream == "stderr" {
        stderr.push_str(text);
    } else {
        stdout.push_str(text);
    }
    emit_tool_progress(app, action, stream, text, false, None);
}

fn run_invocation_output_streaming(
    app: Option<&tauri::AppHandle>,
    action: &str,
    invocation: &CommandInvocation,
    command_label: &str,
) -> Result<(String, String), String> {
    emit_tool_progress(
        app,
        action,
        "system",
        format!("开始执行: {command_label}"),
        false,
        None,
    );

    let mut process = build_invocation_command(invocation);
    process.stdout(Stdio::piped()).stderr(Stdio::piped());
    let mut child = process
        .spawn()
        .map_err(|err| format!("调用 {command_label} 失败: {err}"))?;

    let (sender, receiver) = mpsc::channel::<(String, String)>();
    let mut handles = Vec::new();
    if let Some(stdout) = child.stdout.take() {
        handles.push(read_process_stream(stdout, "stdout", sender.clone()));
    }
    if let Some(stderr) = child.stderr.take() {
        handles.push(read_process_stream(stderr, "stderr", sender.clone()));
    }
    drop(sender);

    let mut stdout = String::new();
    let mut stderr = String::new();
    let status = loop {
        while let Ok((stream, text)) = receiver.try_recv() {
            collect_stream_message(app, action, &stream, &text, &mut stdout, &mut stderr);
        }

        if let Some(status) = child
            .try_wait()
            .map_err(|err| format!("等待 {command_label} 失败: {err}"))?
        {
            break status;
        }

        thread::sleep(Duration::from_millis(80));
    };

    for handle in handles {
        let _ = handle.join();
    }

    for (stream, text) in receiver.try_iter() {
        collect_stream_message(app, action, &stream, &text, &mut stdout, &mut stderr);
    }

    let stdout = stdout.trim().to_string();
    let stderr = stderr.trim().to_string();
    if status.success() {
        emit_tool_progress(app, action, "system", "命令已完成", true, Some(true));
        Ok((stdout, stderr))
    } else {
        let message = if !stderr.is_empty() {
            stderr.clone()
        } else if !stdout.is_empty() {
            stdout.clone()
        } else {
            format!("{command_label} 执行失败，但没有输出错误详情")
        };
        emit_tool_progress(app, action, "system", &message, true, Some(false));
        Err(message)
    }
}

fn run_command_stdout(command: &str, args: &[&str]) -> Result<String, String> {
    run_command_stdout_with_proxy(command, args, None)
}

fn run_command_stdout_with_proxy(
    command: &str,
    args: &[&str],
    proxy_url: Option<&str>,
) -> Result<String, String> {
    let invocation = command_invocation_with_proxy(command, args, proxy_url);
    let (stdout, _) = run_invocation_output(&invocation, command)?;
    Ok(stdout)
}

fn push_unique_path(paths: &mut Vec<String>, path: impl Into<String>) {
    let path = path.into();
    if path.trim().is_empty() {
        return;
    }
    if !paths
        .iter()
        .any(|existing| existing.eq_ignore_ascii_case(&path))
    {
        paths.push(path);
    }
}

fn npm_global_prefix() -> Option<PathBuf> {
    run_command_stdout("npm", &["prefix", "-g"])
        .ok()
        .map(|prefix| prefix.trim().to_string())
        .filter(|prefix| !prefix.is_empty())
        .map(PathBuf::from)
}

fn npm_global_command_candidates(command: &str, prefix: &Path) -> Vec<PathBuf> {
    if cfg!(target_os = "windows") {
        vec![
            prefix.join(format!("{command}.cmd")),
            prefix.join(format!("{command}.exe")),
            prefix.join(format!("{command}.bat")),
            prefix.join(format!("{command}.com")),
            prefix.join(command),
            prefix.join(format!("{command}.ps1")),
        ]
    } else {
        vec![prefix.join("bin").join(command), prefix.join(command)]
    }
}

fn npm_global_executable_paths(command: &str) -> Vec<String> {
    npm_global_prefix()
        .map(|prefix| {
            npm_global_command_candidates(command, &prefix)
                .into_iter()
                .filter(|path| path.is_file())
                .map(|path| path.to_string_lossy().to_string())
                .collect()
        })
        .unwrap_or_default()
}

fn executable_paths(command: &str) -> Vec<String> {
    let mut paths = Vec::new();
    if cfg!(target_os = "windows") {
        if let Ok(stdout) = run_command_stdout("where.exe", &[command]) {
            for line in stdout
                .lines()
                .map(str::trim)
                .filter(|line| !line.is_empty())
            {
                push_unique_path(&mut paths, line);
            }
        }
    } else {
        if let Ok(stdout) = run_command_stdout("which", &[command]) {
            for line in stdout
                .lines()
                .map(str::trim)
                .filter(|line| !line.is_empty())
            {
                push_unique_path(&mut paths, line);
            }
        }
    }
    for path in npm_global_executable_paths(command) {
        push_unique_path(&mut paths, path);
    }
    paths
}

fn preferred_executable_path<'a>(command: &'a str, executable_paths: &'a [String]) -> &'a str {
    if executable_paths.is_empty() {
        return command;
    }
    if cfg!(target_os = "windows") {
        for extension in [".cmd", ".exe", ".bat", ".com"] {
            if let Some(path) = executable_paths
                .iter()
                .find(|path| path.to_ascii_lowercase().ends_with(extension))
            {
                return path;
            }
        }
    }
    executable_paths[0].as_str()
}

fn tool_status(name: &str, command: &str, version_args: &[&str]) -> ToolStatus {
    let executable_paths = executable_paths(command);
    if executable_paths.is_empty() {
        return ToolStatus {
            installed: false,
            version: None,
            executable_paths,
            warning: Some(format!("未找到 {command} 可执行文件")),
            message: format!("未安装 {name}"),
        };
    }

    let version_command = preferred_executable_path(command, &executable_paths);
    match run_command_stdout(version_command, version_args) {
        Ok(version_output) => ToolStatus {
            installed: true,
            version: Some(version_output.trim().to_string()),
            executable_paths,
            warning: None,
            message: format!("已检测到 {name}"),
        },
        Err(err) => ToolStatus {
            installed: false,
            version: None,
            executable_paths,
            warning: Some(err),
            message: format!("{name} 检测失败"),
        },
    }
}

fn check_toolchain_status() -> ToolchainStatus {
    ToolchainStatus {
        node: tool_status("Node.js", "node", &["--version"]),
        npm: tool_status("npm", "npm", &["--version"]),
    }
}

fn local_codex_result() -> Result<(String, Vec<String>), String> {
    let paths = executable_paths("codex");
    if paths.is_empty() {
        return Err("未找到 codex 可执行文件，请先安装 Codex CLI".to_string());
    }
    let version_output =
        run_command_stdout(preferred_executable_path("codex", &paths), &["--version"])?;
    Ok((version_output, paths))
}

fn latest_codex_result(proxy_url: Option<&str>) -> Result<String, String> {
    run_command_stdout_with_proxy("npm", &["view", "@openai/codex", "version"], proxy_url)
}

fn codex_status_from_results(
    local_result: Result<(String, Vec<String>), String>,
    latest_result: Result<String, String>,
) -> CodexStatus {
    let mut warnings = Vec::new();
    let (local_version, executable_paths) = match local_result {
        Ok((version_output, paths)) => (parse_codex_version(&version_output), paths),
        Err(err) => {
            warnings.push(format!("本地 Codex 检测失败: {err}"));
            (None, Vec::new())
        }
    };
    let latest_version = match latest_result {
        Ok(output) => parse_codex_version(&output).or_else(|| {
            warnings.push(format!("无法解析 npm 最新版本输出: {output}"));
            None
        }),
        Err(err) => {
            warnings.push(format!("Codex 最新版本检测失败: {err}"));
            None
        }
    };
    let update_available = match (&latest_version, &local_version) {
        (Some(latest), Some(local)) => compare_semver(latest, local) == Ordering::Greater,
        _ => false,
    };
    let message = match (&local_version, &latest_version, update_available) {
        (Some(local), Some(latest), true) => format!("Codex 可更新: {local} -> {latest}"),
        (Some(local), Some(_), false) => format!("Codex 已是最新版本: {local}"),
        (Some(local), None, _) => format!("已检测到本地 Codex {local}"),
        (None, _, _) => "未检测到可用 Codex CLI".to_string(),
    };
    CodexStatus {
        executable_paths,
        local_version,
        latest_version,
        update_available,
        warning: if warnings.is_empty() {
            None
        } else {
            Some(warnings.join(" | "))
        },
        message,
    }
}

fn check_codex_status(proxy_url: Option<&str>, npm_installed: bool) -> CodexStatus {
    let latest_result = if npm_installed {
        latest_codex_result(proxy_url)
    } else {
        Err("npm 未安装，无法检测 Codex 最新版本".to_string())
    };
    codex_status_from_results(local_codex_result(), latest_result)
}

fn check_runtime_status(settings: &AppSettings) -> RuntimeStatus {
    let toolchain_status = check_toolchain_status();
    let codex_status = check_codex_status(tool_proxy_url(settings), toolchain_status.npm.installed);
    RuntimeStatus {
        toolchain_status,
        codex_status,
    }
}

fn selected_profile_path(shell_status: &ShellStatus) -> PathBuf {
    PathBuf::from(&shell_status.profile_path)
}

fn resolve_shell_status(settings: &AppSettings) -> ShellStatus {
    let pwsh_command = resolve_pwsh_command();
    detect_target_shell_with_pwsh_command(
        settings,
        current_host_hint().as_deref(),
        pwsh_command.as_deref(),
    )
}

#[tauri::command]
fn load_app_state() -> Result<AppState, String> {
    let mut settings = load_settings_from_disk()?;
    settings.project_roots = normalized_project_roots(&settings.project_roots);
    let shell_status = resolve_shell_status(&settings);
    let profile_path = selected_profile_path(&shell_status);
    let profile = read_profile(&profile_path)?;
    let (_, initialized_profile) =
        initialize_profile_if_needed(&profile_path, &profile, &settings)?;
    save_settings_to_disk(&settings)?;
    let profile_status = profile_status_for(&profile_path, &initialized_profile);
    let runtime_status = check_runtime_status(&settings);
    Ok(AppState {
        settings,
        shell_status,
        profile_status,
        toolchain_status: runtime_status.toolchain_status,
        codex_status: runtime_status.codex_status,
    })
}

#[tauri::command]
fn save_app_state(mut settings: AppSettings) -> Result<SaveResult, String> {
    if settings.proxy_url.trim().is_empty() {
        settings.proxy_url = DEFAULT_PROXY_URL.to_string();
    } else {
        settings.proxy_url = settings.proxy_url.trim().to_string();
    }
    settings.project_roots = normalized_project_roots(&settings.project_roots);
    let shell_status = resolve_shell_status(&settings);
    let profile_path = selected_profile_path(&shell_status);
    let profile = read_profile(&profile_path)?;
    let final_profile = write_profile(&profile_path, &profile, &settings)?;
    save_settings_to_disk(&settings)?;
    let profile_status = profile_status_for(&profile_path, &final_profile);
    Ok(SaveResult {
        message: format!("已保存到 {}", profile_path.to_string_lossy()),
        shell_status,
        profile_status,
    })
}

#[tauri::command]
fn repair_profile(settings: AppSettings) -> Result<RepairResult, String> {
    let shell_status = resolve_shell_status(&settings);
    let profile_path = selected_profile_path(&shell_status);
    let profile = read_profile(&profile_path)?;
    let (changed, final_profile) =
        initialize_profile_if_needed(&profile_path, &profile, &settings)?;
    let profile_status = profile_status_for(&profile_path, &final_profile);
    Ok(RepairResult {
        message: if changed {
            format!("已修复 {}", profile_path.to_string_lossy())
        } else {
            "Profile 已完整，无需修复".to_string()
        },
        changed,
        profile_status,
    })
}

#[tauri::command]
fn refresh_runtime_status(settings: AppSettings) -> Result<RuntimeStatus, String> {
    Ok(check_runtime_status(&settings))
}

fn tool_action_result_with_runtime(
    message: &str,
    stdout: String,
    stderr: String,
    runtime_status: RuntimeStatus,
) -> Result<UpdateResult, String> {
    Ok(UpdateResult {
        message: message.to_string(),
        stdout,
        stderr,
        toolchain_status: runtime_status.toolchain_status,
        codex_status: runtime_status.codex_status,
    })
}

fn codex_update_completion_message(runtime_status: &RuntimeStatus) -> &'static str {
    if runtime_status.codex_status.local_version.is_some() {
        "Codex 更新命令已完成"
    } else {
        "Codex 更新命令已结束，但仍未检测到 Codex CLI。请查看安装日志，确认 codex 是否在 PATH 中。"
    }
}

fn codex_install_completion_message(runtime_status: &RuntimeStatus) -> &'static str {
    if runtime_status.codex_status.local_version.is_some() {
        "Codex CLI 安装命令已完成"
    } else if !runtime_status.toolchain_status.npm.installed {
        "Codex 安装命令已结束，但仍未检测到 npm。请先安装 Node.js / npm。"
    } else {
        "Codex 安装命令已结束，但仍未检测到 Codex CLI。请查看安装日志，确认 npm global bin 是否在 PATH 中；必要时重启 CX-Manager 或 PowerShell。"
    }
}

fn node_install_completion_message(runtime_status: &RuntimeStatus) -> &'static str {
    if runtime_status.toolchain_status.npm.installed {
        "Node.js LTS 安装命令已完成"
    } else {
        "Node.js LTS 安装命令已结束，但仍未检测到 npm。请查看安装日志；如果刚安装成功，请重启 CX-Manager 后刷新。"
    }
}

#[tauri::command]
fn update_codex(app: tauri::AppHandle, settings: AppSettings) -> Result<UpdateResult, String> {
    let invocation = command_invocation_with_proxy("codex", &["update"], tool_proxy_url(&settings));
    let (stdout, stderr) =
        run_invocation_output_streaming(Some(&app), "更新 Codex", &invocation, "codex update")?;
    let runtime_status = check_runtime_status(&settings);
    let message = codex_update_completion_message(&runtime_status);
    tool_action_result_with_runtime(message, stdout, stderr, runtime_status)
}

#[tauri::command]
fn install_codex(app: tauri::AppHandle, settings: AppSettings) -> Result<UpdateResult, String> {
    let invocation = install_codex_invocation(tool_proxy_url(&settings));
    let (stdout, stderr) = run_invocation_output_streaming(
        Some(&app),
        "安装 Codex CLI",
        &invocation,
        "npm install -g @openai/codex",
    )?;
    let runtime_status = check_runtime_status(&settings);
    let message = codex_install_completion_message(&runtime_status);
    tool_action_result_with_runtime(message, stdout, stderr, runtime_status)
}

#[tauri::command]
fn install_nodejs(app: tauri::AppHandle, settings: AppSettings) -> Result<UpdateResult, String> {
    let invocation = install_nodejs_invocation(tool_proxy_url(&settings));
    let (stdout, stderr) = run_invocation_output_streaming(
        Some(&app),
        "安装 Node.js LTS / npm",
        &invocation,
        "winget install OpenJS.NodeJS.LTS",
    )?;
    let runtime_status = check_runtime_status(&settings);
    let message = node_install_completion_message(&runtime_status);
    tool_action_result_with_runtime(message, stdout, stderr, runtime_status)
}

pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .invoke_handler(tauri::generate_handler![
            load_app_state,
            save_app_state,
            repair_profile,
            refresh_runtime_status,
            update_codex,
            install_codex,
            install_nodejs
        ])
        .run(tauri::generate_context!())
        .expect("error while running CX-Manager");
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;
    use std::sync::Mutex;

    static NPM_PREFIX_LOCK: Mutex<()> = Mutex::new(());

    #[test]
    fn default_settings_use_current_proxy() {
        let settings = AppSettings::default();

        assert_eq!(settings.target_shell, TargetShell::Auto);
        assert_eq!(settings.proxy_url, "http://10.20.34.92:7890");
        assert!(settings.use_proxy_for_tools);
        assert!(settings.project_roots.is_empty());
    }

    #[test]
    fn project_roots_are_trimmed_and_deduplicated() {
        let roots = normalized_project_roots(&[
            " C:\\Users\\Example\\PycharmProjects ".to_string(),
            "C:\\Users\\Example\\PycharmProjects".to_string(),
            "".to_string(),
            "D:\\Code".to_string(),
        ]);

        assert_eq!(
            roots,
            vec![
                "C:\\Users\\Example\\PycharmProjects".to_string(),
                "D:\\Code".to_string()
            ]
        );
    }

    #[test]
    fn pwsh_profile_path_uses_powershell_documents_folder() {
        let home = Path::new("C:\\Users\\Example");
        let path = resolve_profile_path_for(TargetShell::Pwsh, home);

        assert_eq!(
            path.to_string_lossy(),
            "C:\\Users\\Example\\Documents\\PowerShell\\Microsoft.PowerShell_profile.ps1"
        );
    }

    #[test]
    fn windows_powershell_profile_path_uses_legacy_documents_folder() {
        let home = Path::new("C:\\Users\\Example");
        let path = resolve_profile_path_for(TargetShell::Powershell, home);

        assert_eq!(
            path.to_string_lossy(),
            "C:\\Users\\Example\\Documents\\WindowsPowerShell\\Microsoft.PowerShell_profile.ps1"
        );
    }

    #[test]
    fn explicit_target_shell_wins_over_auto_detection() {
        let settings = AppSettings {
            target_shell: TargetShell::Powershell,
            proxy_url: "http://10.20.34.92:7890".to_string(),
            use_proxy_for_tools: true,
            project_roots: Vec::new(),
        };

        let detected = detect_target_shell_with_pwsh_command(&settings, Some("pwsh"), Some("pwsh"));

        assert_eq!(detected.target_shell, TargetShell::Powershell);
        assert_eq!(detected.command, "powershell.exe");
    }

    #[test]
    fn mixed_psmodulepath_does_not_force_windows_powershell() {
        let value = [
            r"C:\Users\Example\Documents\PowerShell\Modules",
            r"C:\Program Files\PowerShell\Modules",
            r"C:\Program Files\WindowsPowerShell\Modules",
            r"C:\Windows\system32\WindowsPowerShell\v1.0\Modules",
        ]
        .join(";");

        assert_eq!(host_hint_from_psmodule_path(&value), None);

        let settings = AppSettings::default();
        let detected = detect_target_shell_with_pwsh_command(
            &settings,
            host_hint_from_psmodule_path(&value).as_deref(),
            Some("pwsh"),
        );

        assert_eq!(detected.target_shell, TargetShell::Pwsh);
        assert_eq!(detected.command, "pwsh");
    }

    #[test]
    fn auto_prefers_available_pwsh_over_windows_powershell_hint() {
        let settings = AppSettings::default();
        let detected =
            detect_target_shell_with_pwsh_command(&settings, Some("powershell"), Some("pwsh"));

        assert_eq!(detected.target_shell, TargetShell::Pwsh);
    }

    #[test]
    fn pwsh_candidate_path_is_used_when_path_lookup_fails() {
        let temp = tempfile::tempdir().expect("tempdir");
        let missing = temp.path().join("missing").join("pwsh.exe");
        let pwsh = temp.path().join("PowerShell").join("7").join("pwsh.exe");
        fs::create_dir_all(pwsh.parent().expect("parent")).expect("create candidate directory");
        fs::write(&pwsh, "").expect("create candidate file");

        let resolved = resolve_pwsh_command_from_candidates(false, &[missing, pwsh.clone()]);

        assert_eq!(resolved.as_deref(), Some(pwsh.to_string_lossy().as_ref()));
    }

    #[test]
    fn legacy_only_psmodulepath_can_hint_windows_powershell() {
        let value = [
            r"C:\Users\Example\Documents\WindowsPowerShell\Modules",
            r"C:\Windows\system32\WindowsPowerShell\v1.0\Modules",
        ]
        .join(";");

        assert_eq!(
            host_hint_from_psmodule_path(&value).as_deref(),
            Some("powershell")
        );
    }

    #[test]
    fn generated_profile_contains_proxy_and_project_roots_blocks() {
        let content = build_profile_content(
            "",
            "http://10.20.34.92:7890",
            &["C:\\Users\\Example\\PycharmProjects".to_string()],
        );

        assert!(content.contains("$CX_MANAGER_PROXY_URL = \"http://10.20.34.92:7890\""));
        assert!(content.contains("$CX_MANAGER_PROJECT_ROOTS = @("));
        assert!(content.contains("\"C:\\Users\\Example\\PycharmProjects\""));
    }

    #[test]
    fn generated_profile_adds_tool_paths_even_when_user_cx_is_preserved() {
        let content = build_profile_content(
            "function cx { codex @args }\n",
            DEFAULT_PROXY_URL,
            &["C:\\Users\\Example\\PycharmProjects".to_string()],
        );

        assert_eq!(content.matches("function cx").count(), 1);
        assert!(content.contains("function Add-CXManagerToolPath"));
        assert!(content.contains("function Invoke-CXManagerCodex"));
        assert!(content.contains("prefix -g"));
        assert!(content.contains("ProgramFiles"));
        validate_powershell_syntax(&content).expect("profile with tool path block should parse");
    }

    #[test]
    fn injects_missing_codex_terminal_helpers() {
        let content = build_profile_content("", DEFAULT_PROXY_URL, &[]);

        for function_name in [
            "proxy",
            "unproxy",
            "Show-CXMenu",
            "Get-CXManagerProjectFolders",
            "Resolve-CXManagerCodexCommand",
            "Invoke-CXManagerCodex",
            "cx",
        ] {
            assert!(
                has_powershell_function(&content, function_name),
                "missing generated helper {function_name}"
            );
        }
        assert!(content.contains("Invoke-CXManagerCodex -s danger-full-access -a never @args"));
    }

    #[test]
    fn preserves_existing_cx_function() {
        let content = "function cx { Write-Host \"custom cx\" }\n";
        let updated = build_profile_content(content, DEFAULT_PROXY_URL, &[]);

        assert_eq!(updated.matches("function cx").count(), 1);
        assert!(updated.contains("Write-Host \"custom cx\""));
        assert!(has_powershell_function(&updated, "proxy"));
    }

    #[test]
    fn updating_managed_blocks_does_not_duplicate_them() {
        let first = build_profile_content("", "http://10.20.34.92:7890", &["C:\\Code".to_string()]);
        let second =
            build_profile_content(&first, "http://127.0.0.1:7890", &["D:\\Work".to_string()]);

        assert_eq!(second.matches("$CX_MANAGER_PROXY_URL =").count(), 1);
        assert_eq!(second.matches("$CX_MANAGER_PROJECT_ROOTS = @(").count(), 1);
        assert!(second.contains("$CX_MANAGER_PROXY_URL = \"http://127.0.0.1:7890\""));
        assert!(second.contains("\"D:\\Work\""));
        assert!(!second.contains("\"C:\\Code\""));
    }

    #[test]
    fn truncated_legacy_helper_block_is_discarded_before_rebuild() {
        let content =
            "Write-Host \"before\"\n\n# CX-Manager default terminal helpers\nfunction proxy {\n";
        let updated = build_profile_content(content, DEFAULT_PROXY_URL, &[]);

        assert!(updated.contains("Write-Host \"before\""));
        assert!(updated.contains("# >>> CX-Manager managed profile"));
        assert!(updated.contains("# <<< CX-Manager managed profile"));
        assert_eq!(updated.matches("function proxy").count(), 1);
        validate_powershell_syntax(&updated).expect("rebuilt profile should parse");
    }

    #[test]
    fn legacy_helper_block_is_replaced_instead_of_preserved() {
        let content = r#"function proxy {
    Write-Host "old proxy"
}

# CX-Manager default terminal helpers
function Show-CXMenu {
    Write-Host "old menu"
}
"#;
        let updated = build_profile_content(content, DEFAULT_PROXY_URL, &[]);

        assert!(updated.contains("Write-Host \"old proxy\""));
        assert!(!updated.contains("Write-Host \"old menu\""));
        assert!(updated.contains("# >>> CX-Manager managed profile"));
        assert_eq!(
            updated
                .matches("# CX-Manager default terminal helpers")
                .count(),
            0
        );
        validate_powershell_syntax(&updated).expect("rebuilt profile should parse");
    }

    #[test]
    fn writing_existing_profile_creates_backup_before_replace() {
        let temp = tempfile::tempdir().expect("tempdir");
        let path = temp.path().join("Microsoft.PowerShell_profile.ps1");
        let original = "Write-Host \"original\"\n";
        let replacement = "Write-Host \"replacement\"\n";
        fs::write(&path, original).expect("write original");

        write_validated_profile(&path, replacement).expect("write replacement");

        assert_eq!(read_profile(&path).expect("read profile"), replacement);
        assert_eq!(
            fs::read_to_string(backup_profile_path(&path)).expect("read backup"),
            original
        );
    }

    #[test]
    fn profile_content_drops_nul_bytes_when_rewritten() {
        let content = "Write-Host \"before\"\n\0\nWrite-Host \"after\"";
        let updated = build_profile_content(content, DEFAULT_PROXY_URL, &[]);

        assert!(!updated.contains('\0'));
        assert!(updated.contains("Write-Host \"before\""));
        assert!(updated.contains("Write-Host \"after\""));
    }

    #[test]
    fn parses_codex_cli_version_output() {
        assert_eq!(
            parse_codex_version("codex-cli 0.136.0"),
            Some("0.136.0".to_string())
        );
        assert_eq!(parse_codex_version("0.137.1"), Some("0.137.1".to_string()));
    }

    #[test]
    fn compares_semantic_versions_for_update_availability() {
        assert_eq!(
            compare_semver("0.137.0", "0.136.0"),
            std::cmp::Ordering::Greater
        );
        assert_eq!(
            compare_semver("0.136.0", "0.136.0"),
            std::cmp::Ordering::Equal
        );
        assert_eq!(
            compare_semver("0.135.9", "0.136.0"),
            std::cmp::Ordering::Less
        );
    }

    #[test]
    fn windows_npm_shim_commands_run_through_cmd() {
        let invocation = command_invocation_with_proxy("codex", &["--version"], None);

        if cfg!(target_os = "windows") {
            assert_eq!(invocation.program, "cmd.exe");
            assert_eq!(
                invocation.args,
                vec![
                    "/C".to_string(),
                    "codex".to_string(),
                    "--version".to_string()
                ]
            );
        } else {
            assert_eq!(invocation.program, "codex");
            assert_eq!(invocation.args, vec!["--version".to_string()]);
        }
    }

    #[test]
    fn windows_exe_commands_still_run_directly() {
        let invocation = command_invocation_with_proxy("where.exe", &["codex"], None);

        assert_eq!(invocation.program, "where.exe");
        assert_eq!(invocation.args, vec!["codex".to_string()]);
    }

    #[test]
    fn proxy_url_is_injected_into_update_commands() {
        let invocation = command_invocation_with_proxy(
            "npm",
            &["view", "@openai/codex", "version"],
            Some(" http://10.20.34.92:7890 "),
        );

        for key in [
            "HTTP_PROXY",
            "HTTPS_PROXY",
            "ALL_PROXY",
            "http_proxy",
            "https_proxy",
            "all_proxy",
        ] {
            assert_eq!(
                invocation
                    .env
                    .iter()
                    .find(|(name, _)| name == key)
                    .map(|(_, value)| value.as_str()),
                Some("http://10.20.34.92:7890")
            );
        }
    }

    #[test]
    fn tool_proxy_setting_can_disable_proxy_env() {
        let settings = AppSettings {
            target_shell: TargetShell::Auto,
            proxy_url: "http://10.20.34.92:7890".to_string(),
            use_proxy_for_tools: false,
            project_roots: Vec::new(),
        };

        assert_eq!(tool_proxy_url(&settings), None);
        assert!(install_codex_invocation(tool_proxy_url(&settings))
            .env
            .is_empty());
        assert!(install_nodejs_invocation(tool_proxy_url(&settings))
            .env
            .is_empty());
    }

    #[test]
    fn install_invocations_use_proxy_when_enabled() {
        let settings = AppSettings {
            target_shell: TargetShell::Auto,
            proxy_url: "http://10.20.34.92:7890".to_string(),
            use_proxy_for_tools: true,
            project_roots: Vec::new(),
        };

        let codex = install_codex_invocation(tool_proxy_url(&settings));
        assert!(codex.args.iter().any(|arg| arg == "@openai/codex"));
        assert!(codex
            .env
            .iter()
            .any(|(key, value)| key == "HTTPS_PROXY" && value == "http://10.20.34.92:7890"));

        let node = install_nodejs_invocation(tool_proxy_url(&settings));
        assert!(node.args.iter().any(|arg| arg == "OpenJS.NodeJS.LTS"));
        assert!(node
            .env
            .iter()
            .any(|(key, value)| key == "HTTPS_PROXY" && value == "http://10.20.34.92:7890"));
    }

    #[test]
    fn installer_invocations_hide_external_console_windows_on_windows() {
        let settings = AppSettings::default();

        for invocation in [
            install_codex_invocation(tool_proxy_url(&settings)),
            install_nodejs_invocation(tool_proxy_url(&settings)),
            command_invocation_with_proxy("codex", &["update"], tool_proxy_url(&settings)),
        ] {
            assert_eq!(invocation.hide_window, cfg!(target_os = "windows"));
        }
    }

    #[test]
    fn executable_paths_include_npm_global_shim_when_path_lookup_misses() {
        let _guard = NPM_PREFIX_LOCK.lock().expect("npm prefix lock");
        let temp = tempfile::tempdir().expect("tempdir");
        let prefix = temp.path().join("npm prefix with space");
        fs::create_dir_all(&prefix).expect("create npm prefix");
        let command = "cxmanager-test-codex";
        let shim = if cfg!(target_os = "windows") {
            prefix.join(format!("{command}.cmd"))
        } else {
            let bin = prefix.join("bin");
            fs::create_dir_all(&bin).expect("create npm bin dir");
            bin.join(command)
        };
        fs::write(&shim, "").expect("create fake npm global shim");
        let previous_prefix = env::var_os("NPM_CONFIG_PREFIX");
        env::set_var("NPM_CONFIG_PREFIX", &prefix);

        let paths = executable_paths(command);

        match previous_prefix {
            Some(value) => env::set_var("NPM_CONFIG_PREFIX", value),
            None => env::remove_var("NPM_CONFIG_PREFIX"),
        }
        assert!(
            paths
                .iter()
                .any(|path| path.eq_ignore_ascii_case(&shim.to_string_lossy())),
            "expected npm global shim path in {paths:?}"
        );
    }

    #[test]
    fn tool_status_uses_detected_npm_global_shim_for_version_check() {
        if !cfg!(target_os = "windows") {
            return;
        }

        let _guard = NPM_PREFIX_LOCK.lock().expect("npm prefix lock");
        let temp = tempfile::tempdir().expect("tempdir");
        let prefix = temp.path().join("npm prefix with space");
        fs::create_dir_all(&prefix).expect("create npm prefix");
        let command = "cxmanager-test-codex-version";
        let shim = prefix.join(format!("{command}.cmd"));
        fs::write(&shim, "@echo 0.137.0\r\n").expect("create fake npm global shim");
        let previous_prefix = env::var_os("NPM_CONFIG_PREFIX");
        env::set_var("NPM_CONFIG_PREFIX", &prefix);

        let status = tool_status("Codex", command, &["--version"]);

        match previous_prefix {
            Some(value) => env::set_var("NPM_CONFIG_PREFIX", value),
            None => env::remove_var("NPM_CONFIG_PREFIX"),
        }
        assert!(status.installed, "{status:?}");
        assert_eq!(status.version.as_deref(), Some("0.137.0"));
        assert!(status
            .executable_paths
            .iter()
            .any(|path| path.eq_ignore_ascii_case(&shim.to_string_lossy())));
    }

    fn tool_status_for_test(installed: bool, name: &str) -> ToolStatus {
        ToolStatus {
            installed,
            version: installed.then(|| "1.0.0".to_string()),
            executable_paths: if installed {
                vec![format!("C:\\Tools\\{name}.cmd")]
            } else {
                Vec::new()
            },
            warning: None,
            message: name.to_string(),
        }
    }

    fn runtime_status_for_test(npm_installed: bool, codex_version: Option<&str>) -> RuntimeStatus {
        RuntimeStatus {
            toolchain_status: ToolchainStatus {
                node: tool_status_for_test(npm_installed, "node"),
                npm: tool_status_for_test(npm_installed, "npm"),
            },
            codex_status: CodexStatus {
                executable_paths: codex_version
                    .map(|_| vec!["C:\\Tools\\codex.cmd".to_string()])
                    .unwrap_or_default(),
                local_version: codex_version.map(ToOwned::to_owned),
                latest_version: Some("0.137.0".to_string()),
                update_available: false,
                warning: None,
                message: "test".to_string(),
            },
        }
    }

    #[test]
    fn install_completion_messages_explain_failed_post_install_detection() {
        let npm_without_codex = runtime_status_for_test(true, None);
        assert!(
            codex_install_completion_message(&npm_without_codex).contains("仍未检测到 Codex CLI")
        );
        assert!(codex_install_completion_message(&npm_without_codex).contains("PATH"));

        let no_npm = runtime_status_for_test(false, None);
        assert!(codex_install_completion_message(&no_npm).contains("仍未检测到 npm"));
        assert!(node_install_completion_message(&no_npm).contains("仍未检测到 npm"));

        let installed = runtime_status_for_test(true, Some("0.137.0"));
        assert_eq!(
            codex_install_completion_message(&installed),
            "Codex CLI 安装命令已完成"
        );
    }

    #[test]
    fn gbk_command_output_is_decoded_for_windows_errors() {
        let bytes = [0xB2, 0xBB, 0xCA, 0xC7];

        assert_eq!(decode_command_output(&bytes), "不是");
    }

    #[test]
    fn stream_decoder_preserves_split_utf8_output() {
        let mut pending = vec![0xE4, 0xBD];

        assert_eq!(decode_stream_pending(&mut pending, false), None);
        pending.push(0xA0);

        assert_eq!(
            decode_stream_pending(&mut pending, false).as_deref(),
            Some("你")
        );
        assert!(pending.is_empty());
    }

    #[test]
    fn blank_proxy_url_does_not_inject_proxy_env() {
        let invocation =
            command_invocation_with_proxy("npm", &["view", "@openai/codex", "version"], Some(" "));

        assert!(invocation.env.is_empty());
    }

    #[test]
    fn latest_version_failure_is_warning_not_load_failure() {
        let status = codex_status_from_results(
            Ok((
                "codex-cli 0.136.0".to_string(),
                vec!["C:\\Program Files\\nodejs\\codex.cmd".to_string()],
            )),
            Err("npm view failed".to_string()),
        );

        assert_eq!(status.local_version.as_deref(), Some("0.136.0"));
        assert_eq!(status.latest_version, None);
        assert!(!status.update_available);
        assert!(status
            .warning
            .as_deref()
            .unwrap_or_default()
            .contains("npm view failed"));
    }
}
