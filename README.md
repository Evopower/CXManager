# CX-Manager

CX-Manager 是一个 Windows 桌面工具，用来管理 Codex 在 PowerShell 里的常用命令和项目入口。

它不会注入第三方模型配置，也不处理登录体系。CX-Manager 默认你已经通过官方 Codex CLI 完成登录，终端函数只负责维护代理、项目目录选择和 `codex` 启动方式。

## 快速开始

1. 在 GitHub Release 下载 Windows 安装包：

```text
https://github.com/Evopower/CXManager/releases/latest
```

2. 安装并启动 CX-Manager。
3. 确认界面里的目标 Shell 是你当前使用的 PowerShell。推荐 PowerShell 7，也就是 `pwsh`。
4. 按需要修改代理地址，默认是：

```text
http://10.20.34.92:7890
```

5. 点击项目根目录区域的 `+`，选择你的项目总目录，例如：

```text
C:\Users\<your-name>\PycharmProjects
```

6. 点击保存。
7. 重启 PowerShell，或者在当前终端重新加载 Profile：

```powershell
. $PROFILE
```

首次启动和保存时，CX-Manager 会检查当前目标 Shell 的 PowerShell Profile。如果缺少必要配置，会自动追加默认实现；如果你已经写过同名函数，CX-Manager 不会覆盖。写入前会先做 PowerShell 语法检查，并在同目录保留一份 `.cxmanager.bak` 备份。

如果 Profile 已经损坏，先用不加载 Profile 的方式打开 PowerShell：

```powershell
pwsh -NoProfile
```

然后检查 `$PROFILE` 指向的文件，或者用同目录的 `.cxmanager.bak` 备份恢复。

## 终端命令用法

CX-Manager 会维护这些 PowerShell 函数：

- `proxy`
- `unproxy`
- `cx`
- `Show-CXMenu`
- `Get-CXManagerProjectFolders`

通常你只需要直接使用 `proxy`、`unproxy` 和 `cx`。另外两个是菜单和项目扫描辅助函数。

### `proxy`

启用当前 PowerShell 进程的代理环境变量。

运行：

```powershell
proxy
```

它会把界面里配置的代理地址写入这些环境变量：

```powershell
HTTP_PROXY
HTTPS_PROXY
ALL_PROXY
http_proxy
https_proxy
all_proxy
```

注意：`proxy` 只影响当前 PowerShell 进程和从这个终端启动的子进程，不会修改 Windows 系统代理。

### `unproxy`

清空当前 PowerShell 进程的代理环境变量。

运行：

```powershell
unproxy
```

它会清空：

```powershell
HTTP_PROXY
HTTPS_PROXY
ALL_PROXY
http_proxy
https_proxy
all_proxy
```

适合临时访问内网服务，或者确认某个问题是否由代理导致。

### `cx`

选择项目目录并启动 Codex。

运行：

```powershell
cx
```

执行流程：

1. 从 CX-Manager 配置的项目根目录读取所有一级子目录。
2. 显示项目选择菜单。
3. 切换到你选中的项目目录。
4. 显示 Codex 启动模式菜单。
5. 根据选择启动 `codex`。

常见启动模式：

- `codex 自动模式`：执行 `codex -s danger-full-access -a never`
- `codex 正常模式`：执行 `codex`

也可以把参数透传给底层 `codex` 命令：

```powershell
cx --help
cx "帮我检查这个项目"
```

如果没有项目可选，先回到 CX-Manager 界面添加项目根目录。项目根目录本身不会作为项目出现，菜单里显示的是这些根目录下面的一级子目录。

### `Get-CXManagerProjectFolders`

读取 `$CX_MANAGER_PROJECT_ROOTS` 中配置的所有项目根目录，并返回其中存在的一级子目录。

一般不需要手动调用。调试项目扫描时可以运行：

```powershell
Get-CXManagerProjectFolders
```

### `Show-CXMenu`

用于在终端里显示上下键选择菜单。

一般不需要手动调用。它被 `cx` 用来显示项目选择和 Codex 启动模式选择。

键盘操作：

- 上下方向键：移动选择
- Enter：确认
- Esc：取消

## PowerShell 版本

CX-Manager 支持 PowerShell 7 和 Windows PowerShell 5.1，但推荐使用 PowerShell 7。

检查当前版本：

```powershell
$PSVersionTable.PSVersion
```

检查 `pwsh` 是否可用：

```powershell
pwsh --version
```

PowerShell 7 Profile 通常位于：

```text
C:\Users\<your-name>\Documents\PowerShell\Microsoft.PowerShell_profile.ps1
```

Windows PowerShell 5.1 Profile 通常位于：

```text
C:\Users\<your-name>\Documents\WindowsPowerShell\Microsoft.PowerShell_profile.ps1
```

CX-Manager 在自动模式下会优先选择可用的 PowerShell 7。如果你在界面中手动选择了某个 Shell，则只会修改对应 Shell 的 Profile。

## Codex 前置条件

CX-Manager 不负责 Codex 登录。使用 `cx` 前，请先确认官方 Codex CLI 已经可用：

```powershell
codex --version
```

如果 `codex` 命令不存在，需要先安装官方 Codex CLI 并完成登录。

## 更新 Codex

CX-Manager 会检测本地 Codex 版本和 npm 上的最新版本。如果发现可以更新，界面会显示更新按钮。

更新检查和更新命令会使用界面里配置的代理地址。

## 常见问题

### 修改设置后终端里命令没变化

PowerShell Profile 只会在新终端启动时自动加载。保存设置后，重启 PowerShell，或者运行：

```powershell
. $PROFILE
```

### `cx` 找不到项目

确认你已经在 CX-Manager 里添加了项目根目录，并且该根目录下存在项目子目录。

例如添加：

```text
C:\Users\<your-name>\PycharmProjects
```

那么 `cx` 菜单会显示 `PycharmProjects` 下的一级子目录。

### 访问内网异常

如果当前终端启用了代理，内网请求可能会被代理影响。可以先运行：

```powershell
unproxy
```

需要恢复代理时再运行：

```powershell
proxy
```
