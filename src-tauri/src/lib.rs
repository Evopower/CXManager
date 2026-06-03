use regex::Regex;
use serde::{Deserialize, Serialize};
use std::cmp::Ordering;
use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

const SETTINGS_FILE: &str = "settings.json";
const DEFAULT_PROXY_URL: &str = "http://10.20.34.92:7890";

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
    pub project_roots: Vec<String>,
}

impl Default for AppSettings {
    fn default() -> Self {
        Self {
            target_shell: TargetShell::Auto,
            proxy_url: DEFAULT_PROXY_URL.to_string(),
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
    Command::new(command)
        .args([
            "-NoProfile",
            "-Command",
            "$PSVersionTable.PSVersion.ToString()",
        ])
        .output()
        .map(|output| output.status.success())
        .unwrap_or(false)
}

fn read_shell_version(command: &str) -> Result<String, String> {
    let output = Command::new(command)
        .args([
            "-NoProfile",
            "-Command",
            "$PSVersionTable.PSVersion.ToString()",
        ])
        .output()
        .map_err(|err| format!("调用 {command} 失败: {err}"))?;
    let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
    let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
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

fn update_proxy_block(content: &str, proxy_url: &str) -> String {
    let block = build_proxy_block(proxy_url);
    let re = Regex::new(r#"(?m)^\s*\$CX_MANAGER_PROXY_URL\s*=\s*(?:"(?:`.|[^"])*"|[^\r\n]*)"#)
        .expect("valid regex");
    if re.is_match(content) {
        re.replace(content, regex::NoExpand(&block)).to_string()
    } else {
        format!("{block}\n\n{content}")
    }
}

fn build_project_roots_block(project_roots: &[String]) -> String {
    let mut lines = vec!["$CX_MANAGER_PROJECT_ROOTS = @(".to_string()];
    for root in normalized_project_roots(project_roots) {
        lines.push(format!("    \"{}\"", ps_escape(&root)));
    }
    lines.push(")".to_string());
    lines.join("\n")
}

fn update_project_roots_block(content: &str, project_roots: &[String]) -> String {
    let block = build_project_roots_block(project_roots);
    let re = Regex::new(r"(?s)\$CX_MANAGER_PROJECT_ROOTS\s*=\s*@\((?:.*?)\)").expect("valid regex");
    if re.is_match(content) {
        re.replace(content, regex::NoExpand(&block)).to_string()
    } else {
        format!("{block}\n\n{content}")
    }
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
        0 { codex -s danger-full-access -a never @args }
        1 { codex @args }
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
    "cx",
];

fn ensure_default_profile_functions(content: &str) -> String {
    let mut updated = content.to_string();
    let mut missing_sources = Vec::new();

    for function_name in DEFAULT_TERMINAL_FUNCTION_NAMES {
        if !has_powershell_function(&updated, function_name) {
            if let Some(source) = default_function_source(function_name) {
                missing_sources.push(source);
            }
        }
    }

    if !missing_sources.is_empty() {
        updated.push_str("\n\n# CX-Manager default terminal helpers\n");
        updated.push_str(&missing_sources.join("\n\n"));
        updated.push('\n');
    }

    updated
}

fn sanitize_profile_content(content: &str) -> String {
    content.chars().filter(|ch| *ch != '\0').collect()
}

fn build_profile_content(content: &str, proxy_url: &str, project_roots: &[String]) -> String {
    let sanitized = sanitize_profile_content(content);
    let with_proxy = update_proxy_block(&sanitized, proxy_url);
    let with_project_roots = update_project_roots_block(&with_proxy, project_roots);
    ensure_default_profile_functions(&with_project_roots)
}

fn read_profile(path: &Path) -> Result<String, String> {
    if path.exists() {
        fs::read_to_string(path).map_err(|err| format!("读取 Profile 失败: {err}"))
    } else {
        Ok(String::new())
    }
}

fn validate_powershell_syntax(content: &str) -> Result<(), String> {
    if !cfg!(target_os = "windows") {
        return Ok(());
    }

    let temp_dir = tempfile::tempdir().map_err(|err| format!("创建临时目录失败: {err}"))?;
    let temp_path = temp_dir.path().join("profile.ps1");
    let mut bytes = Vec::with_capacity(3 + content.len());
    bytes.extend_from_slice(&[0xEF, 0xBB, 0xBF]);
    bytes.extend_from_slice(content.as_bytes());
    fs::write(&temp_path, bytes).map_err(|err| format!("写入临时 Profile 失败: {err}"))?;

    let path = ps_single_quote_escape(&temp_path.to_string_lossy());
    let command = format!(
        "$errors = $null; $null = [System.Management.Automation.Language.Parser]::ParseFile('{path}', [ref]$null, [ref]$errors); if ($errors) {{ $errors | ForEach-Object {{ Write-Output $_.ToString() }} }} else {{ Write-Output 'OK' }}"
    );

    let output = Command::new("powershell")
        .args(["-NoProfile", "-Command", &command])
        .output()
        .map_err(|err| format!("调用 PowerShell 失败: {err}"))?;
    let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
    let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();

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

fn write_validated_profile(path: &Path, content: &str) -> Result<(), String> {
    validate_powershell_syntax(content)?;
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|err| format!("创建 Profile 目录失败: {err}"))?;
    }
    fs::write(path, content).map_err(|err| format!("写入 Profile 失败: {err}"))
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
        }
    } else {
        CommandInvocation {
            program: command.to_string(),
            args: args.iter().map(|arg| (*arg).to_string()).collect(),
            env: proxy_env,
        }
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
    let mut process = Command::new(&invocation.program);
    process.args(&invocation.args);
    for (key, value) in &invocation.env {
        process.env(key, value);
    }
    let output = process
        .output()
        .map_err(|err| format!("调用 {command} 失败: {err}"))?;
    let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
    let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
    if output.status.success() {
        Ok(stdout)
    } else if !stderr.is_empty() {
        Err(stderr)
    } else if !stdout.is_empty() {
        Err(stdout)
    } else {
        Err(format!("{command} 执行失败，但没有输出错误详情"))
    }
}

fn codex_paths() -> Vec<String> {
    if cfg!(target_os = "windows") {
        run_command_stdout("where.exe", &["codex"])
            .map(|stdout| {
                stdout
                    .lines()
                    .map(str::trim)
                    .filter(|line| !line.is_empty())
                    .map(ToOwned::to_owned)
                    .collect()
            })
            .unwrap_or_default()
    } else {
        run_command_stdout("which", &["codex"])
            .map(|stdout| vec![stdout])
            .unwrap_or_default()
    }
}

fn local_codex_result() -> Result<(String, Vec<String>), String> {
    let version_output = run_command_stdout("codex", &["--version"])?;
    Ok((version_output, codex_paths()))
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

fn check_codex_status(proxy_url: Option<&str>) -> CodexStatus {
    codex_status_from_results(local_codex_result(), latest_codex_result(proxy_url))
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
    let codex_status = check_codex_status(Some(&settings.proxy_url));
    Ok(AppState {
        settings,
        shell_status,
        profile_status,
        codex_status,
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
fn refresh_codex_status(proxy_url: Option<String>) -> Result<CodexStatus, String> {
    Ok(check_codex_status(proxy_url.as_deref()))
}

#[tauri::command]
fn update_codex(proxy_url: Option<String>) -> Result<UpdateResult, String> {
    let invocation = command_invocation_with_proxy("codex", &["update"], proxy_url.as_deref());
    let mut process = Command::new(&invocation.program);
    process.args(&invocation.args);
    for (key, value) in &invocation.env {
        process.env(key, value);
    }
    let output = process
        .output()
        .map_err(|err| format!("调用 codex update 失败: {err}"))?;
    let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
    let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
    if !output.status.success() {
        return Err(if !stderr.is_empty() {
            stderr
        } else if !stdout.is_empty() {
            stdout
        } else {
            "codex update 执行失败，但没有输出错误详情".to_string()
        });
    }
    Ok(UpdateResult {
        message: "Codex 更新命令已完成".to_string(),
        stdout,
        stderr,
        codex_status: check_codex_status(proxy_url.as_deref()),
    })
}

pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .invoke_handler(tauri::generate_handler![
            load_app_state,
            save_app_state,
            repair_profile,
            refresh_codex_status,
            update_codex
        ])
        .run(tauri::generate_context!())
        .expect("error while running CX-Manager");
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    #[test]
    fn default_settings_use_current_proxy() {
        let settings = AppSettings::default();

        assert_eq!(settings.target_shell, TargetShell::Auto);
        assert_eq!(settings.proxy_url, "http://10.20.34.92:7890");
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
    fn injects_missing_codex_terminal_helpers() {
        let content = build_profile_content("", DEFAULT_PROXY_URL, &[]);

        for function_name in [
            "proxy",
            "unproxy",
            "Show-CXMenu",
            "Get-CXManagerProjectFolders",
            "cx",
        ] {
            assert!(
                has_powershell_function(&content, function_name),
                "missing generated helper {function_name}"
            );
        }
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
